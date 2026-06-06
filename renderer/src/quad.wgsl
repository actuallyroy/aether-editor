struct U {
    res: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: U;

struct VIn {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) params: vec4<f32>, // params.x = corner radius (px)
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,  // pixel offset within the rect
    @location(2) half_size: vec2<f32>,   // half-size of the rect (avoid `half`, reserved in MSL)
    @location(3) radius: f32,
    // params.y>0 ⇒ arc mode: a circular ring between radius params.x (outer) and
    // params.y (inner), centered at params.zw (local px), clipped to this rect.
    @location(4) params: vec4<f32>,
};

@vertex
fn vs_main(in: VIn, @builtin(vertex_index) vid: u32) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vid];
    let local = corner * in.rect.zw;
    let px = in.rect.xy + local;
    let ndc = vec2<f32>(
        px.x / u.res.x * 2.0 - 1.0,
        1.0 - px.y / u.res.y * 2.0,
    );
    var out: VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.color = in.color;
    out.local = local;
    out.half_size = in.rect.zw * 0.5;
    out.radius = in.params.x;
    out.params = in.params;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    // Arc mode: a 1px-AA circular ring band between inner..outer radius, centered at
    // params.zw. The rect bounds it (so a quadrant-sized rect yields a quarter arc).
    if (in.params.y > 0.0) {
        let dist = length(in.local - in.params.zw);
        let a_out = clamp(in.params.x - dist + 0.5, 0.0, 1.0);
        let a_in = clamp(dist - in.params.y + 0.5, 0.0, 1.0);
        return vec4<f32>(in.color.rgb, in.color.a * min(a_out, a_in));
    }
    // Sharp rectangles (the common case) skip the SDF entirely so their edges
    // stay crisp and pixel-exact.
    if (in.radius <= 0.0) {
        return in.color;
    }
    // Signed distance to a rounded box, with ~1px antialiased edge coverage.
    let r = min(in.radius, min(in.half_size.x, in.half_size.y));
    let p = in.local - in.half_size;
    let q = abs(p) - in.half_size + vec2<f32>(r, r);
    let d = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
    let a = clamp(0.5 - d, 0.0, 1.0);
    return vec4<f32>(in.color.rgb, in.color.a * a);
}
