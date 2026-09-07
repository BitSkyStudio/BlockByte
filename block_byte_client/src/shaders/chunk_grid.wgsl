#include common
#include shadow_sample
#include texture_animation

@group(1) @binding(0)
var<uniform> camera: CameraUniform;

@group(4) @binding(0)
var<uniform> time: f32;

struct VertexInput {
    @location(0) position: vec2<f32>,
}

struct InstanceInput {
    @location(1) position: u32,
    @location(2) texture: u32,
    @location(3) color: u32,
    @location(4) face: u32,
}

var<immediate> chunk_position: vec3<f32>;

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
    instance: InstanceInput,
    @builtin(vertex_index) vertex_index: u32
) -> VertexOutput {
    const FACE_VERTICES = array<array<u32,4>, 6>(
        array<u32,4>(3, 2, 0, 1),
        array<u32,4>(6, 7, 5, 4),
        array<u32,4>(2, 3, 7, 6),
        array<u32,4>(1, 0, 4, 5),
        array<u32,4>(2, 6, 4, 0),
        array<u32,4>(7, 3, 1, 5),
    );
    const FACE_NORMALS = array<vec3<f32>, 6>(
        vec3<f32>(0., 0., -1.),
        vec3<f32>(0., 0., 1.),
        vec3<f32>(0., 1., 0.),
        vec3<f32>(0., -1., 0.),
        vec3<f32>(-1., 0., 0.),
        vec3<f32>(1., 0., 0.),
    );
    let position_x = f32(instance.position & 31);
    let position_y = f32((instance.position>>5) & 31);
    let position_z = f32((instance.position>>10) & 31);

    let vertex = FACE_VERTICES[instance.face][vertex_index];

    let position = chunk_position + vec3<f32>(position_x, position_y, position_z) + vec3<f32>(
        select(0., 1., (vertex&1)!=0), 
        select(0., 1., (vertex&2)!=0), 
        select(0., 1., (vertex&4)!=0)
    );

    let texture_cell = texture_dimension(instance.texture);

    var out: VertexOutput;
    out.tex_coords = vec2<f32>(texture_cell.x + texture_cell.z * model.position.x, texture_cell.y + texture_cell.w * model.position.y);
    out.normal = FACE_NORMALS[instance.face];
    out.clip_position = camera.view_proj * vec4<f32>(position, 1.0);
    out.world_position = position;
    out.color = convert_color(instance.color);
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
    let sampled_color: vec4<f32> = textureSample(t_diffuse, s_diffuse, tex_coords);
    if sampled_color.w < 0.1{
        discard;
    }
    let albedo = sampled_color.rgb * (1. - material.r) + in.color * material.r;

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

    let lighting = (ndotl + lightColor + ambient) * shadow_color * normal_shading(in.normal);

    return vec4(albedo * (lighting * (1. - material.g) + 1.5 * material.g), 1.);
}

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, vec3(0.2125f, 0.7153f, 0.0721f));
}