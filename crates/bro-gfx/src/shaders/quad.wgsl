// Rounded rectangles with an optional border and gradient, antialiased by signed distance.

struct QuadIn {
    @location(0) rect: vec4<f32>,
    @location(1) radii: vec4<f32>,
    @location(2) fill: vec4<f32>,
    @location(3) fill2: vec4<f32>,
    @location(4) border: vec4<f32>,
    @location(5) clip: vec4<f32>,
    // border width, gradient (0 none, 1 vertical, 2 horizontal), clip radius, dashed
    @location(6) params: vec4<f32>,
};

struct QuadOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) fill: vec4<f32>,
    @location(4) @interpolate(flat) fill2: vec4<f32>,
    @location(5) @interpolate(flat) border: vec4<f32>,
    @location(6) @interpolate(flat) clip: vec4<f32>,
    @location(7) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs_quad(@builtin(vertex_index) vi: u32, q: QuadIn) -> QuadOut {
    // one pixel of room around the box for the antialiased edge
    let r = vec4<f32>(q.rect.xy - vec2<f32>(1.0, 1.0), q.rect.zw + vec2<f32>(2.0, 2.0));
    let p = r.xy + corner(vi) * r.zw;
    var o: QuadOut;
    o.pos = to_clip(p);
    o.p = p;
    o.rect = q.rect;
    o.radii = q.radii;
    o.fill = q.fill;
    o.fill2 = q.fill2;
    o.border = q.border;
    o.clip = q.clip;
    o.params = q.params;
    return o;
}

@fragment
fn fs_quad(o: QuadOut) -> @location(0) vec4<f32> {
    let clip = clip_alpha(o.p, o.clip, o.params.z);
    if (clip <= 0.0) {
        discard;
    }
    let half = o.rect.zw * 0.5;
    let local = o.p - (o.rect.xy + half);
    let d = sd_rounded_box(local, half, o.radii);
    let outer = clamp(0.5 - d, 0.0, 1.0);
    if (outer <= 0.0) {
        discard;
    }
    var fill = o.fill;
    if (o.params.y > 0.5) {
        var t: f32;
        if (o.params.y < 1.5) { t = (o.p.y - o.rect.y) / max(o.rect.w, 1.0); } else { t = (o.p.x - o.rect.x) / max(o.rect.z, 1.0); }
        fill = mix(o.fill, o.fill2, clamp(t, 0.0, 1.0));
    }
    var color = fill;
    let bw = o.params.x;
    if (bw > 0.0) {
        // 1 in the border band, 0 inside its inner edge
        let band = 1.0 - clamp(0.5 - (d + bw), 0.0, 1.0);
        var border = o.border;
        if (o.params.w > 0.5) {
            // dashed: 6px on, 4px off along the perimeter (approximated per edge)
            let along = select(o.p.x - o.rect.x, o.p.y - o.rect.y, abs(local.x) / max(half.x, 1.0) > abs(local.y) / max(half.y, 1.0));
            if (fract(along / 10.0) > 0.6) { border.a = 0.0; }
        }
        // the border band over the fill (a see-through fill stays see-through inside)
        let ba = border.a * band;
        let fa = fill.a;
        let a = ba + fa * (1.0 - ba);
        var rgb = vec3<f32>(0.0, 0.0, 0.0);
        if (a > 0.0) {
            rgb = (border.rgb * ba + fill.rgb * fa * (1.0 - ba)) / a;
        }
        color = vec4<f32>(rgb, a);
    }
    return vec4<f32>(color.rgb, color.a * outer * clip);
}
