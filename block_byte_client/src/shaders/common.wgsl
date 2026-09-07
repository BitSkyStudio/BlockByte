fn shadow_distort_position(position: vec2<f32>) -> vec2<f32>{
    return position / (length(position) + 0.1);
}

fn normal_shading(normal: vec3<f32>) -> f32{
    let squared_normal = pow(normal, vec3(2.));
    return 1. - abs(squared_normal.x) * 0.2 - abs(squared_normal.z) * 0.1 + min(squared_normal.y, 0.) * 0.3;
}

struct CameraUniform {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    direction: vec3<f32>,
};

fn convert_color(color: u32) -> vec3<f32>{
    let color_r = f32(color&31)/31.;
    let color_g = f32((color>>5)&31)/31.;
    let color_b = f32((color>>10)&31)/31.;
    return vec3<f32>(color_r, color_g, color_b);
}