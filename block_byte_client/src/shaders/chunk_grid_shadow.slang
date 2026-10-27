import "common.slang";

struct ChunkBlockFace
{
    uint16_t block;
    uint16_t texture;
    uint16_t color;
    uint8_t face;
    uint8_t _pad;
}

struct VertexOutput
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    float3 normal;
}

struct ChunkRenderData {
    float4x4 matrix;
    ChunkEntryData* entries;
}

struct ChunkEntryData {
    float3 chunk_position;
    float _pad;
    ChunkBlockFace* faces;
}

[[vk::push_constant]] ChunkRenderData PUSH_DATA;

[shader("vertex")]
VertexOutput vertexMain(uint vert_id : SV_VertexID, uint inst_id: SV_InstanceID, uint draw_id: SV_DrawIndex)
{
    let entry = PUSH_DATA.entries[draw_id];

    let instance = entry.faces[inst_id];

    const uint32_t[4][6] FACE_VERTICES = {
        {3, 2, 0, 1},
        {6, 7, 5, 4},
        {2, 3, 7, 6},
        {1, 0, 4, 5},
        {2, 6, 4, 0},
        {7, 3, 1, 5},
    };
    const float3[6] FACE_NORMALS = {
        float3(0., 0., -1.),
        float3(0., 0., 1.),
        float3(0., 1., 0.),
        float3(0., -1., 0.),
        float3(-1., 0., 0.),
        float3(1., 0., 0.),
    };
    const float2[4] FACE_UVS = {
        {0., 0.},
        {1., 0.},
        {1., 1.},
        {0., 1.},
    };
    let position_x = float(instance.block & 31);
    let position_y = float((instance.block>>5) & 31);
    let position_z = float((instance.block>>10) & 31);

    let vertex = FACE_VERTICES[instance.face&7][vert_id];

    /*let position = entry.chunk_position + float3(position_x, position_y, position_z) + float3(
        ((vertex&1)!=0) ? 1. : 0., 
        ((vertex&2)!=0) ? 1. : 0.,
        ((vertex&4)!=0) ? 1. : 0.,
    );*/
    let position = entry.chunk_position + float3(position_x, position_y, position_z) + float3(
        float(vertex%2), 
        float((vertex/2)%2),
        vertex/4,
    );

    //let texture_cell = texture_dimension(instance.texture);

    let uv = FACE_UVS[(vert_id + 4 - ((instance.face>>3)&3))%4];

    /*var out: VertexOutput;
    out.tex_coords = vec2<f32>(texture_cell.x + texture_cell.z * uv.x, texture_cell.y + texture_cell.w * uv.y);
    out.normal = FACE_NORMALS[instance.face&7];
    out.clip_position = camera.view_proj * vec4<f32>(position, 1.0);
    out.world_position = position;
    out.color = convert_color(instance.color);
    return out;*/


    VertexOutput vert_out;

    vert_out.pos = shadow_distort_position(mul(PUSH_DATA.matrix, float4(position, 1.0)));

    let cell_x = float(instance.texture%128)/128.;
    let cell_y = float(instance.texture/128)/128.;
    vert_out.uv = float2(cell_x + uv.x/128., cell_y + uv.y/128.);
    vert_out.normal = FACE_NORMALS[instance.face&7];
    //vert_out.color = float4(vertex.color) / 255.;
    return vert_out;
}

[shader("fragment")]
void fragmentMain(float2 uv : TEXCOORD0, float3 normal) : SV_Target
{
    //return float4(1., 1., 0., 1.);
    let color = getTexture2DU(0).Sample(getSamplerU(0), uv);
    if(color.a < 0.1){
        discard;
    }
}