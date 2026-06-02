struct CameraUniform {
    view_project: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // Cube corner points
    const p0 = vec3<f32>(-1.0, -1.0, 0.0); // Front-Bottom-Left
    const p1 = vec3<f32>( 1.0, -1.0, 0.0); // Front-Bottom-Right
    const p2 = vec3<f32>( 1.0,  1.0, 0.0); // Front-Top-Right
    const p3 = vec3<f32>(-1.0,  1.0, 0.0); // Front-Top-Left
    const p4 = vec3<f32>(-1.0, -1.0, 1.0); // Back-Bottom-Left
    const p5 = vec3<f32>( 1.0, -1.0, 1.0); // Back-Bottom-Right
    const p6 = vec3<f32>( 1.0,  1.0, 1.0); // Back-Top-Right
    const p7 = vec3<f32>(-1.0,  1.0, 1.0); // Back-Top-Left

    // Cube polygons
    var pos = array<vec3<f32>, 36>(
        p0, p1, p2,  p0, p2, p3, // Front Face
        p5, p4, p7,  p5, p7, p6, // Back Face
        p4, p0, p3,  p4, p3, p7, // Left Face
        p1, p5, p6,  p1, p6, p2, // Right Face
        p3, p2, p6,  p3, p6, p7, // Top Face
        p4, p5, p1,  p4, p1, p0  // Bottom Face
    );

    // UV corner points mapping
    const uv_bl = vec2<f32>(0.0, 1.0); // Bottom-Left
    const uv_br = vec2<f32>(1.0, 1.0); // Bottom-Right
    const uv_tr = vec2<f32>(1.0, 0.0); // Top-Right
    const uv_tl = vec2<f32>(0.0, 0.0); // Top-Left

    // UV polygon mapping
    var uvs = array<vec2<f32>, 36>(
        uv_bl, uv_br, uv_tr,  uv_bl, uv_tr, uv_tl, // Front Face
        uv_bl, uv_br, uv_tr,  uv_bl, uv_tr, uv_tl, // Back Face
        uv_bl, uv_br, uv_tr,  uv_bl, uv_tr, uv_tl, // Left Face
        uv_bl, uv_br, uv_tr,  uv_bl, uv_tr, uv_tl, // Right Face
        uv_bl, uv_br, uv_tr,  uv_bl, uv_tr, uv_tl, // Top Face
        uv_bl, uv_br, uv_tr,  uv_bl, uv_tr, uv_tl  // Bottom Face
    );

    var out: VertexOutput;
    out.clip_position = camera.view_project * vec4<f32>(pos[vertex_index], 1.0);
    out.uv = uvs[vertex_index];
    return out;
}

const YUV2RGB: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(1.164, 1.164, 1.164),
    vec3<f32>(0.000, -0.213, 2.112),
    vec3<f32>(1.793, -0.533, 0.000),
);

@group(1) @binding(0) var t_y: texture_2d<f32>;
@group(1) @binding(1) var t_u: texture_2d<f32>;
@group(1) @binding(2) var t_v: texture_2d<f32>;
@group(1) @binding(3) var s: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let y = textureSample(t_y, s, in.uv).r;
    let u = textureSample(t_u, s, in.uv).r;
    let v = textureSample(t_v, s, in.uv).r;

    // normalize sampled values to correct proportions
    let yuv = vec3<f32>(
        y - (16.0 / 255.0),
        u - 0.5,
        v - 0.5
    );

    // sRGB to RGB, otherwise sRGB adjustment will be done twice
    // TODO: detect output format and handle RGB dynamically, Iced sends the output format during Primitive::prepare() step
    // but this being hardcoded should not be a big deal since most(all?) modern machines will use sRGB anyways
    let rgb = pow(YUV2RGB * yuv, vec3<f32>(2.2));

    return vec4<f32>(clamp(rgb, vec3(0.0), vec3(1.0)), 1.0);
}
