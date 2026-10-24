// The glow (a theme's neon): the bright, saturated colours of the drawn frame at half size, blurred across and
// then down; the effects pass lays it back over the frame.

@group(1) @binding(0) var src_tex: texture_2d<f32>;
@group(1) @binding(1) var src_sampler: sampler;

struct Bloom {
    // xy: the step between blur taps in uv, z: sigma in taps
    step: vec4<f32>,
};

@group(1) @binding(2) var<uniform> bloom: Bloom;

struct BloomOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_bloom(@builtin(vertex_index) vi: u32) -> BloomOut {
    // one triangle covering the target
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var o: BloomOut;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

// Half size: each pixel is the glow of the 4 × 4 frame pixels around it, judged one by one so a thin stroke of
// neon text still counts; alpha is how much of them is lit.
@fragment
fn fs_bright(o: BloomOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(src_tex));
    let base = vec2<i32>(floor(o.pos.xy)) * 2;
    var g = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var y = -1; y <= 2; y = y + 1) {
        for (var x = -1; x <= 2; x = x + 1) {
            let p = clamp(base + vec2<i32>(x, y), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
            g = g + neon(textureLoad(src_tex, p, 0));
        }
    }
    return g / 16.0;
}

// A gaussian along `bloom.step`.
@fragment
fn fs_blur(o: BloomOut) -> @location(0) vec4<f32> {
    let sigma = max(bloom.step.z, 0.5);
    var sum = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var total = 0.0;
    for (var i = -8; i <= 8; i = i + 1) {
        let x = f32(i);
        let w = exp(-(x * x) / (2.0 * sigma * sigma));
        sum = sum + textureSampleLevel(src_tex, src_sampler, o.uv + bloom.step.xy * x, 0.0) * w;
        total = total + w;
    }
    return sum / total;
}
