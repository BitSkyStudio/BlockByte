fn sample_shadow(world_position: vec3<f32>, normal: vec3<f32>) -> f32{
    if dot(normal, shadow_camera.direction) < 0.{
        return 0.7;
    }

    var shadow_space = shadow_camera.view_proj * vec4(world_position, 1.);
    //shadow_space.w -= 0.0001;
    let shadow_undistorted = vec4(shadow_distort_position(shadow_space.xy), shadow_space.zw);
    let shadow_projection = shadow_undistorted.xyz;// / shadow_undistorted.w;
    let shadow_uv = shadow_projection.xy * 0.5 * vec2(1., -1.) + vec2<f32>(0.5);
    let bounds = abs(shadow_uv);
    if max(bounds.x, bounds.y) > 0.99{
        return 1.;
    }
    let shadow_value = textureSample(shadow_texture, shadow_sampler, shadow_uv);
    //return vec4(shadow_uv, 0., 1.);

    return select(0.7, 1., shadow_value + 0.0005 > shadow_projection.z);
}