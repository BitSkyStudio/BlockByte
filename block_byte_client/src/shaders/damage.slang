import "common.slang";

struct DamageVertex
{
    float3 position;
    float2 uv;
    float progress;
}

struct VertexOutput
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    float progress;
}

struct BaseRenderData {
    float4x4 matrix;
    DamageVertex* vertices;
}

[[vk::push_constant]] BaseRenderData PUSH_DATA;

[shader("vertex")]
VertexOutput vertexMain(uint vert_id : SV_VertexID)
{
    let vertex = PUSH_DATA.vertices[vert_id];
    VertexOutput vert_out;

    vert_out.pos = mul(PUSH_DATA.matrix, float4(vertex.position, 1.0));
    vert_out.uv = vertex.uv;
    vert_out.progress = vertex.progress;
    return vert_out;
}

float rand(float2 co){
    return fract(sin(dot(co, float2(12.9898, 78.233))) * 43758.5453);
}

[shader("fragment")]
float4 fragmentMain(float2 uv : TEXCOORD0, float progress) : SV_Target
{
    let r = rand(floor(uv));
    let v = clamp(r+progress-1., 0., 1.);
    return float4(float3(0.6-v), v);
}