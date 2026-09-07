#include common

@group(1) @binding(0)
var<uniform> shadow_camera: CameraUniform;

@group(2) @binding(0)
var<uniform> time: f32;

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0)@binding(1)
var s_diffuse: sampler;

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

    var out: VertexOutput;
    let projected = shadow_camera.view_proj * vec4<f32>(position, 1.0);
    out.clip_position = vec4(shadow_distort_position(projected.xy), projected.z, 1.);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4(1.);
}