// Vertex shader
struct CameraUniform {
    view_proj: mat4x4<f32>,
};
@group(1) @binding(0) // 1.
var<uniform> camera: CameraUniform;

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
}

@vertex
fn vs_main(
    model: VertexInput,
) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coords = model.tex_coords;
    out.world_position = model.position;
    out.clip_position = camera.view_proj * vec4<f32>(model.position, 1.0);
    //let shading = dot(model.normal, normalize(vec3<f32>(1, -1, 0.5)));
    let shade_color = 1. - abs(model.normal.x) * 0.5 - abs(model.normal.z) * 0.2;
    out.color = model.color * vec4<f32>(shade_color, shade_color, shade_color, 1.) * 1.3;
    return out;
}


// Fragment shader

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0)@binding(1)
var s_diffuse: sampler;

@group(2) @binding(0) // 1.
var<uniform> shadow_camera: CameraUniform;

@group(3) @binding(0)
var shadow_texture: texture_depth_2d;
@group(3)@binding(1)
var shadow_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color: vec4<f32> = textureSample(t_diffuse, s_diffuse, in.tex_coords) * in.color;
    if color.w < 0.1{
        discard;
    }

    var shadow_space = shadow_camera.view_proj * vec4(in.world_position, 1.);
    //shadow_space.w -= 0.0001;
    let shadow_undistorted = vec4(DistortPosition(shadow_space.xy), shadow_space.zw);
    let shadow_projection = shadow_undistorted.xyz;// / shadow_undistorted.w;
    let shadow_uv = shadow_projection.xy * 0.5 * vec2(1., -1.) + vec2<f32>(0.5);
    let shadow_value = textureSample(shadow_texture, shadow_sampler, shadow_uv);
    //return vec4(shadow_uv, 0., 1.);

    let shadow_color = select(0.7, 1., shadow_value + 0.0005 > shadow_projection.z);

    return vec4(color.rgb * shadow_color,1.);//* vec4<f32>(5.5,5.5, 5.5, 1.);
}

fn DistortPosition(position: vec2<f32>) -> vec2<f32>{
    //let CenterDistance = length(position);
    //let DistortionFactor = mix(1.0f, CenterDistance, 0.5f);
    //return position / DistortionFactor;
    return position / (length(position) + 0.1);
}