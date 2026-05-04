@group(1) @binding(0) // 1.
var<uniform> shadow_camera: CameraUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vs_main(
    model: VertexInput,
) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coords = model.tex_coords;
    let projected = shadow_camera.view_proj * vec4<f32>(model.position, 1.0);
    out.clip_position = vec4(shadow_distort_position(projected.xy), projected.z, 1.);
    out.color = model.color;
    return out;
}

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0)@binding(1)
var s_diffuse: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color: vec4<f32> = textureSampleLevel(t_diffuse, s_diffuse, in.tex_coords, 0) * in.color;
    if color.w < 0.9{
        discard;
    }
    return vec4(1.);
}