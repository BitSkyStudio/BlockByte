import "common.slang";

struct Particle
{
    float3 position;
    float2 uv1;
    float2 uv2;
    float2 size;
}

struct VertexOutput
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
}

struct ParticleRenderData {
    float4x4 matrix;
    Particle* particles;
    float3 camera_up;
    float3 camera_right;
}

[[vk::push_constant]] ParticleRenderData PUSH_DATA;

[shader("vertex")]
VertexOutput vertexMain(uint vert_id : SV_VertexID, uint inst_id: SV_InstanceID)
{
    const float2[4] FACE_UVS = {
        {0., 0.},
        {1., 0.},
        {1., 1.},
        {0., 1.},
    };
    let fuv = FACE_UVS[vert_id];
    let particle = PUSH_DATA.particles[inst_id];
    VertexOutput vert_out;

    let offset = (fuv - 0.5) * particle.size;
    vert_out.pos = mul(PUSH_DATA.matrix, float4(particle.position + PUSH_DATA.camera_right * offset.x + PUSH_DATA.camera_up * offset.y, 1.0));
    vert_out.uv = particle.uv1 * fuv + particle.uv2 * (float2(1.) - fuv);
    return vert_out;
}

[shader("fragment")]
float4 fragmentMain(float2 uv : TEXCOORD0) : SV_Target
{
    //return float4(1., 1., 0., 1.);
    let co = getTexture2D(0).SampleLevel(getSampler(0), uv, 0);
    if(co.a <= 0.1){
        discard;
    }
    return co;
    //return color;
}