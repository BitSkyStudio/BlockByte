import "common.slang";

struct Vertex
{
    float3 position;
    vector<int8_t, 4> normal;
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
    float3 world;
}

struct BaseRenderData {
    float4x4 matrix;
    Vertex* vertices;
    LightData* light_data;
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

    vert_out.pos = mul(PUSH_DATA.matrix, float4(position, 1.0));
    vert_out.world = position;
    vert_out.uv = vertex.uv;
    //vert_out.color = float4(vertex.color) / 255.;
    vert_out.normal = float3(float(vertex.normal.x), float(vertex.normal.y), float(vertex.normal.z))/127.;
    return vert_out;
}

[shader("fragment")]
float4 fragmentMain(float2 uv : TEXCOORD0, float3 normal, float3 world) : SV_Target
{
    let color = getTexture2DU(0).Sample(getSamplerU(0), uv);
    if(color.a < 0.1){
        discard;
    }
    let material = float4(0., 0., 0., 0.);
    let albedo = color.rgb * (1. - material.r)/* + in.color * material.r*/;

    let ambient = float3(0.02f, 0.04f, 0.08f);
    let sunColor = float3(0.98f, 0.73f, 0.15f);
    let skyColor = float3(0.47, 0.65, 1.0);

    let lightColor = skyColor * 1.;

    var ndotl = sunColor * 1.; // clamp(4 * dot(normal, sunDirection), 0.0f, 1.0f) * sunVisibility;
    //ndotl += moonColor * clamp(4 * dot(normal, -sunDirection), 0.0f, 1.0f) * moonVisibility;
    ndotl *= 1.3;
    ndotl *= (dot(skyColor, float3(0.2125f, 0.7153f, 0.0721f)) + 0.01f);
    //ndotl *= lightmap.g;

    let lighting = (ndotl + lightColor + ambient) * sample_shadow(PUSH_DATA.light_data, world, normal) * normal_shading(normal);

    return float4(albedo * (lighting * (1. - material.g) + 1.5 * material.g), 1.);
    //return color;
    //return color;
}