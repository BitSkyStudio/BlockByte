fn shadow_distort_position(position: vec2<f32>) -> vec2<f32>{
    return position / (length(position) + 0.1);
}