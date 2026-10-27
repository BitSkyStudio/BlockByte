// Standard no_gfx_api bindings, you can copy this into your project

module shared;

#define TEXTURE_HEAP_BINDING 0
#define RWTEXTURE_HEAP_BINDING 1
#define SAMPLER_HEAP_BINDING 2
#define BVH_HEAP_BINDING 3

#pragma warning(disable: 39001)

[[vk::binding(0, TEXTURE_HEAP_BINDING)]] public Texture2D texture2DHeap[];
[[vk::binding(0, TEXTURE_HEAP_BINDING)]] public Texture1D texture1DHeap[];
[[vk::binding(0, TEXTURE_HEAP_BINDING)]] public Texture3D texture3DHeap[];
[[vk::binding(0, TEXTURE_HEAP_BINDING)]] public TextureCube textureCubeHeap[];
[[vk::binding(0, TEXTURE_HEAP_BINDING)]] public Texture2DArray texture2DArrayHeap[];
[[vk::binding(0, TEXTURE_HEAP_BINDING)]] public TextureCubeArray textureCubeArrayHeap[];
[[vk::binding(0, TEXTURE_HEAP_BINDING)]] public Texture1DArray texture1DArrayHeap[];

[[vk::binding(0, RWTEXTURE_HEAP_BINDING)]] public RWTexture2D<float4> texture2DHeapRW[];
[[vk::binding(0, RWTEXTURE_HEAP_BINDING)]] public RWTexture1D<float4> texture1DHeapRW[];
[[vk::binding(0, RWTEXTURE_HEAP_BINDING)]] public RWTexture3D<float4> texture3DHeapRW[];
[[vk::binding(0, RWTEXTURE_HEAP_BINDING)]] public RWTexture2DArray<float4> texture2DArrayHeapRW[];
[[vk::binding(0, RWTEXTURE_HEAP_BINDING)]] public RWTexture1DArray<float4> texture1DArrayHeapRW[];

[[vk::binding(0, SAMPLER_HEAP_BINDING)]] public SamplerState samplerHeap[];
[[vk::binding(0, SAMPLER_HEAP_BINDING)]] public SamplerComparisonState compareSamplerHeap[];

public float normal_shading(float3 normal){
    let squared_normal = pow(normal, float3(2.));
    return 1. - abs(squared_normal.x) * 0.2 - abs(squared_normal.z) * 0.1 + min(squared_normal.y, 0.) * 0.3 - (normal.y > 0. ? 0. : squared_normal.y * 0.25);
}

public float4 shadow_distort_position(float4 position){
    return float4(shadow_distort_position2(position.xy), position.z, 1.);
}
public float2 shadow_distort_position2(float2 position){
    return position / (length(position) + 0.1);
}

public struct LightData{
    float4x4 light_matrix;
    float3 direction;
    uint32_t shadow_texture;
}

public float sample_shadow(LightData* light_data, float3 world_position, float3 normal){
    var floored_world_position = floor(world_position * 16.) / 16.;
    if(normal.x == 0.){
        floored_world_position.x += 1. / 32.;
    }
    if(normal.y == 0.){
        floored_world_position.y += 1. / 32.;
    }
    if(normal.z == 0.){
        floored_world_position.z += 1. / 32.;
    }
    var shadow_space = mul(light_data.light_matrix, float4(floored_world_position, 1.));
    let shadow_projection = float3(shadow_distort_position2(shadow_space.xy), shadow_space.z);

    let dark_color = 0.8;

    if(dot(normal, light_data.direction) <= 0.){
        return dark_color;
    }

    /*if(abs(shadow_projection.x) >= 1. || abs(shadow_projection.y) >= 1.){
        return 1.;
    }*/

    let shadow_uv = shadow_projection.xy * 0.5 * float2(1., 1.) + float2(0.5);
    let shadow_value = getTexture2DU(light_data.shadow_texture).Sample(getSamplerU(0), shadow_uv).x;
    if(shadow_value >= 1.){
        return 1.;
    }
    return (shadow_value + 0.0005 > shadow_projection.z) ? 1. : dark_color;
}

public struct DrawIndexedIndirectCommand
{
    uint indexCount;
    uint instanceCount;
    uint firstIndex;
    int  vertexOffset;
    uint firstInstance;
};

public SamplerState getSampler(uint samplerIndex) 
{
    return samplerHeap[NonUniformResourceIndex(samplerIndex)];
}

public SamplerState getSamplerU(uint samplerIndex) 
{
    return samplerHeap[samplerIndex];
}

public SamplerComparisonState getCompareSampler(uint samplerIndex)
{
    return compareSamplerHeap[NonUniformResourceIndex(samplerIndex)];
}

public Texture2D getTexture2D(uint textureIndex)
{
    return texture2DHeap[NonUniformResourceIndex(textureIndex)];
}

public Texture2D getTexture2DU(uint textureIndex)
{
    return texture2DHeap[textureIndex];
}

public Texture3D getTexture3D(uint textureIndex)
{
    return texture3DHeap[NonUniformResourceIndex(textureIndex)];
}

public Texture1D getTexture1D(uint textureIndex)
{
    return texture1DHeap[NonUniformResourceIndex(textureIndex)];
}

public TextureCube getTextureCube(uint textureIndex)
{
    return textureCubeHeap[NonUniformResourceIndex(textureIndex)];
}

public Texture2DArray getTexture2DArray(uint textureIndex)
{
    return texture2DArrayHeap[NonUniformResourceIndex(textureIndex)];
}

public TextureCubeArray getTextureCubeArray(uint textureIndex)
{
    return textureCubeArrayHeap[NonUniformResourceIndex(textureIndex)];
}

public Texture1DArray getTexture1DArray(uint textureIndex)
{
    return texture1DArrayHeap[NonUniformResourceIndex(textureIndex)];
}

public RWTexture2D<float4> getTexture2DRW(uint textureIndex)
{
    return texture2DHeapRW[NonUniformResourceIndex(textureIndex)];
}

public RWTexture1D<float4> getTexture1DRW(uint textureIndex)
{
    return texture1DHeapRW[NonUniformResourceIndex(textureIndex)];
}

public RWTexture3D<float4> getTexture3DRW(uint textureIndex)
{
    return texture3DHeapRW[NonUniformResourceIndex(textureIndex)];
}

public RWTexture2DArray<float4> getTexture2DArrayRW(uint textureIndex)
{
    return texture2DArrayHeapRW[NonUniformResourceIndex(textureIndex)];
}

public RWTexture1DArray<float4> getTexture1DArrayRW(uint textureIndex)
{
    return texture1DArrayHeapRW[NonUniformResourceIndex(textureIndex)];
}
