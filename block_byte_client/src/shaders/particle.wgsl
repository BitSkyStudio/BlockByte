#include common
#include shadow_sample

struct VertexInput {
    @location(0) position: vec2<f32>,
}

struct InstanceInput {
    @location(1) position: vec3<f32>,
    @location(2) uv1: vec2<f32>,
    @location(3) uv2: vec2<f32>,
    @location(4) size: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
    @location(1) world_position: vec3<f32>,
}

@vertex
fn vs_main(
    model: VertexInput,
    instance: InstanceInput,
) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coords = instance.uv1 + (instance.uv2 - instance.uv1) * model.position;
    out.world_position = instance.position;
    let camera_right = vec3<f32>(camera.view[0][0], camera.view[1][0], camera.view[2][0]);
    let camera_up = vec3<f32>(camera.view[0][1], camera.view[1][1], camera.view[2][1]);
    let billboard_offset = (camera_right * (model.position.x - 0.5) * instance.size.x) - (camera_up * (model.position.y - 0.5) * instance.size.y);
    out.clip_position = camera.view_proj * vec4<f32>(instance.position + billboard_offset, 1.0);
    return out;
}


@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0)@binding(1)
var s_diffuse: sampler;

@group(1) @binding(0)
var<uniform> camera: CameraUniform;

@group(2) @binding(0)
var<uniform> shadow_camera: CameraUniform;

@group(3) @binding(0)
var shadow_texture: texture_depth_2d;
@group(3)@binding(1)
var shadow_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let sampled_color = textureSample(t_diffuse, s_diffuse, in.tex_coords);
    if sampled_color.w < 0.1{
        discard;
    }

    let shadow_color = sample_shadow(in.world_position, shadow_camera.direction);

    return vec4(sampled_color.rgb * shadow_color,sampled_color.a);
}