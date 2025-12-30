use block_byte_common::coord::Vec3;
use block_byte_common::registry::{Key, TextureData};
use cgmath::{Matrix4, SquareMatrix};
use image::RgbaImage;
use std::f64::consts::PI;
use std::iter;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::{
    BindGroup, BindGroupLayout, BlendState, Buffer, BufferUsages, CommandEncoder, Device, LoadOp,
    Queue, Sampler, TextureView,
};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::Window;

pub struct RenderState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    pub queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    window: Arc<Window>,
    chunk_render_pipeline: wgpu::RenderPipeline,
    gui_render_pipeline: wgpu::RenderPipeline,
    texture: GPUTexture,
    camera_uniform: CameraUniform,
    camera_buffer: Buffer,
    gui_camera_buffer: Buffer,
    camera_bind_group: wgpu::BindGroup,
    gui_camera_bind_group: wgpu::BindGroup,
    depth_texture: (wgpu::Texture, Sampler, TextureView),
}

impl RenderState {
    pub async fn new(window: Window, texture_image: RgbaImage) -> Self {
        let window = Arc::new(window);
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
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
        let chunk_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Chunk Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shader.wgsl").into()),
        });
        let gui_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GUI Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/gui_shader.wgsl").into()),
        });
        let camera_uniform = CameraUniform::new();
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera Buffer"),
            contents: bytemuck::cast_slice(&[camera_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let gui_camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GUI Camera Buffer"),
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
        let gui_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: gui_camera_buffer.as_entire_binding(),
            }],
            label: Some("gui_camera_bind_group"),
        });
        let depth_texture = create_depth_texture(&device, &config, "depth_texture");
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
                        blend: Some(wgpu::BlendState::REPLACE),
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
                    blend: Some(wgpu::BlendState::REPLACE),
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
            device,
            gui_camera_buffer,
        }
    }

    pub fn window(&self) -> &Window {
        &self.window
    }
    pub fn device(&self) -> &Device {
        &self.device
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
            self.depth_texture = create_depth_texture(&self.device, &self.config, "depth_texture");
        }
    }

    pub fn render(
        &mut self,
        camera: &ClientPlayer,
        world: &mut ClientWorld,
        dt: f32,
    ) -> Result<(), wgpu::SurfaceError> {
        let mut entity_mesh = Mesh::default();
        let mut gui_mesh = Mesh::default();
        world.tick_client(&self.device, &mut entity_mesh, &mut gui_mesh);
        let aspect_ratio = self.size.width as f32 / self.size.height as f32;
        text_renderer().draw(
            Vec3 {
                x: -aspect_ratio + 0.1,
                y: 0.9,
                z: 0.,
            },
            &format!("{:?} {}", world.player_position, 1. / dt),
            0.05,
            &mut gui_mesh,
        );

        {
            let crosshair_size = Vec3 {
                x: 0.02,
                y: 0.02,
                z: 0.,
            };
            let crosshair_texture = Key::<TextureData>::id("crosshair").unwrap().tex_coords();
            gui_mesh.add_quad(-crosshair_size / 2., crosshair_size, crosshair_texture);
        }

        self.camera_uniform
            .load_view_proj_matrix(camera, self.size.width as f32 / self.size.height as f32);
        self.queue.write_buffer(
            &self.camera_buffer,
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

        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Chunk Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.2,
                            b: 0.3,
                            a: 1.0,
                        }),
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

            for (_, chunk) in &world.chunks {
                if let Some((buffer, count)) = &chunk.buffer {
                    render_pass.set_vertex_buffer(0, buffer.slice(..));
                    render_pass.draw(0..*count, 0..1);
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

        {
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

            if !gui_mesh.vertices.is_empty() {
                let gui_buffer =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("Entity Vertex Buffer"),
                            contents: bytemuck::cast_slice(gui_mesh.vertices.as_slice()),
                            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        });
                render_pass.set_vertex_buffer(0, gui_buffer.slice(..));
                render_pass.draw(0..gui_mesh.vertices.len() as u32, 0..1);
            }
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
}
impl Vertex {
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
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GUIVertex {
    pub position: [f32; 2],
    pub tex_coords: [f32; 2],
}
impl GUIVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];

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
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
}
impl CameraUniform {
    fn new() -> Self {
        Self {
            view_proj: cgmath::Matrix4::identity().into(),
        }
    }
    fn load_view_proj_matrix(&mut self, camera: &ClientPlayer, aspect_ratio: f32) {
        self.view_proj = (Self::OPENGL_TO_WGPU_MATRIX
            * ClientPlayer::create_projection_matrix(aspect_ratio)
            * camera.create_view_matrix())
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

use crate::{ClientPlayer, ClientWorld, Mesh, TexCoordsExt, text_renderer};

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
    config: &wgpu::SurfaceConfiguration,
    label: &str,
) -> (wgpu::Texture, Sampler, TextureView) {
    let size = wgpu::Extent3d {
        width: config.width,
        height: config.height,
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
