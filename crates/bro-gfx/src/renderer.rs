//! The wgpu side: device and surface, the pipelines (shadows, quads, images, glyphs, and subpixel glyphs where
//! the GPU blends two sources), the glyph atlases' textures and the image textures. [`Renderer::render`] draws a
//! [`Scene`] and presents it, through the effects pass (a theme's glow, CRT and vignette) when one is on.
//!
//! A see-through window ([`Options::transparent`]) gets a swap chain that carries alpha: on Windows a
//! DirectComposition one (DX12), which is what lets Acrylic and Mica show behind the frame. The frame is
//! premultiplied throughout, which is what the blending below produces when the clear colour is.

use crate::atlas::{Atlas, Kind};
use crate::scene::{GlyphInstance, ImageId, ImageInstance, QuadInstance, Scene, ShadowInstance};
use crate::text::{Antialias, Text};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const COMMON: &str = include_str!("shaders/common.wgsl");
const QUAD: &str = include_str!("shaders/quad.wgsl");
const SHADOW: &str = include_str!("shaders/shadow.wgsl");
const GLYPH: &str = include_str!("shaders/glyph.wgsl");
const GLYPH_LCD: &str = include_str!("shaders/glyph_lcd.wgsl");
const IMAGE: &str = include_str!("shaders/image.wgsl");
const POST: &str = include_str!("shaders/post.wgsl");
const BLOOM: &str = include_str!("shaders/bloom.wgsl");
/// The glow's half-size textures (smooth falloff without banding).
const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// How the window's drawing surface is set up.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Options {
    /// wait for the display (no tearing)
    pub vsync: bool,
    /// the window can be see-through: a swap chain with alpha (on Windows a DX12 DirectComposition one). Falls
    /// back to an opaque one when the system can't; [`Renderer::see_through`] tells.
    pub transparent: bool,
}

/// Whole-screen effects (a theme's skin), applied after the scene is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Effects {
    /// every colour becomes a shade between these two (ground, ink), like a monochrome CRT
    pub phosphor: Option<([f32; 3], [f32; 3])>,
    /// how much the edges and corners darken (0 = off)
    pub vignette: f32,
    /// how much every other band of `scanline_px` pixels darkens (0 = off)
    pub scanlines: f32,
    pub scanline_px: f32,
    /// how strongly bright, saturated colours glow like neon (0 = off, 1 = strong)
    pub glow: f32,
    /// how far the glow spreads (its blur's sigma), physical pixels
    pub glow_px: f32,
}

impl Effects {
    pub fn active(&self) -> bool {
        self.phosphor.is_some() || self.vignette > 0.0 || self.scanlines > 0.0 || self.glow > 0.0
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BloomUniform {
    step: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PostUniform {
    ground: [f32; 4],
    ink: [f32; 4],
    lines: [f32; 4],
}

struct Offscreen {
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    w: u32,
    h: u32,
    /// the glow at half size: bright pixels land in `bloom_a`, blur across into `bloom_b`, then down into `bloom_a`
    bloom_a: wgpu::TextureView,
    bloom_b: wgpu::TextureView,
    bright_bind: wgpu::BindGroup,
    across_bind: wgpu::BindGroup,
    down_bind: wgpu::BindGroup,
    bw: u32,
    bh: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    viewport: [f32; 2],
    text: [f32; 2],
}

/// A GPU buffer of instances that grows as needed.
struct InstanceBuf {
    buf: wgpu::Buffer,
    cap: u64,
    label: &'static str,
}

impl InstanceBuf {
    fn new(device: &wgpu::Device, label: &'static str) -> InstanceBuf {
        let cap = 64 * 1024;
        InstanceBuf { buf: Self::alloc(device, label, cap), cap, label }
    }
    fn alloc(device: &wgpu::Device, label: &'static str, cap: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: cap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    }
    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let need = data.len() as u64;
        if need > self.cap {
            self.cap = need.next_power_of_two();
            self.buf = Self::alloc(device, self.label, self.cap);
        }
        queue.write_buffer(&self.buf, 0, data);
    }
}

struct GpuImage {
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    w: u32,
    h: u32,
}

struct AtlasTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: u32,
    generation: u64,
}

/// The swapchain rebuilds the renderer does by itself to get frames again (after a stall or an outdated swapchain; a
/// resize's isn't one). Each hands the driver a new set of window-sized images. A Wayland window off screen never
/// gets an image, and rebuilding for it once a second for hours is the likeliest way bro 2.1.4 ran a machine with
/// NVIDIA's driver out of memory. So after the first, each waits twice as long as the one before (1 s, 2 s, 4 s ...
/// a minute), until ten quiet minutes or a resize start the count over.
#[derive(Default)]
struct Rebuilds {
    /// rebuilds in this run of trouble
    count: u32,
    last: Option<Instant>,
}

impl Rebuilds {
    const MAX_WAIT: Duration = Duration::from_secs(60);
    const QUIET: Duration = Duration::from_secs(600);

    /// Whether a rebuild may go at `now` (it's counted when it may).
    fn allow(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last {
            let since = now.saturating_duration_since(last);
            if since >= Self::QUIET {
                self.count = 0;
            }
            let wait = match self.count {
                0 => Duration::ZERO,
                n => Duration::from_secs(1 << (n - 1).min(6)).min(Self::MAX_WAIT),
            };
            if since < wait {
                return false;
            }
        }
        self.count += 1;
        self.last = Some(now);
        true
    }
}

/// The GPU to draw with: the one `WGPU_POWER_PREF` asks for; else the one driving the window's display (`display`,
/// see [`crate::display`]) when it can draw to `surface`; else the low-power one (a laptop's integrated GPU, which
/// usually drives its own screen). Of a GPU's backends the usual first (Vulkan, then DX12) wins, as below.
fn pick_adapter(instance: &wgpu::Instance, surface: &wgpu::Surface<'_>, display: Option<(u32, u32)>) -> Result<wgpu::Adapter, String> {
    let preference = wgpu::PowerPreference::from_env();
    if preference.is_none()
        && let Some((vendor, device)) = display
    {
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        if let Some(a) = adapters.into_iter().find(|a| {
            let info = a.get_info();
            info.vendor == vendor && info.device == device && a.is_surface_supported(surface)
        }) {
            return Ok(a);
        }
    }
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: preference.unwrap_or(wgpu::PowerPreference::LowPower),
        compatible_surface: Some(surface),
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .map_err(|e| format!("no graphics adapter: {e}"))
}

/// Why a frame wasn't drawn.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// window minimised or hidden: try again later
    Skipped,
    /// the surface was reconfigured: draw again
    Reconfigured,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    view_format: wgpu::TextureFormat,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    quad_pipe: wgpu::RenderPipeline,
    /// quads painted into the window's glass (see [`crate::scene::Layer::glass`])
    glass_pipe: wgpu::RenderPipeline,
    shadow_pipe: wgpu::RenderPipeline,
    glyph_pipe: wgpu::RenderPipeline,
    /// glyphs with subpixel text among them (dual-source blending), when the GPU has it
    lcd_pipe: Option<wgpu::RenderPipeline>,
    image_pipe: wgpu::RenderPipeline,
    atlas_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    mask: AtlasTexture,
    color: AtlasTexture,
    atlas_bind: wgpu::BindGroup,
    quads: InstanceBuf,
    shadows: InstanceBuf,
    glyphs: InstanceBuf,
    images: InstanceBuf,
    textures: HashMap<ImageId, GpuImage>,
    next_image: u64,
    post_pipe: wgpu::RenderPipeline,
    post_layout: wgpu::BindGroupLayout,
    post_uniform: wgpu::Buffer,
    bright_pipe: wgpu::RenderPipeline,
    blur_pipe: wgpu::RenderPipeline,
    bloom_layout: wgpu::BindGroupLayout,
    /// the bright pass's, the blur across's and the blur down's
    bloom_uniforms: [wgpu::Buffer; 3],
    offscreen: Option<Offscreen>,
    /// the swap chain carries alpha: the window shows through where the frame is see-through
    see_through: bool,
    /// Whole-screen effects for the next frames.
    pub effects: Effects,
    /// Glyph coverage exponents: (light text, dark text). Below 1 thickens, above 1 thins.
    pub text_gamma: [f32; 2],
    pub adapter_info: wgpu::AdapterInfo,
    /// The window was minimised (a zero size): its swapchain is rebuilt when it comes back, even at the old size.
    hidden: bool,
    /// Frames in a row whose swapchain image never came (each waits wgpu's 1 s acquire timeout).
    stalls: u32,
    rebuilds: Rebuilds,
    /// The swapchain was rebuilt for being suboptimal at this size: one the driver still calls that is drawn to as it is.
    suboptimal_rebuilt: bool,
}

impl Renderer {
    /// Set up the GPU for `target` (a window) of `width`×`height` physical pixels.
    pub fn new(target: impl Into<wgpu::SurfaceTarget<'static>>, width: u32, height: u32, vsync: bool) -> Result<Renderer, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        Self::build(instance, target, width, height, Options { vsync, transparent: false })
    }

    /// [`Renderer::new`] with more say: a see-through window. When the see-through set-up can't start (no DX12
    /// on this PC), the usual one is used instead.
    pub fn with_options(target: impl Into<wgpu::SurfaceTarget<'static>> + Clone, width: u32, height: u32, options: Options) -> Result<Renderer, String> {
        let usual = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        // Windows composes a window's alpha only from a DirectComposition swap chain: DX12's (unless WGPU_BACKEND
        // asks for another API)
        if options.transparent && cfg!(windows) && std::env::var_os("WGPU_BACKEND").is_none() {
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
            desc.backends = wgpu::Backends::DX12;
            desc.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
            match Self::build(wgpu::Instance::new(desc), target.clone(), width, height, options) {
                Ok(r) => return Ok(r),
                Err(e) => log::warn!("gpu: no see-through swap chain ({e}), drawing opaque"),
            }
        }
        Self::build(wgpu::Instance::new(usual), target, width, height, options)
    }

    fn build(instance: wgpu::Instance, target: impl Into<wgpu::SurfaceTarget<'static>>, width: u32, height: u32, options: Options) -> Result<Renderer, String> {
        let vsync = options.vsync;
        let target = target.into();
        let display = crate::display::gpu_of_target(&target);
        let surface = instance.create_surface(target).map_err(|e| format!("couldn't create a drawing surface: {e}"))?;
        let adapter = pick_adapter(&instance, &surface, display)?;
        let adapter_info = adapter.get_info();
        // subpixel text blends each colour channel by its own coverage: two outputs into one target
        let dual_source = adapter.features() & wgpu::Features::DUAL_SOURCE_BLENDING;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("bro"),
            required_features: dual_source,
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .map_err(|e| format!("couldn't open the graphics device: {e}"))?;
        // a validation slip costs a frame, not the app: wgpu's default is to panic
        device.on_uncaptured_error(std::sync::Arc::new(|e: wgpu::Error| log::error!("wgpu: {e}")));

        let caps = surface.get_capabilities(&adapter);
        // blend in sRGB space (like browsers): a non-sRGB view of the swapchain
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let view_format = format.remove_srgb_suffix();
        let present_mode = if vsync {
            wgpu::PresentMode::Fifo
        } else if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Immediate
        };
        // see-through: the frame is premultiplied (Core Animation composes Metal's "post-multiplied" layers that way too)
        let see_through_mode = options.transparent.then(|| [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied].into_iter().find(|m| caps.alpha_modes.contains(m))).flatten();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: Default::default(),
            width: width.max(1),
            height: height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: see_through_mode.or(caps.alpha_modes.first().copied()).unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: if view_format != format { vec![view_format] } else { vec![] },
        };
        surface.configure(&device, &config);
        let see_through = see_through_mode.is_some();

        let globals = device.create_buffer(&wgpu::BufferDescriptor { label: Some("globals"), size: std::mem::size_of::<Globals>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("globals"), layout: &globals_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }] });

        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None };
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("atlas"), entries: &[tex_entry(0), tex_entry(1), sampler_entry(2)] });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("image"), entries: &[tex_entry(0), sampler_entry(1)] });

        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let nearest = sampler(wgpu::FilterMode::Nearest);
        let linear = sampler(wgpu::FilterMode::Linear);

        let module = |name: &str, src: &str| device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: wgpu::ShaderSource::Wgsl(format!("{COMMON}\n{src}").into()) });
        let quad_mod = module("quad", QUAD);
        let shadow_mod = module("shadow", SHADOW);
        let glyph_mod = module("glyph", GLYPH);
        let image_mod = module("image", IMAGE);

        let layout = |name: &str, groups: &[Option<&wgpu::BindGroupLayout>]| device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(name), bind_group_layouts: groups, immediate_size: 0 });
        let plain_layout = layout("plain", &[Some(&globals_layout)]);
        let atlas_pipe_layout = layout("glyphs", &[Some(&globals_layout), Some(&atlas_layout)]);
        let image_pipe_layout = layout("images", &[Some(&globals_layout), Some(&image_layout)]);

        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        };
        // glass: premultiplied colour scaled by the destination's alpha, the destination's alpha kept, so the colour
        // changes and the see-through doesn't
        let glass_blend = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::DstAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        // premultiplied colour, each channel over the destination by its own coverage (the second output)
        let lcd_blend = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrc1, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrc1Alpha, operation: wgpu::BlendOperation::Add },
        };
        let blended = |name: &str, pl: &wgpu::PipelineLayout, m: &wgpu::ShaderModule, vs: &str, fs: &str, stride: usize, attrs: &[wgpu::VertexAttribute], blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(pl),
                vertex: wgpu::VertexState {
                    module: m,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout { array_stride: stride as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: attrs })],
                },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: m,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: view_format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = |name: &str, pl: &wgpu::PipelineLayout, m: &wgpu::ShaderModule, vs: &str, fs: &str, stride: usize, attrs: &[wgpu::VertexAttribute]| blended(name, pl, m, vs, fs, stride, attrs, blend);
        let quad_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4];
        let shadow_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4];
        let glyph_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4];
        let image_attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
        let quad_pipe = pipeline("quads", &plain_layout, &quad_mod, "vs_quad", "fs_quad", std::mem::size_of::<QuadInstance>(), &quad_attrs);
        let glass_pipe = blended("glass", &plain_layout, &quad_mod, "vs_quad", "fs_quad_glass", std::mem::size_of::<QuadInstance>(), &quad_attrs, glass_blend);
        let shadow_pipe = pipeline("shadows", &plain_layout, &shadow_mod, "vs_shadow", "fs_shadow", std::mem::size_of::<ShadowInstance>(), &shadow_attrs);
        let glyph_pipe = pipeline("glyphs", &atlas_pipe_layout, &glyph_mod, "vs_glyph", "fs_glyph", std::mem::size_of::<GlyphInstance>(), &glyph_attrs);
        let lcd_pipe = (!dual_source.is_empty()).then(|| {
            let src = format!("enable dual_source_blending;\n{COMMON}\n{GLYPH}\n{GLYPH_LCD}");
            let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("glyph-lcd"), source: wgpu::ShaderSource::Wgsl(src.into()) });
            blended("glyphs-lcd", &atlas_pipe_layout, &m, "vs_glyph", "fs_glyph_lcd", std::mem::size_of::<GlyphInstance>(), &glyph_attrs, lcd_blend)
        });
        let image_pipe = pipeline("images", &image_pipe_layout, &image_mod, "vs_image", "fs_image", std::mem::size_of::<ImageInstance>(), &image_attrs);

        // the effects pass: the drawn frame and its glow in, the swapchain out
        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("post"), entries: &[tex_entry(0), sampler_entry(1), uniform_entry(2), tex_entry(3), sampler_entry(4)] });
        let post_pipe_layout = layout("post", &[Some(&globals_layout), Some(&post_layout)]);
        let post_mod = module("post", POST);
        let post_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post"),
            layout: Some(&post_pipe_layout),
            vertex: wgpu::VertexState { module: &post_mod, entry_point: Some("vs_post"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &post_mod,
                entry_point: Some("fs_post"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: view_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let post_uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post"), size: std::mem::size_of::<PostUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        // the glow: a bright pass at half size, then a blur across and one down
        let bloom_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("bloom"), entries: &[tex_entry(0), sampler_entry(1), uniform_entry(2)] });
        let bloom_pipe_layout = layout("bloom", &[Some(&globals_layout), Some(&bloom_layout)]);
        let bloom_mod = module("bloom", BLOOM);
        let fullscreen = |name: &str, fs: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(&bloom_pipe_layout),
                vertex: wgpu::VertexState { module: &bloom_mod, entry_point: Some("vs_bloom"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &bloom_mod,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: BLOOM_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let bright_pipe = fullscreen("bloom-bright", "fs_bright");
        let blur_pipe = fullscreen("bloom-blur", "fs_blur");
        let bloom_uniform = || device.create_buffer(&wgpu::BufferDescriptor { label: Some("bloom"), size: std::mem::size_of::<BloomUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bloom_uniforms = [bloom_uniform(), bloom_uniform(), bloom_uniform()];

        let mask = Self::atlas_texture(&device, Kind::Mask, 2048, 0);
        let color = Self::atlas_texture(&device, Kind::Color, 1024, 0);
        let atlas_bind = Self::atlas_bind(&device, &atlas_layout, &mask, &color, &nearest);
        Ok(Renderer {
            quads: InstanceBuf::new(&device, "quads"),
            shadows: InstanceBuf::new(&device, "shadows"),
            glyphs: InstanceBuf::new(&device, "glyphs"),
            images: InstanceBuf::new(&device, "images"),
            device,
            queue,
            surface,
            config,
            view_format,
            globals,
            globals_bind,
            quad_pipe,
            glass_pipe,
            shadow_pipe,
            glyph_pipe,
            lcd_pipe,
            image_pipe,
            atlas_layout,
            image_layout,
            nearest,
            linear,
            mask,
            color,
            atlas_bind,
            textures: HashMap::new(),
            next_image: 1,
            post_pipe,
            post_layout,
            post_uniform,
            bright_pipe,
            blur_pipe,
            bloom_layout,
            bloom_uniforms,
            offscreen: None,
            see_through,
            effects: Effects::default(),
            text_gamma: std::env::var("BRO_TEXT_GAMMA")
                .ok()
                .and_then(|v| {
                    let (a, b) = v.split_once(',')?;
                    Some([a.trim().parse().ok()?, b.trim().parse().ok()?])
                })
                .unwrap_or([0.82, 1.12]),
            adapter_info,
            hidden: false,
            stalls: 0,
            rebuilds: Rebuilds::default(),
            suboptimal_rebuilt: false,
        })
    }

    /// Whether this GPU draws subpixel text ([`Antialias::Rgb`] / [`Antialias::Bgr`]); without it such glyphs
    /// draw greyscale.
    pub fn subpixel_text(&self) -> bool {
        self.lcd_pipe.is_some()
    }

    /// The window shows through where the frame is see-through (a [`Scene::background`] with alpha below 1):
    /// asked for with [`Options::transparent`], and the system could.
    pub fn see_through(&self) -> bool {
        self.see_through
    }

    fn atlas_texture(device: &wgpu::Device, kind: Kind, size: u32, generation: u64) -> AtlasTexture {
        let format = match kind {
            Kind::Mask => wgpu::TextureFormat::R8Unorm,
            Kind::Color | Kind::Lcd => wgpu::TextureFormat::Rgba8Unorm,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        AtlasTexture { texture, view, size, generation }
    }

    fn atlas_bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, mask: &AtlasTexture, color: &AtlasTexture, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&mask.view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&color.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        })
    }

    /// The drawing area changed size (physical pixels).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.hidden = true;
            return;
        }
        // restored from the taskbar at the same size: a fresh swapchain anyway (Intel's Vulkan driver can keep
        // the old one starved of images for good, and the window looks hung)
        if width == self.config.width && height == self.config.height && !std::mem::take(&mut self.hidden) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.rebuilds = Rebuilds::default();
        self.suboptimal_rebuilt = false;
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn set_vsync(&mut self, on: bool) {
        let mode = if on { wgpu::PresentMode::Fifo } else { wgpu::PresentMode::Immediate };
        if self.config.present_mode != mode {
            self.config.present_mode = mode;
            self.surface.configure(&self.device, &self.config);
        }
    }

    // ------------------------------------------------------------------ images

    /// Upload an RGBA image (straight alpha, sRGB); returns its id for [`Scene::image`].
    pub fn create_image(&mut self, w: u32, h: u32, rgba: &[u8]) -> ImageId {
        let id = ImageId(self.next_image);
        self.next_image += 1;
        self.put_image(id, w, h, rgba);
        id
    }

    /// Replace an image's pixels (video frames): reuses the texture when the size is unchanged.
    pub fn update_image(&mut self, id: ImageId, w: u32, h: u32, rgba: &[u8]) {
        match self.textures.get(&id) {
            Some(t) if t.w == w && t.h == h => self.write_image(&t.texture, w, h, rgba),
            _ => self.put_image(id, w, h, rgba),
        }
    }

    pub fn remove_image(&mut self, id: ImageId) {
        self.textures.remove(&id);
    }

    pub fn image_size(&self, id: ImageId) -> Option<(u32, u32)> {
        self.textures.get(&id).map(|t| (t.w, t.h))
    }

    fn put_image(&mut self, id: ImageId, w: u32, h: u32, rgba: &[u8]) {
        let (w, h) = (w.max(1), h.max(1));
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.write_image(&texture, w, h, rgba);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image"),
            layout: &self.image_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.linear) }],
        });
        self.textures.insert(id, GpuImage { texture, bind, w, h });
    }

    fn write_image(&self, texture: &wgpu::Texture, w: u32, h: u32, rgba: &[u8]) {
        if rgba.len() < (w * h * 4) as usize {
            return;
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &rgba[..(w * h * 4) as usize],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    // ------------------------------------------------------------------ atlases

    fn sync_atlas(&mut self, text: &mut Text) {
        let (mask, color) = text.atlases_mut();
        let mut rebind = false;
        for (atlas, tex) in [(mask, &mut self.mask), (color, &mut self.color)] {
            if atlas.size != tex.size || atlas.generation != tex.generation {
                *tex = Self::atlas_texture(&self.device, atlas.kind, atlas.size, atlas.generation);
                rebind = true;
            }
            Self::upload(&self.queue, atlas, tex);
        }
        if rebind {
            self.atlas_bind = Self::atlas_bind(&self.device, &self.atlas_layout, &self.mask, &self.color, &self.nearest);
        }
    }

    fn upload(queue: &wgpu::Queue, atlas: &mut Atlas, tex: &AtlasTexture) {
        let bpp = atlas.kind.bytes_per_pixel() as u32;
        for u in atlas.uploads.drain(..) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex.texture, mip_level: 0, origin: wgpu::Origin3d { x: u.x, y: u.y, z: 0 }, aspect: wgpu::TextureAspect::All },
                &u.data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(u.w * bpp), rows_per_image: Some(u.h) },
                wgpu::Extent3d { width: u.w, height: u.h, depth_or_array_layers: 1 },
            );
        }
    }

    // ------------------------------------------------------------------ frame

    /// Draw `scene` and present it.
    pub fn render(&mut self, scene: &Scene, text: &mut Text) -> Result<(), FrameError> {
        self.render_with(scene, text, || {})
    }

    /// [`Renderer::render`], calling `pre_present` just before the frame goes to the window: winit's
    /// `Window::pre_present_notify`, which on Wayland holds the next redraw until the compositor wants a frame (and
    /// none comes while the window is off screen).
    pub fn render_with(&mut self, scene: &Scene, text: &mut Text, pre_present: impl FnOnce()) -> Result<(), FrameError> {
        self.sync_atlas(text);
        let lcd = text.antialias() != Antialias::Grayscale;
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => (f, false),
            // draw this one; rebuild once it's presented (never while a frame is held)
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => (f, true),
            wgpu::CurrentSurfaceTexture::Occluded => return self.skip(),
            // no image for a whole second: the swapchain is wedged (seen on Intel Vulkan after the window was
            // minimised or a display changed) and waiting on it again would freeze the window for good, so
            // build a new one, which is what a resize does and what brings such a window back
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.stalls += 1;
                return self.rebuild(&format!("no swapchain image within 1 s ({} in a row)", self.stalls));
            }
            other => return self.rebuild(&format!("{other:?}")),
        };
        self.stalls = 0;
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(self.view_format), ..Default::default() });
        self.compose(&view, scene, lcd);
        pre_present();
        self.queue.present(frame);
        // once per size: a fresh swapchain the driver still calls suboptimal is drawn to as it is
        if suboptimal && !std::mem::replace(&mut self.suboptimal_rebuilt, true) {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(())
    }

    /// No frame this time: what was written for it (glyphs, a video frame) goes to the GPU now, not held for a frame
    /// that may be a long time coming.
    fn skip(&mut self) -> Result<(), FrameError> {
        self.queue.submit([]);
        Err(FrameError::Skipped)
    }

    /// No image from the swapchain: build a new one, unless the last rebuilds didn't help and their back-off hasn't
    /// run out (see [`Rebuilds`]).
    fn rebuild(&mut self, why: &str) -> Result<(), FrameError> {
        if !self.rebuilds.allow(Instant::now()) {
            return self.skip();
        }
        log::warn!("gpu: {why}, rebuilding the swapchain");
        self.surface.configure(&self.device, &self.config);
        self.queue.submit([]);
        Err(FrameError::Reconfigured)
    }

    /// Draw `scene` into `view`: straight there, or through the offscreen frame and the effects pass (and the glow's
    /// passes) when a theme has effects on.
    fn compose(&mut self, view: &wgpu::TextureView, scene: &Scene, lcd: bool) {
        if !self.effects.active() {
            self.draw_into(view, scene, lcd);
            return;
        }
        self.ensure_offscreen();
        let Some(off) = self.offscreen.as_ref().map(|o| o.view.clone()) else {
            self.draw_into(view, scene, lcd);
            return;
        };
        self.draw_into(&off, scene, lcd);
        if self.effects.glow > 0.0 {
            self.bloom_pass();
        }
        self.post_pass(view);
    }

    /// An offscreen frame the size of the window, and the glow's half-size pair (for the effects pass).
    fn ensure_offscreen(&mut self) {
        let (w, h) = (self.config.width, self.config.height);
        if self.offscreen.as_ref().is_some_and(|o| o.w == w && o.h == h) {
            return;
        }
        let texture = |label: &str, w: u32, h: u32, format: wgpu::TextureFormat| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let view = texture("offscreen", w, h, self.view_format);
        let (bw, bh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
        let bloom_a = texture("bloom-a", bw, bh, BLOOM_FORMAT);
        let bloom_b = texture("bloom-b", bw, bh, BLOOM_FORMAT);
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.post_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.nearest) },
                wgpu::BindGroupEntry { binding: 2, resource: self.post_uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&bloom_a) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.linear) },
            ],
        });
        let bloom_bind = |src: &wgpu::TextureView, uniform: &wgpu::Buffer| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom"),
                layout: &self.bloom_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.linear) },
                    wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
                ],
            })
        };
        let bright_bind = bloom_bind(&view, &self.bloom_uniforms[0]);
        let across_bind = bloom_bind(&bloom_a, &self.bloom_uniforms[1]);
        let down_bind = bloom_bind(&bloom_b, &self.bloom_uniforms[2]);
        self.offscreen = Some(Offscreen { view, bind, w, h, bloom_a, bloom_b, bright_bind, across_bind, down_bind, bw, bh });
    }

    /// The glow of the offscreen frame: its neon at half size, blurred across then down, left in `bloom_a`.
    fn bloom_pass(&mut self) {
        let Some(off) = &self.offscreen else { return };
        // the blur's sigma in half-size pixels; past ~3 taps its 17 taps spread out (linear filtering fills in)
        let sigma = (self.effects.glow_px / 2.0).max(0.75);
        let spacing = (sigma / 2.75).max(1.0);
        let put = |buf: &wgpu::Buffer, step: [f32; 4]| self.queue.write_buffer(buf, 0, bytemuck::bytes_of(&BloomUniform { step }));
        put(&self.bloom_uniforms[0], [0.0; 4]);
        put(&self.bloom_uniforms[1], [spacing / off.bw as f32, 0.0, sigma / spacing, 0.0]);
        put(&self.bloom_uniforms[2], [0.0, spacing / off.bh as f32, sigma / spacing, 0.0]);
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bloom") });
        for (pipe, bind, target) in [(&self.bright_pipe, &off.bright_bind, &off.bloom_a), (&self.blur_pipe, &off.across_bind, &off.bloom_b), (&self.blur_pipe, &off.down_bind, &off.bloom_a)] {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(1, bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }

    /// Draw the offscreen frame into `target` through the effects.
    fn post_pass(&mut self, target: &wgpu::TextureView) {
        let Some(off) = &self.offscreen else { return };
        let e = self.effects;
        let (ground, ink, on) = match e.phosphor {
            Some((g, i)) => (g, i, 1.0),
            None => ([0.0; 3], [1.0; 3], 0.0),
        };
        // the glow's half-size average dims thin strokes: brought back up here
        let glow = e.glow.max(0.0) * 2.6;
        let u = PostUniform { ground: [ground[0], ground[1], ground[2], on], ink: [ink[0], ink[1], ink[2], e.vignette], lines: [e.scanlines, e.scanline_px.max(1.0), glow, 0.0] };
        self.queue.write_buffer(&self.post_uniform, 0, bytemuck::bytes_of(&u));
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("post") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.post_pipe);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_bind_group(1, &off.bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }

    /// Draw `scene` into an offscreen texture and read it back as RGBA rows (screenshots and tests).
    pub fn capture(&mut self, scene: &Scene, text: &mut Text) -> Option<(u32, u32, Vec<u8>)> {
        self.sync_atlas(text);
        let lcd = text.antialias() != Antialias::Grayscale;
        let (w, h) = (self.config.width, self.config.height);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.view_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.compose(&view, scene, lcd);
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("capture") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let bgra = matches!(self.view_format, wgpu::TextureFormat::Bgra8Unorm);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let start = (y * row) as usize;
            let line = &data[start..start + (w * 4) as usize];
            if bgra {
                for px in line.as_chunks::<4>().0 {
                    out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            } else {
                out.extend_from_slice(line);
            }
        }
        // a see-through frame is premultiplied: pictures want the colour without the alpha in it
        if self.see_through {
            for px in out.as_chunks_mut::<4>().0 {
                let a = px[3] as u32;
                if a > 0 && a < 255 {
                    for c in &mut px[..3] {
                        *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                    }
                }
            }
        }
        drop(data);
        buf.unmap();
        Some((w, h, out))
    }

    /// Draw the scene's layers into `view`: per layer glass, shadows, quads, images, quads over the images,
    /// glyphs, top quads, then top glass. `lcd`: the glyphs include subpixel text (drawn by the dual-source
    /// pipeline when there is one).
    fn draw_into(&mut self, view: &wgpu::TextureView, scene: &Scene, lcd: bool) {
        let glyph_pipe = match &self.lcd_pipe {
            Some(p) if lcd => p,
            _ => &self.glyph_pipe,
        };
        let g = Globals { viewport: [self.config.width as f32, self.config.height as f32], text: self.text_gamma };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        // premultiplied, so the blending keeps the frame premultiplied; opaque unless the window can show through
        let bg = scene.background;
        let a = if self.see_through { bg.a.clamp(0.0, 1.0) as f64 } else { 1.0 };
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let layers = scene.used_layers();
        let mut quads: Vec<QuadInstance> = Vec::new();
        let mut shadows: Vec<ShadowInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();
        let mut images: Vec<ImageInstance> = Vec::new();
        let mut plan = vec![];
        for l in layers {
            let gl0 = quads.len() as u32..(quads.len() + l.glass.len()) as u32;
            quads.extend_from_slice(&l.glass);
            let s = shadows.len() as u32..(shadows.len() + l.shadows.len()) as u32;
            shadows.extend_from_slice(&l.shadows);
            let q = quads.len() as u32..(quads.len() + l.quads.len()) as u32;
            quads.extend_from_slice(&l.quads);
            let mut im: Vec<(ImageId, std::ops::Range<u32>)> = vec![];
            for (id, inst) in &l.images {
                let i = images.len() as u32;
                images.push(*inst);
                match im.last_mut() {
                    Some((last, r)) if last == id && r.end == i => r.end = i + 1,
                    _ => im.push((*id, i..i + 1)),
                }
            }
            let m = quads.len() as u32..(quads.len() + l.mid.len()) as u32;
            quads.extend_from_slice(&l.mid);
            let gl = glyphs.len() as u32..(glyphs.len() + l.glyphs.len()) as u32;
            glyphs.extend_from_slice(&l.glyphs);
            let t = quads.len() as u32..(quads.len() + l.top.len()) as u32;
            quads.extend_from_slice(&l.top);
            let tg = quads.len() as u32..(quads.len() + l.top_glass.len()) as u32;
            quads.extend_from_slice(&l.top_glass);
            plan.push((gl0, s, q, im, m, gl, t, tg));
        }
        self.quads.write(&self.device, &self.queue, bytemuck::cast_slice(&quads));
        self.shadows.write(&self.device, &self.queue, bytemuck::cast_slice(&shadows));
        self.glyphs.write(&self.device, &self.queue, bytemuck::cast_slice(&glyphs));
        self.images.write(&self.device, &self.queue, bytemuck::cast_slice(&images));
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: bg.r as f64 * a, g: bg.g as f64 * a, b: bg.b as f64 * a, a }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);
            for (gl0, s, q, im, m, gl, t, tg) in plan {
                if !gl0.is_empty() {
                    pass.set_pipeline(&self.glass_pipe);
                    pass.set_vertex_buffer(0, self.quads.buf.slice(..));
                    pass.draw(0..4, gl0);
                }
                if !s.is_empty() {
                    pass.set_pipeline(&self.shadow_pipe);
                    pass.set_vertex_buffer(0, self.shadows.buf.slice(..));
                    pass.draw(0..4, s);
                }
                if !q.is_empty() {
                    pass.set_pipeline(&self.quad_pipe);
                    pass.set_vertex_buffer(0, self.quads.buf.slice(..));
                    pass.draw(0..4, q);
                }
                if !im.is_empty() {
                    pass.set_pipeline(&self.image_pipe);
                    pass.set_vertex_buffer(0, self.images.buf.slice(..));
                    for (id, range) in im {
                        if let Some(tex) = self.textures.get(&id) {
                            pass.set_bind_group(1, &tex.bind, &[]);
                            pass.draw(0..4, range);
                        }
                    }
                }
                if !m.is_empty() {
                    pass.set_pipeline(&self.quad_pipe);
                    pass.set_vertex_buffer(0, self.quads.buf.slice(..));
                    pass.draw(0..4, m);
                }
                if !gl.is_empty() {
                    pass.set_pipeline(glyph_pipe);
                    pass.set_bind_group(1, &self.atlas_bind, &[]);
                    pass.set_vertex_buffer(0, self.glyphs.buf.slice(..));
                    pass.draw(0..4, gl);
                }
                if !t.is_empty() {
                    pass.set_pipeline(&self.quad_pipe);
                    pass.set_vertex_buffer(0, self.quads.buf.slice(..));
                    pass.draw(0..4, t);
                }
                if !tg.is_empty() {
                    pass.set_pipeline(&self.glass_pipe);
                    pass.set_vertex_buffer(0, self.quads.buf.slice(..));
                    pass.draw(0..4, tg);
                }
            }
        }
        self.queue.submit([enc.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_back_off_until_quiet() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut r = Rebuilds::default();
        // the first goes at once, the next after a second, then two, four ...
        assert!(r.allow(at(0.0)));
        assert!(!r.allow(at(0.5)));
        assert!(r.allow(at(1.0)));
        assert!(!r.allow(at(2.9)));
        assert!(r.allow(at(3.0)));
        assert!(!r.allow(at(6.9)));
        assert!(r.allow(at(7.0)));
        // a window that never gets an image: a minute apart at most, not once a second
        let mut t = 7.0;
        let mut n = 0;
        while t < 3600.0 {
            t += 1.0;
            if r.allow(at(t)) {
                n += 1;
            }
        }
        assert!(n <= 70, "{n} rebuilds in an hour");
        // ten quiet minutes: the next one goes at once again
        assert!(r.allow(at(t + 600.0)));
        assert!(!r.allow(at(t + 600.5)));
    }
}
