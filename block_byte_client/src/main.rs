mod render;

use std::{
    collections::HashSet,
    net::{SocketAddr, UdpSocket},
    ops::ControlFlow,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};

use block_byte_common::{
    coord::Pos,
    net::NetworkMessageS2C,
    registry::{self, Registry, TextureData, TextureKey, load_registries},
};
use image::RgbaImage;
use renet::{ConnectionConfig, DefaultChannel, RenetClient};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, Event, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::KeyCode,
    window::{Window, WindowAttributes, WindowId},
};

use crate::render::RenderState;

fn main() {
    load_registries(&Path::new("assets"));
    use block_byte_common::registry::RegistryProvider;
    let (atlas, image) = TextureAtlas::pack(registry::REGISTRIES.get().unwrap().get_registry());
    TEXTURE_ATLAS.set(atlas).map_err(|_| ()).unwrap();

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
            world: World::default(),
            network_client: client,
            network_transport: transport,
        })
        .unwrap();
}

struct App {
    texture_image: Option<RgbaImage>,
    render_state: Option<RenderState>,
    world: World,
    camera: ClientPlayer,
    network_client: RenetClient,
    network_transport: NetcodeClientTransport,
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("Close was requested; stopping");
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                self.render_state.as_mut().unwrap().resize(new_size);
            }
            WindowEvent::RedrawRequested => {
                // Redraw the application.
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

                let delta_time = Duration::from_millis(16); //todo
                self.network_client.update(delta_time);
                self.network_transport
                    .update(delta_time, &mut self.network_client)
                    .unwrap();

                if self.network_client.is_connected() {
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
                                println!("load chunk {:?}", position);
                            }
                            NetworkMessageS2C::UnloadChunk { position } => todo!(),
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
const TEXTURE_ATLAS: OnceLock<TextureAtlas> = OnceLock::new();
pub fn tex_coords(key: TextureKey) -> TexCoords {
    TEXTURE_ATLAS.get().unwrap()[key]
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
            speed: 1.,
        }
    }
}
impl ClientPlayer {
    const UP: cgmath::Vector3<f32> = cgmath::Vector3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    pub fn make_front(&self) -> cgmath::Vector3<f32> {
        let pitch_rad = f32::to_radians(self.pitch_deg);
        let yaw_rad = f32::to_radians(self.yaw_deg);
        cgmath::Vector3 {
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
    pub fn update_position(&mut self, keys: &HashSet<KeyCode>, delta_time: f32, world: &World) {
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
        cgmath::Matrix4::look_at_rh(eye, eye + self.make_front(), Self::UP)
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
#[derive(Default, Debug)]
pub struct World {}
