import "common.slang";

struct Vertex
{
    float3 position;
    float2 uv;
    float3 normal;
    vector<uint8_t, 4> color;
}

struct VertexOutput
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
}

struct BaseRenderData {
    float4x4 matrix;
    Vertex* vertices;
}

[[vk::push_constant]] BaseRenderData PUSH_DATA;

[shader("vertex")]
VertexOutput vertexMain(uint vert_id : SV_VertexID)
{
    let vertex = PUSH_DATA.vertices[vert_id];
    VertexOutput vert_out;

    vert_out.pos = mul(PUSH_DATA.matrix, float4(vertex.position, 0.0));
    vert_out.uv = vertex.uv;
    return vert_out;
}

[shader("fragment")]
float4 fragmentMain(float2 uv : TEXCOORD0) : SV_Target
{
    return getTexture2DU(1).SampleLevel(getSamplerU(0), uv, 0);
}