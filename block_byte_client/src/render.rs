use block_byte_common::coord::{AABB, Axis, BlockPos, CHUNK_SIZE, Face, Pos};
use block_byte_common::model::{DrawAnimation, ModelGeometry, ModelTexture};
use block_byte_common::registry::{
    BlockColor, BlockData, BlockKey, BlockRenderData, EntityData, ItemModel, ModelInstance,
};
use block_byte_common::rotation::BlockRotation;
use block_byte_common::{Color, InternString, TexCoords};
use cgmath::{InnerSpace, Matrix4, Point3, SquareMatrix, Vector3};
use image::RgbaImage;
use simple_game_engine::GameEngineInterface;
use simple_gpu::{
    BlendFactor, BlendOp, BlendState, DrawIndexedIndirectCommand, GPU, GPUArena, GPUBuffer,
    GPUCullMode, GPUDepthTestConfig, GPUDescriptorHeap, GPUPointer, GPUTexture, GraphicsShaders,
    IndexBufferFormat, MemoryKind, QueueKind, RenderPassAttachment, RenderPassDesc,
    RenderPassLoadOp, SamplerFilter, TextureDesc, TextureFormat, TextureRegion, TextureSampler,
    TextureUsage, TextureViewDesc,
};
use std::alloc::Layout;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::atlas::{TEXTURE_ATLAS, TexCoordsExt, TexCoordsIndexExt};
use crate::game::clipping::Frustum;
use crate::game::{ChunkGPUData, ClientGame, ClientPlayer, ModifiedChunkEntry};
use crate::ui::{UIPos, UIRect};

pub struct RenderState {
    pub gpu: Arc<GPU>,
    pub time: f32,
    #[allow(unused)]
    pub skybox: GPUTexture,
    #[allow(unused)]
    pub texture_atlas: GPUTexture,
    #[allow(unused)]
    pub persistent_arena: GPUArena,
    pub quad_index_buffer: GPUPointer,
    pub skybox_buffer: (GPUPointer, GPUPointer),
    per_frame: [PerFrameRenderData; 3],
    buffer_graveyard: Vec<(GPUBuffer, u32)>,
    shaders: ShaderLoader,
}

struct PerFrameRenderData {
    pub depth_buffer: GPUTexture,
    pub hdr_buffer: GPUTexture,
    pub shadow_buffer: GPUTexture,
}

const BASE_BLEND: BlendState = BlendState {
    color_op: BlendOp::Add,
    src_color_factor: BlendFactor::SrcAlpha,
    dst_color_factor: BlendFactor::OneMinusSrcAlpha,
    alpha_op: BlendOp::Add,
    src_alpha_factor: BlendFactor::One,
    dst_alpha_factor: BlendFactor::OneMinusSrcAlpha,
};

impl RenderState {
    pub fn new(gpu: Arc<GPU>, desc_heap: &mut GPUDescriptorHeap, skybox: RgbaImage) -> Self {
        desc_heap.set_samplers(
            0,
            &[TextureSampler {
                min: SamplerFilter::Nearest,
                mag: SamplerFilter::Nearest,
                ..Default::default()
            }],
        );
        desc_heap.set_samplers(
            1,
            &[TextureSampler {
                ..Default::default()
            }],
        );
        let texture_atlas = {
            let upload_semaphore = gpu.create_semaphore(0);
            let mip_levels = &TEXTURE_ATLAS.get().unwrap().texture_mips;
            let texture = gpu.create_texture(
                TextureDesc {
                    dimensions: [mip_levels[0].width(), mip_levels[0].height(), 1],
                    usage: TextureUsage::SAMPLED,
                    format: TextureFormat::ABGR8Unorm,
                    mip_count: mip_levels.len() as u32,
                    ..Default::default()
                },
                QueueKind::Main,
                Some((&upload_semaphore, 1)),
            );
            let mut upload_arena = gpu.allocate_arena(16 * 1024 * 1024);
            let mut upload_buffer = gpu.begin_commands(QueueKind::Main);
            upload_buffer.add_wait(&upload_semaphore, 1);
            upload_buffer.add_signal(&upload_semaphore, 2);
            for (i, image) in mip_levels.iter().enumerate() {
                let image_buffer = image
                    .pixels()
                    .map(|rgba| rgba.0.into_iter())
                    .flatten()
                    .collect::<Vec<_>>();
                upload_buffer.copy_to_texture(
                    &texture,
                    TextureRegion {
                        mip_level: i as u32,
                        ..Default::default()
                    },
                    upload_arena.alloc(image_buffer.as_slice()),
                );
            }
            upload_buffer.submit();
            upload_semaphore.wait(2);
            desc_heap.set_textures(
                0,
                &[(
                    &texture,
                    TextureViewDesc {
                        mip_count: mip_levels.len() as u8,
                        ..Default::default()
                    },
                )],
            );
            texture
        };
        let skybox_texture = {
            let upload_semaphore = gpu.create_semaphore(0);
            let texture = gpu.create_texture(
                TextureDesc {
                    dimensions: [skybox.width(), skybox.height(), 1],
                    usage: TextureUsage::SAMPLED,
                    format: TextureFormat::ABGR8SRGB,
                    mip_count: 1,
                    ..Default::default()
                },
                QueueKind::Main,
                Some((&upload_semaphore, 1)),
            );
            let mut upload_arena = gpu.allocate_arena(16 * 1024 * 1024);
            let mut upload_buffer = gpu.begin_commands(QueueKind::Main);
            upload_buffer.add_wait(&upload_semaphore, 1);
            upload_buffer.add_signal(&upload_semaphore, 2);
            let image_buffer = skybox
                .pixels()
                .map(|rgba| rgba.0.into_iter())
                .flatten()
                .collect::<Vec<_>>();
            upload_buffer.copy_to_texture(
                &texture,
                TextureRegion {
                    ..Default::default()
                },
                upload_arena.alloc(image_buffer.as_slice()),
            );
            upload_buffer.submit();
            upload_semaphore.wait(2);
            desc_heap.set_textures(1, &[(&texture, TextureViewDesc::default())]);
            texture
        };
        let mut skybox_mesh: Mesh<Vertex> = Mesh::default();
        {
            let mut skybox_vertex_consumer = skybox_mesh.consumer(Color::WHITE);
            for (face, tx, ty) in [
                (Face::Front, 3, 1),
                (Face::Back, 1, 1),
                (Face::Left, 2, 1),
                (Face::Right, 0, 1),
                (Face::Down, 1, 2),
                (Face::Up, 1, 0),
            ] {
                skybox_vertex_consumer.add_quad(
                    face.get_vertices(
                        TexCoords {
                            u2: tx as f32 / 4.,
                            v1: ty as f32 / 3.,
                            u1: (tx + 1) as f32 / 4.,
                            v2: (ty + 1) as f32 / 3.,
                        },
                        match face {
                            Face::Down => 2,
                            _ => 0,
                        },
                    )
                    .map(|(position, uv)| MeshVertex {
                        position: position * 2. - Pos::all(1.),
                        normal: Pos::all(0.),
                        uv,
                    }),
                );
            }
        }
        let mut persistent_arena = gpu.allocate_arena(1024 * 1024);
        let skybox_buffer = (
            persistent_arena.alloc(&skybox_mesh.vertices),
            persistent_arena.alloc(&skybox_mesh.indices),
        );
        let quad_index_buffer = persistent_arena.alloc::<u16>(&[2, 1, 0, 0, 3, 2]);
        let per_frame = std::array::from_fn(|i| {
            let depth_buffer = gpu.create_texture(
                TextureDesc {
                    dimensions: [128, 128, 1],
                    format: TextureFormat::D32Float,
                    mip_count: 1,
                    layer_count: 1,
                    sample_count: 1,
                    usage: TextureUsage::DEPTH_STENCIL_ATTACHMENT,
                    ..Default::default()
                },
                QueueKind::Main,
                None,
            );
            let hdr_buffer = gpu.create_texture(
                TextureDesc {
                    dimensions: [128, 128, 1],
                    format: TextureFormat::RGBA16Float,
                    mip_count: 1,
                    layer_count: 1,
                    sample_count: 1,
                    usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
                    ..Default::default()
                },
                QueueKind::Main,
                None,
            );
            let shadow_buffer = gpu.create_texture(
                TextureDesc {
                    dimensions: [2048 * 2, 2048 * 2, 1],
                    format: TextureFormat::D32Float,
                    mip_count: 1,
                    layer_count: 1,
                    sample_count: 1,
                    usage: TextureUsage::DEPTH_STENCIL_ATTACHMENT | TextureUsage::SAMPLED,
                    ..Default::default()
                },
                QueueKind::Main,
                None,
            );
            desc_heap.set_textures(5 + i, &[(&shadow_buffer, TextureViewDesc::default())]);
            PerFrameRenderData {
                depth_buffer,
                hdr_buffer,
                shadow_buffer,
            }
        });
        gpu.queue_wait_idle(QueueKind::Main);
        RenderState {
            gpu: gpu.clone(),
            time: 0.,
            persistent_arena,
            quad_index_buffer,
            texture_atlas,
            per_frame,
            buffer_graveyard: Vec::new(),
            skybox: skybox_texture,
            skybox_buffer,
            shaders: ShaderLoader {
                gpu,
                shaders: HashMap::new(),
            },
        }
    }
    pub fn tick(&mut self, engine: &mut GameEngineInterface) {
        if engine.resized {
            for (i, per_frame) in self.per_frame.iter_mut().enumerate() {
                let depth_buffer = self.gpu.create_texture(
                    TextureDesc {
                        dimensions: [engine.width, engine.height, 1],
                        format: TextureFormat::D32Float,
                        mip_count: 1,
                        layer_count: 1,
                        sample_count: 1,
                        usage: TextureUsage::DEPTH_STENCIL_ATTACHMENT,
                        ..Default::default()
                    },
                    QueueKind::Main,
                    None,
                );
                per_frame.depth_buffer = depth_buffer;
                let hdr_buffer = self.gpu.create_texture(
                    TextureDesc {
                        dimensions: [engine.width, engine.height, 1],
                        format: TextureFormat::RGBA16Float,
                        mip_count: 1,
                        layer_count: 1,
                        sample_count: 1,
                        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
                        ..Default::default()
                    },
                    QueueKind::Main,
                    None,
                );
                per_frame.hdr_buffer = hdr_buffer;
                engine.desc_heap.set_textures(
                    2 + i,
                    &[(&per_frame.hdr_buffer, TextureViewDesc::default())],
                );
            }
            self.gpu.queue_wait_idle(QueueKind::Main);
        }
        self.buffer_graveyard.retain_mut(|(_, i)| {
            if *i == 0 {
                return false;
            }
            *i -= 1;
            true
        });
    }
    pub fn render(
        &mut self,
        engine: &mut GameEngineInterface,
        game: &mut ClientGame,
        entity_mesh: BaseMesh,
        _local_player_mesh: BaseMesh,
        gui_mesh: GUIMesh,
        viewmodel_mesh: BaseMesh,
        damage_mesh: DamageMesh,
        particles: Vec<GPUParticleInstance>,
        frustum: &Frustum,
    ) {
        let per_frame = &mut self.per_frame[engine.frame_flight_index as usize];
        while let Ok((chunk, mesh, detail_mesh, version, lod_only_faces)) =
            game.chunk_mesh_channels.1.try_recv()
        {
            let Some(chunk) = game.chunks.get_mut(&chunk) else {
                continue;
            };
            chunk.scheduled = false;
            if version
                < chunk
                    .mesh_build_data
                    .version
                    .load(std::sync::atomic::Ordering::Relaxed)
            {
                if !chunk.modified {
                    chunk.modified = true;
                    game.modified_chunks.push(ModifiedChunkEntry {
                        distance: game
                            .player_position
                            .to_chunk_pos()
                            .distance_squared(chunk.position)
                            as usize,
                        chunk: chunk.position,
                    });
                }
            }
            let grid_buffer = if mesh.vertices.is_empty() {
                None
            } else {
                let buffer = self.gpu.allocate_buffer(
                    Layout::array::<GPUBlockFace>(mesh.vertices.len()).unwrap(),
                    MemoryKind::GPU,
                );
                let vertices = engine.frame_arena.alloc(&mesh.vertices);

                engine
                    .commands
                    .copy(vertices, buffer.gpu_pointer().slice(0, vertices.size()));
                Some(buffer)
                //Some(self.gpu.allocate_buffer_with(&mesh.vertices))
            };
            let (detail_buffer, detail_index_offset, detail_index_count) =
                if detail_mesh.vertices.is_empty() {
                    (None, 0, 0)
                } else {
                    let mut data = Vec::new();
                    data.extend_from_slice(bytemuck::cast_slice::<_, u8>(&detail_mesh.vertices));
                    let indices_start = data.len();
                    data.extend_from_slice(bytemuck::cast_slice::<_, u8>(&detail_mesh.indices)); //todo: maybe alignment broken?
                    let buffer = self
                        .gpu
                        .allocate_buffer(Layout::array::<u8>(data.len()).unwrap(), MemoryKind::GPU);
                    let data_gpu = engine.frame_arena.alloc(&data);

                    engine
                        .commands
                        .copy(data_gpu, buffer.gpu_pointer().slice(0, data.len() as u64));
                    (
                        Some(buffer),
                        indices_start as u32,
                        ((data.len() - indices_start) / 4) as u32,
                    )
                };
            if let Some(previous) = chunk.render_data.take() {
                if let Some(grid_buffer) = previous.grid_buffer {
                    self.buffer_graveyard.push((grid_buffer, 5));
                }
                if let Some(detail_buffer) = previous.detail_buffer {
                    self.buffer_graveyard.push((detail_buffer, 5));
                }
            }
            chunk.render_data = Some(ChunkGPUData {
                grid_buffer,
                grid_count: mesh.vertices.len(),
                lod_only_faces,
                detail_buffer,
                detail_index_count,
                detail_index_offset,
            });
        }
        let entity_vertices = engine.frame_arena.alloc(&entity_mesh.vertices);
        let entity_indices = engine.frame_arena.alloc(&entity_mesh.indices);
        let mut lcu = CameraUniform::new();
        lcu.load_light(game.camera.position);
        #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
        #[repr(C)]
        struct LightData {
            matrix: [[f32; 4]; 4],
            direction: [f32; 3],
            shadow_index: u32,
        }
        let light_data = engine.frame_arena.alloc(&[LightData {
            matrix: lcu.view_proj,
            direction: lcu.direction,
            shadow_index: 5 + engine.frame_flight_index,
        }]);
        {
            let mut render_pass = engine.commands.begin_render(RenderPassDesc {
                color_attachments: &[],
                depth_attachment: Some(RenderPassAttachment {
                    load: RenderPassLoadOp::Clear(1.),
                    store: simple_gpu::RenderPassStoreOp::Store,
                    resolve: None,
                    texture: &per_frame.shadow_buffer,
                    view: TextureViewDesc::default(),
                }),
                ..Default::default()
            });

            render_pass.set_shaders(self.shaders.get_shader("chunk_detail_shadow"));
            render_pass.set_blend(0, Some(BASE_BLEND));
            render_pass.set_culling(GPUCullMode::KeepCCW);
            render_pass.set_depth_test(Some(GPUDepthTestConfig::default()));
            #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
            #[repr(C)]
            struct ChunkDataEntry {
                chunk_position: [f32; 3],
                _pad: f32,
                pointer: u64,
            }
            let mut indirect_commands = Vec::new();
            let mut buffer_pointers = Vec::new();
            for chunk in game.chunks.values() {
                if let Some(render_data) = &chunk.render_data {
                    let chunk_pos = chunk.position.to_block_pos().to_pos();
                    let is_near = chunk
                        .position
                        .with(Axis::Y, 0)
                        .distance_squared(game.player_position.to_chunk_pos().with(Axis::Y, 0))
                        <= 3;
                    let is_far = chunk
                        .position
                        .with(Axis::Y, 0)
                        .distance_squared(game.player_position.to_chunk_pos().with(Axis::Y, 0))
                        >= 6_i16.pow(2);
                    if let Some(grid_buffer) = &render_data.grid_buffer
                        && !is_far
                    {
                        indirect_commands.push(DrawIndexedIndirectCommand {
                            first_index: 0,
                            first_instance: 0,
                            index_count: 6,
                            instance_count: render_data.grid_count as u32
                                - if is_near {
                                    render_data.lod_only_faces as u32
                                } else {
                                    0
                                },
                            vertex_offset: 0,
                        });
                        buffer_pointers.push(ChunkDataEntry {
                            pointer: grid_buffer.gpu(),
                            chunk_position: chunk_pos.into_array(),
                            _pad: 0.,
                        });
                    }
                    if let Some(detail_buffer) = &render_data.detail_buffer
                        && is_near
                    {
                        render_pass.draw(
                            BaseRenderData {
                                matrix: lcu.view_proj,
                                vertices: detail_buffer.gpu(),
                                light: light_data.gpu(),
                                time: self.time,
                                _pad: 0.,
                            },
                            render_data.detail_index_count,
                            1,
                            Some((
                                detail_buffer.gpu_pointer().slice(
                                    render_data.detail_index_offset as u64,
                                    render_data.detail_index_count as u64,
                                ),
                                IndexBufferFormat::U32,
                            )),
                        );
                    }
                }
            }
            render_pass.set_shaders(self.shaders.get_shader("chunk_grid_shadow"));
            render_pass.draw_indirect(
                BaseRenderData {
                    matrix: lcu.view_proj,
                    vertices: engine.frame_arena.alloc(&buffer_pointers).gpu(),
                    light: light_data.gpu(),
                    time: self.time,
                    _pad: 0.,
                },
                engine.frame_arena.alloc(&indirect_commands),
                indirect_commands.len() as u32,
                Some((self.quad_index_buffer, IndexBufferFormat::U16)),
            );
            {
                render_pass.set_shaders(self.shaders.get_shader("entity_shadow"));
                render_pass.draw(
                    BaseRenderData {
                        matrix: lcu.view_proj,
                        vertices: entity_vertices.gpu(),
                        light: light_data.gpu(),
                        time: self.time,
                        _pad: 0.,
                    },
                    entity_mesh.indices.len() as u32,
                    1,
                    Some((entity_indices, IndexBufferFormat::U32)),
                );
            }
        }
        engine.commands.texture_barrier(
            simple_gpu::GPUStage::All,
            simple_gpu::GPUStage::All,
            &per_frame.shadow_buffer,
        );
        let mut cu = CameraUniform::new();
        cu.load_camera_proj_matrix(
            &game.camera,
            1. / (engine.height as f32 / engine.width as f32),
            90.,
            game.get_player_data(),
        );
        {
            let mut render_pass = engine.commands.begin_render(RenderPassDesc {
                color_attachments: &[RenderPassAttachment {
                    load: RenderPassLoadOp::Clear([0., 0., 0., 0.]),
                    store: simple_gpu::RenderPassStoreOp::Store,
                    texture: &per_frame.hdr_buffer,
                    view: TextureViewDesc::default(),
                    resolve: None,
                }],
                depth_attachment: Some(RenderPassAttachment {
                    load: RenderPassLoadOp::Clear(1.),
                    store: simple_gpu::RenderPassStoreOp::Store,
                    resolve: None,
                    texture: &per_frame.depth_buffer,
                    view: TextureViewDesc::default(),
                }),
                ..Default::default()
            });

            render_pass.set_shaders(self.shaders.get_shader("chunk_detail"));
            render_pass.set_blend(0, Some(BASE_BLEND));
            render_pass.set_culling(GPUCullMode::KeepCCW);
            render_pass.set_depth_test(Some(GPUDepthTestConfig::default()));
            #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
            #[repr(C)]
            struct ChunkDataEntry {
                chunk_position: [f32; 3],
                _pad: f32,
                pointer: u64,
            }
            let mut indirect_commands = Vec::new();
            let mut buffer_pointers = Vec::new();
            for chunk in game.chunks.values() {
                if let Some(render_data) = &chunk.render_data {
                    let chunk_pos = chunk.position.to_block_pos().to_pos();
                    if !frustum.intersects_aabb(
                        &AABB {
                            min: Pos::all(0.),
                            max: Pos::all(CHUNK_SIZE as f32),
                        }
                        .offset(chunk_pos),
                    ) {
                        continue;
                    }
                    let is_far = chunk
                        .position
                        .with(Axis::Y, 0)
                        .distance_squared(game.player_position.to_chunk_pos().with(Axis::Y, 0))
                        >= 6_i16.pow(2);
                    if let Some(grid_buffer) = &render_data.grid_buffer {
                        indirect_commands.push(DrawIndexedIndirectCommand {
                            first_index: 0,
                            first_instance: 0,
                            index_count: 6,
                            instance_count: render_data.grid_count as u32
                                - if is_far {
                                    0
                                } else {
                                    render_data.lod_only_faces as u32
                                },
                            vertex_offset: 0,
                        });
                        buffer_pointers.push(ChunkDataEntry {
                            pointer: grid_buffer.gpu(),
                            chunk_position: chunk_pos.into_array(),
                            _pad: 0.,
                        });
                    }
                    if let Some(detail_buffer) = &render_data.detail_buffer
                        && !is_far
                    {
                        render_pass.draw(
                            BaseRenderData {
                                matrix: cu.view_proj,
                                vertices: detail_buffer.gpu(),
                                light: light_data.gpu(),
                                time: self.time,
                                _pad: 0.,
                            },
                            render_data.detail_index_count,
                            1,
                            Some((
                                detail_buffer.gpu_pointer().slice(
                                    render_data.detail_index_offset as u64,
                                    render_data.detail_index_count as u64,
                                ),
                                IndexBufferFormat::U32,
                            )),
                        );
                    }
                }
            }
            render_pass.set_shaders(self.shaders.get_shader("chunk_grid"));
            render_pass.draw_indirect(
                BaseRenderData {
                    matrix: cu.view_proj,
                    vertices: engine.frame_arena.alloc(&buffer_pointers).gpu(),
                    light: light_data.gpu(),
                    time: self.time,
                    _pad: 0.,
                },
                engine.frame_arena.alloc(&indirect_commands),
                indirect_commands.len() as u32,
                Some((self.quad_index_buffer, IndexBufferFormat::U16)),
            );
            {
                render_pass.set_shaders(self.shaders.get_shader("entity"));
                render_pass.draw(
                    BaseRenderData {
                        matrix: cu.view_proj,
                        vertices: entity_vertices.gpu(),
                        light: light_data.gpu(),
                        time: self.time,
                        _pad: 0.,
                    },
                    entity_mesh.indices.len() as u32,
                    1,
                    Some((entity_indices, IndexBufferFormat::U32)),
                );
            }
            if !damage_mesh.vertices.is_empty() {
                let damage_vertices = engine.frame_arena.alloc(&damage_mesh.vertices);
                let damage_indices = engine.frame_arena.alloc(&damage_mesh.indices);
                render_pass.set_shaders(self.shaders.get_shader("damage"));
                render_pass.set_depth_test(Some(GPUDepthTestConfig {
                    compare: simple_gpu::GPUCompareOp::LessEqual,
                    ..Default::default()
                }));
                render_pass.draw(
                    BaseRenderData {
                        matrix: cu.view_proj,
                        vertices: damage_vertices.gpu(),
                        light: light_data.gpu(),
                        time: self.time,
                        _pad: 0.,
                    },
                    damage_mesh.indices.len() as u32,
                    1,
                    Some((damage_indices, IndexBufferFormat::U32)),
                );
            }
            if !particles.is_empty() {
                let particle_instances = engine.frame_arena.alloc(&particles);
                render_pass.set_shaders(self.shaders.get_shader("particle"));
                #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
                #[repr(C)]
                struct ParticleRenderData {
                    matrix: [[f32; 4]; 4],
                    pointer: u64,
                    camera_up: [f32; 3],
                    camera_right: [f32; 3],
                }

                render_pass.draw(
                    ParticleRenderData {
                        matrix: cu.view_proj,
                        pointer: particle_instances.gpu(),
                        camera_right: [cu.view[0][0], cu.view[1][0], cu.view[2][0]],
                        camera_up: [cu.view[0][1], cu.view[1][1], cu.view[2][1]],
                    },
                    6,
                    particles.len() as u32,
                    Some((self.quad_index_buffer, IndexBufferFormat::U16)),
                );
            }
        }
        if !viewmodel_mesh.vertices.is_empty() {
            let mut render_pass = engine.commands.begin_render(RenderPassDesc {
                color_attachments: &[RenderPassAttachment {
                    load: RenderPassLoadOp::Load,
                    store: simple_gpu::RenderPassStoreOp::Store,
                    texture: &per_frame.hdr_buffer,
                    view: TextureViewDesc::default(),
                    resolve: None,
                }],
                depth_attachment: Some(RenderPassAttachment {
                    load: RenderPassLoadOp::Clear(1.),
                    store: simple_gpu::RenderPassStoreOp::Store,
                    resolve: None,
                    texture: &per_frame.depth_buffer,
                    view: TextureViewDesc::default(),
                }),
                ..Default::default()
            });
            let viewmodel_vertices = engine.frame_arena.alloc(&viewmodel_mesh.vertices);
            let viewmodel_indices = engine.frame_arena.alloc(&viewmodel_mesh.indices);
            render_pass.set_shaders(self.shaders.get_shader("entity"));
            render_pass.set_culling(GPUCullMode::KeepCCW);
            render_pass.set_depth_test(Some(GPUDepthTestConfig::default()));
            render_pass.draw(
                BaseRenderData {
                    matrix: cu.view_proj,
                    vertices: viewmodel_vertices.gpu(),
                    light: light_data.gpu(),
                    time: self.time,
                    _pad: 0.,
                },
                viewmodel_mesh.indices.len() as u32,
                1,
                Some((viewmodel_indices, IndexBufferFormat::U32)),
            );
        }
        engine.commands.texture_barrier(
            simple_gpu::GPUStage::All,
            simple_gpu::GPUStage::All,
            &per_frame.hdr_buffer,
        );
        {
            let mut render_pass = engine.commands.begin_render(RenderPassDesc {
                color_attachments: &[RenderPassAttachment {
                    load: RenderPassLoadOp::Clear([0., 0., 0., 1.]),
                    store: simple_gpu::RenderPassStoreOp::Store,
                    texture: &engine.framebuffer,
                    view: TextureViewDesc::default(),
                    resolve: None,
                }],
                ..Default::default()
            });
            render_pass.set_blend(0, Some(BASE_BLEND));
            render_pass.set_shaders(self.shaders.get_shader("skybox"));
            render_pass.draw(
                BaseRenderData {
                    matrix: cu.view_proj,
                    vertices: self.skybox_buffer.0.gpu(),
                    light: light_data.gpu(),
                    time: self.time,
                    _pad: 0.,
                },
                6 * 6,
                1,
                Some((self.skybox_buffer.1, IndexBufferFormat::U32)),
            );

            render_pass.set_shaders(self.shaders.get_shader("postprocess"));
            render_pass.draw(
                2 + engine.frame_flight_index,
                6,
                1,
                Some((self.quad_index_buffer, IndexBufferFormat::U16)),
            );
        }

        self.render_gui(engine, gui_mesh, false);
    }
    pub fn render_gui(&mut self, engine: &mut GameEngineInterface, gui_mesh: GUIMesh, clear: bool) {
        let indices = engine.frame_arena.alloc(&gui_mesh.indices);
        let vertices = engine.frame_arena.alloc(&gui_mesh.vertices);
        let mut render_pass = engine.commands.begin_render(RenderPassDesc {
            color_attachments: &[RenderPassAttachment {
                load: if clear {
                    RenderPassLoadOp::Clear([0., 0., 0., 1.])
                } else {
                    RenderPassLoadOp::Load
                },
                store: simple_gpu::RenderPassStoreOp::Store,
                texture: &engine.framebuffer,
                view: TextureViewDesc::default(),
                resolve: None,
            }],
            ..Default::default()
        });
        render_pass.set_shaders(self.shaders.get_shader("gui"));
        render_pass.set_blend(0, Some(BASE_BLEND));
        let mut cu = CameraUniform::new();
        cu.load_gui_matrix(engine.height as f32 / engine.width as f32);
        render_pass.draw(
            BaseRenderData {
                matrix: cu.view_proj,
                vertices: vertices.gpu(),
                light: 0,
                time: self.time,
                _pad: 0.,
            },
            gui_mesh.indices.len() as u32,
            1,
            Some((indices, IndexBufferFormat::U32)),
        );
    }
}

struct ShaderLoader {
    gpu: Arc<GPU>,
    shaders: HashMap<String, GraphicsShaders>,
}
static SHADER_DIR: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/src/shaders/spirv");
impl ShaderLoader {
    fn get_shader(&mut self, shader: &str) -> &GraphicsShaders {
        self.shaders.entry(shader.to_string()).or_insert_with(|| {
            let buffer = SHADER_DIR
                .get_file(&format!("{}.spv", shader))
                .expect(shader)
                .contents();
            self.gpu.create_shaders_graphic_spirv(
                buffer,
                "vertexMain",
                "fragmentMain",
                &HashMap::new(),
            )
        })
    }
}

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct BaseRenderData {
    matrix: [[f32; 4]; 4],
    vertices: u64,
    light: u64,
    time: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkVertex {
    pub position: [f32; 3],
    pub normals: [i8; 4],
    pub tex_coords: [f32; 2],
    pub color: u16,
    pub shade: u8,
    pub flags: u8,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
    pub normals: [f32; 3],
    pub color: [u8; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GUIVertex {
    pub position: [f32; 2],
    pub tex_coords: [f32; 2],
    pub color: [u8; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DamageVertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
    pub progress: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BillboardVertex {
    pub position: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GPUParticleInstance {
    pub position: [f32; 3],
    pub uv1: [f32; 2],
    pub uv2: [f32; 2],
    pub size: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GPUBlockFace {
    pub block: u16,
    pub texture: u16,
    pub color: u16,
    pub face: u8,
    pub _pad: [u8; 1],
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    direction: [f32; 3],
    _pad: f32,
}
impl CameraUniform {
    fn new() -> Self {
        Self {
            view_proj: cgmath::Matrix4::identity().into(),
            view: cgmath::Matrix4::identity().into(),
            direction: [0.; 3],
            _pad: 0.,
        }
    }
    fn load_light(&mut self, player_pos: Pos) {
        let dir = Vector3 {
            x: -10.,
            y: 20.,
            z: -5.,
        };
        self.direction = dir.normalize().into();
        let eye = Point3 {
            x: player_pos.x.floor(),
            y: player_pos.y.floor() + 0.,
            z: player_pos.z.floor(),
        } + dir * 4.;
        let shadow_size = 160.;
        self.view_proj = (Self::OPENGL_TO_WGPU_MATRIX
            * cgmath::ortho(
                -shadow_size,
                shadow_size,
                -shadow_size,
                shadow_size,
                -shadow_size,
                shadow_size,
            )
            * cgmath::Matrix4::look_to_rh(eye, -dir, Vector3::unit_y()))
        .into();
    }
    fn load_camera_proj_matrix(
        &mut self,
        camera: &ClientPlayer,
        aspect_ratio: f32,
        fov: f32,
        player_entity_data: Option<&EntityData>,
    ) {
        self.load_view_proj_matrix(
            aspect_ratio,
            fov,
            camera.get_eye(player_entity_data),
            camera.direction.make_front(),
        );
    }
    fn load_view_proj_matrix(&mut self, aspect_ratio: f32, fov: f32, eye: Pos, front: Pos) {
        let eye = Point3 {
            x: eye.x,
            y: eye.y,
            z: eye.z,
        };
        self.view_proj = (Self::OPENGL_TO_WGPU_MATRIX
            * ClientPlayer::create_projection_matrix(aspect_ratio, fov)
            * cgmath::Matrix4::look_at_rh(
                eye,
                eye + cgmath::Vector3 {
                    x: front.x,
                    y: front.y,
                    z: front.z,
                },
                Vector3::unit_y(),
            ))
        .into();
        self.view = (Self::OPENGL_TO_WGPU_MATRIX
            * cgmath::Matrix4::look_at_rh(
                eye,
                eye + cgmath::Vector3 {
                    x: front.x,
                    y: front.y,
                    z: front.z,
                },
                Vector3::unit_y(),
            ))
        .into();
    }
    #[rustfmt::skip]
    fn load_gui_matrix(&mut self, aspect_ratio: f32) {
        self.view_proj = (Self::OPENGL_TO_WGPU_MATRIX
            * cgmath::Matrix4::new(
                aspect_ratio, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ))
        .into();
    }
    #[rustfmt::skip]
    pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::new(
        1.0, 0.0, 0.0, 0.0,
        0.0, -1.0, 0.0, 0.0,
        0.0, 0.0, 0.5, 0.5,
        0.0, 0.0, 0.0, 1.0,
    );
}

pub fn draw_block_model(
    block_key: BlockKey,
    matrix: Matrix4<f32>,
    vertex_consumer: &mut impl MeshVertexConsumer,
) {
    let block = block_key.data();
    match &block.render_data {
        BlockRenderData::Air => {}
        BlockRenderData::Full { faces, .. } => {
            for face in Face::all() {
                vertex_consumer.add_quad(
                    face.get_vertices(faces[face].tex_coords(face as usize), 0)
                        .map(|(position, uv)| {
                            let position = position - Pos::XZ_HALF;
                            MeshVertex {
                                position: position.multiply_point(matrix),
                                normal: face.get_offset().multiply_vector(matrix).normalize(),
                                uv,
                            }
                        }),
                );
            }
        }
        BlockRenderData::Model { model, .. } => {
            draw_model(model, matrix, vertex_consumer, &[], |_, _| None);
        }
    }
}

pub fn draw_model<C: MeshVertexConsumer>(
    model: &ModelInstance,
    matrix: Matrix4<f32>,
    vertex_consumer: &mut C,
    animations: &[DrawAnimation],
    binding_query: impl Fn(InternString, &mut C) -> Option<Cow<'static, ItemModel>>,
) {
    let model_data = &model.model.data().model;
    let embed_textures = &TEXTURE_ATLAS.get().unwrap().models[model.model.numeric_id()];
    let mut item_models = Vec::new();
    model_data.draw(
        matrix,
        animations,
        |geometry| match geometry {
            ModelGeometry::Quad(vertices, texture) => {
                let texture = match &model_data.textures[texture].0 {
                    ModelTexture::Embed(_, index) => embed_textures[*index],
                    ModelTexture::Variable(variable) => model.textures[*variable].tex_coords(),
                    ModelTexture::Texture(key) => key.tex_coords(),
                };
                vertex_consumer.add_quad(vertices.map(|vertex| MeshVertex {
                    position: vertex.position,
                    normal: vertex.normal,
                    uv: texture.map(vertex.uv),
                }));
            }
            ModelGeometry::Triangle(_vertices, _texture) => todo!(),
        },
        |matrix, binding| {
            item_models.push((matrix, binding));
        },
    );
    for (matrix, binding) in item_models {
        if let Some(model) = binding_query(binding, vertex_consumer) {
            let anchor = match &*model {
                ItemModel::Block(block) => match &block.data().render_data {
                    BlockRenderData::Air => Matrix4::identity(),
                    BlockRenderData::Full { .. } => Matrix4::identity(),
                    BlockRenderData::Model { model: key, .. } => key
                        .model
                        .data()
                        .model
                        .anchor(binding, Matrix4::from_scale(block.data().item_scale), &[])
                        .map(|matrix| {
                            Matrix4::from_scale(block.data().item_scale) * matrix.invert().unwrap()
                        })
                        .unwrap_or(Matrix4::identity()),
                },
                ItemModel::Model(key) => key
                    .model
                    .data()
                    .model
                    .anchor(binding, Matrix4::identity(), &[])
                    .map(|matrix| matrix.invert().unwrap())
                    .unwrap_or(Matrix4::identity()),
            };
            draw_item_model(&*model, matrix * anchor, vertex_consumer);
        }
    }
}
pub fn draw_item_model(
    model: &ItemModel,
    matrix: Matrix4<f32>,
    vertex_consumer: &mut impl MeshVertexConsumer,
) {
    match model {
        ItemModel::Block(key) => {
            draw_block_model(
                *key,
                matrix * Matrix4::from_scale(key.data().item_scale),
                vertex_consumer,
            );
        }
        ItemModel::Model(model) => {
            draw_model(model, matrix, vertex_consumer, &[], |_, _| None);
        }
    }
}
pub fn item_model_icon_view(model: &ItemModel) -> Matrix4<f32> {
    fn default_view() -> Matrix4<f32> {
        let distance = 1.;
        cgmath::Matrix4::look_at_rh(
            cgmath::point3(distance, distance + (0.5 * 0.35), distance),
            cgmath::point3(0., 0.5 * 0.35, 0.),
            ClientPlayer::UP,
        )
    }
    fn block_model_icon_view(model: &BlockData) -> Matrix4<f32> {
        match &model.render_data {
            BlockRenderData::Air | BlockRenderData::Full { .. } => default_view(),
            BlockRenderData::Model { model, .. } => model_icon_view(model),
        }
    }
    fn model_icon_view(model: &ModelInstance) -> Matrix4<f32> {
        //todo: cache intern string
        model
            .model
            .data()
            .model
            .anchor(InternString::intern("icon"), Matrix4::identity(), &[])
            .map(|m| m.invert().unwrap())
            .unwrap_or_else(default_view)
    }
    match model {
        ItemModel::Block(key) => block_model_icon_view(key.data()),
        ItemModel::Model(model_instance) => model_icon_view(model_instance),
    }
}
pub fn get_block_matrix(block_pos: BlockPos, rotation: BlockRotation) -> Matrix4<f32> {
    let position = block_pos.to_pos();
    let right = rotation.right_face().get_offset();
    let up = rotation.up_face().get_offset();
    let front = rotation.front_face().get_offset();
    use cgmath::Vector4;
    Matrix4::from_translation(Vector3::new(
        position.x + 0.5,
        position.y + 0.5,
        position.z + 0.5,
    )) * Matrix4::from_cols(
        Vector4::new(right.x, right.y, right.z, 0.),
        Vector4::new(up.x, up.y, up.z, 0.),
        Vector4::new(-front.x, -front.y, -front.z, 0.),
        Vector4::new(0., 0., 0., 1.),
    ) * Matrix4::from_translation(Vector3::new(0., -0.5, 0.))
}

pub trait MeshVertexConsumer {
    fn add_vertex(&mut self, vertex: MeshVertex) -> u32;
    fn add_index(&mut self, index: u32);
    fn add_quad(&mut self, vertices: [MeshVertex; 4]) {
        let indices = vertices.map(|vertex| self.add_vertex(vertex));
        self.add_index(indices[0]);
        self.add_index(indices[3]);
        self.add_index(indices[2]);
        self.add_index(indices[2]);
        self.add_index(indices[1]);
        self.add_index(indices[0]);
    }
}
pub struct Mesh<T> {
    pub vertices: Vec<T>,
    pub indices: Vec<u32>,
}
impl<T> Mesh<T> {
    pub fn add_vertex(&mut self, vertex: T) -> u32 {
        self.vertices.push(vertex);
        self.vertices.len() as u32 - 1
    }
    pub fn add_index(&mut self, index: u32) {
        self.indices.push(index);
    }
    pub fn append_mesh(&mut self, mut other_mesh: Mesh<T>) {
        let vertex_count = self.vertices.len() as u32;
        self.vertices.append(&mut other_mesh.vertices);
        self.indices
            .extend(other_mesh.indices.into_iter().map(|i| i + vertex_count));
    }
}
impl<T> Default for Mesh<T> {
    fn default() -> Self {
        Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
        }
    }
}
#[derive(Copy, Clone)]
pub struct MeshVertex {
    pub position: Pos,
    pub normal: Pos,
    pub uv: [f32; 2],
}
pub type BaseMesh = Mesh<Vertex>;
impl BaseMesh {
    pub fn consumer<'a>(&'a mut self, color: Color) -> BaseMeshVertexConsumer<'a> {
        BaseMeshVertexConsumer { mesh: self, color }
    }
}
pub struct BaseMeshVertexConsumer<'a> {
    mesh: &'a mut BaseMesh,
    pub color: Color,
}
impl MeshVertexConsumer for BaseMeshVertexConsumer<'_> {
    fn add_vertex(&mut self, vertex: MeshVertex) -> u32 {
        self.mesh.add_vertex(Vertex {
            position: vertex.position.into_array(),
            tex_coords: vertex.uv,
            normals: vertex.normal.into_array(),
            color: self.color.into(),
        })
    }
    fn add_index(&mut self, index: u32) {
        self.mesh.add_index(index);
    }
}
pub type GridChunkMesh = Mesh<GPUBlockFace>;
pub type ChunkMesh = Mesh<ChunkVertex>;
impl ChunkMesh {
    pub fn consumer<'a>(
        &'a mut self,
        block_color: BlockColor,
        flags: u8,
    ) -> ChunkMeshVertexConsumer<'a> {
        ChunkMeshVertexConsumer {
            mesh: self,
            block_color,
            flags,
        }
    }
}
pub struct ChunkMeshVertexConsumer<'a> {
    pub mesh: &'a mut ChunkMesh,
    pub block_color: BlockColor,
    pub flags: u8,
}
impl MeshVertexConsumer for ChunkMeshVertexConsumer<'_> {
    fn add_vertex(&mut self, vertex: MeshVertex) -> u32 {
        self.mesh.add_vertex(ChunkVertex {
            position: vertex.position.into_array(),
            //tex_coords: vertex.uv.map(|v| (v * u16::MAX as f32) as u16),
            tex_coords: vertex.uv,
            shade: 255,
            color: self.block_color.0,
            flags: self.flags,
            normals: [
                (vertex.normal.x * i8::MAX as f32) as i8,
                (vertex.normal.y * i8::MAX as f32) as i8,
                (vertex.normal.z * i8::MAX as f32) as i8,
                0,
            ],
        })
    }
    fn add_index(&mut self, index: u32) {
        self.mesh.add_index(index);
    }
}
pub type GUIMesh = Mesh<GUIVertex>;
pub type DamageMesh = Mesh<DamageVertex>;
impl DamageMesh {
    pub fn consumer<'a>(&'a mut self, progress: f32) -> DamageMeshVertexConsumer<'a> {
        DamageMeshVertexConsumer {
            mesh: self,
            progress,
        }
    }
}
pub struct DamageMeshVertexConsumer<'a> {
    mesh: &'a mut DamageMesh,
    progress: f32,
}
impl MeshVertexConsumer for DamageMeshVertexConsumer<'_> {
    fn add_vertex(&mut self, vertex: MeshVertex) -> u32 {
        self.mesh.add_vertex(DamageVertex {
            position: vertex.position.into_array(),
            tex_coords: vertex.uv,
            progress: self.progress,
        })
    }
    fn add_index(&mut self, index: u32) {
        self.mesh.add_index(index);
    }
}

impl GUIMesh {
    pub fn add_quad_clip(&mut self, quad: UIRect, texture: TexCoords, color: Color, clip: UIRect) {
        if quad.pos.x + quad.size.x < clip.pos.x
            || quad.pos.x > clip.pos.x + clip.size.x
            || quad.pos.y + quad.size.y < clip.pos.y
            || quad.pos.y > clip.pos.y + clip.size.y
        {
            return;
        }
        let pos1 = UIPos {
            x: quad.pos.x.max(clip.pos.x),
            y: quad.pos.y.max(clip.pos.y),
        };
        let pos2 = UIPos {
            x: (quad.pos.x + quad.size.x).min(clip.pos.x + clip.size.x),
            y: (quad.pos.y + quad.size.y).min(clip.pos.y + clip.size.y),
        };
        let clipped_quad = UIRect {
            pos: pos1,
            size: UIPos {
                x: pos2.x - pos1.x,
                y: pos2.y - pos1.y,
            },
        };
        let clipped_texture = TexCoords {
            u1: texture.u1 + (pos1.x - quad.pos.x) / quad.size.x * (texture.u2 - texture.u1),
            v1: texture.v1 + (pos1.y - quad.pos.y) / quad.size.y * (texture.v2 - texture.v1),
            u2: texture.u1 + (pos2.x - quad.pos.x) / quad.size.x * (texture.u2 - texture.u1),
            v2: texture.v1 + (pos2.y - quad.pos.y) / quad.size.y * (texture.v2 - texture.v1),
        };
        self.add_quad(clipped_quad, clipped_texture, color);
    }
    pub fn add_quad(&mut self, quad: UIRect, texture: TexCoords, color: Color) {
        let color: [u8; 4] = color.into();
        let position = quad.pos;
        let size = quad.size;
        let a = GUIVertex {
            position: [position.x, position.y],
            tex_coords: [texture.u1, texture.v2],
            color,
        };
        let b = GUIVertex {
            position: [position.x + size.x, position.y],
            tex_coords: [texture.u2, texture.v2],
            color,
        };
        let c = GUIVertex {
            position: [position.x, position.y + size.y],
            tex_coords: [texture.u1, texture.v1],
            color,
        };
        let d = GUIVertex {
            position: [position.x + size.x, position.y + size.y],
            tex_coords: [texture.u2, texture.v1],
            color,
        };

        let start_index = self.vertices.len() as u32;

        self.add_vertex(a);
        self.add_vertex(b);
        self.add_vertex(c);
        self.add_vertex(d);

        self.add_index(start_index + 0);
        self.add_index(start_index + 1);
        self.add_index(start_index + 3);
        self.add_index(start_index + 3);
        self.add_index(start_index + 2);
        self.add_index(start_index + 0);
    }
    pub fn add_line(&mut self, p1: UIPos, p2: UIPos, texture: TexCoords, color: Color, width: f32) {
        let color: [u8; 4] = color.into();
        let mut perp = UIPos {
            x: (p2.y - p1.y),
            y: -(p2.x - p1.x),
        };
        let perp_norm = width / (perp.x.powi(2) + perp.y.powi(2)).sqrt();
        perp.x *= perp_norm;
        perp.y *= perp_norm;
        let a = GUIVertex {
            position: [p1.x + perp.x, p1.y + perp.y],
            tex_coords: [texture.u1, texture.v2],
            color,
        };
        let b = GUIVertex {
            position: [p2.x + perp.x, p2.y + perp.y],
            tex_coords: [texture.u2, texture.v2],
            color,
        };
        let c = GUIVertex {
            position: [p1.x - perp.x, p1.y - perp.y],
            tex_coords: [texture.u1, texture.v1],
            color,
        };
        let d = GUIVertex {
            position: [p2.x - perp.x, p2.y - perp.y],
            tex_coords: [texture.u2, texture.v1],
            color,
        };

        let start_index = self.vertices.len() as u32;

        self.add_vertex(a);
        self.add_vertex(b);
        self.add_vertex(c);
        self.add_vertex(d);

        self.add_index(start_index + 0);
        self.add_index(start_index + 1);
        self.add_index(start_index + 3);
        self.add_index(start_index + 3);
        self.add_index(start_index + 2);
        self.add_index(start_index + 0);
    }
    pub fn add_line_clip(
        &mut self,
        p1: UIPos,
        p2: UIPos,
        texture: TexCoords,
        color: Color,
        width: f32,
        clip: UIRect,
    ) {
        let xmin = clip.pos.x;
        let ymin = clip.pos.y;
        let xmax = clip.pos.x + clip.size.x;
        let ymax = clip.pos.y + clip.size.y;
        let dx = p2.x - p1.x;
        let dy = p2.y - p1.y;
        let p = [-dx, dx, -dy, dy];
        let q = [p1.x - xmin, xmax - p1.x, p1.y - ymin, ymax - p1.y];
        let mut u1 = 0f32;
        let mut u2 = 1f32;
        for i in 0..4 {
            if p[i] == 0. {
                if q[i] < 0. {
                    return;
                }
            } else {
                let t = q[i] / p[i];
                if p[i] < 0. {
                    u1 = u1.max(t);
                } else {
                    u2 = u2.min(t);
                }
            }
        }
        if u1 > u2 {
            return;
        }
        self.add_line(
            UIPos {
                x: p1.x + u1 * dx,
                y: p1.y + u1 * dy,
            },
            UIPos {
                x: p1.x + u2 * dx,
                y: p1.y + u2 * dy,
            },
            texture,
            color,
            width,
        );
    }
}
