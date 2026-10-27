import "common.slang";

struct Vertex
{
    float3 position;
    vector<uint8_t, 4> normal;
    float2 uv;
    uint16_t color;
    uint8_t shade;
    uint8_t flags;
}

struct VertexOutput
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    float3 normal;
}

struct BaseRenderData {
    float4x4 matrix;
    Vertex* vertices;
    LightData* light;
    float time;
}

[[vk::push_constant]] BaseRenderData PUSH_DATA;

[shader("vertex")]
VertexOutput vertexMain(uint vert_id : SV_VertexID)
{
    let vertex = PUSH_DATA.vertices[vert_id];
    VertexOutput vert_out;

    var position = vertex.position;

    if((vertex.flags & 1) != 0){
        position.x += sin(PUSH_DATA.time * 0.8 + position.x * 0.2) * 0.1;
        position.z += sin(PUSH_DATA.time * 0.8 + 10 + position.z * 0.2) * 0.1;
    }

    vert_out.pos = shadow_distort_position(mul(PUSH_DATA.matrix, float4(position, 1.0)));
    vert_out.uv = vertex.uv;
    //vert_out.color = float4(vertex.color) / 255.;
    vert_out.normal = max(float3(float(vertex.normal.x), float(vertex.normal.y), float(vertex.normal.z))/127., float3(-1.));
    return vert_out;
}

[shader("fragment")]
void fragmentMain(float2 uv : TEXCOORD0, float3 normal) : SV_Target
{
    let color = getTexture2DU(0).Sample(getSamplerU(0), uv);
    if(color.a < 0.1){
        discard;
    }
}