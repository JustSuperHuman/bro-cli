//! Which GPU drives the display a window is on. A desktop with its monitor on a graphics card still has the CPU's
//! integrated GPU, and that's the one a low-power choice picks: it then draws every frame and copies it across to
//! the card for the monitor. Drawing on the monitor's own GPU saves both.

/// The PCI (vendor, device) of the GPU whose outputs include the monitor most of `hwnd` is on (DXGI's adapters).
#[cfg(windows)]
pub fn gpu_of_window(hwnd: isize) -> Option<(u32, u32)> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
    // SAFETY: plain Win32 / DXGI queries; every COM object is released when it drops
    unsafe {
        let monitor = MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return None;
        }
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            a += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                if output.GetDesc().is_ok_and(|d| d.Monitor == monitor) {
                    return Some((desc.VendorId, desc.DeviceId));
                }
            }
        }
        None
    }
}

/// Elsewhere the system's choice stands.
#[cfg(not(windows))]
pub fn gpu_of_window(_hwnd: isize) -> Option<(u32, u32)> {
    None
}

/// The GPU driving the display `target` (a window) is on, when the system can say.
pub fn gpu_of_target(target: &wgpu::SurfaceTarget<'_>) -> Option<(u32, u32)> {
    use wgpu::rwh::HasWindowHandle;
    let handle = match target {
        wgpu::SurfaceTarget::DisplayAndWindow(w) => w.window_handle().ok()?,
        wgpu::SurfaceTarget::Window(w) => w.window_handle().ok()?,
        _ => return None,
    };
    match handle.as_raw() {
        wgpu::rwh::RawWindowHandle::Win32(h) => gpu_of_window(h.hwnd.get()),
        _ => None,
    }
}
