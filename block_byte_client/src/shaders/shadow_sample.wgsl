fn sample_shadow(world_position: vec3<f32>, normal: vec3<f32>) -> f32{
    let floored_world_position = world_position;//floor(world_position * 16.) / 16.;
    var shadow_space = shadow_camera.view_proj * vec4(floored_world_position, 1.);
    //shadow_space.w -= 0.0001;
    let shadow_undistorted = vec4(shadow_distort_position(shadow_space.xy), shadow_space.zw);
    let shadow_projection = shadow_undistorted.xyz;// / shadow_undistorted.w;

    let dark_color = min(length(shadow_projection.xy)/10., 0.25) + 0.75;

    if dot(normal, shadow_camera.direction) <= 0.{
        return dark_color;
    }

    let shadow_uv = shadow_projection.xy * 0.5 * vec2(1., -1.) + vec2<f32>(0.5);
    let bounds = abs(shadow_projection.xy);
    if max(bounds.x, bounds.y) > 0.99{
        return 1.;
    }
    let shadow_value = textureSample(shadow_texture, shadow_sampler, shadow_uv);

    if shadow_value >= 1.{
        return 1.;
    }

    return select(dark_color, 1., shadow_value + 0.0005 > shadow_projection.z);
}