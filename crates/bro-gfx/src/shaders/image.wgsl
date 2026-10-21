// Images (photos, video frames, logos) with rounded corners, opacity and an optional greyscale.

@group(1) @binding(0) var image_tex: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

struct ImageIn {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) clip: vec4<f32>,
    // radius, opacity, clip radius, grey
    @location(3) params: vec4<f32>,
};

struct ImageOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) rect: vec4<f32>,
    @location(3) @interpolate(flat) clip: vec4<f32>,
    @location(4) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs_image(@builtin(vertex_index) vi: u32, i: ImageIn) -> ImageOut {
    let c = corner(vi);
    let p = i.rect.xy + c * i.rect.zw;
    var o: ImageOut;
    o.pos = to_clip(p);
    o.p = p;
    o.uv = mix(i.uv.xy, i.uv.zw, c);
    o.rect = i.rect;
    o.clip = i.clip;
    o.params = i.params;
    return o;
}

@fragment
fn fs_image(o: ImageOut) -> @location(0) vec4<f32> {
    let clip = clip_alpha(o.p, o.clip, o.params.z);
    if (clip <= 0.0) {
        discard;
    }
    var c = textureSample(image_tex, image_sampler, o.uv);
    let r = o.params.x;
    var edge = 1.0;
    if (r > 0.0) {
        let half = o.rect.zw * 0.5;
        let d = sd_rounded_box(o.p - (o.rect.xy + half), half, vec4<f32>(r, r, r, r));
        edge = clamp(0.5 - d, 0.0, 1.0);
    }
    if (o.params.w > 0.0) {
        let g = dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        c = vec4<f32>(mix(c.rgb, vec3<f32>(g, g, g), o.params.w), c.a);
    }
    return vec4<f32>(c.rgb, c.a * o.params.y * edge * clip);
}
