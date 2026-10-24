// Shared by every pipeline: the viewport and clipping helpers.

struct Globals {
    // viewport size in physical pixels
    viewport: vec2<f32>,
    // text gamma tweak: x = contrast boost for light text, y = for dark text
    text: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

// Pixel position → clip space.
fn to_clip(p: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(p.x / globals.viewport.x * 2.0 - 1.0, 1.0 - p.y / globals.viewport.y * 2.0, 0.0, 1.0);
}

// Corner of a unit quad for vertex 0..3 of a triangle strip.
fn corner(vi: u32) -> vec2<f32> {
    return vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
}

// Signed distance to a rounded box centred on the origin (negative inside). `r` = radii tl, tr, br, bl.
fn sd_rounded_box(p: vec2<f32>, half: vec2<f32>, r: vec4<f32>) -> f32 {
    var radius: f32;
    if (p.x < 0.0) {
        if (p.y < 0.0) { radius = r.x; } else { radius = r.w; }
    } else {
        if (p.y < 0.0) { radius = r.y; } else { radius = r.z; }
    }
    let q = abs(p) - half + vec2<f32>(radius, radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0, 0.0))) - radius;
}

// Coverage (0..1) of pixel `p` by the clip rect `clip` (x, y, w, h) with rounded corners of `radius`.
fn clip_alpha(p: vec2<f32>, clip: vec4<f32>, radius: f32) -> f32 {
    if (p.x < clip.x || p.y < clip.y || p.x > clip.x + clip.z || p.y > clip.y + clip.w) {
        return 0.0;
    }
    if (radius <= 0.0) {
        return 1.0;
    }
    let half = clip.zw * 0.5;
    let d = sd_rounded_box(p - (clip.xy + half), half, vec4<f32>(radius, radius, radius, radius));
    return clamp(0.5 - d, 0.0, 1.0);
}

// How much of a pixel glows: bright, saturated colours (neon) do; white text and dark grounds don't. The colour
// it gives off, and how lit the pixel is (alpha). The frame is premultiplied (a see-through window), so its colour
// is taken back out of the alpha first.
fn neon(c: vec4<f32>) -> vec4<f32> {
    let rgb = c.rgb / max(c.a, 0.0001);
    let hi = max(rgb.r, max(rgb.g, rgb.b));
    let lo = min(rgb.r, min(rgb.g, rgb.b));
    let sat = (hi - lo) / max(hi, 0.0001);
    let w = smoothstep(0.42, 0.9, hi) * smoothstep(0.18, 0.55, sat) * c.a;
    return vec4<f32>(rgb * w, w);
}
