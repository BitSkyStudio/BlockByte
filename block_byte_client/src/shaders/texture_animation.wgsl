fn animate_texture(uv: vec2<f32>) -> vec2<f32>{
    let cell_x = i32(uv.x * f32(animation_data.width));
    let cell_y = i32(uv.y * f32(animation_data.width));
    let info = animation_data.cells[cell_x + cell_y * animation_data.width];
    let index = (u32(time/info.time)%info.frames);
    return vec2<f32>(uv.x + info.w * f32(index), uv.y);
}

fn texture_dimension(texture: u32) -> vec4<f32>{
    let x = f32(texture % u32(animation_data.width)) / f32(animation_data.width);
    let y = f32(texture / u32(animation_data.width)) / f32(animation_data.width);
    let info = animation_data.cells[texture];
    return vec4<f32>(x, y, info.w, info.h);
}

struct AnimatedCell {
    time: f32,
    w: f32,
    h: f32,
    frames: u32,
};

struct AnimationData{
    width: i32,
    cells: array<AnimatedCell>,
}