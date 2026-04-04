// Vertex shader
struct CameraUniform {
    view_proj: mat4x4<f32>,
};
@group(1) @binding(0) // 1.
var<uniform> camera: CameraUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) color: u32,
    @location(3) shade: f32,
    @location(4) flags: u32,
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
    out.clip_position = camera.view_proj * vec4<f32>(model.position, 1.0);
    out.world_position = model.position;
    let color_r = f32(model.color&31)/31.;
    let color_g = f32((model.color>>5)&31)/31.;
    let color_b = f32((model.color>>10)&31)/31.;
    out.color = vec4<f32>(color_r * model.shade, color_g * model.shade, color_b * model.shade, 1.);
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
    let albedo: vec4<f32> = textureSample(t_diffuse, s_diffuse, in.tex_coords) * in.color;
    if albedo.w < 0.1{
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

    //let shadow_color = textureSampleCompare(shadow_texture, shadow_sampler, shadow_uv, shadow_projection.z) * 0.3 + 0.7;

    const ambient = vec3<f32>(0.02f, 0.04f, 0.08f);
    const sunColor = vec3<f32>(0.98f, 0.73f, 0.15f);
    const skyColor = vec3<f32>(0.47, 0.65, 1.0);

    let lightColor = skyColor * 1.;

    var ndotl = sunColor * 1.; // clamp(4 * dot(normal, sunDirection), 0.0f, 1.0f) * sunVisibility;
    //ndotl += moonColor * clamp(4 * dot(normal, -sunDirection), 0.0f, 1.0f) * moonVisibility;
    ndotl *= 1.3;
    ndotl *= (luminance(skyColor) + 0.01f);
    //ndotl *= lightmap.g;

    let lighting = ndotl + lightColor + ambient;

    var diffuse = albedo.rgb;
    diffuse *= lighting;
    diffuse *= shadow_color;

    return vec4(diffuse, 1.) ;//* vec4<f32>(5.5,5.5, 5.5, 1.);
}

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, vec3(0.2125f, 0.7153f, 0.0721f));
}

fn DistortPosition(position: vec2<f32>) -> vec2<f32>{
    //let CenterDistance = length(position);
    //let DistortionFactor = mix(1.0f, CenterDistance, 0.5f);
    //return position / DistortionFactor;
    return position / (length(position) + 0.1);
}