fn sample_shadow(world_position: vec3<f32>, normal: vec3<f32>) -> f32{
    if dot(normal, shadow_camera.direction) < 0.{
        return 0.7;
    }

    var shadow_space = shadow_camera.view_proj * vec4(world_position, 1.);
    let shadow_undistorted = shadow_distort_position(shadow_space.xy);
    let shadow_uv = shadow_undistorted.xy * 0.5 * vec2(1., -1.) + vec2<f32>(0.5);
    let bounds = abs(shadow_undistorted.xy);
    if max(bounds.x, bounds.y) > 0.99{
        return 1.;
    }

    let moments = textureSample(shadow_texture, shadow_sampler, shadow_uv).xy;
    if moments.x >= 1.{
        return 1;
    }
    let light = chebyshevUpperBound(moments, shadow_space.z);
    //return 0.75 + 0.25 * light;

    var sum = 0.;
    for(var i: i32 = 0; i <= 0; i++) {
        for(var j: i32 = 0; j <= 0; j++) {
            let shadow_value = textureSample(shadow_texture, shadow_sampler, shadow_uv + vec2(f32(i)/2048., f32(j)/2048.)).x;
            if shadow_value >= 1.{
                sum += 1;
            } else {
                sum += select(0., 1., shadow_value > shadow_space.z);
            }
        }
    }


    //let blend_area = 0.001;
    //return mix(0.75, 1., 1-clamp(-(shadow_value +  - shadow_space.z)/blend_area,0,1));
    
    return 0.75 + 0.25 * (sum / 1.);
}

fn chebyshevUpperBound( moments: vec2<f32>, distance: f32) -> f32 {
    if distance <= moments.x {
        return 1.0 ;
    }

    var variance = moments.y - (moments.x*moments.x);
    variance = max(variance,0.0002);

    let d = distance - moments.x;
    let p_max = variance / (variance + d*d);

    return p_max;
}