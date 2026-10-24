// Whole-screen effects after everything is drawn (a theme's skin): every colour a shade of one ink like a
// monochrome CRT, a neon glow, darker edges, and scanlines. The frame is premultiplied, and its alpha (a
// see-through window) is kept.

@group(1) @binding(0) var frame_tex: texture_2d<f32>;
@group(1) @binding(1) var frame_sampler: sampler;

struct Post {
    // rgb ground, a = phosphor on (1) / off (0)
    ground: vec4<f32>,
    // rgb ink, a = vignette strength
    ink: vec4<f32>,
    // x = scanline strength, y = scanline period in pixels, z = glow strength
    lines: vec4<f32>,
};

@group(1) @binding(2) var<uniform> post: Post;
@group(1) @binding(3) var glow_tex: texture_2d<f32>;
@group(1) @binding(4) var glow_sampler: sampler;

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
    let frame = textureSampleLevel(frame_tex, frame_sampler, o.uv, 0.0);
    var c = frame.rgb;
    var a = frame.a;
    if (post.ground.a > 0.5) {
        let l = dot(c / max(a, 0.0001), vec3<f32>(0.299, 0.587, 0.114));
        c = mix(post.ground.rgb, post.ink.rgb, pow(l, 0.8)) * a;
    }
    if (post.lines.z > 0.0) {
        // a halo around what's lit, never over it: a neon stroke keeps its colour, and inside a lit area (a neon
        // button, its label) nothing lands; it saturates softly and lightens like light does (screen), lighting up
        // the glass it falls on
        let glow = textureSampleLevel(glow_tex, glow_sampler, o.uv, 0.0);
        let lit = max(glow.rgb - neon(frame).rgb, vec3<f32>(0.0)) * (1.0 - smoothstep(0.3, 0.75, glow.a));
        let g = vec3<f32>(1.0, 1.0, 1.0) - exp(-lit * post.lines.z);
        c = c + g * (vec3<f32>(1.0, 1.0, 1.0) - c);
        a = a + max(g.r, max(g.g, g.b)) * (1.0 - a);
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
    return vec4<f32>(min(c, vec3<f32>(a, a, a)), a);
}
