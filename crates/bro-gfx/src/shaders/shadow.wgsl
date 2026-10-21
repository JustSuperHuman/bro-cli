// Soft shadows of rounded rectangles: a gaussian blur evaluated in closed form along x and integrated
// over a few samples in y (Evan Wallace's "fast rounded rectangle shadows").

struct ShadowIn {
    @location(0) rect: vec4<f32>,
    @location(1) radii: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) clip: vec4<f32>,
    // sigma, clip radius
    @location(4) params: vec4<f32>,
};

struct ShadowOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) clip: vec4<f32>,
    @location(5) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs_shadow(@builtin(vertex_index) vi: u32, s: ShadowIn) -> ShadowOut {
    let pad = s.params.x * 3.0;
    let r = vec4<f32>(s.rect.xy - vec2<f32>(pad, pad), s.rect.zw + vec2<f32>(pad * 2.0, pad * 2.0));
    let p = r.xy + corner(vi) * r.zw;
    var o: ShadowOut;
    o.pos = to_clip(p);
    o.p = p;
    o.rect = s.rect;
    o.radii = s.radii;
    o.color = s.color;
    o.clip = s.clip;
    o.params = s.params;
    return o;
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (2.5066283 * sigma);
}

fn erf2(x: vec2<f32>) -> vec2<f32> {
    let s = sign(x);
    let a = abs(x);
    var r = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    r = r * r;
    return s - s / (r * r);
}

fn shadow_x(x: f32, y: f32, sigma: f32, corner_r: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - corner_r - abs(y), 0.0);
    let curved = half.x - corner_r + sqrt(max(0.0, corner_r * corner_r - delta * delta));
    let integral = 0.5 + 0.5 * erf2((x + vec2<f32>(-curved, curved)) * (0.70710678 / sigma));
    return integral.y - integral.x;
}

@fragment
fn fs_shadow(o: ShadowOut) -> @location(0) vec4<f32> {
    let clip = clip_alpha(o.p, o.clip, o.params.y);
    if (clip <= 0.0) {
        discard;
    }
    let sigma = o.params.x;
    let half = o.rect.zw * 0.5;
    let center = o.rect.xy + half;
    let point = o.p - center;
    // pick the radius of the nearest corner
    var r: f32;
    if (point.x < 0.0) {
        if (point.y < 0.0) { r = o.radii.x; } else { r = o.radii.w; }
    } else {
        if (point.y < 0.0) { r = o.radii.y; } else { r = o.radii.z; }
    }
    let low = point.y - half.y;
    let high = point.y + half.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var value = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        value = value + shadow_x(point.x, point.y - y, sigma, r, half) * gaussian(y, sigma) * step;
        y = y + step;
    }
    return vec4<f32>(o.color.rgb, o.color.a * value * clip);
}
