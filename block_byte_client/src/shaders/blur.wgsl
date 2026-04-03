struct VertexOutput {
    @location(0) uv: vec2<f32>,
    @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
) -> VertexOutput {
    var out: VertexOutput;
    // Generate a triangle that covers the whole screen
    out.uv = vec2<f32>(
        f32((vi << 1u) & 2u),
        f32(vi & 2u),
    );
    out.clip_position = vec4<f32>(out.uv * 2.0 - 1.0, 0.0, 1.0);
    // We need to invert the y coordinate so the image
    // is not upside down
    out.uv.y = 1.0 - out.uv.y;
    return out;
}

@group(0)
@binding(0)
var hdr_image: texture_2d<f32>;

@group(0)
@binding(1)
var hdr_sampler: sampler;

@group(1)
@binding(0)
var<uniform> texel_size: vec2<f32>;

@fragment
fn fs_main(vs: VertexOutput) -> @location(0) vec4<f32> {
    const PI = radians(180.0);
    let sigma = 1.;
    let k = 2.0 * sigma * sigma;

    let size = i32(floor(sigma * 3.0));

    var rgba = vec4<f32>(0.0, 0.0, 0.0, 1.0);

    for(var i: i32 = -size; i <= size; i++) {
        for(var j: i32 = -size; j <= size; j++) {
            let i_f32 = f32(i);
            let j_f32 = f32(j);

            let fac = exp(-(i_f32*i_f32 + j_f32*j_f32) / k) / (PI * k);

            let sampled = textureSample(
                hdr_image, hdr_sampler,
                vec2<f32>(i_f32 * texel_size.x, j_f32 * texel_size.y) + vs.uv,
            );

            rgba += vec4<f32>(
                sampled.x * fac,
                sampled.y * fac,
                sampled.z * fac,
                0.0
            );
        }
    }
    return rgba;
}