// Whole-screen effects after everything is drawn (a theme's skin): every colour a shade of one ink like a
// monochrome CRT, darker edges, and scanlines.

@group(1) @binding(0) var frame_tex: texture_2d<f32>;
@group(1) @binding(1) var frame_sampler: sampler;

struct Post {
    // rgb ground, a = phosphor on (1) / off (0)
    ground: vec4<f32>,
    // rgb ink, a = vignette strength
    ink: vec4<f32>,
    // x = scanline strength, y = scanline period in pixels
    lines: vec4<f32>,
};

@group(1) @binding(2) var<uniform> post: Post;

struct PostOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_post(@builtin(vertex_index) vi: u32) -> PostOut {
    // one triangle covering the screen
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var o: PostOut;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

@fragment
fn fs_post(o: PostOut) -> @location(0) vec4<f32> {
    var c = textureSampleLevel(frame_tex, frame_sampler, o.uv, 0.0).rgb;
    if (post.ground.a > 0.5) {
        let l = dot(c, vec3<f32>(0.299, 0.587, 0.114));
        c = mix(post.ground.rgb, post.ink.rgb, pow(l, 0.8));
    }
    var dim = 0.0;
    if (post.ink.a > 0.0) {
        let d = (o.uv - vec2<f32>(0.5, 0.5)) * 2.0;
        dim = dim + pow((d.x * d.x + d.y * d.y) / 2.0, 1.5) * post.ink.a;
    }
    if (post.lines.x > 0.0) {
        let row = floor(o.pos.y / max(post.lines.y, 1.0));
        if (row - 2.0 * floor(row / 2.0) > 0.5) {
            dim = dim + post.lines.x;
        }
    }
    c = c * (1.0 - clamp(dim, 0.0, 0.9));
    return vec4<f32>(c, 1.0);
}
