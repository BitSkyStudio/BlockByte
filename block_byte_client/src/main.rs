mod render;
mod ui;

use core::f32;
use std::{
    collections::{HashMap, HashSet},
    net::{SocketAddr, UdpSocket},
    ops::ControlFlow,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};

use base64::{Engine, prelude::BASE64_STANDARD};
use block_byte_common::{
    ClientItem, Color, LookDirection, MoveMode, PlayerAbilities, TexCoords,
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, Pos, Ray, Vec3},
    net::{NetworkMessageC2S, NetworkMessageS2C, make_connection_config},
    registry::{
        self, BlockPalette, BlockRenderData, EntityData, EntityKey, Key, ModelData, ModelKey,
        Registry, TextureData, TextureKey, ToolData, air_block, load_registries,
    },
    ui::PropertyMap,
    world::{self, ClientChunkBlockComponents},
};
use cgmath::Matrix4;
use image::{DynamicImage, RgbaImage};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use renet::{ConnectionConfig, DefaultChannel, RenetClient};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};
use uuid::Uuid;
use wgpu::{Buffer, Device, util::DeviceExt};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, Event, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::KeyCode,
    window::{Window, WindowAttributes, WindowId},
};

use crate::{
    render::{DamageVertex, GUIVertex, RenderState, Vertex},
    ui::{ScreenData, TEXT_RENDERER, TextRenderer, text_renderer},
};

fn main() {
    load_registries(&Path::new("assets"));
    use block_byte_common::registry::RegistryProvider;
    let registries = &registry::REGISTRIES.get().unwrap();
    let (atlas, text_renderer, image) =
        TextureAtlas::pack(registries.get_registry(), registries.get_registry());
    TEXTURE_ATLAS.set(atlas);
    TEXT_RENDERER.set(text_renderer);
    let mut client = RenetClient::new(make_connection_config());
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
            buttons: HashSet::new(),
            last_update: Instant::now(),
            teleport_id: 0,
            player_abilities: PlayerAbilities {
                move_mode: MoveMode::Normal,
                speed: 1.,
            },
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
    buttons: HashSet<MouseButton>,
    last_update: Instant,
    teleport_id: u32,
    player_abilities: PlayerAbilities,
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
            image::load_from_memory(std::fs::read("assets/skybox.png").unwrap().as_slice())
                .unwrap()
                .to_rgba8(),
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
                if self.world.screen.is_none() {
                    self.camera
                        .update_orientation(-delta.1 as f32, -delta.0 as f32);
                }
            }
            _ => {}
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.network_client.disconnect();
                self.network_transport.disconnect();
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
                        for (slot, key) in [
                            KeyCode::Digit1,
                            KeyCode::Digit2,
                            KeyCode::Digit3,
                            KeyCode::Digit4,
                            KeyCode::Digit5,
                            KeyCode::Digit6,
                            KeyCode::Digit7,
                            KeyCode::Digit8,
                            KeyCode::Digit9,
                            KeyCode::Digit0,
                        ]
                        .iter()
                        .enumerate()
                        {
                            if key_code == *key {
                                self.send_message(NetworkMessageC2S::HotbarSelect {
                                    slot: slot as isize,
                                    relative: false,
                                });
                            }
                        }
                        if key_code == KeyCode::KeyE && self.world.screen.is_none() {
                            match self.camera.raycast(&self.world) {
                                RayCastResult::Empty => {}
                                RayCastResult::Block(position, _) => {
                                    self.send_message(NetworkMessageC2S::InteractBlock {
                                        position,
                                    });
                                }
                                RayCastResult::Entity(entity) => {
                                    self.send_message(NetworkMessageC2S::InteractEntity { entity });
                                }
                            }
                        }
                        if key_code == KeyCode::Escape {
                            self.send_message(NetworkMessageC2S::CloseUI);
                        }
                    } else {
                        self.keys.remove(&key_code);
                    }
                }
                winit::keyboard::PhysicalKey::Unidentified(native_key_code) => {}
            },
            WindowEvent::MouseWheel {
                device_id,
                delta,
                phase,
            } => match delta {
                winit::event::MouseScrollDelta::LineDelta(_, scroll) => {
                    self.send_message(NetworkMessageC2S::HotbarSelect {
                        slot: -scroll as isize,
                        relative: true,
                    });
                }
                winit::event::MouseScrollDelta::PixelDelta(physical_position) => {}
            },
            WindowEvent::MouseInput {
                device_id,
                state,
                button,
            } => {
                match state {
                    ElementState::Pressed => {
                        self.buttons.insert(button);
                    }
                    ElementState::Released => {
                        self.buttons.remove(&button);
                    }
                }
                if state == ElementState::Pressed && self.world.screen.is_none() {
                    match (self.camera.raycast(&self.world), button) {
                        (RayCastResult::Block(position, face), MouseButton::Right) => {
                            self.send_message(NetworkMessageC2S::PlaceBlock { position, face });
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
                match render_state.render(&self.camera, &mut self.world, dt) {
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
                if !self.network_client.is_disconnected() {
                    let delta_time_duration = Duration::from_secs_f32(dt);
                    self.network_client.update(delta_time_duration);
                    self.network_transport
                        .update(delta_time_duration, &mut self.network_client)
                        .unwrap();
                }
                if self.network_client.is_connected() {
                    if self.buttons.contains(&MouseButton::Left) && self.world.hit_timer.is_none() {
                        self.world.hit_timer = Some(0.);
                    }
                    self.camera.update_position(
                        &self.keys,
                        dt,
                        &self.world,
                        &self.player_abilities,
                        self.world.get_player_data(),
                    );
                    self.world.player_position = self.camera.position;
                    self.send_message(NetworkMessageC2S::PlayerPosition {
                        position: self.camera.position,
                        teleport_id: self.teleport_id,
                        direction: self.camera.direction,
                    });
                    if let Some(hit_timer) = self.world.hit_timer {
                        let new_hit_timer = hit_timer + dt;
                        let active_tool = self.world.active_tool();
                        if hit_timer < active_tool.hit_time && new_hit_timer >= active_tool.hit_time
                        {
                            match self.camera.raycast(&self.world) {
                                RayCastResult::Block(position, _) => {
                                    self.send_message(NetworkMessageC2S::AttackBlock { position });
                                }
                                RayCastResult::Entity(entity) => {
                                    self.send_message(NetworkMessageC2S::AttackEntity { entity });
                                }
                                RayCastResult::Empty => {}
                            }
                        }
                        self.world.hit_timer = Some(new_hit_timer);
                        if new_hit_timer > active_tool.swing_time {
                            self.world.hit_timer = None;
                        }
                    }
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
                            NetworkMessageS2C::LoadChunk {
                                position,
                                blocks,
                                components,
                            } => {
                                self.world.chunks.insert(
                                    position,
                                    ClientChunk {
                                        blocks,
                                        buffer: None,
                                        position,
                                        components,
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
                                {
                                    let chunk = self.world.chunks.get_mut(&chunk).unwrap();
                                    chunk.blocks.set(offset.index(), &block);
                                }
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
                            NetworkMessageS2C::AddEntity {
                                uuid,
                                key,
                                position,
                                direction,
                            } => {
                                self.world.entities.insert(
                                    uuid,
                                    ClientEntity {
                                        key,
                                        position,
                                        direction,
                                        previous_position: position,
                                        previous_direction: direction,
                                        update_timestamp: Instant::now(),
                                    },
                                );
                            }
                            NetworkMessageS2C::MoveEntity {
                                uuid,
                                position,
                                direction,
                            } => {
                                if let Some(entity) = self.world.entities.get_mut(&uuid) {
                                    entity.previous_position = entity.position;
                                    entity.previous_direction = entity.direction;
                                    entity.update_timestamp = Instant::now();
                                    entity.position = position;
                                    entity.direction = direction;
                                }
                            }
                            NetworkMessageS2C::RemoveEntity { uuid } => {
                                self.world.entities.remove(&uuid);
                            }
                            NetworkMessageS2C::UpdateBlockComponents {
                                chunk,
                                offset,
                                data,
                            } => {
                                if let Some(chunk) = self.world.chunks.get_mut(&chunk) {
                                    data.update(offset, &mut chunk.components);
                                }
                            }
                            NetworkMessageS2C::SetPlayerEntity { uuid } => {
                                self.world.player_entity = uuid;
                            }
                            NetworkMessageS2C::TeleportPlayer {
                                position,
                                teleport_id,
                            } => {
                                self.camera.position = position;
                                self.teleport_id = teleport_id;
                            }
                            NetworkMessageS2C::PlayerAbilities { abilities } => {
                                self.player_abilities = abilities;
                            }
                            NetworkMessageS2C::UIOpen { screen, slots } => {
                                self.world.screen = Some(ScreenData {
                                    screen,
                                    slots,
                                    properties: PropertyMap(HashMap::new()),
                                });
                            }
                            NetworkMessageS2C::UISetSlot { slot, item } => {
                                if let Some(screen) = &mut self.world.screen {
                                    if slot < screen.slots.len() {
                                        screen.slots[slot] = item;
                                    }
                                }
                            }
                            NetworkMessageS2C::UIClose => {
                                self.world.screen = None;
                            }
                            NetworkMessageS2C::HUDUpdate {
                                items,
                                properties,
                                held_item,
                            } => {
                                self.world.hud.slots = items;
                                self.world.hud.properties = properties;
                                match (&self.world.held_item, &held_item) {
                                    (None, None) => {}
                                    (Some(first), Some(second)) => {
                                        if first.item != second.item {
                                            self.world.hit_timer = None;
                                        }
                                    }
                                    _ => {
                                        self.world.hit_timer = None;
                                    }
                                }
                                self.world.held_item = held_item;
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
    models: Vec<Vec<TexCoords>>,
}

impl TextureAtlas {
    pub fn pack(
        texture_registry: &Registry<TextureData>,
        model_registry: &Registry<ModelData>,
    ) -> (Self, TextRenderer, RgbaImage) {
        #[derive(Hash, PartialEq, Eq, Clone)]
        enum TextureAtlasEntry {
            Texture(usize),
            Model(usize, usize),
            Glyph(usize),
        }
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
            packer
                .pack_ref(TextureAtlasEntry::Texture(i), &texture.texture)
                .unwrap();
        }
        for (i, model) in model_registry.data_entries().enumerate() {
            for (j, texture) in model.model.textures.iter().enumerate() {
                let image = image::load_from_memory_with_format(
                    BASE64_STANDARD
                        .decode(&texture["data:image/png;base64,".len()..])
                        .unwrap()
                        .as_slice(),
                    image::ImageFormat::Png,
                )
                .unwrap();

                packer
                    .pack_own(TextureAtlasEntry::Model(i, j), image)
                    .unwrap();
            }
        }
        let font = rusttype::Font::try_from_vec(std::fs::read("assets/font.ttf").unwrap()).unwrap();
        {
            let glyphs: Vec<_> = (0..font.glyph_count())
                .map(|i| {
                    font.glyph(rusttype::GlyphId(i as u16))
                        .scaled(rusttype::Scale::uniform(60.))
                        .positioned(rusttype::Point { x: 0., y: 0. })
                })
                .collect();
            for (i, g) in glyphs.iter().enumerate() {
                if let Some(bb) = g.pixel_bounding_box() {
                    let mut font_texture =
                        DynamicImage::new_rgba8(bb.width() as u32, bb.height() as u32);
                    let font_buffer = match &mut font_texture {
                        DynamicImage::ImageRgba8(buffer) => buffer,
                        _ => panic!(),
                    };
                    g.draw(|x, y, v| {
                        font_buffer.put_pixel(x, y, image::Rgba([255, 255, 255, (255. * v) as u8]));
                    });
                    packer
                        .pack_own(TextureAtlasEntry::Glyph(i), font_texture)
                        .unwrap();
                }
            }
        }
        use texture_packer::exporter::ImageExporter;
        use texture_packer::texture::Texture;
        let exporter = ImageExporter::export(&packer).unwrap();
        if false {
            exporter.save(Path::new("textureatlasdump.png")).unwrap();
        }
        (
            TextureAtlas {
                textures: texture_registry
                    .data_entries()
                    .enumerate()
                    .map(|(i, _)| {
                        let frame = packer.get_frame(&TextureAtlasEntry::Texture(i)).unwrap();
                        TexCoords {
                            u1: frame.frame.x as f32 / packer.width() as f32,
                            v1: frame.frame.y as f32 / packer.height() as f32,
                            u2: (frame.frame.x + frame.frame.w) as f32 / packer.width() as f32,
                            v2: (frame.frame.y + frame.frame.h) as f32 / packer.height() as f32,
                        }
                    })
                    .collect(),
                models: model_registry
                    .data_entries()
                    .enumerate()
                    .map(|(i, model)| {
                        model
                            .model
                            .textures
                            .iter()
                            .enumerate()
                            .map(|(j, _)| {
                                let frame =
                                    packer.get_frame(&TextureAtlasEntry::Model(i, j)).unwrap();
                                TexCoords {
                                    u1: frame.frame.x as f32 / packer.width() as f32,
                                    v1: frame.frame.y as f32 / packer.height() as f32,
                                    u2: (frame.frame.x + frame.frame.w) as f32
                                        / packer.width() as f32,
                                    v2: (frame.frame.y + frame.frame.h) as f32
                                        / packer.height() as f32,
                                }
                            })
                            .collect()
                    })
                    .collect(),
            },
            TextRenderer {
                glyphs: (0..font.glyph_count())
                    .map(|i| match packer.get_frame(&TextureAtlasEntry::Glyph(i)) {
                        Some(frame) => TexCoords {
                            u1: frame.frame.x as f32 / packer.width() as f32,
                            v1: frame.frame.y as f32 / packer.height() as f32,
                            u2: (frame.frame.x + frame.frame.w) as f32 / packer.width() as f32,
                            v2: (frame.frame.y + frame.frame.h) as f32 / packer.height() as f32,
                        },
                        None => TexCoords {
                            u1: 0.,
                            v1: 0.,
                            u2: 0.,
                            v2: 0.,
                        },
                    })
                    .collect(),
                font,
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

static TEXTURE_ATLAS: OnceLock<TextureAtlas> = OnceLock::new();
trait TexCoordsExt {
    fn tex_coords(self) -> TexCoords;
}
impl TexCoordsExt for TextureKey {
    fn tex_coords(self) -> TexCoords {
        TEXTURE_ATLAS.get().unwrap()[self]
    }
}
pub enum RayCastResult {
    Empty,
    Block(BlockPos, Face),
    Entity(Uuid),
}
pub struct ClientPlayer {
    pub position: Pos,
    pub velocity: Pos,
    pub direction: LookDirection,
    pub on_ground: bool,
}
impl Default for ClientPlayer {
    fn default() -> Self {
        ClientPlayer {
            position: Pos {
                x: 0.,
                y: 0.,
                z: 0.,
            },
            velocity: Pos {
                x: 0.,
                y: 0.,
                z: 0.,
            },
            direction: LookDirection { pitch: 0., yaw: 0. },
            on_ground: false,
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
        Vec3 {
            x: self.direction.yaw.sin() * self.direction.pitch.cos(),
            y: self.direction.pitch.sin(),
            z: self.direction.yaw.cos() * self.direction.pitch.cos(),
        }
    }
    pub fn update_orientation(&mut self, d_pitch_deg: f32, d_yaw_deg: f32) {
        let d_pitch = d_pitch_deg.to_radians();
        let d_yaw = d_yaw_deg.to_radians();
        use std::f32::consts::*;
        self.direction.pitch = (self.direction.pitch + d_pitch)
            .max(-PI / 2. + 0.01)
            .min(PI / 2. - 0.01);
        self.direction.yaw = (self.direction.yaw + d_yaw) % (PI * 2.);
    }
    pub fn get_eye(&self, player_entity_data: Option<&EntityData>) -> Pos {
        self.position
            + Pos {
                x: 0.,
                y: player_entity_data.map(|data| data.eye_height).unwrap_or(0.),
                z: 0.,
            }
    }
    pub fn raycast(&self, world: &ClientWorld) -> RayCastResult {
        let ray = Ray {
            position: self.get_eye(world.get_player_data()),
            direction: self.make_front() * 10.,
        };
        let mut min_distance = f32::INFINITY;
        let mut raycast_result = RayCastResult::Empty;

        if let Some((block, position, face)) = ray.block_raycast(|block, position, face| {
            let (chunk, offset) = block.to_chunk_pos_offset();
            if world
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
                Some((block, position, face))
            } else {
                None
            }
        }) {
            min_distance = ray.position.distance(position);
            raycast_result = RayCastResult::Block(block, face);
        }
        for (id, entity) in &world.entities {
            if Some(*id) == world.player_entity {
                continue;
            }
            let entity_data = entity.key.data();
            if let Some(result) = ray.aabb_raycast(entity_data.hitbox().offset(entity.position)) {
                let distance = result.position.distance(ray.position);
                if distance < min_distance {
                    min_distance = distance;
                    raycast_result = RayCastResult::Entity(*id);
                }
            }
        }
        raycast_result
    }
    pub fn update_position(
        &mut self,
        keys: &HashSet<KeyCode>,
        delta_time: f32,
        world: &ClientWorld,
        abilities: &PlayerAbilities,
        player_entity_data: Option<&EntityData>,
    ) {
        let mut forward =
            cgmath::Vector3::new(self.direction.yaw.sin(), 0., self.direction.yaw.cos());
        use cgmath::InnerSpace;
        let cross_normalized = forward.cross(Self::UP).normalize();
        let move_vector = keys.iter().copied().fold(
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
                KeyCode::Space => vec + Self::UP,
                KeyCode::ShiftLeft => vec - Self::UP,
                _ => vec,
            },
        );
        let mut move_vector = Pos {
            x: move_vector.x,
            y: move_vector.y,
            z: move_vector.z,
        };
        if !(move_vector.x == 0.0 && move_vector.z == 0.0) {
            let xz_mag = (move_vector.x.powi(2) + move_vector.z.powi(2)).sqrt();
            move_vector.x /= xz_mag;
            move_vector.z /= xz_mag;
        }
        move_vector *= abilities.speed;
        move_vector *= 7.;
        let total_move = match abilities.move_mode {
            MoveMode::Normal => {
                move_vector.y = 0.;
                if keys.contains(&KeyCode::ShiftLeft) {
                    move_vector /= 2.;
                }
                if keys.contains(&KeyCode::Space) && self.on_ground {
                    self.velocity.y += 5.;
                }
                self.velocity.y -= 10. * delta_time;
                (move_vector + self.velocity) * delta_time
            }
            MoveMode::Fly | MoveMode::NoClip => move_vector * delta_time,
        };

        match abilities.move_mode {
            MoveMode::Normal | MoveMode::Fly => {
                if !Self::collides_at(
                    self.position
                        + Pos {
                            x: total_move.x,
                            y: 0.,
                            z: 0.,
                        },
                    world,
                    player_entity_data,
                ) {
                    self.position.x += total_move.x;
                } else {
                    self.velocity.x = 0.;
                }
                if !Self::collides_at(
                    self.position
                        + Pos {
                            x: 0.,
                            y: total_move.y,
                            z: 0.,
                        },
                    world,
                    player_entity_data,
                ) {
                    self.position.y += total_move.y;
                    self.on_ground = false;
                } else {
                    self.on_ground = self.velocity.y < 0.;
                    self.velocity.y = 0.;
                }
                if !Self::collides_at(
                    self.position
                        + Pos {
                            x: 0.,
                            y: 0.,
                            z: total_move.z,
                        },
                    world,
                    player_entity_data,
                ) {
                    self.position.z += total_move.z;
                } else {
                    self.velocity.z = 0.;
                }
            }
            MoveMode::NoClip => {
                self.position += total_move;
            }
        }
    }
    fn collides_at(
        position: Pos,
        world: &ClientWorld,
        player_entity_data: Option<&EntityData>,
    ) -> bool {
        match player_entity_data {
            Some(player_entity_data) => {
                let collider = AABB {
                    min: Pos {
                        x: -player_entity_data.hitbox_size,
                        y: 0.,
                        z: -player_entity_data.hitbox_size,
                    },
                    max: Pos {
                        x: player_entity_data.hitbox_size,
                        y: player_entity_data.hitbox_height,
                        z: player_entity_data.hitbox_size,
                    },
                }
                .offset(position)
                .to_block();
                for block in collider {
                    let (chunk, offset) = block.to_chunk_pos_offset();
                    match world.chunks.get(&chunk) {
                        Some(chunk) => {
                            if !chunk
                                .blocks
                                .get(offset.index())
                                .unwrap()
                                .data()
                                .selection
                                .is_empty()
                            {
                                return true;
                            }
                        }
                        None => return true,
                    }
                }
                false
            }
            None => true,
        }
    }
    fn eye_height_diff(&self) -> f32 {
        2. - 0.15
    }
    pub fn create_view_matrix(
        &self,
        player_entity_data: Option<&EntityData>,
    ) -> cgmath::Matrix4<f32> {
        let eye = self.get_eye(player_entity_data);
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
pub struct ClientWorld {
    pub player_position: Pos,
    pub chunks: HashMap<ChunkPos, ClientChunk>,
    pub modified_chunks: HashSet<ChunkPos>,
    pub entities: HashMap<Uuid, ClientEntity>,
    pub player_entity: Option<Uuid>,
    pub screen: Option<ScreenData>,
    pub hud: ScreenData,
    pub hit_timer: Option<f32>,
    pub held_item: Option<ClientItem>,
}
impl Default for ClientWorld {
    fn default() -> Self {
        Self {
            player_position: Pos {
                x: 0.,
                y: 0.,
                z: 0.,
            },
            chunks: Default::default(),
            modified_chunks: Default::default(),
            entities: Default::default(),
            player_entity: None,
            screen: None,
            hud: ScreenData {
                screen: Key::id("hud").unwrap(),
                slots: vec![],
                properties: PropertyMap(HashMap::new()),
            },
            hit_timer: None,
            held_item: None,
        }
    }
}
impl ClientWorld {
    pub fn active_tool(&self) -> ToolData {
        self.held_item
            .as_ref()
            .and_then(|item| item.item.data().tool)
            .unwrap_or(ToolData::hand())
    }
    pub fn tick_client(
        &mut self,
        device: &Device,
        entity_mesh: &mut BaseMesh,
        gui_mesh: &mut GUIMesh,
        viewmodel_mesh: &mut BaseMesh,
        damage_mesh: &mut DamageMesh,
    ) {
        render::draw_model(
            ModelKey::id("viewmodel").unwrap(),
            Pos {
                x: 0.,
                y: 0.,
                z: 0.,
            },
            0.,
            viewmodel_mesh,
            Some("hit"),
            self.hit_timer.unwrap_or(0.) / self.active_tool().swing_time,
            |binding| match binding {
                "hand" => self.held_item.clone(),
                _ => None,
            },
        );

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
        for (id, entity) in &self.entities {
            if Some(*id) == self.player_entity {
                continue;
            }
            let lerp_time = (entity.update_timestamp.elapsed().as_secs_f32() / (1. / 40.)).min(1.);
            let position = entity.previous_position.lerp(entity.position, lerp_time);
            let rotation = block_byte_common::coord::lerp_number(
                entity.previous_direction.yaw,
                entity.direction.yaw,
                lerp_time,
            );
            render::draw_model(
                entity.key.data().model,
                position,
                rotation,
                entity_mesh,
                None,
                0.,
                |_| None,
            );
        }
        /*let crack_textures: Vec<_> = (0..=3)
        .map(|i| {
            TextureKey::id(&format!("block_damage.{i}"))
                .unwrap()
                .tex_coords()
        })
        .collect();*/
        let player_chunk = self.player_position.to_chunk_pos();
        let view_distance = 2;
        for chunk_position in AABB::new(
            Vec3 {
                x: player_chunk.x - view_distance,
                y: player_chunk.y - view_distance,
                z: player_chunk.z - view_distance,
            },
            Vec3 {
                x: player_chunk.x + view_distance,
                y: player_chunk.y + view_distance,
                z: player_chunk.z + view_distance,
            },
        ) {
            if let Some(chunk) = self.chunks.get(&chunk_position) {
                for (offset, damage) in &chunk.components.damage.components {
                    let base_position = (chunk_position.to_block_pos() + offset.xyz()).to_pos();
                    if base_position.distance_squared(self.player_position)
                        > (view_distance * view_distance) as f32
                            * CHUNK_SIZE as f32
                            * CHUNK_SIZE as f32
                    {
                        continue;
                    }
                    let block = chunk.blocks.get(offset.index()).unwrap().data();
                    let progress = (damage.damage
                        / block
                            .health
                            .as_ref()
                            .map(|health| health.health)
                            .unwrap_or(1.));
                    /*let crack_texture = *crack_textures
                    .get(
                        ((progress * crack_textures.len() as f32) as usize)
                            .min(crack_textures.len()),
                    )
                    .unwrap();*/

                    for face in Face::all() {
                        face.add_vertices(
                            TexCoords {
                                u1: 0.,
                                v1: 0.,
                                u2: 32.,
                                v2: 32.,
                            },
                            |position, coords| {
                                let border = 0.;
                                let vt_pos = base_position + position * (1. + border * 2.)
                                    - Pos {
                                        x: border,
                                        y: border,
                                        z: border,
                                    };
                                damage_mesh.vertices.push(DamageVertex {
                                    position: [vt_pos.x, vt_pos.y, vt_pos.z],
                                    tex_coords: [coords.0, coords.1],
                                    progress,
                                });
                            },
                        );
                    }
                }
                for (offset, plants) in &chunk.components.plant.components {
                    let base_position = (chunk_position.to_block_pos() + offset.xyz()).to_pos();
                    if base_position.distance_squared(self.player_position)
                        > (view_distance * view_distance) as f32
                            * CHUNK_SIZE as f32
                            * CHUNK_SIZE as f32
                    {
                        continue;
                    }
                    for plant in &plants.plants {
                        let plant = plant.data();
                        let position = base_position
                            + Pos {
                                x: 0.5,
                                y: 1.,
                                z: 0.5,
                            };
                        for blade in 0..plant.blades * 2 {
                            let first_angle =
                                f32::consts::PI * (blade as f32 / plant.blades as f32 + 0.25);
                            let second_angle = f32::consts::PI + first_angle;
                            let first = Pos {
                                x: first_angle.cos(),
                                y: 0.,
                                z: first_angle.sin(),
                            } * (plant.size / 2.)
                                + position;
                            let second = Pos {
                                x: second_angle.cos(),
                                y: 0.,
                                z: second_angle.sin(),
                            } * (plant.size / 2.)
                                + position;
                            let texture = plant.texture.tex_coords();
                            let vertices = [
                                Vertex {
                                    position: [first.x, first.y, first.z],
                                    tex_coords: [texture.u1, texture.v2],
                                    normals: [0., 1., 0.],
                                    color: Color::WHITE.into(),
                                },
                                Vertex {
                                    position: [second.x, second.y, second.z],
                                    tex_coords: [texture.u2, texture.v2],
                                    normals: [0., 1., 0.],
                                    color: Color::WHITE.into(),
                                },
                                Vertex {
                                    position: [first.x, first.y + plant.height, first.z],
                                    tex_coords: [texture.u1, texture.v1],
                                    normals: [0., 1., 0.],
                                    color: Color::WHITE.into(),
                                },
                                Vertex {
                                    position: [second.x, second.y + plant.height, second.z],
                                    tex_coords: [texture.u2, texture.v1],
                                    normals: [0., 1., 0.],
                                    color: Color::WHITE.into(),
                                },
                            ];
                            entity_mesh.vertices.push(vertices[0]);
                            entity_mesh.vertices.push(vertices[3]);
                            entity_mesh.vertices.push(vertices[2]);
                            entity_mesh.vertices.push(vertices[2]);
                            entity_mesh.vertices.push(vertices[1]);
                            entity_mesh.vertices.push(vertices[0]);
                        }
                    }
                }
            }
        }
    }
    pub fn tick_server(&mut self, dt: f32) {
        for (_, chunk) in &mut self.chunks {
            chunk
                .components
                .damage
                .components
                .retain_mut(|(offset, health)| {
                    let data = chunk.blocks.get(offset.index()).unwrap().data();
                    if let Some(health_data) = &data.health {
                        health.damage -= dt
                            * chunk
                                .blocks
                                .get(offset.index())
                                .unwrap()
                                .data()
                                .health
                                .as_ref()
                                .map(|health| health.health_regen)
                                .unwrap_or(1.);
                        health.damage > 0.
                    } else {
                        false
                    }
                });
        }
    }
    pub fn get_player_data(&self) -> Option<&'static EntityData> {
        Some(self.entities.get(&self.player_entity?)?.key.data())
    }
}
pub struct ClientEntity {
    key: EntityKey,
    position: Pos,
    direction: LookDirection,
    previous_position: Pos,
    previous_direction: LookDirection,
    update_timestamp: Instant,
}
pub struct ClientChunk {
    pub position: ChunkPos,
    pub blocks: BlockPalette,
    pub components: ClientChunkBlockComponents,
    pub buffer: Option<(Buffer, u32)>,
}
pub struct Mesh<T> {
    pub vertices: Vec<T>,
}
impl<T> Default for Mesh<T> {
    fn default() -> Self {
        Mesh {
            vertices: Vec::new(),
        }
    }
}
pub type BaseMesh = Mesh<Vertex>;
pub type GUIMesh = Mesh<GUIVertex>;
pub type DamageMesh = Mesh<DamageVertex>;

impl GUIMesh {
    pub fn add_quad(&mut self, position: Pos, size: Pos, texture: TexCoords, color: Color) {
        let color = [color.r, color.g, color.b, color.a];
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
        self.vertices.push(a);
        self.vertices.push(b);
        self.vertices.push(d);
        self.vertices.push(d);
        self.vertices.push(c);
        self.vertices.push(a);
    }
}
impl ClientChunk {
    pub fn rebuild_chunk_mesh(&self, neighbor_chunks: FaceMap<Option<&ClientChunk>>) -> BaseMesh {
        let mut mesh: BaseMesh = Mesh {
            vertices: Vec::new(),
        };

        if self.blocks.unique_values() == 1 && *self.blocks.get(0).unwrap() == air_block() {
            return mesh;
        }

        for x in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    let block = *self.blocks.get(ChunkOffset::new(x, y, z).index()).unwrap();
                    let block_data = block.data();
                    match &block_data.render_data {
                        BlockRenderData::Air => {}
                        BlockRenderData::Full { faces } => {
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
                                        BlockRenderData::Air | BlockRenderData::Model(_) => {}
                                        BlockRenderData::Full { faces } => {
                                            continue;
                                        }
                                    }
                                }

                                let texture = faces.by_face(*face).tex_coords();
                                face.add_vertices(texture, |position, coords| {
                                    let vt_pos = base_position + position;
                                    let normal = face.get_offset();
                                    mesh.vertices.push(Vertex {
                                        position: [vt_pos.x, vt_pos.y, vt_pos.z],
                                        tex_coords: [coords.0, coords.1],
                                        normals: [normal.x, normal.y, normal.z],
                                        color: Color::WHITE.into(),
                                    });
                                });
                            }
                        }
                        BlockRenderData::Model(model) => {
                            let position = Pos {
                                x: (self.position.x as f32 * CHUNK_SIZE as f32) + x as f32 + 0.5,
                                y: (self.position.y as f32 * CHUNK_SIZE as f32) + y as f32,
                                z: (self.position.z as f32 * CHUNK_SIZE as f32) + z as f32 + 0.5,
                            };
                            render::draw_model(*model, position, 0., &mut mesh, None, 0., |_| None);
                        }
                    }
                }
            }
        }

        mesh
    }
}
