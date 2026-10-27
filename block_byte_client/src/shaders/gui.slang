import "common.slang";

struct GUIVertex
{
    float2 position;
    float2 uv;
    vector<uint8_t, 4> color;
}

struct VertexOutput
{
    float4 pos : SV_Position;
    float4 color;
    float2 uv : TEXCOORD0;
}

struct BaseRenderData {
    float4x4 matrix;
    GUIVertex* vertices;
}

[[vk::push_constant]] BaseRenderData PUSH_DATA;

[shader("vertex")]
VertexOutput vertexMain(uint vert_id : SV_VertexID)
{
    let vertex = PUSH_DATA.vertices[vert_id];
    VertexOutput vert_out;

    vert_out.pos = mul(float4(vertex.position, 0.0, 1.0), PUSH_DATA.matrix);
    vert_out.uv = vertex.uv;
    vert_out.color = float4(vertex.color) / 255.;
    return vert_out;
}

[shader("fragment")]
float4 fragmentMain(float4 color, float2 uv : TEXCOORD0) : SV_Target
{
    //return float4(1., 1., 0., 1.);
    let co = getTexture2D(0).SampleLevel(getSampler(0), uv, 0) * color;
    if(co.a <= 0.1){
        discard;
    }
    return float4(pow(co.rgb, float3(2.2)), co.a);
    //return color;
}