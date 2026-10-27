import "common.slang";

struct VertexOutput
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
}


[[vk::push_constant]] uint32_t PUSH_DATA;

[shader("vertex")]
VertexOutput vertexMain(uint vert_id : SV_VertexID)
{
    VertexOutput vert_out;
    const float2[4] FACE_UVS = {
        {0., 0.},
        {1., 0.},
        {1., 1.},
        {0., 1.},
    };
    vert_out.uv = FACE_UVS[vert_id];
    vert_out.pos = float4(vert_out.uv.x * 2. - 1., vert_out.uv.y * 2. - 1., 0., 1.);
    return vert_out;
}

float3 aces_tone_map(float3 hdr)
{
    const float3x3 m1 = float3x3(
        0.59719, 0.07600, 0.02840,
        0.35458, 0.90834, 0.13383,
        0.04823, 0.01566, 0.83777
    );

    const float3x3 m2 = float3x3(
         1.60475, -0.10208, -0.00327,
        -0.53108,  1.10813, -0.07276,
        -0.07367, -0.00605,  1.07602
    );

    float3 v = mul(hdr, m1);

    float3 a = v * (v + 0.0245786) - 0.000090537;
    float3 b = v * (0.983729 * v + 0.4329510) + 0.238081;

    return clamp(mul(a / b, m2), 0.0, 1.0);
}

[shader("fragment")]
float4 fragmentMain(float2 uv : TEXCOORD0) : SV_Target
{
    //return float4(1., 1., 0., 1.);
    float4 hdr = getTexture2D(PUSH_DATA).Sample(getSampler(0), uv);
    
    //float3 blur = blur_image.Sample(blur_sampler, input.uv).rgb;
    //float4 hdr = hdr_image.Sample(hdr_sampler, input.uv);


    float3 gamma_corrected = pow(hdr.rgb * 1.05, float3(2.2));

    float3 sdr = aces_tone_map(gamma_corrected);

    return float4(sdr.rgb, hdr.a);
}