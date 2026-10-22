// Glyphs from the atlases. Coverage from the mask atlas is gamma-adjusted by text brightness so light text
// on dark backgrounds doesn't look thin and dark text on light backgrounds doesn't look bold (blending
// happens in sRGB space, like browsers).

@group(1) @binding(0) var mask_tex: texture_2d<f32>;
@group(1) @binding(1) var color_tex: texture_2d<f32>;
@group(1) @binding(2) var atlas_sampler: sampler;

struct GlyphIn {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) clip: vec4<f32>,
    // kind (0 mask, 1 colour, 2 subpixel), clip radius
    @location(4) params: vec4<f32>,
};

struct GlyphOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) clip: vec4<f32>,
    @location(4) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs_glyph(@builtin(vertex_index) vi: u32, g: GlyphIn) -> GlyphOut {
    let c = corner(vi);
    let p = g.rect.xy + c * g.rect.zw;
    var o: GlyphOut;
    o.pos = to_clip(p);
    o.p = p;
    o.uv = mix(g.uv.xy, g.uv.zw, c);
    o.color = g.color;
    o.clip = g.clip;
    o.params = g.params;
    return o;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// The exponent coverage is raised to: brighter text gets a boost (below 1), darker text a trim (above 1).
fn text_exponent(c: vec3<f32>) -> f32 {
    return mix(globals.text.y, globals.text.x, smoothstep(0.25, 0.75, luma(c)));
}

@fragment
fn fs_glyph(o: GlyphOut) -> @location(0) vec4<f32> {
    let clip = clip_alpha(o.p, o.clip, o.params.y);
    if (clip <= 0.0) {
        discard;
    }
    if (o.params.x > 1.5) {
        // subpixel coverage without dual-source blending: its average
        let m = textureSampleLevel(color_tex, atlas_sampler, o.uv, 0.0).rgb;
        let a = pow(dot(m, vec3<f32>(1.0 / 3.0)), text_exponent(o.color.rgb));
        return vec4<f32>(o.color.rgb, a * o.color.a * clip);
    }
    if (o.params.x > 0.5) {
        let c = textureSampleLevel(color_tex, atlas_sampler, o.uv, 0.0);
        return vec4<f32>(c.rgb, c.a * o.color.a * clip);
    }
    let coverage = textureSampleLevel(mask_tex, atlas_sampler, o.uv, 0.0).r;
    let a = pow(coverage, text_exponent(o.color.rgb));
    return vec4<f32>(o.color.rgb, a * o.color.a * clip);
}
