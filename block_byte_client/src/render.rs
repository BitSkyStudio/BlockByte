use block_byte_common::coord::{AABB, CHUNK_SIZE, Face, Pos, Vec3};
use block_byte_common::model::{DrawAnimation, ModelTexture};
use block_byte_common::registry::{
    BlockData, BlockKey, BlockRenderData, EntityData, ItemKey, ItemModel, Key, ModelData,
    ModelInstance, ModelKey, TextureData, TextureKey,
};
use block_byte_common::ui::UIScreen;
use block_byte_common::{ClientItem, Color, TexCoords};
use cgmath::{Deg, EuclideanSpace, Matrix4, Point3, Rad, SquareMatrix, Transform, Vector3};
use image::RgbaImage;
use rand::rngs::StdRng;
use rand_seeder::Seeder;
use std::f64::consts::PI;
use std::iter;
use std::mem::size_of;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::{
    BindGroup, BindGroupLayout, BlendState, Buffer, BufferUsages, CommandEncoder, Device, LoadOp,
    Queue, Sampler, TextureView,
};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::MouseButton;
use winit::window::Window;

pub struct RenderState {
    surface: wgpu::Surface<'static>,
    pub device: Arc<wgpu::Device>,
    pub queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    window: Arc<Window>,
    chunk_render_pipeline: wgpu::RenderPipeline,
    gui_render_pipeline: wgpu::RenderPipeline,
    damage_render_pipeline: wgpu::RenderPipeline,
    skybox_render_pipeline: wgpu::RenderPipeline,
    shadow_render_pipeline: wgpu::RenderPipeline,
    skybox_buffer: Buffer,
    skybox_texture: GPUTexture,
    texture: GPUTexture,
    camera_uniform: CameraUniform,
    camera_buffer: Buffer,
    viewmodel_camera_buffer: Buffer,
    gui_camera_buffer: Buffer,
    camera_bind_group: wgpu::BindGroup,
    viewmodel_camera_bind_group: wgpu::BindGroup,
    gui_camera_bind_group: wgpu::BindGroup,
    depth_texture: (wgpu::Texture, Sampler, TextureView),
    shadow_texture: (wgpu::Texture, Sampler, TextureView),
    shadow_camera_buffer: Buffer,
    shadow_camera_bind_group: wgpu::BindGroup,
}

impl RenderState {
    pub async fn new(window: Window, texture_image: RgbaImage, skybox: RgbaImage) -> Self {
        let window = Arc::new(window);
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits::default(),
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
        let texture = GPUTexture::from_image(&device, &queue, &texture_image, Some("main texture"));
        let skybox_texture =
            GPUTexture::from_image(&device, &queue, &skybox, Some("skybox texture"));

        let chunk_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Chunk Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/chunk.wgsl").into()),
        });
        let gui_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GUI Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/gui.wgsl").into()),
        });
        let damage_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Damage Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/damage.wgsl").into()),
        });
        let skybox_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skybox Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/skybox.wgsl").into()),
        });
        let camera_uniform = CameraUniform::new();
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera Buffer"),
            contents: bytemuck::cast_slice(&[camera_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let viewmodel_camera_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Viewmodel Camera Buffer"),
                contents: bytemuck::cast_slice(&[camera_uniform]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let gui_camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GUI Camera Buffer"),
            contents: bytemuck::cast_slice(&[camera_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shadow_camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Shadow Camera Buffer"),
            contents: bytemuck::cast_slice(&[camera_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("camera_bind_group_layout"),
            });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
            label: Some("camera_bind_group"),
        });
        let viewmodel_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewmodel_camera_buffer.as_entire_binding(),
            }],
            label: Some("viewmodel_camera_bind_group"),
        });
        let gui_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: gui_camera_buffer.as_entire_binding(),
            }],
            label: Some("gui_camera_bind_group"),
        });
        let shadow_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_camera_buffer.as_entire_binding(),
            }],
            label: Some("shadow_camera_bind_group"),
        });
        let depth_texture = create_depth_texture(&device, size, "depth_texture");
        let shadow_texture = create_depth_texture(
            &device,
            PhysicalSize {
                width: 2048,
                height: 2048,
            },
            "shadow_texture",
        );
        let chunk_render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Chunk Render Pipeline Layout"),
                bind_group_layouts: &[
                    &texture.texture_bind_group_layout,
                    &camera_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });
        let chunk_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Chunk Render Pipeline"),
                layout: Some(&chunk_render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &chunk_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &chunk_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview: None,
                cache: None,
            });
        let gui_render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("GUI Render Pipeline Layout"),
                bind_group_layouts: &[
                    &texture.texture_bind_group_layout,
                    &camera_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });
        let gui_render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("GUI Render Pipeline"),
            layout: Some(&gui_render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &gui_shader,
                entry_point: Some("vs_main"),
                buffers: &[GUIVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &gui_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });
        let damage_render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Damage Render Pipeline Layout"),
                bind_group_layouts: &[&camera_bind_group_layout],
                push_constant_ranges: &[],
            });
        let damage_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Damage Render Pipeline"),
                layout: Some(&damage_render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &damage_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[DamageVertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &damage_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview: None,
                cache: None,
            });
        let skybox_render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Skybox Render Pipeline Layout"),
                bind_group_layouts: &[
                    &skybox_texture.texture_bind_group_layout,
                    &camera_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });
        let skybox_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Skybox Render Pipeline"),
                layout: Some(&skybox_render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &skybox_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[SkyboxVertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &skybox_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Always,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview: None,
                cache: None,
            });
        let mut skybox_mesh: Mesh<SkyboxVertex> = Mesh {
            vertices: Vec::new(),
        };
        for (face, tx, ty) in [
            (Face::Front, 3, 1),
            (Face::Back, 1, 1),
            (Face::Left, 2, 1),
            (Face::Right, 0, 1),
            (Face::Down, 1, 2),
            (Face::Up, 1, 0),
        ] {
            face.add_vertices(
                TexCoords {
                    u2: tx as f32 / 4.,
                    v1: ty as f32 / 3.,
                    u1: (tx + 1) as f32 / 4.,
                    v2: (ty + 1) as f32 / 3.,
                },
                0,
                |pos, (u, v)| {
                    skybox_mesh.vertices.push(SkyboxVertex {
                        position: [pos.x * 2. - 1., pos.y * 2. - 1., pos.z * 2. - 1.],
                        tex_coords: [u, v],
                    });
                },
            );
        }
        let shadow_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Shadow Render Pipeline"),
                layout: Some(&chunk_render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &chunk_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &chunk_shader,
                    entry_point: Some("fs_main"),
                    targets: &[],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview: None,
                cache: None,
            });
        let skybox_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skybox Vertex Buffer"),
            contents: bytemuck::cast_slice(skybox_mesh.vertices.as_slice()),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            window,
            surface,
            queue,
            config,
            size,
            chunk_render_pipeline,
            gui_render_pipeline,
            texture,
            camera_uniform,
            camera_buffer,
            camera_bind_group,
            gui_camera_bind_group,
            depth_texture,
            device: Arc::new(device),
            gui_camera_buffer,
            viewmodel_camera_bind_group,
            viewmodel_camera_buffer,
            damage_render_pipeline,
            skybox_buffer,
            skybox_render_pipeline,
            skybox_texture,
            shadow_texture,
            shadow_camera_bind_group,
            shadow_camera_buffer,
            shadow_render_pipeline,
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
            self.depth_texture = create_depth_texture(&self.device, new_size, "depth_texture");
        }
    }

    pub fn render(
        &mut self,
        camera: &ClientPlayer,
        game: &ClientGame,
        aspect_ratio: f32,
        entity_mesh: BaseMesh,
        gui_mesh: GUIMesh,
        viewmodel_mesh: BaseMesh,
        damage_mesh: DamageMesh,
        frustum: &Frustum,
    ) -> Result<(), wgpu::SurfaceError> {
        self.camera_uniform.load_camera_proj_matrix(
            camera,
            self.size.width as f32 / self.size.height as f32,
            90.,
            game.get_player_data(),
        );
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera_uniform]),
        );

        self.camera_uniform
            .load_view_proj_matrix(aspect_ratio, 70., Pos::ZERO, -Pos::Z);
        self.queue.write_buffer(
            &self.viewmodel_camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera_uniform]),
        );

        self.camera_uniform
            .load_gui_matrix(self.size.height as f32 / self.size.width as f32);
        self.queue.write_buffer(
            &self.gui_camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera_uniform]),
        );

        self.camera_uniform.load_light(camera.position);
        self.queue.write_buffer(
            &self.shadow_camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera_uniform]),
        );

        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });
        if false {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Shadow Render Pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.shadow_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.shadow_camera_bind_group, &[]);

            for (_, chunk) in game.chunks.iter() {
                if let Some((buffer, count)) = &chunk.buffer {
                    render_pass.set_vertex_buffer(0, buffer.slice(..));
                    render_pass.draw(0..*count, 0..1);
                }
            }
        }
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
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.skybox_render_pipeline);
            render_pass.set_bind_group(0, &self.skybox_texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.skybox_buffer.slice(..));
            render_pass.draw(0..(6 * 6), 0..1);
        }
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Chunk Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.chunk_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_bind_group, &[]);

            for (_, chunk) in &game.chunks {
                if let Some((buffer, count)) = &chunk.buffer {
                    if frustum.intersects_aabb(
                        &AABB {
                            min: Pos::all(0.),
                            max: Pos::all(CHUNK_SIZE as f32),
                        }
                        .offset(chunk.position.to_block_pos().to_pos()),
                    ) {
                        render_pass.set_vertex_buffer(0, buffer.slice(..));
                        render_pass.draw(0..*count, 0..1);
                    }
                    /*} else {
                        culled += 1;
                    }
                    total += 1;*/
                }
            }
            if !entity_mesh.vertices.is_empty() {
                let entity_buffer =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("Entity Vertex Buffer"),
                            contents: bytemuck::cast_slice(entity_mesh.vertices.as_slice()),
                            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        });
                render_pass.set_vertex_buffer(0, entity_buffer.slice(..));
                render_pass.draw(0..entity_mesh.vertices.len() as u32, 0..1);
            }
        }
        if !damage_mesh.vertices.is_empty() {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Damage Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.damage_render_pipeline);
            render_pass.set_bind_group(0, &self.camera_bind_group, &[]);
            let damage_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Damage Vertex Buffer"),
                    contents: bytemuck::cast_slice(damage_mesh.vertices.as_slice()),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });
            render_pass.set_vertex_buffer(0, damage_buffer.slice(..));
            render_pass.draw(0..damage_mesh.vertices.len() as u32, 0..1);
        }
        if !viewmodel_mesh.vertices.is_empty() {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Chunk Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.chunk_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);

            render_pass.set_bind_group(1, &self.viewmodel_camera_bind_group, &[]);
            let viewmodel_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Viewmodel Vertex Buffer"),
                        contents: bytemuck::cast_slice(viewmodel_mesh.vertices.as_slice()),
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    });
            render_pass.set_vertex_buffer(0, viewmodel_buffer.slice(..));
            render_pass.draw(0..viewmodel_mesh.vertices.len() as u32, 0..1);
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
            });
            render_pass.set_pipeline(&self.gui_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.gui_camera_bind_group, &[]);

            let gui_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Gui Vertex Buffer"),
                    contents: bytemuck::cast_slice(gui_mesh.vertices.as_slice()),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });
            render_pass.set_vertex_buffer(0, gui_buffer.slice(..));
            render_pass.draw(0..gui_mesh.vertices.len() as u32, 0..1);
        }

        self.queue.submit(iter::once(encoder.finish()));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        self.queue.submit(iter::once(encoder.finish()));
        output.present();

        Ok(())
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

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;

        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
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

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;

        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
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

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;

        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SkyboxVertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
}
impl SkyboxVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;

        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    view_proj: [[f32; 4]; 4],
}
impl CameraUniform {
    fn new() -> Self {
        Self {
            view_proj: cgmath::Matrix4::identity().into(),
        }
    }
    fn load_light(&mut self, player_pos: Pos) {
        let eye = Point3 {
            x: player_pos.x - 10.,
            y: player_pos.y + 100.,
            z: player_pos.z - 5.,
        };
        self.view_proj = (Self::OPENGL_TO_WGPU_MATRIX
            * cgmath::ortho(-100., 100., -100., 100., 0.05, 500.)
            * cgmath::Matrix4::look_at_rh(
                eye,
                cgmath::Point3 {
                    x: player_pos.x,
                    y: player_pos.y,
                    z: player_pos.z,
                },
                Vector3::unit_y(),
            ))
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
use image::{DynamicImage, Rgba};
use std::collections::HashMap;
use std::path::Path;
use texture_packer::exporter::ImageExporter;
use texture_packer::importer::ImageImporter;

use crate::clipping::Frustum;
use crate::ui::{ScreenData, render_screen, text_renderer};
use crate::{
    BaseMesh, ClientGame, ClientPlayer, DamageMesh, GUIMesh, Mesh, TEXTURE_ATLAS, TexCoordsExt,
    TexCoordsIndexExt,
};

pub struct GPUTexture {
    pub texture: wgpu::Texture,
    pub view: TextureView,
    pub sampler: Sampler,
    pub texture_bind_group_layout: BindGroupLayout,
    pub diffuse_bind_group: BindGroup,
}

impl GPUTexture {
    pub fn from_image(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: &RgbaImage,
        label: Option<&str>,
    ) -> Self {
        let dimensions = rgba.dimensions();
        let size = wgpu::Extent3d {
            width: dimensions.0,
            height: dimensions.1,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                aspect: wgpu::TextureAspect::All,
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * dimensions.0),
                rows_per_image: Some(dimensions.1),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                label: Some("texture_bind_group_layout"),
            });

        let diffuse_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &texture_bind_group_layout,
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
            label: Some("diffuse_bind_group"),
        });

        Self {
            texture,
            view,
            sampler,
            texture_bind_group_layout,
            diffuse_bind_group,
        }
    }
}
pub fn create_depth_texture(
    device: &wgpu::Device,
    size: PhysicalSize<u32>,
    label: &str,
) -> (wgpu::Texture, Sampler, TextureView) {
    let size = wgpu::Extent3d {
        width: size.width,
        height: size.height,
        depth_or_array_layers: 1,
    };
    let desc = wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    let texture = device.create_texture(&desc);

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        // 4.
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        compare: Some(wgpu::CompareFunction::LessEqual),
        lod_min_clamp: 0.0,
        lod_max_clamp: 100.0,
        ..Default::default()
    });

    (texture, sampler, view)
}
pub fn draw_block_model(
    block_key: BlockKey,
    matrix: Matrix4<f32>,
    vertex_consumer: &mut impl FnMut([f32; 3], [f32; 2], [f32; 3]),
) {
    let block = block_key.data();
    match &block.render_data {
        BlockRenderData::Air => {}
        BlockRenderData::Full { faces } => {
            for face in Face::all() {
                face.add_vertices(faces.by_face(*face).tex_coords(0), 0, |pos, (u, v)| {
                    let result_pos = (pos
                        - Pos {
                            x: 0.5,
                            y: 0.,
                            z: 0.5,
                        });
                    let result_pos = Point3::new(result_pos.x, result_pos.y, result_pos.z);
                    let result_pos = matrix.transform_point(result_pos);
                    let normal = face.get_offset();
                    let normal = Vector3::new(normal.x, normal.y, normal.z);
                    let normal = matrix.transform_vector(normal);
                    vertex_consumer(
                        [result_pos.x, result_pos.y, result_pos.z],
                        [u, v],
                        [normal.x, normal.y, normal.z],
                    );
                });
            }
        }
        BlockRenderData::Model(model) => {
            draw_model(model, matrix, vertex_consumer, &[], |_| None);
        }
    }
}
pub fn draw_model(
    model: &ModelInstance,
    matrix: Matrix4<f32>,
    vertex_consumer: &mut impl FnMut([f32; 3], [f32; 2], [f32; 3]),
    animations: &[DrawAnimation],
    item_query: impl Fn(&str) -> Option<ClientItem>,
) {
    let model_data = &model.model.data().model;
    let embed_textures = &TEXTURE_ATLAS.get().unwrap().models[model.model.numeric_id()];
    let mut item_models = Vec::new();
    model_data.draw(
        matrix,
        animations,
        |position, normal, uv, texture| {
            let texture = match &model_data.textures[texture].0 {
                ModelTexture::Embed(_, index) => embed_textures[*index],
                ModelTexture::Variable(variable) => model.textures[*variable].tex_coords(),
                ModelTexture::Texture(key) => key.tex_coords(),
            };
            let uv = texture.map((uv.0, uv.1));
            vertex_consumer(
                [position.x, position.y, position.z],
                [uv.0, uv.1],
                [normal.x, normal.y, normal.z],
            );
        },
        |matrix, binding| {
            if let Some(item) = item_query(binding) {
                item_models.push((matrix, item, binding.to_string()));
            }
        },
    );
    for (matrix, item, binding) in item_models {
        let anchor = match &item.item.data().model {
            ItemModel::Block(block) => match &block.data().render_data {
                BlockRenderData::Air => Matrix4::identity(),
                BlockRenderData::Full { faces } => Matrix4::identity(),
                BlockRenderData::Model(key) => key
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
        draw_item_model(&item.item.data().model, matrix * anchor, vertex_consumer);
    }
}
pub fn draw_item_model(
    model: &ItemModel,
    matrix: Matrix4<f32>,
    vertex_consumer: &mut impl FnMut([f32; 3], [f32; 2], [f32; 3]),
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
            draw_model(model, matrix, vertex_consumer, &[], |_| None);
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
            BlockRenderData::Model(model_instance) => model_icon_view(model_instance),
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
