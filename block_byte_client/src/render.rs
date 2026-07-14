use block_byte_common::coord::{AABB, BlockPos, CHUNK_SIZE, ChunkPos, Face, Pos};
use block_byte_common::model::{DrawAnimation, ModelGeometry, ModelTexture};
use block_byte_common::registry::{
    BlockColor, BlockData, BlockKey, BlockRenderData, EntityData, ItemModel, ModelInstance,
};
use block_byte_common::rotation::BlockRotation;
use block_byte_common::{Color, TexCoords};
use bytemuck::{NoUninit, Pod};
use cgmath::{
    InnerSpace, Matrix4, Point3, SquareMatrix, Transform, Vector3,
};
use image::RgbaImage;
use std::borrow::Cow;
use std::iter;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};
use wgpu::util::StagingBelt;
use wgpu::{
    BackendOptions, BindGroup, BindGroupLayout, BlendState, Buffer, BufferDescriptor, BufferSize, CommandEncoder, CompareFunction, Device, FilterMode, IndexFormat, InstanceFlags, MemoryBudgetThresholds, Queue, RenderPass, Sampler, TextureFormat, TextureView,
};
use winit::dpi::PhysicalSize;
use winit::window::Window;

pub struct RenderState {
    surface: wgpu::Surface<'static>,
    pub device: Arc<wgpu::Device>,
    pub queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    window: Arc<Window>,
    base_render_pipeline: GPURenderPipeline,
    chunk_render_pipeline: GPURenderPipeline,
    gui_render_pipeline: GPURenderPipeline,
    damage_render_pipeline: GPURenderPipeline,
    skybox_render_pipeline: GPURenderPipeline,
    hdr_render_pipeline: GPURenderPipeline,
    skybox_mesh: GPUMesh,
    skybox_texture: GPUTexture,
    texture_atlas: GPUTexture,
    camera_uniform: GPUUniform<CameraUniform>,
    gui_camera_uniform: GPUUniform<CameraUniform>,
    depth_texture: GPUTexture,
    hdr_texture: GPUTexture,
    texel_size_uniform: GPUUniform<[f32; 2]>,
    blur_texture: GPUTexture,
    blur_render_pipeline: GPURenderPipeline,
    shadow_texture: GPUTexture,
    shadow_camera: GPUUniform<CameraUniform>,
    shadow_chunk_render_pipeline: GPURenderPipeline,
    shadow_base_render_pipeline: GPURenderPipeline,
    time_uniform: GPUUniform<f32>,
    material_texture: GPUTexture,
    entity_gpu_mesh: GPUMesh,
    local_player_gpu_mesh: GPUMesh,
    damage_gpu_mesh: GPUMesh,
    viewmodel_gpu_mesh: GPUMesh,
    gui_gpu_mesh: GPUMesh,
    pub animation_time: f32,
    pub staging_belt: StagingBelt,
    animation_data_uniform: GPUUniform<()>,
}

pub enum SurfaceError {
    Recreate,
    Crash,
}

impl RenderState {
    pub async fn new(window: Window, skybox: RgbaImage) -> Self {
        let window = Arc::new(window);
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            backend_options: BackendOptions::default(),
            display: None,
            flags: InstanceFlags::default(),
            memory_budget_thresholds: MemoryBudgetThresholds::default(),
        });
        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();

        //println!("{:?}", adapter.get_info());

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits {
                    max_bind_groups: 8,
                    ..Default::default()
                },
                memory_hints: Default::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .unwrap();
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let texture_images = &TEXTURE_ATLAS.get().unwrap().texture_mips;
        let texture_atlas = GPUTexture::new(
            &device,
            texture_images[0].dimensions(),
            texture_images.len() as u32,
            TextureFormat::Rgba8Unorm,
            wgpu::FilterMode::Nearest,
        );
        for (mip_level, texture_image) in texture_images.into_iter().enumerate() {
            texture_atlas.write_image(&texture_image, mip_level as u32, &queue);
        }
        let material_texture = GPUTexture::new(
            &device,
            texture_images[0].dimensions(),
            1,
            TextureFormat::Rgba8Unorm,
            wgpu::FilterMode::Nearest,
        );
        material_texture.write_image(&TEXTURE_ATLAS.get().unwrap().texture_material, 0, &queue);

        let skybox_texture = GPUTexture::new(
            &device,
            skybox.dimensions(),
            1,
            TextureFormat::Rgba8UnormSrgb,
            wgpu::FilterMode::Nearest,
        );
        skybox_texture.write_image(&skybox, 0, &queue);

        let animation_data_buffer = device.create_buffer(&BufferDescriptor {
            label: None,
            mapped_at_creation: false,
            size: 4 * 2 + std::mem::size_of::<AnimatedCell>() as u64 * (2048u64 / 16).pow(2),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        queue.write_buffer(
            &animation_data_buffer,
            0,
            bytemuck::cast_slice(std::slice::from_ref(&(16_f32 / 2048_f32))),
        );
        queue.write_buffer(
            &animation_data_buffer,
            4,
            bytemuck::cast_slice(std::slice::from_ref(&128_u32)),
        );
        queue.write_buffer(
            &animation_data_buffer,
            8,
            bytemuck::cast_slice(TEXTURE_ATLAS.get().unwrap().animation_data.as_slice()),
        );
        let animation_data_uniform =
            GPUUniform::new_with_buffer(&device, true, animation_data_buffer);

        let camera_uniform = GPUUniform::new(&device, false);
        let gui_camera_uniform = GPUUniform::new(&device, false);
        let texel_size_uniform = GPUUniform::new(&device, false);
        let shadow_camera = GPUUniform::new(&device, false);
        let time_uniform = GPUUniform::new(&device, false);
        texel_size_uniform.write(&queue, &[1. / size.width as f32, 1. / size.height as f32]);
        let depth_texture = GPUTexture::new(
            &device,
            (size.width, size.height),
            1,
            TextureFormat::Depth32Float,
            wgpu::FilterMode::Nearest,
        );
        let hdr_texture = GPUTexture::new(
            &device,
            (size.width, size.height),
            1,
            TextureFormat::Rgba16Float,
            wgpu::FilterMode::Nearest,
        );
        let blur_texture = GPUTexture::new(
            &device,
            (size.width, size.height),
            1,
            TextureFormat::Rgba16Float,
            wgpu::FilterMode::Nearest,
        );
        let shadow_texture = GPUTexture::new(
            &device,
            (2048 * 2, 2048 * 2),
            1,
            TextureFormat::Depth32Float,
            wgpu::FilterMode::Linear,
        );
        let base_render_pipeline = GPURenderPipeline::new::<Vertex>(
            &device,
            "base",
            &[
                Some(&texture_atlas.bind_group_layout),
                Some(&camera_uniform.bind_group_layout),
                Some(&shadow_camera.bind_group_layout),
                Some(&shadow_texture.bind_group_layout),
                Some(&time_uniform.bind_group_layout),
                Some(&material_texture.bind_group_layout),
                Some(&animation_data_uniform.bind_group_layout),
            ],
            Some(BlendState::ALPHA_BLENDING),
            Some(wgpu::Face::Back),
            Some(hdr_texture.format),
            Some(TextureFormat::Depth32Float),
        );

        let chunk_render_pipeline = GPURenderPipeline::new::<ChunkVertex>(
            &device,
            "chunk",
            &[
                Some(&texture_atlas.bind_group_layout),
                Some(&camera_uniform.bind_group_layout),
                Some(&shadow_camera.bind_group_layout),
                Some(&shadow_texture.bind_group_layout),
                Some(&time_uniform.bind_group_layout),
                Some(&material_texture.bind_group_layout),
                Some(&animation_data_uniform.bind_group_layout),
            ],
            Some(BlendState::REPLACE),
            Some(wgpu::Face::Back),
            Some(hdr_texture.format),
            Some(TextureFormat::Depth32Float),
        );

        let gui_render_pipeline = GPURenderPipeline::new::<GUIVertex>(
            &device,
            "gui",
            &[
                Some(&texture_atlas.bind_group_layout),
                Some(&gui_camera_uniform.bind_group_layout),
            ],
            Some(BlendState::REPLACE),
            Some(wgpu::Face::Back),
            Some(config.format),
            None,
        );

        let damage_render_pipeline = GPURenderPipeline::new::<DamageVertex>(
            &device,
            "damage",
            &[Some(&camera_uniform.bind_group_layout)],
            Some(BlendState::ALPHA_BLENDING),
            Some(wgpu::Face::Back),
            Some(hdr_texture.format),
            Some(TextureFormat::Depth32Float),
        );

        let skybox_render_pipeline = GPURenderPipeline::new::<Vertex>(
            &device,
            "skybox",
            &[
                Some(&skybox_texture.bind_group_layout),
                Some(&camera_uniform.bind_group_layout),
            ],
            Some(BlendState::REPLACE),
            None,
            Some(config.format),
            None,
        );

        let blur_render_pipeline = GPURenderPipeline::new::<()>(
            &device,
            "blur",
            &[
                Some(&hdr_texture.bind_group_layout),
                Some(&texel_size_uniform.bind_group_layout),
            ],
            Some(BlendState::ALPHA_BLENDING),
            None,
            Some(hdr_texture.format),
            None,
        );

        let hdr_render_pipeline = GPURenderPipeline::new::<()>(
            &device,
            "postprocess",
            &[
                Some(&hdr_texture.bind_group_layout),
                Some(&blur_texture.bind_group_layout),
            ],
            Some(BlendState::ALPHA_BLENDING),
            None,
            Some(config.format),
            None,
        );
        let shadow_chunk_render_pipeline = GPURenderPipeline::new::<ChunkVertex>(
            &device,
            "chunk_shadow",
            &[
                Some(&texture_atlas.bind_group_layout),
                Some(&shadow_camera.bind_group_layout),
                Some(&time_uniform.bind_group_layout),
            ],
            None,
            Some(wgpu::Face::Back),
            None,
            Some(TextureFormat::Depth32Float),
        );

        let shadow_base_render_pipeline = GPURenderPipeline::new::<Vertex>(
            &device,
            "base_shadow",
            &[
                Some(&texture_atlas.bind_group_layout),
                Some(&camera_uniform.bind_group_layout),
            ],
            None,
            Some(wgpu::Face::Back),
            None,
            Some(TextureFormat::Depth32Float),
        );

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
        Self {
            staging_belt: StagingBelt::new(device.clone(), 4 * 1024 * 1024),
            skybox_mesh: GPUMesh::allocate(&skybox_mesh, 0, &device),
            window,
            surface,
            queue,
            config,
            size,
            base_render_pipeline,
            chunk_render_pipeline,
            gui_render_pipeline,
            hdr_render_pipeline,
            texture_atlas,
            camera_uniform,
            gui_camera_uniform,
            depth_texture,
            hdr_texture,
            device: Arc::new(device),
            damage_render_pipeline,
            skybox_render_pipeline,
            skybox_texture,
            texel_size_uniform,
            blur_render_pipeline,
            blur_texture,
            shadow_camera,
            shadow_texture,
            shadow_chunk_render_pipeline,
            shadow_base_render_pipeline,
            animation_time: 0.,
            material_texture,
            time_uniform,
            damage_gpu_mesh: GPUMesh::empty(),
            entity_gpu_mesh: GPUMesh::empty(),
            gui_gpu_mesh: GPUMesh::empty(),
            viewmodel_gpu_mesh: GPUMesh::empty(),
            local_player_gpu_mesh: GPUMesh::empty(),
            animation_data_uniform,
        }
    }

    pub fn window(&self) -> &Window {
        &self.window
    }
    pub fn size(&self) -> PhysicalSize<u32> {
        self.size
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            self.depth_texture
                .resize((new_size.width, new_size.height), &self.device);
            self.hdr_texture
                .resize((new_size.width, new_size.height), &self.device);
            self.texel_size_uniform.write(
                &self.queue,
                &[1. / new_size.width as f32, 1. / new_size.height as f32],
            );
        }
    }

    pub fn render(
        &mut self,
        game: &mut ClientGame,
        entity_mesh: BaseMesh,
        local_player_mesh: BaseMesh,
        gui_mesh: GUIMesh,
        viewmodel_mesh: BaseMesh,
        damage_mesh: DamageMesh,
        frustum: &Frustum,
    ) -> Result<(), SurfaceError> {
        let should_update_shadowmap = true;

        let ps = profiler::profiler_scope("load uniforms");
        self.time_uniform.write(&self.queue, &self.animation_time);

        let mut camera_uniform = CameraUniform::new();
        camera_uniform.load_camera_proj_matrix(
            &game.camera,
            self.size.width as f32 / self.size.height as f32,
            90.,
            game.get_player_data(),
        );
        self.camera_uniform.write(&self.queue, &camera_uniform);

        camera_uniform.load_gui_matrix(self.size.height as f32 / self.size.width as f32);
        self.gui_camera_uniform.write(&self.queue, &camera_uniform);

        if should_update_shadowmap {
            camera_uniform.load_light(game.camera.position);
            self.shadow_camera.write(&self.queue, &camera_uniform);
        }
        ps.end();

        let ps = profiler::profiler_scope("render mesh alloc");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Upload Encoder"),
            });

        self.entity_gpu_mesh.upload(
            &entity_mesh,
            &self.device,
            &mut self.staging_belt,
            &mut encoder,
        );
        self.gui_gpu_mesh.upload(
            &gui_mesh,
            &self.device,
            &mut self.staging_belt,
            &mut encoder,
        );
        self.viewmodel_gpu_mesh.upload(
            &viewmodel_mesh,
            &self.device,
            &mut self.staging_belt,
            &mut encoder,
        );
        self.damage_gpu_mesh.upload(
            &damage_mesh,
            &self.device,
            &mut self.staging_belt,
            &mut encoder,
        );
        self.local_player_gpu_mesh.upload(
            &local_player_mesh,
            &self.device,
            &mut self.staging_belt,
            &mut encoder,
        );
        let ps2 = profiler::profiler_scope("chunk download");
        let mut frame_load_limit = 0;
        while let Ok((position, buffer, buffer_high_res, version)) =
            game.chunk_mesh_channels.1.try_recv()
        {
            game.chunk_mesh_queue_size -= 1;
            if let Some(chunk) = game.chunks.get_mut(&position) {
                chunk.scheduled = false;

                chunk.gpu_mesh = game.chunk_buffer_pool.allocate_or_reuse(
                    &buffer,
                    chunk.gpu_mesh.take(),
                    &mut self.staging_belt,
                    &mut encoder,
                    &self.device,
                );
                chunk.gpu_mesh_high_res = game.chunk_buffer_pool.allocate_or_reuse(
                    &buffer_high_res,
                    chunk.gpu_mesh_high_res.take(),
                    &mut self.staging_belt,
                    &mut encoder,
                    &self.device,
                );

                if let Some(render_data) = &chunk.gpu_mesh.render_data {
                    frame_load_limit += render_data.memory_size();
                }
                if let Some(render_data) = &chunk.gpu_mesh_high_res.render_data {
                    frame_load_limit += render_data.memory_size();
                }

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
                if frame_load_limit > (1. * 1024. * 1024.) as usize && false {
                    break;
                }
            }
        }
        ps2.end();
        self.staging_belt.finish();
        self.queue.submit(iter::once(encoder.finish()));
        ps.end();
        self.staging_belt.recall();

        let ps = profiler::profiler_scope("render texture");
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(surface_texture) => surface_texture,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                ps.end();
                return Err(SurfaceError::Recreate);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(SurfaceError::Crash);
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Validation
            | wgpu::CurrentSurfaceTexture::Occluded => {
                ps.end();
                return Ok(());
            }
        };
        ps.end();

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        let ps = profiler::profiler_scope("render shadow");
        let camera_chunk_position = game.camera.position.to_chunk_pos();
        if should_update_shadowmap {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Shadow Render Pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_texture.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.shadow_chunk_render_pipeline.render_pipeline);
            render_pass.set_bind_group(0, &self.texture_atlas.bind_group, &[]);
            render_pass.set_bind_group(1, &self.shadow_camera.bind_group, &[]);
            render_pass.set_bind_group(2, &self.time_uniform.bind_group, &[]);

            for chunk_position in (AABB {
                min: ChunkPos::all(-5),
                max: ChunkPos::all(5),
            })
            .offset(camera_chunk_position)
            {
                if let Some(chunk) = game.chunks.get(&chunk_position) {
                    chunk.gpu_mesh.draw(&mut render_pass);
                    if chunk_position.distance_squared(camera_chunk_position) <= 2 {
                        chunk.gpu_mesh_high_res.draw(&mut render_pass);
                    }
                }
            }

            render_pass.set_pipeline(&self.shadow_base_render_pipeline.render_pipeline);

            self.entity_gpu_mesh.draw(&mut render_pass);
            self.local_player_gpu_mesh.draw(&mut render_pass);
        }
        ps.end();
        if true {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Skybox Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.skybox_render_pipeline.render_pipeline);
            render_pass.set_bind_group(0, &self.skybox_texture.bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_uniform.bind_group, &[]);
            self.skybox_mesh.draw(&mut render_pass);
        }
        let ps = profiler::profiler_scope("render base");
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Base Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.hdr_texture.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.,
                            g: 0.,
                            b: 0.,
                            a: 0.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            render_pass.set_bind_group(0, &self.texture_atlas.bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_uniform.bind_group, &[]);
            render_pass.set_bind_group(2, &self.shadow_camera.bind_group, &[]);
            render_pass.set_bind_group(3, &self.shadow_texture.bind_group, &[]);
            render_pass.set_bind_group(4, &self.time_uniform.bind_group, &[]);
            render_pass.set_bind_group(5, &self.material_texture.bind_group, &[]);
            render_pass.set_bind_group(6, &self.animation_data_uniform.bind_group, &[]);

            render_pass.set_pipeline(&self.chunk_render_pipeline.render_pipeline);
            for (chunk_position, chunk) in &game.chunks {
                if frustum.intersects_aabb(
                    &AABB {
                        min: Pos::all(0.),
                        max: Pos::all(CHUNK_SIZE as f32),
                    }
                    .offset(chunk.position.to_block_pos().to_pos()),
                ) {
                    chunk.gpu_mesh.draw(&mut render_pass);
                    if chunk_position.distance_squared(camera_chunk_position) <= 5_i16.pow(2) {
                        chunk.gpu_mesh_high_res.draw(&mut render_pass);
                    }
                }
            }

            render_pass.set_pipeline(&self.base_render_pipeline.render_pipeline);
            self.entity_gpu_mesh.draw(&mut render_pass);
        }
        if !damage_mesh.vertices.is_empty() {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Damage Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.hdr_texture.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.damage_render_pipeline.render_pipeline);
            render_pass.set_bind_group(0, &self.camera_uniform.bind_group, &[]);
            self.damage_gpu_mesh.draw(&mut render_pass);
        }
        if !viewmodel_mesh.vertices.is_empty() {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ViewModel Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.hdr_texture.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.base_render_pipeline.render_pipeline);
            render_pass.set_bind_group(0, &self.texture_atlas.bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_uniform.bind_group, &[]);
            render_pass.set_bind_group(2, &self.shadow_camera.bind_group, &[]);
            render_pass.set_bind_group(3, &self.shadow_texture.bind_group, &[]);
            render_pass.set_bind_group(4, &self.time_uniform.bind_group, &[]);
            render_pass.set_bind_group(5, &self.material_texture.bind_group, &[]);
            render_pass.set_bind_group(6, &self.animation_data_uniform.bind_group, &[]);

            self.viewmodel_gpu_mesh.draw(&mut render_pass);
        }
        ps.end();
        if false {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Blur Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.blur_texture.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.,
                            g: 0.,
                            b: 0.,
                            a: 0.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.blur_render_pipeline.render_pipeline);
            render_pass.set_bind_group(0, &self.hdr_texture.bind_group, &[]);
            render_pass.set_bind_group(1, &self.texel_size_uniform.bind_group, &[]);

            render_pass.draw(0..3, 0..1);
        }
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HDR Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.hdr_render_pipeline.render_pipeline);
            render_pass.set_bind_group(0, &self.hdr_texture.bind_group, &[]);
            render_pass.set_bind_group(1, &self.blur_texture.bind_group, &[]);

            render_pass.draw(0..3, 0..1);
        }

        if !gui_mesh.vertices.is_empty() {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("GUI Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.gui_render_pipeline.render_pipeline);
            render_pass.set_bind_group(0, &self.texture_atlas.bind_group, &[]);
            render_pass.set_bind_group(1, &self.gui_camera_uniform.bind_group, &[]);

            self.gui_gpu_mesh.draw(&mut render_pass);
        }
        let ps = profiler::profiler_scope("render queue");
        self.queue.submit(iter::once(encoder.finish()));
        output.present();
        ps.end();

        Ok(())
    }
    pub fn render_gui(&mut self, gui_mesh: GUIMesh) -> Result<(), SurfaceError> {
        let ps = profiler::profiler_scope("load uniforms");
        self.time_uniform.write(&self.queue, &self.animation_time);

        let mut camera_uniform = CameraUniform::new();
        camera_uniform.load_gui_matrix(self.size.height as f32 / self.size.width as f32);
        self.gui_camera_uniform.write(&self.queue, &camera_uniform);
        ps.end();

        let ps = profiler::profiler_scope("render mesh alloc");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Upload Encoder"),
            });

        self.gui_gpu_mesh.upload(
            &gui_mesh,
            &self.device,
            &mut self.staging_belt,
            &mut encoder,
        );
        self.staging_belt.finish();
        self.queue.submit(iter::once(encoder.finish()));
        ps.end();
        self.staging_belt.recall();

        let ps = profiler::profiler_scope("render texture");
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(surface_texture) => surface_texture,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                ps.end();
                return Err(SurfaceError::Recreate);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(SurfaceError::Crash);
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Validation
            | wgpu::CurrentSurfaceTexture::Occluded => {
                ps.end();
                return Ok(());
            }
        };
        ps.end();

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        if !gui_mesh.vertices.is_empty() {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("GUI Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.,
                            g: 0.,
                            b: 0.,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            render_pass.set_pipeline(&self.gui_render_pipeline.render_pipeline);
            render_pass.set_bind_group(0, &self.texture_atlas.bind_group, &[]);
            render_pass.set_bind_group(1, &self.gui_camera_uniform.bind_group, &[]);

            self.gui_gpu_mesh.draw(&mut render_pass);
        }
        let ps = profiler::profiler_scope("render queue");
        self.queue.submit(iter::once(encoder.finish()));
        output.present();
        ps.end();

        Ok(())
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkVertex {
    pub position: [f32; 3],
    pub normals: [i8; 4],
    //pub tex_coords: [u16; 2],
    pub tex_coords: [f32; 2],
    pub color: u16,
    pub shade: u8,
    pub flags: u8,
}

static SHADER_DIR: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/src/shaders");
pub fn load_shader_source(shader: &str) -> String {
    let content = SHADER_DIR
        .get_file(format!("{}.wgsl", shader))
        .unwrap()
        .contents_utf8()
        .unwrap();
    content
        .lines()
        .map(|line| {
            let include_string = "#include ";
            if line.starts_with(include_string) {
                load_shader_source(&line[include_string.len()..])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub trait VertexDescription {
    fn vertex_description() -> Option<wgpu::VertexBufferLayout<'static>>;
}

impl VertexDescription for () {
    fn vertex_description() -> Option<wgpu::VertexBufferLayout<'static>> {
        None
    }
}

impl ChunkVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm8x4, 2 => Float32x2, 3 => Uint16, 4 => Unorm8, 5 => Uint8];
}
impl VertexDescription for ChunkVertex {
    fn vertex_description() -> Option<wgpu::VertexBufferLayout<'static>> {
        use std::mem;

        Some(wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        })
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
    pub normals: [f32; 3],
    pub color: [u8; 4],
}
impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x3, 3 => Unorm8x4];
}
impl VertexDescription for Vertex {
    fn vertex_description() -> Option<wgpu::VertexBufferLayout<'static>> {
        use std::mem;

        Some(wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        })
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GUIVertex {
    pub position: [f32; 2],
    pub tex_coords: [f32; 2],
    pub color: [u8; 4],
}
impl GUIVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4];
}
impl VertexDescription for GUIVertex {
    fn vertex_description() -> Option<wgpu::VertexBufferLayout<'static>> {
        use std::mem;

        Some(wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        })
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DamageVertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
    pub progress: f32,
}
impl DamageVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32];
}
impl VertexDescription for DamageVertex {
    fn vertex_description() -> Option<wgpu::VertexBufferLayout<'static>> {
        use std::mem;

        Some(wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        })
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    direction: [f32; 3],
    _pad: f32,
}
impl CameraUniform {
    fn new() -> Self {
        Self {
            view_proj: cgmath::Matrix4::identity().into(),
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
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 0.5, 0.5,
        0.0, 0.0, 0.0, 1.0,
    );
}

use crate::atlas::{AnimatedCell, TEXTURE_ATLAS, TexCoordsExt, TexCoordsIndexExt};
use crate::game::clipping::Frustum;
use crate::game::{ClientGame, ClientPlayer, ModifiedChunkEntry, profiler};
use crate::ui::{UIPos, UIRect};

pub struct GPUTexture {
    pub texture: wgpu::Texture,
    pub view: TextureView,
    pub sampler: Sampler,
    pub bind_group_layout: BindGroupLayout,
    pub bind_group: BindGroup,
    pub dimensions: (u32, u32),
    pub format: wgpu::TextureFormat,
    pub mip_levels: u32,
    pub filter_mode: wgpu::FilterMode,
}

impl GPUTexture {
    pub fn new(
        device: &wgpu::Device,
        dimensions: (u32, u32),
        mip_levels: u32,
        format: wgpu::TextureFormat,
        filter_mode: wgpu::FilterMode,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: match GPUTexture::is_format_depth(format) {
                            true => wgpu::TextureSampleType::Depth,
                            false => wgpu::TextureSampleType::Float { filterable: true },
                        },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(
                        match Self::is_format_depth(format)
                            && filter_mode == FilterMode::Linear
                            && false
                        {
                            true => wgpu::SamplerBindingType::Comparison,
                            false => wgpu::SamplerBindingType::Filtering,
                        },
                    ),
                    count: None,
                },
            ],
            label: None,
        });
        let (texture, view, sampler, bind_group) = GPUTexture::init_on_gpu(
            device,
            dimensions,
            format,
            &bind_group_layout,
            mip_levels,
            filter_mode,
        );
        Self {
            texture,
            view,
            sampler,
            bind_group_layout,
            bind_group,
            dimensions,
            format,
            mip_levels,
            filter_mode,
        }
    }
    fn is_format_depth(format: wgpu::TextureFormat) -> bool {
        match format {
            TextureFormat::Depth16Unorm
            | TextureFormat::Depth24Plus
            | TextureFormat::Depth24PlusStencil8
            | TextureFormat::Depth32Float
            | TextureFormat::Depth32FloatStencil8 => true,
            _ => false,
        }
    }
    fn init_on_gpu(
        device: &Device,
        dimensions: (u32, u32),
        format: wgpu::TextureFormat,
        bind_group_layout: &BindGroupLayout,
        mip_levels: u32,
        filter_mode: wgpu::FilterMode,
    ) -> (
        wgpu::Texture,
        wgpu::TextureView,
        wgpu::Sampler,
        wgpu::BindGroup,
    ) {
        let size = wgpu::Extent3d {
            width: dimensions.0,
            height: dimensions.1,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: filter_mode,
            min_filter: filter_mode,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            //border_color: Some(wgpu::SamplerBorderColor::TransparentBlack),
            compare: match Self::is_format_depth(format)
                && filter_mode == FilterMode::Linear
                && false
            {
                true => Some(CompareFunction::Less),
                false => None,
            },
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: None,
        });
        (texture, view, sampler, bind_group)
    }
    pub fn resize(&mut self, dimensions: (u32, u32), device: &Device) {
        self.dimensions = dimensions;
        let (texture, view, sampler, bind_group) = GPUTexture::init_on_gpu(
            device,
            dimensions,
            self.format,
            &self.bind_group_layout,
            self.mip_levels,
            self.filter_mode,
        );
        self.texture = texture;
        self.view = view;
        self.sampler = sampler;
        self.bind_group = bind_group;
    }
    pub fn write_image(&self, rgba: &RgbaImage, mip_level: u32, queue: &Queue) {
        //assert_eq!(rgba.dimensions(), self.dimensions);
        let size = wgpu::Extent3d {
            width: rgba.width(),
            height: rgba.height(),
            depth_or_array_layers: 1,
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                aspect: wgpu::TextureAspect::All,
                texture: &self.texture,
                mip_level,
                origin: wgpu::Origin3d::ZERO,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * rgba.width()),
                rows_per_image: Some(rgba.height()),
            },
            size,
        );
    }
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
                    face.get_vertices(faces.by_face(face).tex_coords(face as usize), 0)
                        .map(|(position, uv)| {
                            let position = position
                                - Pos {
                                    x: 0.5,
                                    y: 0.,
                                    z: 0.5,
                                };
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
    binding_query: impl Fn(&str, &mut C) -> Option<Cow<'static, ItemModel>>,
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
            item_models.push((matrix, binding.to_string()));
        },
    );
    for (matrix, binding) in item_models {
        if let Some(model) = binding_query(&binding, vertex_consumer) {
            let anchor = match &*model {
                ItemModel::Block(block) => match &block.data().render_data {
                    BlockRenderData::Air => Matrix4::identity(),
                    BlockRenderData::Full { .. } => Matrix4::identity(),
                    BlockRenderData::Model { model: key, .. } => key
                        .model
                        .data()
                        .model
                        .anchor(
                            binding.as_str(),
                            Matrix4::from_scale(block.data().item_scale),
                            &[],
                        )
                        .map(|matrix| {
                            Matrix4::from_scale(block.data().item_scale) * matrix.invert().unwrap()
                        })
                        .unwrap_or(Matrix4::identity()),
                },
                ItemModel::Model(key) => key
                    .model
                    .data()
                    .model
                    .anchor(binding.as_str(), Matrix4::identity(), &[])
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
        model
            .model
            .data()
            .model
            .anchor("icon", Matrix4::identity(), &[])
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
pub struct GPUMesh {
    pub buffer: Option<Buffer>,
    pub render_data: Option<GPUMeshRenderData>,
}
pub struct GPUMeshRenderData {
    pub vertex_length: u64,
    pub index_count: u32,
    pub index_format: IndexFormat,
}
impl GPUMeshRenderData {
    pub fn memory_size(&self) -> usize {
        self.vertex_length as usize
            + match self.index_format {
                IndexFormat::Uint16 => 2,
                IndexFormat::Uint32 => 4,
            } * self.index_count as usize
    }
}
impl GPUMesh {
    pub fn empty() -> GPUMesh {
        GPUMesh {
            buffer: None,
            render_data: None,
        }
    }
    pub fn take(&mut self) -> GPUMesh {
        GPUMesh {
            buffer: self.buffer.take(),
            render_data: self.render_data.take(),
        }
    }
    fn size_align(unpadded_size: u64) -> u64 {
        let align_mask = wgpu::COPY_BUFFER_ALIGNMENT - 1;
        ((unpadded_size + align_mask) & !align_mask).max(wgpu::COPY_BUFFER_ALIGNMENT)
    }
    pub fn allocate<T: Pod>(mesh: &Mesh<T>, min_size: usize, device: &Device) -> GPUMesh {
        if mesh.indices.is_empty() && min_size == 0 {
            return GPUMesh::empty();
        }
        let (vertex_buffer_size, index_buffer_size, index_format) = mesh.get_data_size();
        GPUMesh {
            buffer: Some({
                let unpadded_size = (vertex_buffer_size + index_buffer_size).max(min_size) as u64;
                let padded_size = Self::size_align(unpadded_size);
                let descriptor = wgpu::BufferDescriptor {
                    label: None,
                    mapped_at_creation: true,
                    size: padded_size,
                    usage: wgpu::BufferUsages::VERTEX
                        | wgpu::BufferUsages::INDEX
                        | wgpu::BufferUsages::COPY_DST,
                };
                let buffer = device.create_buffer(&descriptor);

                {
                    let mut mapped = buffer.slice(..).get_mapped_range_mut();
                    mapped
                        .slice(..vertex_buffer_size)
                        .copy_from_slice(bytemuck::cast_slice(mesh.vertices.as_slice()));
                    let mut mapped_indices = mapped.slice(
                        vertex_buffer_size..(vertex_buffer_size + index_buffer_size) as usize,
                    );
                    match index_format {
                        IndexFormat::Uint16 => {
                            let mapped_indices_u16: &mut [u16] = bytemuck::cast_slice_mut(unsafe {
                                mapped_indices.as_raw_ptr().as_ptr().as_mut().unwrap()
                                    as &mut [u8]
                            });
                            for i in 0..mesh.indices.len() {
                                mapped_indices_u16[i] = mesh.indices[i] as u16;
                            }
                        }
                        IndexFormat::Uint32 => {
                            mapped_indices
                                .copy_from_slice(bytemuck::cast_slice(mesh.indices.as_slice()));
                        }
                    }
                }
                buffer.unmap();
                buffer
            }),
            render_data: Some(GPUMeshRenderData {
                vertex_length: vertex_buffer_size as u64,
                index_format,
                index_count: mesh.indices.len() as u32,
            }),
        }
    }
    pub fn upload<T: Pod>(
        &mut self,
        new_mesh: &Mesh<T>,
        device: &Device,
        staging_belt: &mut StagingBelt,
        command_encoder: &mut CommandEncoder,
    ) {
        if new_mesh.indices.is_empty() {
            self.render_data = None;
            return;
        }
        if self.buffer.is_none() {
            *self = Self::allocate(new_mesh, 0, device);
            return;
        }
        let (vertex_buffer_size, index_buffer_size, index_format) = new_mesh.get_data_size();
        let total_size = vertex_buffer_size + index_buffer_size;
        let buffer = self.buffer.as_ref().unwrap();
        if buffer.size() < total_size as u64 {
            *self = Self::allocate(new_mesh, total_size + total_size / 2, device);
            return;
        }
        let mut buffer_view = staging_belt.write_buffer(
            command_encoder,
            buffer,
            0,
            BufferSize::new(Self::size_align(total_size as u64)).unwrap(),
        );
        buffer_view
            .slice(..vertex_buffer_size)
            .copy_from_slice(bytemuck::cast_slice(new_mesh.vertices.as_slice()));
        match index_format {
            IndexFormat::Uint16 => {
                let mut buffer_slice = buffer_view.slice(vertex_buffer_size..);
                let buffer_slice = unsafe {
                    wgpu::WriteOnly::new(NonNull::slice_from_raw_parts(
                        buffer_slice.as_raw_ptr().cast::<u16>(),
                        buffer_slice.len() / 2,
                    ))
                };
                buffer_slice.write_iter(new_mesh.indices.iter().map(|i| *i as u16));
            }
            IndexFormat::Uint32 => {
                buffer_view
                    .slice(vertex_buffer_size..)
                    .copy_from_slice(bytemuck::cast_slice(new_mesh.indices.as_slice()));
            }
        }
        self.render_data = Some(GPUMeshRenderData {
            vertex_length: vertex_buffer_size as u64,
            index_count: new_mesh.indices.len() as u32,
            index_format,
        });
    }
    pub fn draw(&self, render_pass: &mut RenderPass<'_>) {
        match (&self.buffer, &self.render_data) {
            (Some(buffer), Some(render_data)) => {
                render_pass.set_vertex_buffer(0, buffer.slice(..render_data.vertex_length));
                render_pass.set_index_buffer(
                    buffer.slice(render_data.vertex_length..),
                    render_data.index_format,
                );
                render_pass.draw_indexed(0..render_data.index_count, 0, 0..1);
            }
            _ => {}
        }
    }
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
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
    pub fn get_data_size(&self) -> (usize, usize, IndexFormat) {
        let index_format = if self.vertices.len() <= u16::MAX as usize {
            IndexFormat::Uint16
        } else {
            IndexFormat::Uint32
        };
        let vertex_buffer_size = self.vertices.len() * std::mem::size_of::<T>();
        (
            vertex_buffer_size,
            self.indices.len()
                * match index_format {
                    IndexFormat::Uint16 => 2,
                    IndexFormat::Uint32 => 4,
                },
            index_format,
        )
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
}

pub struct GPUUniform<T: Pod + NoUninit> {
    pub buffer: wgpu::Buffer,
    pub bind_group_layout: BindGroupLayout,
    pub bind_group: BindGroup,
    _pd: PhantomData<T>,
}
impl<T: Pod + NoUninit> GPUUniform<T> {
    pub fn new(device: &Device, storage: bool) -> GPUUniform<T> {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: None,
            mapped_at_creation: false,
            size: std::mem::size_of::<T>() as u64,
            usage: if storage {
                wgpu::BufferUsages::STORAGE
            } else {
                wgpu::BufferUsages::UNIFORM
            } | wgpu::BufferUsages::COPY_DST,
        });
        Self::new_with_buffer(device, storage, buffer)
    }
    pub fn new_with_buffer(device: &Device, storage: bool, buffer: Buffer) -> GPUUniform<T> {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: if storage {
                        wgpu::BufferBindingType::Storage { read_only: true }
                    } else {
                        wgpu::BufferBindingType::Uniform
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
            label: None,
        });
        GPUUniform {
            buffer,
            bind_group_layout,
            bind_group,
            _pd: PhantomData,
        }
    }
    pub fn write(&self, queue: &Queue, data: &T) {
        queue.write_buffer(
            &self.buffer,
            0,
            bytemuck::cast_slice(std::slice::from_ref(data)),
        );
    }
}
pub struct GPURenderPipeline {
    pub render_pipeline: wgpu::RenderPipeline,
}
impl GPURenderPipeline {
    pub fn new<T: VertexDescription>(
        device: &Device,
        shader: &str,
        bind_group_layouts: &[Option<&BindGroupLayout>],
        alpha_blending: Option<wgpu::BlendState>,
        face_cull: Option<wgpu::Face>,
        target_format: Option<wgpu::TextureFormat>,
        depth_format: Option<wgpu::TextureFormat>,
    ) -> GPURenderPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(shader),
            source: wgpu::ShaderSource::Wgsl(load_shader_source(shader).into()),
        });
        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts,
                immediate_size: 0,
            });
        let vertex_description = T::vertex_description();
        let targets = match target_format {
            Some(target_format) => Some(wgpu::ColorTargetState {
                format: target_format,
                blend: alpha_blending,
                write_mask: wgpu::ColorWrites::ALL,
            }),
            None => None,
        };
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(std::any::type_name::<T>()),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: match &vertex_description {
                    Some(desc) => std::slice::from_ref(desc),
                    None => &[],
                },
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: match target_format {
                    Some(_) => std::slice::from_ref(&targets),
                    None => &[],
                },
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: face_cull,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: match depth_format {
                Some(depth_format) => Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                None => None,
            },
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            cache: None,
            multiview_mask: None,
        });
        GPURenderPipeline { render_pipeline }
    }
}

struct BlockRotationFacePrecompute {
    rotation_face_coords: [([MeshVertex; 4], Face); 24 * 6],
}
static BLOCK_ROTATION_FACE_TABLE: OnceLock<BlockRotationFacePrecompute> = OnceLock::new();
pub fn get_block_rotation_face_vertices(
    rotation: BlockRotation,
    world_face: Face,
) -> &'static ([MeshVertex; 4], Face) {
    let table = BLOCK_ROTATION_FACE_TABLE.get_or_init(|| BlockRotationFacePrecompute {
        rotation_face_coords: std::array::from_fn(|i| {
            let rotation: BlockRotation = unsafe { std::mem::transmute((i % 24) as u8) };
            let world_face = Face::all()[i / 24];
            let local_face = rotation.inverse_rotate_face(world_face);
            let matrix = get_block_matrix(BlockPos::all(0), rotation);
            (
                local_face
                    .get_vertices(
                        TexCoords {
                            u1: 0.,
                            u2: 1.,
                            v1: 0.,
                            v2: 1.,
                        },
                        0,
                    )
                    .map(|(position, uv)| MeshVertex {
                        position: {
                            let position =
                                cgmath::Point3::new(position.x - 0.5, position.y, position.z - 0.5);
                            let rotated = matrix.transform_point(position);
                            Pos {
                                x: rotated.x,
                                y: rotated.y,
                                z: rotated.z,
                            }
                        },
                        normal: world_face.get_offset(),
                        uv,
                    }),
                local_face,
            )
        }),
    });
    &table.rotation_face_coords[rotation as usize + world_face as usize * 24]
}
