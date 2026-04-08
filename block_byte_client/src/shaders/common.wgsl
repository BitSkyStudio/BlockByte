fn shadow_distort_position(position: vec2<f32>) -> vec2<f32>{
    return position / (length(position) + 0.1);
}

fn normal_shading(normal: vec3<f32>) -> f32{
    let squared_normal = pow(normal, vec3(2.));
    return 1. - abs(squared_normal.x) * 0.3 - abs(squared_normal.z) * 0.2 + min(squared_normal.y, 0.) * 0.4;
}

struct CameraUniform {
    view_proj: mat4x4<f32>,
    direction: vec3<f32>,
};