// Textured-quad renderer for extension icons. Instanced: each instance is a
// screen-space rect plus a normalized UV sub-rect into the shared atlas texture.

struct U {
    res: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_smp: sampler;

struct VIn {
    @location(0) rect: vec4<f32>, // x, y, w, h in pixels
    @location(1) uv: vec4<f32>,   // u0, v0, du, dv normalized
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
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
    let px = in.rect.xy + corner * in.rect.zw;
    let ndc = vec2<f32>(
        px.x / u.res.x * 2.0 - 1.0,
        1.0 - px.y / u.res.y * 2.0,
    );
    var out: VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = in.uv.xy + corner * in.uv.zw;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    return textureSample(atlas_tex, atlas_smp, in.uv);
}
