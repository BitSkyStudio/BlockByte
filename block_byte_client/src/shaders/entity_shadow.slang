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

    vert_out.pos = shadow_distort_position(mul(PUSH_DATA.matrix, float4(vertex.position, 1.0)));
    vert_out.uv = vertex.uv;
    //vert_out.color = float4(vertex.color) / 255.;
    return vert_out;
}

[shader("fragment")]
void fragmentMain(float2 uv : TEXCOORD0) : SV_Target
{
    //return float4(1., 1., 0., 1.);
    float4 color = getTexture2D(0).SampleLevel(getSampler(0), uv, 0);
    if(color.a <= 0.1){
        discard;
    }
}