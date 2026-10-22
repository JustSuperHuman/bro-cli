//! The wgpu side: device and surface, the pipelines (shadows, quads, images, glyphs, and subpixel glyphs where
//! the GPU blends two sources), the glyph atlases' textures and the image textures. [`Renderer::render`] draws a
//! [`Scene`] and presents it.

use crate::atlas::{Atlas, Kind};
use crate::scene::{GlyphInstance, ImageId, ImageInstance, QuadInstance, Scene, ShadowInstance};
use crate::text::{Antialias, Text};
use std::collections::HashMap;

const COMMON: &str = include_str!("shaders/common.wgsl");
const QUAD: &str = include_str!("shaders/quad.wgsl");
const SHADOW: &str = include_str!("shaders/shadow.wgsl");
const GLYPH: &str = include_str!("shaders/glyph.wgsl");
const GLYPH_LCD: &str = include_str!("shaders/glyph_lcd.wgsl");
const IMAGE: &str = include_str!("shaders/image.wgsl");
const POST: &str = include_str!("shaders/post.wgsl");

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
}

impl Effects {
    pub fn active(&self) -> bool {
        self.phosphor.is_some() || self.vignette > 0.0 || self.scanlines > 0.0
    }
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
    offscreen: Option<Offscreen>,
    /// Whole-screen effects for the next frames.
    pub effects: Effects,
    /// Glyph coverage exponents: (light text, dark text). Below 1 thickens, above 1 thins.
    pub text_gamma: [f32; 2],
    pub adapter_info: wgpu::AdapterInfo,
    /// The window was minimised (a zero size): its swapchain is rebuilt when it comes back, even at the old size.
    hidden: bool,
    /// Frames in a row whose swapchain image never came (each waits wgpu's 1 s acquire timeout).
    stalls: u32,
}

impl Renderer {
    /// Set up the GPU for `target` (a window) of `width`×`height` physical pixels.
    pub fn new(target: impl Into<wgpu::SurfaceTarget<'static>>, width: u32, height: u32, vsync: bool) -> Result<Renderer, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(target).map_err(|e| format!("couldn't create a drawing surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no graphics adapter: {e}"))?;
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
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: Default::default(),
            width: width.max(1),
            height: height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: if view_format != format { vec![view_format] } else { vec![] },
        };
        surface.configure(&device, &config);

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
        let shadow_pipe = pipeline("shadows", &plain_layout, &shadow_mod, "vs_shadow", "fs_shadow", std::mem::size_of::<ShadowInstance>(), &shadow_attrs);
        let glyph_pipe = pipeline("glyphs", &atlas_pipe_layout, &glyph_mod, "vs_glyph", "fs_glyph", std::mem::size_of::<GlyphInstance>(), &glyph_attrs);
        let lcd_pipe = (!dual_source.is_empty()).then(|| {
            let src = format!("enable dual_source_blending;\n{COMMON}\n{GLYPH}\n{GLYPH_LCD}");
            let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("glyph-lcd"), source: wgpu::ShaderSource::Wgsl(src.into()) });
            blended("glyphs-lcd", &atlas_pipe_layout, &m, "vs_glyph", "fs_glyph_lcd", std::mem::size_of::<GlyphInstance>(), &glyph_attrs, lcd_blend)
        });
        let image_pipe = pipeline("images", &image_pipe_layout, &image_mod, "vs_image", "fs_image", std::mem::size_of::<ImageInstance>(), &image_attrs);

        // the effects pass: the drawn frame in, the swapchain out
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
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
            offscreen: None,
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
        })
    }

    /// Whether this GPU draws subpixel text ([`Antialias::Rgb`] / [`Antialias::Bgr`]); without it such glyphs
    /// draw greyscale.
    pub fn subpixel_text(&self) -> bool {
        self.lcd_pipe.is_some()
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
        self.sync_atlas(text);
        let lcd = text.antialias() != Antialias::Grayscale;
        let (frame, stale) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => (f, false),
            // draw this one; reconfigure once it's presented (never while a frame is held)
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => (f, true),
            wgpu::CurrentSurfaceTexture::Occluded => return Err(FrameError::Skipped),
            // no image for a whole second: the swapchain is wedged (seen on Intel Vulkan after the window was
            // minimised or a display changed) and waiting on it again would freeze the window for good, so
            // build a new one, which is what a resize does and what brings such a window back
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.stalls += 1;
                log::warn!("gpu: no swapchain image within 1 s ({} in a row), rebuilding the swapchain", self.stalls);
                self.surface.configure(&self.device, &self.config);
                return Err(FrameError::Reconfigured);
            }
            _ => {
                self.surface.configure(&self.device, &self.config);
                return Err(FrameError::Reconfigured);
            }
        };
        self.stalls = 0;
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(self.view_format), ..Default::default() });
        if self.effects.active() {
            self.ensure_offscreen();
            let off = self.offscreen.as_ref().map(|o| o.view.clone());
            if let Some(off) = off {
                self.draw_into(&off, scene, lcd);
                self.post_pass(&view);
            } else {
                self.draw_into(&view, scene, lcd);
            }
        } else {
            self.draw_into(&view, scene, lcd);
        }
        self.queue.present(frame);
        if stale {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(())
    }

    /// An offscreen frame the size of the window (for the effects pass).
    fn ensure_offscreen(&mut self) {
        let (w, h) = (self.config.width, self.config.height);
        if self.offscreen.as_ref().is_some_and(|o| o.w == w && o.h == h) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.view_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.post_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.nearest) },
                wgpu::BindGroupEntry { binding: 2, resource: self.post_uniform.as_entire_binding() },
            ],
        });
        self.offscreen = Some(Offscreen { view, bind, w, h });
    }

    /// Draw the offscreen frame into `target` through the effects.
    fn post_pass(&mut self, target: &wgpu::TextureView) {
        let Some(off) = &self.offscreen else { return };
        let e = self.effects;
        let (ground, ink, on) = match e.phosphor {
            Some((g, i)) => (g, i, 1.0),
            None => ([0.0; 3], [1.0; 3], 0.0),
        };
        let u = PostUniform { ground: [ground[0], ground[1], ground[2], on], ink: [ink[0], ink[1], ink[2], e.vignette], lines: [e.scanlines, e.scanline_px.max(1.0), 0.0, 0.0] };
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
        if self.effects.active() {
            self.ensure_offscreen();
            let off = self.offscreen.as_ref().map(|o| o.view.clone());
            match off {
                Some(off) => {
                    self.draw_into(&off, scene, lcd);
                    self.post_pass(&view);
                }
                None => self.draw_into(&view, scene, lcd),
            }
        } else {
            self.draw_into(&view, scene, lcd);
        }
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
        drop(data);
        buf.unmap();
        Some((w, h, out))
    }

    /// Draw the scene's layers into `view`: per layer shadows, quads, images, quads over the images, glyphs,
    /// then top quads. `lcd`: the glyphs include subpixel text (drawn by the dual-source pipeline when there is
    /// one).
    fn draw_into(&mut self, view: &wgpu::TextureView, scene: &Scene, lcd: bool) {
        let glyph_pipe = match &self.lcd_pipe {
            Some(p) if lcd => p,
            _ => &self.glyph_pipe,
        };
        let g = Globals { viewport: [self.config.width as f32, self.config.height as f32], text: self.text_gamma };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        let bg = scene.background;
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let layers = scene.used_layers();
        let mut quads: Vec<QuadInstance> = Vec::new();
        let mut shadows: Vec<ShadowInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();
        let mut images: Vec<ImageInstance> = Vec::new();
        let mut plan = vec![];
        for l in layers {
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
            plan.push((s, q, im, m, gl, t));
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
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: bg.r as f64, g: bg.g as f64, b: bg.b as f64, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);
            for (s, q, im, m, gl, t) in plan {
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
            }
        }
        self.queue.submit([enc.finish()]);
    }
}
