#include common
#include shadow_sample
#include texture_animation

@group(1) @binding(0) // 1.
var<uniform> camera: CameraUniform;

@group(4) @binding(0)
var<uniform> time: f32;

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
    @location(2) world_position: vec3<f32>,
    @location(3) normal: vec3<f32>,
}

@vertex
fn vs_main(
    model: VertexInput,
) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coords = model.tex_coords;
    out.world_position = model.position;
    out.normal = model.normal;
    out.clip_position = camera.view_proj * vec4<f32>(model.position, 1.0);
    let shade_color = normal_shading(model.normal);
    out.color = model.color * vec4(vec3(shade_color * 1.3), 1.);
    return out;
}


@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0)@binding(1)
var s_diffuse: sampler;

@group(5) @binding(0)
var material_texture: texture_2d<f32>;
@group(5) @binding(1)
var material_sampler: sampler;

@group(2) @binding(0)
var<uniform> shadow_camera: CameraUniform;

@group(3) @binding(0)
var shadow_texture: texture_depth_2d;
@group(3)@binding(1)
var shadow_sampler: sampler;

@group(6) @binding(0)
var<storage, read> animation_data: AnimationData;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_coords = animate_texture(in.tex_coords);
    let material = textureSample(material_texture, material_sampler, tex_coords);
    let sampled_color = textureSample(t_diffuse, s_diffuse, tex_coords);
    if sampled_color.w < 0.1{
        discard;
    }
    //we dont use material r for this
    let color = sampled_color.rgb * in.color.rgb;

    let shadow_color = sample_shadow(in.world_position, in.normal);

    return vec4(color * shadow_color,sampled_color.a * in.color.a);//* vec4<f32>(5.5,5.5, 5.5, 1.);
}