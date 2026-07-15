fn animate_texture(uv: vec2<f32>) -> vec2<f32>{
    let cell_x = i32(uv.x * f32(animation_data.width));
    let cell_y = i32(uv.y * f32(animation_data.width));
    let info = animation_data.cells[cell_x + cell_y * animation_data.width];
    let index = (u32(time/info.time)%info.frames);
    return vec2<f32>(uv.x + info.shift * f32(index), uv.y);
}

struct AnimatedCell {
    time: f32,
    shift: f32,
    frames: u32,
};

struct AnimationData{
    width: i32,
    cells: array<AnimatedCell>,
}