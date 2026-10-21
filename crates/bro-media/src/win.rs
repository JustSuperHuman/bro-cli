//! Windows: Media Foundation decodes video and audio, the Windows Imaging Component decodes AVIF,
//! HEIC and other images the `image` crate can't. All of the crate's `unsafe` lives here.
//!
//! Media Foundation's few free functions are looked up at run time instead of linked: Windows N
//! editions without the Media Feature Pack have no mfplat.dll, and a load-time import would keep
//! the whole app from starting there. Everything else is COM (vtable calls, no imports).

use crate::decode::{AudioChunk, AudioDecoder, OpenError, Opened, VideoDecoder};
use crate::{Info, Rgba};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;
use windows::Win32::Foundation::{E_FAIL, GENERIC_READ, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device, ID3D11Multithread,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_MINIMUM_POWER, IDXGIAdapter1, IDXGIFactory1, IDXGIFactory6};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::Win32::System::Variant::{VT_I8, VT_UI8};
use windows::core::{GUID, HRESULT, Interface, PCSTR, PCWSTR, s, w};

const BACKEND: &str = "Media Foundation";
const BACKEND_GPU: &str = "Media Foundation (GPU)";
const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const MEDIA_SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;

type Result<T> = windows::core::Result<T>;

/// COM on this thread while it lives: multithreaded, unless the thread already joined a
/// single-threaded apartment (a UI thread), which in-proc MF and WIC objects work in too.
pub(crate) struct Com {
    owned: bool,
    _thread_bound: PhantomData<*const ()>,
}

impl Com {
    pub(crate) fn init() -> Com {
        // SAFETY: no preconditions. A success (S_OK, S_FALSE) is balanced by CoUninitialize in
        // drop; RPC_E_CHANGED_MODE (the thread is already an STA) isn't ours to undo.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Com { owned: hr.is_ok(), _thread_bound: PhantomData }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: balances this thread's successful CoInitializeEx (Com is !Send).
            unsafe { CoUninitialize() }
        }
    }
}

type StartupFn = unsafe extern "system" fn(u32, u32) -> HRESULT;
type ShutdownFn = unsafe extern "system" fn() -> HRESULT;
type CreateAttributesFn = unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT;
type CreateMediaTypeFn = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type CreateDxgiManagerFn = unsafe extern "system" fn(*mut u32, *mut *mut c_void) -> HRESULT;
type CreateReaderFn = unsafe extern "system" fn(PCWSTR, *mut c_void, *mut *mut c_void) -> HRESULT;

/// Media Foundation's free functions, from the DLLs in System32.
struct Api {
    startup: StartupFn,
    shutdown: ShutdownFn,
    create_attributes: CreateAttributesFn,
    create_media_type: CreateMediaTypeFn,
    create_dxgi_manager: CreateDxgiManagerFn,
    create_reader: CreateReaderFn,
}

/// The export `name` of `module` as the function pointer type `F`.
///
/// # Safety
/// `F` must be an `extern "system"` function pointer with the export's real signature.
unsafe fn export<F: Copy>(module: HMODULE, name: PCSTR) -> Option<F> {
    // SAFETY: a lookup by name in a loaded module.
    let f = unsafe { GetProcAddress(module, name) }?;
    // SAFETY: function pointers share one size; the caller vouches for the signature.
    Some(unsafe { std::mem::transmute_copy::<unsafe extern "system" fn() -> isize, F>(&f) })
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: the DLLs load from System32 only and stay loaded for the process; each export
        // gets the signature Microsoft documents for it.
        unsafe {
            let plat = LoadLibraryExW(w!("mfplat.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            let readwrite = LoadLibraryExW(w!("mfreadwrite.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            Some(Api {
                startup: export(plat, s!("MFStartup"))?,
                shutdown: export(plat, s!("MFShutdown"))?,
                create_attributes: export(plat, s!("MFCreateAttributes"))?,
                create_media_type: export(plat, s!("MFCreateMediaType"))?,
                create_dxgi_manager: export(plat, s!("MFCreateDXGIDeviceManager"))?,
                create_reader: export(readwrite, s!("MFCreateSourceReaderFromURL"))?,
            })
        }
    })
    .as_ref()
}

/// Media Foundation started while this lives (MFStartup is reference counted).
struct MfLife {
    api: &'static Api,
}

impl MfLife {
    fn start() -> std::result::Result<MfLife, OpenError> {
        // No Media Foundation at all (Windows N without the Media Feature Pack).
        let api = api().ok_or(OpenError::Unsupported)?;
        // SAFETY: MFStartup with the SDK version these bindings target; balanced by MFShutdown in drop.
        unsafe { (api.startup)(MF_VERSION, MFSTARTUP_NOSOCKET) }.ok().map_err(|e| OpenError::Failed(format!("Media Foundation won't start: {}", e.message())))?;
        Ok(MfLife { api })
    }

    fn attributes(&self, size: u32) -> Result<IMFAttributes> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: on success the out-pointer holds one reference to a new IMFAttributes, which
        // from_raw takes over.
        unsafe {
            (self.api.create_attributes)(&mut raw, size).ok()?;
            Ok(IMFAttributes::from_raw(raw))
        }
    }

    fn media_type(&self) -> Result<IMFMediaType> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: as in `attributes`, for an IMFMediaType.
        unsafe {
            (self.api.create_media_type)(&mut raw).ok()?;
            Ok(IMFMediaType::from_raw(raw))
        }
    }

    fn reader(&self, path: &Path, attributes: Option<&IMFAttributes>) -> Result<IMFSourceReader> {
        let wide = wide(path);
        let mut raw = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and outlives the call; the attributes pointer is a live
        // interface or null; on success the out-pointer holds one IMFSourceReader reference.
        unsafe {
            (self.api.create_reader)(PCWSTR(wide.as_ptr()), attributes.map_or(std::ptr::null_mut(), |a| a.as_raw()), &mut raw).ok()?;
            Ok(IMFSourceReader::from_raw(raw))
        }
    }
}

impl Drop for MfLife {
    fn drop(&mut self) {
        // SAFETY: balances the MFStartup in `start`.
        unsafe {
            let _ = (self.api.shutdown)();
        }
    }
}

/// A Direct3D 11 device for Media Foundation to decode and scale on.
#[derive(Clone)]
struct Gpu {
    manager: IMFDXGIDeviceManager,
    device: ID3D11Device,
}

enum GpuState {
    Untried,
    Ready(Gpu),
    /// No adapter does video (a VM, a remote session): decode in software from now on.
    Missing,
}

/// One device for every reader in the process: creating one costs 40-150 ms.
struct SharedGpu(GpuState);

// SAFETY: D3D11 devices are free-threaded (`Gpu::wire` turns on the multithread protection that
// guards their immediate context), and a DXGI device manager exists to hand one device to many
// threads' decoders.
unsafe impl Send for SharedGpu {}

static GPU: Mutex<SharedGpu> = Mutex::new(SharedGpu(GpuState::Untried));

impl Gpu {
    /// The process's video device, made on first use (and again after the device was lost).
    fn shared(mf: &MfLife) -> Option<Gpu> {
        let mut shared = GPU.lock().unwrap_or_else(PoisonError::into_inner);
        if let GpuState::Ready(gpu) = &shared.0 {
            // SAFETY: a status query on a live device.
            if unsafe { gpu.device.GetDeviceRemovedReason() }.is_ok() {
                return Some(gpu.clone());
            }
        }
        if matches!(shared.0, GpuState::Missing) {
            return None;
        }
        shared.0 = match Gpu::new(mf) {
            Ok(gpu) => GpuState::Ready(gpu),
            Err(_) => GpuState::Missing,
        };
        match &shared.0 {
            GpuState::Ready(gpu) => Some(gpu.clone()),
            _ => None,
        }
    }

    /// The first hardware adapter that does video, the power-saving one first (an integrated GPU
    /// decodes video as well as a discrete one, and lets a laptop's discrete GPU sleep).
    fn new(mf: &MfLife) -> Result<Gpu> {
        // SAFETY: D3D11/DXGI object creation and queries on this thread; out-pointers are locals.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let by_power = factory.cast::<IDXGIFactory6>().ok();
            let mut last = None;
            for i in 0..8 {
                let adapter: Result<IDXGIAdapter1> = match &by_power {
                    Some(f) => f.EnumAdapterByGpuPreference(i, DXGI_GPU_PREFERENCE_MINIMUM_POWER),
                    None => factory.EnumAdapters1(i),
                };
                let Ok(adapter) = adapter else { break };
                if adapter.GetDesc1().is_ok_and(|d| d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0) {
                    continue;
                }
                let mut device = None;
                let flags = D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT;
                match D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut device), None, None) {
                    Ok(()) => {
                        if let Some(device) = device {
                            return Gpu::wire(mf, device);
                        }
                    }
                    Err(e) => last = Some(e),
                }
            }
            Err(last.unwrap_or_else(|| windows::core::Error::from_hresult(E_FAIL)))
        }
    }

    fn wire(mf: &MfLife, device: ID3D11Device) -> Result<Gpu> {
        let (mut token, mut raw) = (0u32, std::ptr::null_mut());
        // SAFETY: Media Foundation calls the device from its own threads, so it's only used with
        // multithread protection on; on success the out-pointer holds one IMFDXGIDeviceManager
        // reference.
        unsafe {
            let _ = device.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);
            (mf.api.create_dxgi_manager)(&mut token, &mut raw).ok()?;
            let manager = IMFDXGIDeviceManager::from_raw(raw);
            manager.ResetDevice(&device, token)?;
            Ok(Gpu { manager, device })
        }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

fn failed(e: windows::core::Error) -> OpenError {
    OpenError::Failed(e.message())
}

/// Media Foundation couldn't open the file: missing or locked files say so, anything else means
/// MF can't read this kind of file.
fn open_error(e: windows::core::Error) -> OpenError {
    let code = e.code().0 as u32;
    match code {
        0x8007_0002 | 0x8007_0003 => OpenError::Failed("the file doesn't exist".into()),
        0x8007_0005 => OpenError::Failed("access to the file is denied".into()),
        0x8007_0020 => OpenError::Failed("another app has the file locked".into()),
        _ => OpenError::Unsupported,
    }
}

fn duration(reader: &IMFSourceReader) -> Option<Duration> {
    // SAFETY: the union is read only after checking its tag; a VT_UI8 owns no memory, so there
    // is nothing to clear.
    unsafe {
        let pv = reader.GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION).ok()?;
        let v = &pv.Anonymous.Anonymous;
        (v.vt == VT_UI8 && v.Anonymous.uhVal > 0).then(|| Duration::from_nanos(v.Anonymous.uhVal.saturating_mul(100)))
    }
}

fn set_position(reader: &IMFSourceReader, to: f64) -> std::result::Result<(), String> {
    let mut pv = PROPVARIANT::default();
    // SAFETY: a VT_I8 PROPVARIANT (tag and value written together) owns no memory and outlives the call.
    unsafe {
        let v = &mut *pv.Anonymous.Anonymous;
        v.vt = VT_I8;
        v.Anonymous.hVal = (to.max(0.0) * 1e7).round() as i64;
        reader.SetCurrentPosition(&GUID::zeroed(), &pv)
    }
    .map_err(|e| format!("seek failed: {}", e.message()))
}

/// One ReadSample: (flags, timestamp in seconds, sample).
fn read_sample(reader: &IMFSourceReader, stream: u32) -> Result<(u32, f64, Option<IMFSample>)> {
    let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
    // SAFETY: the out-pointers are locals that outlive the call.
    unsafe { reader.ReadSample(stream, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))? };
    Ok((flags, ts as f64 / 1e7, sample))
}

fn has(flags: u32, flag: MF_SOURCE_READER_FLAG) -> bool {
    flags & flag.0 as u32 != 0
}

fn subtype(t: &IMFMediaType) -> GUID {
    // SAFETY: an attribute read on a media type this thread owns.
    unsafe { t.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or_default()
}

/// A readable codec name for a stream subtype ("H.264", "HEVC", "AAC"...).
fn codec_name(g: &GUID, video: bool) -> String {
    const BASE: (u16, u16, [u8; 8]) = (0x0000, 0x0010, [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]);
    let named = [
        (MFVideoFormat_MPEG2, "MPEG-2"),
        (MFAudioFormat_Dolby_AC3, "AC-3"),
        (MFAudioFormat_Dolby_DDPlus, "E-AC-3"),
        (MFAudioFormat_Vorbis, "Vorbis"),
    ];
    if let Some((_, name)) = named.iter().find(|(guid, _)| guid == g) {
        return name.to_string();
    }
    if (g.data2, g.data3, g.data4) != BASE {
        return "unknown".into();
    }
    if !video {
        return match g.data1 {
            0x0001 => "PCM",
            0x0003 => "PCM",
            0x0050 => "MP2",
            0x0055 => "MP3",
            0x0160..=0x0163 => "WMA",
            0x1600 | 0x1610 => "AAC",
            0xf1ac => "FLAC",
            0x704f => "Opus",
            0x6c61 => "ALAC",
            0x7361 | 0x7362 => "AMR",
            0x0008 | 0x2001 => "DTS",
            0x0092 | 0x2000 => "AC-3",
            0xac40 => "AC-4",
            tag => return format!("audio format 0x{tag:04x}"),
        }
        .into();
    }
    let fourcc: String = g.data1.to_le_bytes().iter().map(|&b| b as char).collect();
    match fourcc.to_ascii_uppercase().as_str() {
        "H264" | "AVC1" => "H.264",
        "HEVC" | "H265" | "HVC1" | "HEV1" => "HEVC",
        "VP90" => "VP9",
        "VP80" => "VP8",
        "AV01" => "AV1",
        "MP4V" | "MP4S" | "M4S2" | "FMP4" | "XVID" | "DIVX" | "DX50" => "MPEG-4",
        "MP43" | "MP42" | "MPG4" => "MPEG-4 v3",
        "WMV1" | "WMV2" | "WMV3" => "WMV",
        "WVC1" => "VC-1",
        "MJPG" => "MJPEG",
        "MPG1" => "MPEG-1",
        "DVSD" | "DVHD" | "DVSL" | "DV25" | "DV50" => "DV",
        _ => return fourcc.trim().to_string(),
    }
    .into()
}

/// The display rectangle (x, y, w, h) within a `size` frame: decoders pad 1080 to 1088 rows.
fn aperture(t: &IMFMediaType, (w, h): (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    for key in [MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_GEOMETRIC_APERTURE] {
        // MFVideoArea: OffsetX {fract: u16, value: i16}, OffsetY {..}, Area {cx: i32, cy: i32}
        let mut blob = [0u8; 16];
        // SAFETY: GetBlob writes at most blob.len() bytes.
        if unsafe { t.GetBlob(&key, &mut blob, None) }.is_err() {
            continue;
        }
        let x = i16::from_le_bytes([blob[2], blob[3]]).max(0) as u32;
        let y = i16::from_le_bytes([blob[6], blob[7]]).max(0) as u32;
        let cw = i32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]).max(0) as u32;
        let ch = i32::from_le_bytes([blob[12], blob[13], blob[14], blob[15]]).max(0) as u32;
        if cw > 0 && ch > 0 && x + cw <= w && y + ch <= h {
            return Some((x, y, cw, ch));
        }
    }
    None
}

/// What the file's video stream looks like when shown.
struct NativeVideo {
    /// Width x height with the pixel aspect applied, as stored (not yet rotated).
    display: (u32, u32),
    /// MF_MT_VIDEO_ROTATION: degrees to turn the stored frame clockwise to show it upright.
    rotation: u32,
    fps: Option<f32>,
}

fn native_video(t: &IMFMediaType) -> NativeVideo {
    // SAFETY: attribute reads on a media type this thread owns.
    unsafe {
        let size = t.GetUINT64(&MF_MT_FRAME_SIZE).map(unpack).unwrap_or((0, 0));
        let (_, _, w, h) = aperture(t, size).unwrap_or((0, 0, size.0, size.1));
        let par = t.GetUINT64(&MF_MT_PIXEL_ASPECT_RATIO).map(unpack).ok().filter(|&(n, d)| n > 0 && d > 0).unwrap_or((1, 1));
        let w = (w as u64 * par.0 as u64 / par.1 as u64).min(u32::MAX as u64) as u32;
        let rotation = t.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(0) % 360;
        let fps = t.GetUINT64(&MF_MT_FRAME_RATE).map(unpack).ok().filter(|&(n, d)| n > 0 && d > 0).map(|(n, d)| n as f32 / d as f32);
        NativeVideo { display: (w.max(1), h.max(1)), rotation, fps }
    }
}

/// The RGB32 frames the reader now delivers.
#[derive(Clone, Copy)]
struct OutFormat {
    size: (u32, u32),
    stride: i32,
    crop: (u32, u32, u32, u32),
    /// Clockwise degrees the frames still need: the video processor rotates (and then drops the
    /// attribute), plain colour conversion doesn't.
    rotation: u32,
}

fn out_format(reader: &IMFSourceReader) -> Result<OutFormat> {
    // SAFETY: reads on the reader and media type this thread owns.
    unsafe {
        let current = reader.GetCurrentMediaType(VIDEO)?;
        let (w, h) = unpack(current.GetUINT64(&MF_MT_FRAME_SIZE)?);
        let stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(w as i32 * 4);
        let crop = aperture(&current, (w, h)).unwrap_or((0, 0, w, h));
        let rotation = current.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(0) % 360;
        Ok(OutFormat { size: (w, h), stride, crop, rotation })
    }
}

/// Ask the reader for RGB32 frames; with `scale` = (output size, display size in the same
/// orientation), the video processor scales and rotates them too. See [`tune_transforms`] for
/// `low_latency`.
fn set_rgb32(mf: &MfLife, reader: &IMFSourceReader, scale: Option<((u32, u32), (u32, u32))>, low_latency: bool) -> Result<OutFormat> {
    let mt = mf.media_type()?;
    // SAFETY: attribute writes on a media type this thread created; the reader copies it.
    unsafe {
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        if let Some(((w, h), (dw, dh))) = scale {
            mt.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
            // The processor letterboxes when the output's shape differs from the source's even by
            // a rounding error; pixels a hair off square make the shapes match exactly.
            let (n, d) = (dw as u64 * h as u64, dh as u64 * w as u64);
            let g = gcd(n, d).max(1);
            mt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack((n / g).min(u32::MAX as u64) as u32, (d / g).min(u32::MAX as u64) as u32))?;
        }
        reader.SetCurrentMediaType(VIDEO, None, &mt)?;
    }
    tune_transforms(reader, low_latency);
    out_format(reader)
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Tune the transforms the reader built for the video stream:
/// - the video processor converts to the frame rate the container declares, and Media
///   Foundation's Matroska/WebM source declares half the real rate: every other frame would
///   vanish, so frame-rate conversion goes;
/// - with `low_latency` (HEVC) the decoder stops holding frames back, which otherwise stalls
///   every seek for about a second. HEVC always declares how far its frames reorder, so they still
///   come out in order; H.264 doesn't have to, and its B-frames come out garbled in that mode.
fn tune_transforms(reader: &IMFSourceReader, low_latency: bool) {
    let Ok(reader) = reader.cast::<IMFSourceReaderEx>() else { return };
    for i in 0..8 {
        let (mut category, mut transform) = (GUID::zeroed(), None);
        // SAFETY: out-pointers to locals; the transform is used on this thread only.
        if unsafe { reader.GetTransformForStream(VIDEO, i, Some(&mut category), &mut transform) }.is_err() {
            break;
        }
        let Some(transform) = transform else { continue };
        let key = if category == MFT_CATEGORY_VIDEO_PROCESSOR {
            MF_XVP_DISABLE_FRC
        } else if category == MFT_CATEGORY_VIDEO_DECODER && low_latency {
            MF_LOW_LATENCY
        } else {
            continue;
        };
        // SAFETY: an attribute write on the transform's own store.
        let _ = unsafe { transform.GetAttributes().and_then(|a| a.SetUINT32(&key, 1)) };
    }
}

/// Video larger than this (720p) decodes on the GPU when one can: in software it costs a core or
/// more per stream (4K HEVC: about two cores and half a gigabyte). Smaller video stays in
/// software, which starts sooner than a GPU decoder spins up.
const GPU_FROM_PIXELS: u64 = 1280 * 720;

/// A source reader for `path` that converts and scales video, on `gpu` if given.
fn new_reader(mf: &MfLife, path: &Path, gpu: Option<&Gpu>) -> std::result::Result<IMFSourceReader, OpenError> {
    let attributes = mf.attributes(3).map_err(failed)?;
    // SAFETY: attribute writes on an object this thread created; the device manager outlives the
    // reader (MfVideo drops the reader first).
    unsafe {
        attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1).map_err(failed)?;
        if let Some(gpu) = gpu {
            attributes.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &gpu.manager).map_err(failed)?;
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(failed)?;
        }
    }
    mf.reader(path, Some(&attributes)).map_err(open_error)
}

/// Video and audio streams of `path`, with the video decoding to RGBA frames fitted inside
/// `max_side`. Video larger than [`GPU_FROM_PIXELS`] tries the GPU first, then software.
pub(crate) fn open_video(path: &Path, max_side: u32) -> std::result::Result<Opened, OpenError> {
    let _com = Com::init();
    let mf = MfLife::start()?;
    let reader = new_reader(&mf, path, None)?;
    // SAFETY: reads on the reader this thread created.
    let pixels = unsafe { reader.GetNativeMediaType(VIDEO, 0).and_then(|t| t.GetUINT64(&MF_MT_FRAME_SIZE)) }.map(unpack).map_or(0, |(w, h)| w as u64 * h as u64);
    // A device manager can only be handed to a new reader, so the GPU gets one of its own.
    if pixels > GPU_FROM_PIXELS
        && let Some(gpu) = Gpu::shared(&mf)
        && let Ok(on_gpu) = new_reader(&mf, path, Some(&gpu))
        && let Ok(opened) = configure(on_gpu, Some(gpu), max_side)
    {
        return Ok(opened);
    }
    configure(reader, None, max_side)
}

/// Pick the streams and output formats of a fresh reader and decode the first frames.
fn configure(reader: IMFSourceReader, gpu: Option<Gpu>, max_side: u32) -> std::result::Result<Opened, OpenError> {
    let com = Com::init();
    let mf = MfLife::start()?;
    let duration = duration(&reader);
    // SAFETY: reads on the reader this thread created.
    let (video, audio) = unsafe { (reader.GetNativeMediaType(VIDEO, 0).ok(), reader.GetNativeMediaType(AUDIO, 0).ok()) };
    let audio_codec = audio.as_ref().map(|t| codec_name(&subtype(t), false));
    let Some(native) = video else {
        if audio.is_none() {
            return Err(OpenError::Failed("no video or audio in this file".into()));
        }
        let info = Info { duration, has_audio: true, codec: audio_codec.clone(), audio_codec, backend: BACKEND, ..Info::default() };
        return Ok(Opened { info, video: None });
    };
    let codec = codec_name(&subtype(&native), true);
    let no_codec = || OpenError::NoCodec { codec: codec.clone(), video: true };
    let format = native_video(&native);
    let (dw, dh) = format.display;
    let shown = if format.rotation % 180 == 90 { (dh, dw) } else { (dw, dh) };
    let (tw, th) = crate::still::fit(shown.0, shown.1, max_side);
    // The video processor wants even sizes.
    let target = ((tw & !1).max(2), (th & !1).max(2));
    // SAFETY: stream selection on the reader this thread created.
    unsafe { reader.SetStreamSelection(ALL_STREAMS, false).and_then(|_| reader.SetStreamSelection(VIDEO, true)) }.map_err(failed)?;
    // Scaled (and rotated) by Media Foundation when it can, else converted at the native size
    // and finished on the CPU.
    let low_latency = codec == "HEVC";
    let out = set_rgb32(&mf, &reader, Some((target, shown)), low_latency)
        .or_else(|_| set_rgb32(&mf, &reader, None, low_latency))
        .map_err(|_| no_codec())?;
    let backend = if gpu.is_some() { BACKEND_GPU } else { BACKEND };
    let mut fps = format.fps;
    let mut video = MfVideo {
        reader,
        out,
        target,
        low_latency,
        interval: fps.filter(|f| *f >= 1.0).map_or(1.0 / 30.0, |f| 1.0 / f as f64),
        offset: 0.0,
        pending: VecDeque::new(),
        _gpu: gpu,
        _mf: mf,
        _com: com,
    };
    // Decode two frames now. Some decoders accept a stream and then fail on its first frame (the
    // Store's VP9 decoder on 4:4:4 video), which hands the file to ffmpeg; the frames also give
    // the true start and frame rate.
    if let Some((t0, f0)) = video.read().map_err(|_| no_codec())? {
        // Media Foundation ignores MP4 edit lists, so B-frame video starts a couple of frames late.
        video.offset = if t0 > 0.0 && t0 <= 0.5 { t0 } else { 0.0 };
        video.pending.push_back((t0 - video.offset, f0));
        if let Ok(Some((t1, f1))) = video.read() {
            if t1 - t0 > 0.001 {
                video.interval = t1 - t0;
                let measured = (1.0 / video.interval) as f32;
                // The container's rate when the frames agree (it's exact), theirs when they don't:
                // Media Foundation reads half the real rate from Matroska.
                fps = Some(match fps {
                    Some(declared) if (declared - measured).abs() <= declared * 0.01 => declared,
                    _ => (measured * 1000.0).round() / 1000.0,
                });
            }
            video.pending.push_back((t1 - video.offset, f1));
        }
    }
    let info = Info { duration, size: Some(shown), has_video: true, has_audio: audio.is_some(), codec: Some(codec), audio_codec, fps, backend };
    Ok(Opened { info, video: Some(Box::new(video)) })
}

// RGBA frames from a source reader. Field order is drop order: the reader goes before its GPU,
/// Media Foundation shuts down, and COM uninitialises.
struct MfVideo {
    reader: IMFSourceReader,
    out: OutFormat,
    /// Frame size handed out (display orientation).
    target: (u32, u32),
    low_latency: bool,
    interval: f64,
    /// Subtracted from the stream's timestamps so its first frame is at zero.
    offset: f64,
    /// Frames decoded at open, handed out first.
    pending: VecDeque<(f64, Rgba)>,
    _gpu: Option<Gpu>,
    _mf: MfLife,
    _com: Com,
}

impl MfVideo {
    /// The next frame as the reader stamps it.
    fn read(&mut self) -> std::result::Result<Option<(f64, Rgba)>, String> {
        loop {
            let (flags, t, sample) = read_sample(&self.reader, VIDEO).map_err(|e| format!("video decoding failed: {}", e.message()))?;
            if has(flags, MF_SOURCE_READERF_ERROR) {
                return Err("video decoding failed".into());
            }
            if has(flags, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED) {
                tune_transforms(&self.reader, self.low_latency);
                self.out = out_format(&self.reader).map_err(|e| e.message())?;
            }
            if let Some(sample) = sample {
                return self.frame(&sample).map(|frame| Some((t, frame)));
            }
            if has(flags, MF_SOURCE_READERF_ENDOFSTREAM) {
                return Ok(None);
            }
        }
    }

    fn frame(&self, sample: &IMFSample) -> std::result::Result<Rgba, String> {
        let OutFormat { size: (w, h), stride, crop, rotation } = self.out;
        let mut px = vec![0u8; Rgba::byte_len(crop.2, crop.3)];
        let bad = || "the video decoder returned a frame of the wrong size".to_string();
        // SAFETY: each buffer stays locked while its memory is read. Lock2D/Lock give `h` rows of
        // |pitch| >= w * 4 bytes (checked, and for Lock against the buffer's length), and the crop
        // rectangle was checked to lie within w x h.
        unsafe {
            let buffer = sample.ConvertToContiguousBuffer().map_err(|e| e.message())?;
            if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer>() {
                let (mut scan0, mut pitch) = (std::ptr::null_mut(), 0i32);
                buffer2d.Lock2D(&mut scan0, &mut pitch).map_err(|e| e.message())?;
                let fits = pitch.unsigned_abs() as usize >= w as usize * 4;
                if fits {
                    copy_bgrx(scan0, pitch as isize, crop, &mut px);
                }
                let _ = buffer2d.Unlock2D();
                if !fits {
                    return Err(bad());
                }
            } else {
                let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
                buffer.Lock(&mut data, None, Some(&mut len)).map_err(|e| e.message())?;
                let abs = stride.unsigned_abs() as usize;
                let fits = abs >= w as usize * 4 && len as usize >= abs * h as usize && h > 0;
                if fits {
                    // a negative stride is a bottom-up image: the top row is the last in memory
                    let (scan0, pitch) = if stride >= 0 { (data, abs as isize) } else { (data.add(abs * (h as usize - 1)), -(abs as isize)) };
                    copy_bgrx(scan0, pitch, crop, &mut px);
                }
                let _ = buffer.Unlock();
                if !fits {
                    return Err(bad());
                }
            }
        }
        let frame = crate::still::rotate(Rgba { w: crop.2, h: crop.3, px }, rotation);
        Ok(if (frame.w, frame.h) == self.target { frame } else { crate::still::resize_opaque(frame, self.target.0, self.target.1) })
    }
}

/// BGRX rows (RGB32) to opaque RGBA, cropped.
///
/// # Safety
/// For every row `y` and column `x` of `crop`, `scan0 + y * pitch + x * 4 .. +4` must be readable.
unsafe fn copy_bgrx(scan0: *const u8, pitch: isize, (cx, cy, cw, ch): (u32, u32, u32, u32), out: &mut [u8]) {
    let row = cw as usize * 4;
    for y in 0..ch as usize {
        // SAFETY: the caller guarantees the crop rectangle lies within the locked image.
        let src = unsafe { std::slice::from_raw_parts(scan0.offset((cy as usize + y) as isize * pitch).add(cx as usize * 4), row) };
        for (d, s) in out[y * row..(y + 1) * row].as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = 255;
        }
    }
}

impl VideoDecoder for MfVideo {
    fn next_frame(&mut self) -> std::result::Result<Option<(f64, Rgba)>, String> {
        if let Some(frame) = self.pending.pop_front() {
            return Ok(Some(frame));
        }
        Ok(self.read()?.map(|(t, frame)| (t - self.offset, frame)))
    }

    fn seek(&mut self, to: f64) -> std::result::Result<(), String> {
        self.pending.clear();
        set_position(&self.reader, to + self.offset)
    }

    fn frame_interval(&self) -> f64 {
        self.interval
    }
}

// Float samples from a source reader: stereo at the device rate when Media Foundation can
/// convert, the decoder's own layout otherwise.
pub(crate) struct MfAudio {
    reader: IMFSourceReader,
    rate: u32,
    channels: usize,
    /// Subtracted from the stream's timestamps so its first sample is at zero.
    offset: f64,
    /// The block decoded at open, handed out first.
    pending: Option<AudioChunk>,
    _mf: MfLife,
    _com: Com,
}

fn set_float(mf: &MfLife, reader: &IMFSourceReader, want: Option<(u32, u32)>) -> Result<()> {
    let mt = mf.media_type()?;
    // SAFETY: attribute writes on a media type this thread created; the reader copies it.
    unsafe {
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
        if let Some((rate, channels)) = want {
            mt.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
            mt.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
            mt.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32)?;
            mt.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, channels * 4)?;
            mt.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * channels * 4)?;
        }
        reader.SetCurrentMediaType(AUDIO, None, &mt)
    }
}

fn audio_format(reader: &IMFSourceReader) -> Result<(u32, usize)> {
    // SAFETY: reads on the reader and media type this thread owns.
    unsafe {
        let current = reader.GetCurrentMediaType(AUDIO)?;
        Ok((current.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?, current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)? as usize))
    }
}

impl MfAudio {
    pub(crate) fn open(path: &Path, rate: u32) -> std::result::Result<MfAudio, OpenError> {
        let com = Com::init();
        let mf = MfLife::start()?;
        let reader = mf.reader(path, None).map_err(open_error)?;
        // SAFETY: reads and stream selection on the reader this thread created.
        let native = unsafe { reader.GetNativeMediaType(AUDIO, 0) }.map_err(|_| OpenError::Failed("no audio in this file".into()))?;
        let codec = codec_name(&subtype(&native), false);
        let no_codec = || OpenError::NoCodec { codec: codec.clone(), video: false };
        // SAFETY: as above.
        unsafe { reader.SetStreamSelection(ALL_STREAMS, false).and_then(|_| reader.SetStreamSelection(AUDIO, true)) }.map_err(failed)?;
        set_float(&mf, &reader, Some((rate, 2))).or_else(|_| set_float(&mf, &reader, None)).map_err(|_| no_codec())?;
        let (rate, channels) = audio_format(&reader).map_err(failed)?;
        let mut audio = MfAudio { reader, rate, channels: channels.max(1), offset: 0.0, pending: None, _mf: mf, _com: com };
        // Decode the first block now: a decoder that can't run here sends the file to ffmpeg.
        if let Some(first) = audio.read().map_err(|_| no_codec())? {
            audio.offset = if first.t > 0.0 && first.t <= 0.5 { first.t } else { 0.0 };
            audio.pending = Some(AudioChunk { t: first.t - audio.offset, ..first });
        }
        Ok(audio)
    }

    /// The next block as the reader stamps it.
    fn read(&mut self) -> std::result::Result<Option<AudioChunk>, String> {
        loop {
            let (flags, t, sample) = read_sample(&self.reader, AUDIO).map_err(|e| format!("audio decoding failed: {}", e.message()))?;
            if has(flags, MF_SOURCE_READERF_ERROR) {
                return Err("audio decoding failed".into());
            }
            if has(flags, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED) {
                (self.rate, self.channels) = audio_format(&self.reader).map_err(|e| e.message())?;
                self.channels = self.channels.max(1);
            }
            if let Some(sample) = sample {
                let data = samples(&sample).map_err(|e| e.message())?;
                if data.is_empty() {
                    continue;
                }
                return Ok(Some(AudioChunk { t, rate: self.rate, channels: self.channels, data }));
            }
            if has(flags, MF_SOURCE_READERF_ENDOFSTREAM) {
                return Ok(None);
            }
        }
    }
}

impl AudioDecoder for MfAudio {
    fn next_chunk(&mut self) -> std::result::Result<Option<AudioChunk>, String> {
        if let Some(first) = self.pending.take() {
            return Ok(Some(first));
        }
        Ok(self.read()?.map(|chunk| AudioChunk { t: chunk.t - self.offset, ..chunk }))
    }

    fn seek(&mut self, to: f64) -> std::result::Result<(), String> {
        self.pending = None;
        set_position(&self.reader, to + self.offset)
    }
}

/// A sample's float data, copied out.
fn samples(sample: &IMFSample) -> Result<Vec<f32>> {
    // SAFETY: the buffer stays locked while its `len` bytes are read.
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
        buffer.Lock(&mut data, None, Some(&mut len))?;
        let bytes = std::slice::from_raw_parts(data, len as usize);
        let out = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let _ = buffer.Unlock();
        Ok(out)
    }
}

// ---- Windows Imaging Component ----

/// Decode the first frame of an image through WIC, fitted inside `max_side`: the pixels, the
/// original size, and the container's name.
pub(crate) fn wic_load(path: &Path, max_side: u32) -> std::result::Result<(Rgba, (u32, u32), String), String> {
    let _com = Com::init();
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let wide = wide(path);
    let wic = |e: windows::core::Error| wic_error(&e, &ext);
    // SAFETY: COM is initialised on this thread; the WIC objects are created and used here, every
    // pointer argument outlives its call, and CopyPixels gets a buffer of exactly stride x height.
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(wic)?;
        let decoder = factory.CreateDecoderFromFilename(PCWSTR(wide.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).map_err(wic)?;
        let container = container_name(&decoder.GetContainerFormat().unwrap_or_default());
        let frame = decoder.GetFrame(0).map_err(wic)?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).map_err(wic)?;
        if w == 0 || h == 0 {
            return Err("the image is empty".into());
        }
        let (nw, nh) = crate::still::fit(w, h, max_side);
        let source: IWICBitmapSource = if (nw, nh) == (w, h) { frame.cast().map_err(wic)? } else { scaled(&factory, &frame, nw, nh).map_err(wic)? };
        let converter = factory.CreateFormatConverter().map_err(wic)?;
        converter.Initialize(&source, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).map_err(wic)?;
        let mut px = vec![0u8; Rgba::byte_len(nw, nh)];
        converter.CopyPixels(std::ptr::null(), nw * 4, &mut px).map_err(wic)?;
        Ok((Rgba { w: nw, h: nh, px }, (w, h), container))
    }
}

fn scaled(factory: &IWICImagingFactory, frame: &IWICBitmapFrameDecode, w: u32, h: u32) -> Result<IWICBitmapSource> {
    let mut last = None;
    // High-quality cubic is Windows 10+; Fant everywhere.
    for mode in [WICBitmapInterpolationModeHighQualityCubic, WICBitmapInterpolationModeFant] {
        // SAFETY: a fresh scaler per attempt, initialised from a frame that outlives it.
        let scaler = unsafe { factory.CreateBitmapScaler()? };
        match unsafe { scaler.Initialize(frame, w, h, mode) } {
            Ok(()) => return scaler.cast(),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(windows::core::Error::empty))
}

fn wic_error(e: &windows::core::Error, ext: &str) -> String {
    const COMPONENT_NOT_FOUND: u32 = 0x8898_2f50;
    const UNKNOWN_FORMAT: u32 = 0x8898_2f07;
    if matches!(e.code().0 as u32, COMPONENT_NOT_FOUND | UNKNOWN_FORMAT) {
        return match ext {
            "heic" | "heif" | "hif" => "Windows can't decode HEIC images yet; install HEIF Image Extensions (and HEVC Video Extensions) from the Microsoft Store".into(),
            "avif" => "Windows can't decode AVIF images yet; install AV1 Video Extension from the Microsoft Store".into(),
            "" => "can't preview this file".into(),
            ext => format!("can't preview .{ext} images"),
        };
    }
    format!("can't decode this image: {}", e.message())
}

fn container_name(g: &GUID) -> String {
    let names = [
        (GUID_ContainerFormatHeif, "HEIF"),
        (GUID_ContainerFormatWebp, "WebP"),
        (GUID_ContainerFormatJpeg, "JPEG"),
        (GUID_ContainerFormatPng, "PNG"),
        (GUID_ContainerFormatTiff, "TIFF"),
        (GUID_ContainerFormatGif, "GIF"),
        (GUID_ContainerFormatBmp, "BMP"),
        (GUID_ContainerFormatIco, "ICO"),
        (GUID_ContainerFormatDds, "DDS"),
        (GUID_ContainerFormatWmp, "JPEG XR"),
        (GUID_ContainerFormatRaw, "RAW"),
        (GUID_ContainerFormatAdng, "DNG"),
    ];
    names.iter().find(|(guid, _)| guid == g).map(|(_, name)| name.to_string()).unwrap_or_default()
}
