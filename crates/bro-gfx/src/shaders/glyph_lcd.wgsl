// Subpixel text, drawn with dual-source blending: each colour channel blends by its own coverage, so a glyph's
// edges land on the screen's red, green and blue stripes. Masks and colour glyphs blend as they do in the plain
// glyph pass (premultiplied here, the same result). Appended to glyph.wgsl.

struct LcdOut {
    @location(0) @blend_src(0) color: vec4<f32>,
    @location(0) @blend_src(1) cover: vec4<f32>,
};

@fragment
fn fs_glyph_lcd(o: GlyphOut) -> LcdOut {
    let clip = clip_alpha(o.p, o.clip, o.params.y);
    if (clip <= 0.0) {
        discard;
    }
    let fade = o.color.a * clip;
    var out: LcdOut;
    if (o.params.x > 1.5) {
        let m = textureSampleLevel(color_tex, atlas_sampler, o.uv, 0.0).rgb;
        let cov = pow(m, vec3<f32>(text_exponent(o.color.rgb))) * fade;
        let a = max(cov.r, max(cov.g, cov.b));
        out.color = vec4<f32>(o.color.rgb * cov, a);
        out.cover = vec4<f32>(cov, a);
        return out;
    }
    if (o.params.x > 0.5) {
        let c = textureSampleLevel(color_tex, atlas_sampler, o.uv, 0.0);
        let a = c.a * fade;
        out.color = vec4<f32>(c.rgb * a, a);
        out.cover = vec4<f32>(a);
        return out;
    }
    let coverage = textureSampleLevel(mask_tex, atlas_sampler, o.uv, 0.0).r;
    let a = pow(coverage, text_exponent(o.color.rgb)) * fade;
    out.color = vec4<f32>(o.color.rgb * a, a);
    out.cover = vec4<f32>(a);
    return out;
}
