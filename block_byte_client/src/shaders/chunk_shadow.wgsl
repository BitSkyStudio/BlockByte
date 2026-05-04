@group(1) @binding(0)
var<uniform> shadow_camera: CameraUniform;

@group(2) @binding(0)
var<uniform> time: f32;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tex_coords: vec2<f32>,
    @location(3) color: u32,
    @location(4) shade: f32,
    @location(5) flags: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

@vertex
fn vs_main(
    model: VertexInput,
) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coords = model.tex_coords;
    var position = model.position;
    if (model.flags & 1) != 0u{
        position.x += sin(time * 0.8 + model.position.x * 0.2) * 0.1;
        position.z += sin(time * 0.8 + 10 + model.position.z * 0.2) * 0.1;
    }
    let projected = shadow_camera.view_proj * vec4<f32>(position, 1.0);
    out.clip_position = vec4(shadow_distort_position(projected.xy), projected.z, 1.);
    return out;
}

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0)@binding(1)
var s_diffuse: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let albedo: vec4<f32> = textureSampleLevel(t_diffuse, s_diffuse, in.tex_coords, 0);
    if albedo.w < 0.9{
        discard;
    }
    return vec4(1.);
}