@group(1) @binding(0)
var<uniform> camera: CameraUniform;

@group(4) @binding(0)
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
    @location(1) color: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) normal: vec3<f32>,
}

@vertex
fn vs_main(
    model: VertexInput,
) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coords = model.tex_coords;
    out.normal = model.normal;
    var position = model.position;
    if (model.flags & 1) != 0u{
        position.x += sin(time * 0.8 + model.position.x * 0.2) * 0.1;
        position.z += sin(time * 0.8 + 10 + model.position.z * 0.2) * 0.1;
    }
    out.clip_position = camera.view_proj * vec4<f32>(position, 1.0);
    out.world_position = position;
    let normal_shading = normal_shading(model.normal);
    let color_r = f32(model.color&31)/31.;
    let color_g = f32((model.color>>5)&31)/31.;
    let color_b = f32((model.color>>10)&31)/31.;
    out.color = vec3<f32>(color_r, color_g, color_b);
    return out;
}


@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0)@binding(1)
var s_diffuse: sampler;

@group(2) @binding(0)
var<uniform> shadow_camera: CameraUniform;

@group(3) @binding(0)
var shadow_texture: texture_depth_2d;
@group(3)@binding(1)
var shadow_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let albedo: vec4<f32> = textureSample(t_diffuse, s_diffuse, in.tex_coords) * vec4(in.color, 1.);
    if albedo.w < 0.1{
        discard;
    }

    let shadow_color = sample_shadow(in.world_position, in.normal);

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
    diffuse *= normal_shading(in.normal);

    return vec4(diffuse, 1.) ;//* vec4<f32>(5.5,5.5, 5.5, 1.);
}

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, vec3(0.2125f, 0.7153f, 0.0721f));
}