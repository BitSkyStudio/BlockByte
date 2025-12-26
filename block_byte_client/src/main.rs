mod render;

use std::{
    collections::{HashMap, HashSet},
    net::{SocketAddr, UdpSocket},
    ops::ControlFlow,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};

use block_byte_common::{
    coord::{BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, Pos, Ray, Vec3},
    net::{NetworkMessageC2S, NetworkMessageS2C},
    registry::{self, BlockPalette, Registry, TextureData, TextureKey, load_registries},
};
use image::RgbaImage;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use renet::{ConnectionConfig, DefaultChannel, RenetClient};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};
use wgpu::{Buffer, Device, util::DeviceExt};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, Event, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::KeyCode,
    window::{Window, WindowAttributes, WindowId},
};

use crate::render::{RenderState, Vertex};

fn main() {
    load_registries(&Path::new("assets"));
    use block_byte_common::registry::RegistryProvider;
    let (atlas, image) = TextureAtlas::pack(registry::REGISTRIES.get().unwrap().get_registry());
    TEXTURE_ATLAS.set(atlas);

    let mut client = RenetClient::new(ConnectionConfig::default());
    let server_addr: SocketAddr = "127.0.0.1:5000".parse().unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let authentication = ClientAuthentication::Unsecure {
        server_addr,
        client_id: current_time.as_millis() as u64,
        user_data: None,
        protocol_id: 0,
    };

    let mut transport = NetcodeClientTransport::new(current_time, authentication, socket).unwrap();

    let event_loop = EventLoop::new().unwrap();
    event_loop
        .run_app(&mut App {
            camera: ClientPlayer::default(),
            render_state: None,
            texture_image: Some(image),
            world: ClientWorld::default(),
            network_client: client,
            network_transport: transport,
            keys: HashSet::new(),
            last_update: Instant::now(),
        })
        .unwrap();
}

struct App {
    texture_image: Option<RgbaImage>,
    render_state: Option<RenderState>,
    world: ClientWorld,
    camera: ClientPlayer,
    network_client: RenetClient,
    network_transport: NetcodeClientTransport,
    keys: HashSet<KeyCode>,
    last_update: Instant,
}
impl App {
    pub fn send_message(&mut self, message: NetworkMessageC2S) {
        self.network_client.send_message(
            DefaultChannel::ReliableOrdered,
            bincode::serde::encode_to_vec(message, bincode::config::standard()).unwrap(),
        );
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window_attributes = WindowAttributes::default();
        let window = match event_loop.create_window(window_attributes) {
            Ok(window) => window,
            Err(err) => {
                eprintln!("error creating window: {err}");
                event_loop.exit();
                return;
            }
        };
        self.render_state = Some(pollster::block_on(RenderState::new(
            window,
            self.texture_image.take().unwrap(),
        )));
    }
    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        match event {
            DeviceEvent::MouseMotion { delta } => {
                self.camera
                    .update_orientation(-delta.1 as f32, -delta.0 as f32);
            }
            _ => {}
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("Close was requested; stopping");
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                self.render_state.as_mut().unwrap().resize(new_size);
            }
            WindowEvent::KeyboardInput {
                device_id,
                event,
                is_synthetic,
            } => match event.physical_key {
                winit::keyboard::PhysicalKey::Code(key_code) => {
                    if event.state == ElementState::Pressed {
                        self.keys.insert(key_code);
                    } else {
                        self.keys.remove(&key_code);
                    }
                }
                winit::keyboard::PhysicalKey::Unidentified(native_key_code) => {}
            },
            WindowEvent::MouseInput {
                device_id,
                state,
                button,
            } => {
                if state == ElementState::Pressed {
                    let ray = Ray {
                        position: self.camera.get_eye(),
                        direction: self.camera.make_front() * 10.,
                    };
                    let hit = ray.block_raycast(|block, _, face| {
                        let (chunk, offset) = block.to_chunk_pos_offset();
                        if self
                            .world
                            .chunks
                            .get(&chunk)?
                            .blocks
                            .get(offset.index())
                            .unwrap()
                            .data()
                            .selection
                            .len()
                            > 0
                        {
                            Some((block, face))
                        } else {
                            None
                        }
                    });
                    match button {
                        MouseButton::Left => {
                            if let Some(hit) = hit {
                                self.send_message(NetworkMessageC2S::AttackBlock {
                                    position: hit.0,
                                });
                            }
                        }
                        MouseButton::Right => {
                            if let Some(hit) = hit {
                                self.send_message(NetworkMessageC2S::InteractBlock {
                                    position: hit.0,
                                    face: hit.1,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let dt = self.last_update.elapsed().as_secs_f32();
                self.last_update = Instant::now();
                //println!("{}", 1. / dt);
                // Ref the application.
                //
                // It's preferable for applications that do not render continuously to render in
                // this event rather than in AboutToWait, since rendering in here allows
                // the program to gracefully handle redraws requested by the OS.

                self.render_state
                    .as_ref()
                    .unwrap()
                    .window()
                    .pre_present_notify();
                // Notify that you're about to draw.

                // Draw.
                let render_state = self.render_state.as_mut().unwrap();
                match render_state.render(&self.camera, &mut self.world) {
                    Ok(_) => {}
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        render_state.resize(render_state.size())
                    }
                    Err(err) => {
                        println!("error: {:?}", err);
                        event_loop.exit();
                    }
                }

                self.render_state
                    .as_ref()
                    .unwrap()
                    .window()
                    .request_redraw();
                {
                    let delta_time_duration = Duration::from_secs_f32(dt);
                    self.network_client.update(delta_time_duration);
                    self.network_transport
                        .update(delta_time_duration, &mut self.network_client)
                        .unwrap();
                }
                if self.network_client.is_connected() {
                    self.camera.update_position(&self.keys, dt, &self.world);
                    self.send_message(NetworkMessageC2S::PlayerPosition {
                        position: self.camera.position,
                    });
                    while let Some(message) = self
                        .network_client
                        .receive_message(DefaultChannel::ReliableOrdered)
                    {
                        let (message, _): (NetworkMessageS2C, _) =
                            bincode::serde::decode_from_slice(
                                &message,
                                bincode::config::standard(),
                            )
                            .unwrap();
                        match message {
                            NetworkMessageS2C::LoadChunk { position, blocks } => {
                                self.world.chunks.insert(
                                    position,
                                    ClientChunk {
                                        blocks,
                                        buffer: None,
                                        position,
                                    },
                                );
                                self.world.modified_chunks.insert(position);
                                for face in Face::all() {
                                    self.world
                                        .modified_chunks
                                        .insert(position + face.get_chunk_offset());
                                }
                            }
                            NetworkMessageS2C::UnloadChunk { position } => {
                                self.world.chunks.remove(&position);
                            }
                            NetworkMessageS2C::SetBlock { position, block } => {
                                let (chunk, offset) = position.to_chunk_pos_offset();
                                self.world
                                    .chunks
                                    .get_mut(&chunk)
                                    .unwrap()
                                    .blocks
                                    .set(offset.index(), &block);
                                self.world.modified_chunks.insert(chunk);
                                let offset_xyz = offset.xyz();
                                for face in Face::all() {
                                    let face_offset = face.get_block_offset();
                                    fn o(x: i32) -> i32 {
                                        if x == 0 {
                                            return -1;
                                        }
                                        if x == CHUNK_SIZE as i32 - 1 {
                                            return 1;
                                        }
                                        return 0;
                                    }
                                    if o(offset_xyz.x) == face_offset.x
                                        || o(offset_xyz.y) == face_offset.y
                                        || o(offset_xyz.z) == face_offset.z
                                    {
                                        self.world
                                            .modified_chunks
                                            .insert(chunk + face.get_chunk_offset());
                                    }
                                }
                            }
                            NetworkMessageS2C::GameTick { ticks_passed, dt } => {
                                self.world.tick_server(dt);
                            }
                        }
                    }
                } else if self.network_client.is_disconnected() {
                    println!("disconnected");
                    event_loop.exit();
                    return;
                }
                self.network_transport
                    .send_packets(&mut self.network_client)
                    .unwrap();
            }
            _ => (),
        }
    }
}

pub struct TextureAtlas {
    textures: Vec<TexCoords>,
}
impl TextureAtlas {
    pub fn pack(texture_registry: &Registry<TextureData>) -> (Self, RgbaImage) {
        let mut packer =
            texture_packer::TexturePacker::new_skyline(texture_packer::TexturePackerConfig {
                max_width: 2048,
                max_height: 2048,
                allow_rotation: false,
                texture_outlines: false,
                border_padding: 0,
                texture_padding: 0,
                trim: false,
                texture_extrusion: 0,
            });
        for (i, texture) in texture_registry.data_entries().enumerate() {
            packer.pack_ref(i, &texture.texture).unwrap();
        }
        use texture_packer::exporter::ImageExporter;
        use texture_packer::texture::Texture;
        let exporter = ImageExporter::export(&packer).unwrap();
        if false {
            exporter.save(Path::new("textureatlasdump.png")).unwrap();
        }
        (
            TextureAtlas {
                textures: (0..packer.get_frames().len())
                    .map(|i| {
                        let frame = packer.get_frame(&i).unwrap();
                        TexCoords {
                            u1: frame.frame.x as f32 / packer.width() as f32,
                            v1: frame.frame.y as f32 / packer.height() as f32,
                            u2: (frame.frame.x + frame.frame.w) as f32 / packer.width() as f32,
                            v2: (frame.frame.y + frame.frame.h) as f32 / packer.height() as f32,
                        }
                    })
                    .collect(),
            },
            exporter.to_rgba8(),
        )
    }
}
impl std::ops::Index<TextureKey> for TextureAtlas {
    type Output = TexCoords;
    fn index(&self, texture: TextureKey) -> &Self::Output {
        &self.textures[texture.numeric_id()]
    }
}
#[derive(Clone, Copy)]
pub struct TexCoords {
    pub u1: f32,
    pub v1: f32,
    pub u2: f32,
    pub v2: f32,
}
static TEXTURE_ATLAS: OnceLock<TextureAtlas> = OnceLock::new();
trait TexCoordsExt {
    fn tex_coords(self) -> TexCoords;
}
impl TexCoordsExt for TextureKey {
    fn tex_coords(self) -> TexCoords {
        TEXTURE_ATLAS.get().unwrap()[self]
    }
}
#[derive(Debug)]
pub struct ClientPlayer {
    pub position: Pos,
    pub pitch_deg: f32,
    pub yaw_deg: f32,
    speed: f32,
}
impl Default for ClientPlayer {
    fn default() -> Self {
        ClientPlayer {
            position: Pos {
                x: 0.,
                y: 0.,
                z: 0.,
            },
            pitch_deg: 0.,
            yaw_deg: 0.,
            speed: 10.,
        }
    }
}
impl ClientPlayer {
    const UP: cgmath::Vector3<f32> = cgmath::Vector3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    pub fn make_front(&self) -> Vec3<f32> {
        let pitch_rad = f32::to_radians(self.pitch_deg);
        let yaw_rad = f32::to_radians(self.yaw_deg);
        Vec3 {
            x: yaw_rad.sin() * pitch_rad.cos(),
            y: pitch_rad.sin(),
            z: yaw_rad.cos() * pitch_rad.cos(),
        }
    }
    pub fn update_orientation(&mut self, d_pitch_deg: f32, d_yaw_deg: f32) {
        self.pitch_deg = (self.pitch_deg + d_pitch_deg).max(-89.0).min(89.0);
        self.yaw_deg = (self.yaw_deg + d_yaw_deg) % 360.0;
    }
    pub fn get_eye(&self) -> Pos {
        self.position
            + Pos {
                x: 0.,
                y: self.eye_height_diff(),
                z: 0.,
            }
    }
    pub fn update_position(
        &mut self,
        keys: &HashSet<KeyCode>,
        delta_time: f32,
        world: &ClientWorld,
    ) {
        let mut forward = cgmath::Vector3::new(
            f32::to_radians(self.yaw_deg).sin(),
            0.,
            f32::to_radians(self.yaw_deg).cos(),
        );
        use cgmath::InnerSpace;
        let cross_normalized = forward.cross(Self::UP).normalize();
        let mut move_vector = keys.iter().copied().fold(
            cgmath::Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            |vec, key| match key {
                KeyCode::KeyW => vec + forward,
                KeyCode::KeyS => vec - forward,
                KeyCode::KeyA => vec - cross_normalized,
                KeyCode::KeyD => vec + cross_normalized,
                _ => vec,
            },
        );

        if !(move_vector.x == 0.0 && move_vector.y == 0.0 && move_vector.z == 0.0) {
            move_vector = move_vector.normalize();
        }
        if keys.contains(&KeyCode::Space) {
            move_vector.y += 1.;
        }
        if keys.contains(&KeyCode::ShiftLeft) {
            move_vector.y -= 1.;
        }

        move_vector *= self.speed;
        move_vector *= 5.;

        let mut total_move = move_vector * delta_time;
        self.position += Pos {
            x: total_move.x,
            y: total_move.y,
            z: total_move.z,
        };
    }
    fn eye_height_diff(&self) -> f32 {
        2. - 0.15
    }
    pub fn create_view_matrix(&self) -> cgmath::Matrix4<f32> {
        let eye = self.get_eye();
        let eye = cgmath::Point3 {
            x: eye.x as f32,
            y: eye.y as f32,
            z: eye.z as f32,
        };
        let front = self.make_front();
        cgmath::Matrix4::look_at_rh(
            eye,
            eye + cgmath::Vector3 {
                x: front.x,
                y: front.y,
                z: front.z,
            },
            Self::UP,
        )
    }
    pub fn create_default_view_matrix() -> cgmath::Matrix4<f32> {
        cgmath::Matrix4::look_at_rh(
            cgmath::point3(0., 0., 0.),
            cgmath::point3(0., 0., -1.),
            ClientPlayer::UP,
        )
    }
    pub fn create_projection_matrix(aspect: f32) -> cgmath::Matrix4<f32> {
        cgmath::perspective(cgmath::Deg(90.), aspect, 0.05, 500.)
    }
}
#[derive(Default)]
pub struct ClientWorld {
    pub chunks: HashMap<ChunkPos, ClientChunk>,
    pub modified_chunks: HashSet<ChunkPos>,
}
impl ClientWorld {
    pub fn tick_client(&mut self, device: &Device) {
        let max_chunk_meshes_per_frame = 64;
        for (position, mesh) in self
            .modified_chunks
            .extract_if(|_| true)
            .take(max_chunk_meshes_per_frame)
            .collect::<Vec<_>>()
            .into_par_iter()
            .filter_map(|chunk_position| {
                let chunk = self.chunks.get(&chunk_position);
                let neighbors = FaceMap::init(|face| {
                    self.chunks.get(&(chunk_position + face.get_chunk_offset()))
                });
                match chunk {
                    Some(chunk) => Some((chunk_position, chunk.rebuild_chunk_mesh(neighbors))),
                    None => None,
                }
            })
            .collect::<Vec<_>>()
        {
            self.chunks.get_mut(&position).unwrap().buffer = if mesh.vertices.len() == 0 {
                None
            } else {
                Some((
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Chunk Vertex Buffer"),
                        contents: bytemuck::cast_slice(mesh.vertices.as_slice()),
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    }),
                    mesh.vertices.len() as u32,
                ))
            };
        }
    }
    pub fn tick_server(&mut self, dt: f32) {}
}
pub struct ClientChunk {
    pub position: ChunkPos,
    pub blocks: BlockPalette,
    pub buffer: Option<(Buffer, u32)>,
}
pub struct ChunkMesh {
    pub vertices: Vec<Vertex>,
}
impl ClientChunk {
    pub fn rebuild_chunk_mesh(&self, neighbor_chunks: FaceMap<Option<&ClientChunk>>) -> ChunkMesh {
        let mut vertices: Vec<Vertex> = Vec::new();

        for x in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    let block = *self.blocks.get(ChunkOffset::new(x, y, z).index()).unwrap();
                    let block_data = block.data();
                    match &block_data.render_data {
                        registry::BlockRenderData::Air => {}
                        registry::BlockRenderData::Full { faces } => {
                            let base_position = Pos {
                                x: (self.position.x as f32 * CHUNK_SIZE as f32) + x as f32,
                                y: (self.position.y as f32 * CHUNK_SIZE as f32) + y as f32,
                                z: (self.position.z as f32 * CHUNK_SIZE as f32) + z as f32,
                            };
                            for face in Face::all() {
                                let neighbor_position = BlockPos {
                                    x: x as i32,
                                    y: y as i32,
                                    z: z as i32,
                                } + face.get_block_offset();
                                let (neighbor_chunk, neighbor_offset) =
                                    neighbor_position.to_chunk_pos_offset();
                                let neighbor_chunk: Option<&ClientChunk> = match Face::all()
                                    .iter()
                                    .find(|f| f.get_chunk_offset() == neighbor_chunk)
                                {
                                    Some(face) => *neighbor_chunks.by_face(*face),
                                    None => Some(self),
                                };
                                if let Some(neighbor_chunk) = neighbor_chunk {
                                    let neighbor_block_data = neighbor_chunk
                                        .blocks
                                        .get(neighbor_offset.index())
                                        .unwrap()
                                        .data();
                                    match &neighbor_block_data.render_data {
                                        registry::BlockRenderData::Air => {}
                                        registry::BlockRenderData::Full { faces } => {
                                            continue;
                                        }
                                    }
                                }

                                let texture = faces.by_face(*face).tex_coords();
                                Self::add_vertices(*face, texture, |position, coords| {
                                    let vt_pos = base_position + position;
                                    vertices.push(Vertex {
                                        position: [vt_pos.x, vt_pos.y, vt_pos.z],
                                        tex_coords: [coords.0, coords.1],
                                    });
                                });
                            }
                        }
                    }
                }
            }
        }

        ChunkMesh { vertices }
    }
    fn add_vertices(
        face: Face,
        coords: TexCoords,
        mut vertex_consumer: impl FnMut(Pos, (f32, f32)),
    ) {
        let (first, second, third, fourth) = match face {
            Face::Front => (
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 0.,
                },
            ),
            Face::Back => (
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 1.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 1.,
                },
            ),
            Face::Up => (
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 1.,
                },
            ),
            Face::Down => (
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 1.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 1.,
                },
            ),
            Face::Left => (
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 1.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
            ),
            Face::Right => (
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 1.,
                },
            ),
        };
        vertex_consumer(first, (coords.u1, coords.v1));
        vertex_consumer(fourth, (coords.u1, coords.v2));
        vertex_consumer(third, (coords.u2, coords.v2));

        vertex_consumer(third, (coords.u2, coords.v2));
        vertex_consumer(second, (coords.u2, coords.v1));
        vertex_consumer(first, (coords.u1, coords.v1));
    }
}
