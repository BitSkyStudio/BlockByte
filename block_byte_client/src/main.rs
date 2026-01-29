mod render;
mod ui;

use core::f32;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::Hash,
    net::{SocketAddr, UdpSocket},
    ops::ControlFlow,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};

use base64::{Engine, prelude::BASE64_STANDARD};
use block_byte_common::{
    ClientItem, Color, ItemMoveMode, LookDirection, MoveMode, PlayerAbilities, TexCoords,
    coord::{
        AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, Orientation, Pos, Ray,
        Vec3,
    },
    net::{NetworkMessageC2S, NetworkMessageS2C, make_connection_config},
    registry::{
        self, BlockPalette, BlockRenderData, BlockRotation, EntityData, EntityKey, ItemAction,
        ItemKey, Key, ModelData, ModelKey, Registry, TextureData, TextureKey, ToolData,
        TranslationLanguage, air_block, load_registries,
    },
    ui::PropertyMap,
    world::{self, ClientChunkBlockComponents},
};
use cgmath::{Matrix4, Rad, SquareMatrix, Transform, Vector3, Vector4};
use image::{DynamicImage, RgbaImage};
use parking_lot::Mutex;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use renet::{ConnectionConfig, DefaultChannel, RenetClient};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};
use uuid::Uuid;
use wgpu::{Buffer, Device, util::DeviceExt};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalPosition,
    event::{DeviceEvent, ElementState, Event, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::KeyCode,
    window::{Fullscreen, Window, WindowAttributes, WindowId},
};

use crate::{
    render::{DamageVertex, GUIVertex, RenderState, Vertex},
    ui::{ScreenData, TEXT_RENDERER, TextRenderer, render_screen, text_renderer},
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
            game: ClientGame::default(),
            network_client: client,
            network_transport: transport,
            teleport_id: 0,
            last_update: Instant::now(),
            player_abilities: PlayerAbilities {
                move_mode: MoveMode::Normal,
                speed: 1.,
            },
            mspt: 0.,
        })
        .unwrap();
}

struct App {
    texture_image: Option<RgbaImage>,
    render_state: Option<RenderState>,
    game: ClientGame,
    camera: ClientPlayer,
    network_client: RenetClient,
    network_transport: NetcodeClientTransport,
    player_abilities: PlayerAbilities,
    last_update: Instant,
    teleport_id: u32,
    mspt: f32,
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
        window.set_cursor_grab(winit::window::CursorGrabMode::Confined);
        window.set_cursor_visible(false);
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
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
                if self.game.screen.is_none() {
                    let sensitivity = 0.4;
                    self.camera.update_orientation(
                        -delta.1 as f32 * sensitivity,
                        delta.0 as f32 * sensitivity,
                    );
                }
            }
            DeviceEvent::Key(event) => match event.physical_key {
                winit::keyboard::PhysicalKey::Code(key_code) => {
                    if event.state == ElementState::Pressed {
                        self.game.keys.press(key_code);
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
                                if self.game.keys.is_down(KeyCode::KeyG) {
                                    if let Some(held_item) = self.game.held_item() {
                                        let variation_count = match &held_item.item.data().action {
                                            ItemAction::Ignore => 1,
                                            ItemAction::Place(item_block_placements) => {
                                                item_block_placements.len()
                                            }
                                        };
                                        if slot < variation_count {
                                            let variation = self
                                                .game
                                                .item_variation
                                                .entry(held_item.item)
                                                .or_insert(0);
                                            *variation = slot;
                                        }
                                    }
                                } else {
                                    self.game.hotbar_slot = slot;
                                    self.send_message(NetworkMessageC2S::HotbarSelect { slot });
                                }
                            }
                        }
                        if key_code == KeyCode::KeyE && self.game.screen.is_none() {
                            match self.camera.raycast(&self.game) {
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
                        if key_code == KeyCode::KeyQ && self.game.screen.is_none() {
                            self.send_message(NetworkMessageC2S::DropItem {
                                stack: self.game.keys.is_down(KeyCode::ControlLeft),
                            });
                        }
                    } else {
                        self.game.keys.release(key_code);
                    }
                }
                winit::keyboard::PhysicalKey::Unidentified(native_key_code) => {}
            },
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
            WindowEvent::CursorMoved {
                device_id,
                position,
            } => {
                self.game.cursor_position = position;
            }
            WindowEvent::Resized(new_size) => {
                self.render_state.as_mut().unwrap().resize(new_size);
            }
            WindowEvent::MouseWheel {
                device_id,
                delta,
                phase,
            } => match delta {
                winit::event::MouseScrollDelta::LineDelta(_, scroll) => {
                    if self.game.screen.is_none() {
                        let mut new_slot = self.game.hotbar_slot as isize;
                        new_slot += -scroll as isize;
                        new_slot = ((new_slot % 10) + 10) % 10;
                        self.game.hotbar_slot = new_slot as usize;
                        self.send_message(NetworkMessageC2S::HotbarSelect {
                            slot: new_slot as usize,
                        });
                    }
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
                        self.game.buttons.press(button);
                    }
                    ElementState::Released => {
                        self.game.buttons.release(button);
                    }
                }
                if state == ElementState::Pressed && self.game.screen.is_none() {
                    match (self.camera.raycast(&self.game), button) {
                        (RayCastResult::Block(position, face), MouseButton::Right) => {
                            self.game.build_animation = 0.5;
                            self.send_message(NetworkMessageC2S::PlaceBlock {
                                position,
                                face,
                                variant: self
                                    .game
                                    .held_item()
                                    .as_ref()
                                    .and_then(|item| {
                                        self.game.item_variation.get(&item.item).cloned()
                                    })
                                    .unwrap_or(0),
                            });
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

                let render_state = self.render_state.as_mut().unwrap();
                let mut entity_mesh = Mesh::default();
                let mut gui_mesh = Mesh::default();
                let mut viewmodel_mesh = Mesh::default();
                let mut damage_mesh = Mesh::default();
                self.game.tick_client(
                    render_state.device(),
                    &self.camera,
                    &mut entity_mesh,
                    &mut gui_mesh,
                    &mut viewmodel_mesh,
                    &mut damage_mesh,
                );
                self.game.build_animation = (self.game.build_animation - dt).max(0.);
                {
                    let viewmodel_light = Matrix4::from_angle_x(Rad(self.camera.direction.pitch))
                        * Matrix4::from_angle_y(Rad(self.camera.direction.yaw));
                    for vertex in &mut viewmodel_mesh.vertices {
                        let normal = cgmath::Vector3::new(
                            vertex.normals[0],
                            vertex.normals[1],
                            vertex.normals[2],
                        );
                        let new_normal = viewmodel_light.transform_vector(normal);
                        vertex.normals = [new_normal.x, new_normal.y, new_normal.z];
                    }
                }
                let aspect_ratio =
                    render_state.size().width as f32 / render_state.size().height as f32;
                text_renderer().draw(
                    Vec3 {
                        x: -aspect_ratio + 0.1,
                        y: 0.9,
                        z: 0.,
                    },
                    &format!(
                        "{:.2} {:.2} {:.2} fps: {:.0}, mspt: {:.2}",
                        self.game.player_position.x,
                        self.game.player_position.y,
                        self.game.player_position.z,
                        1. / dt,
                        self.mspt,
                    ),
                    0.05,
                    Color::WHITE,
                    &mut gui_mesh,
                );
                self.game
                    .hud
                    .properties
                    .0
                    .insert("hotbar_slot".to_string(), self.game.hotbar_slot as f32);
                render_screen(
                    &self.game.hud,
                    render_state.size(),
                    &self.game,
                    &mut gui_mesh,
                    false,
                );

                if let Some(screen) = &self.game.screen {
                    if let Some(target_slot) =
                        render_screen(screen, render_state.size(), &self.game, &mut gui_mesh, true)
                    {
                        match self.game.selected_slot {
                            Some((slot, button)) => match button {
                                MouseButton::Left => {
                                    if self.game.buttons.is_just_up(MouseButton::Left) {
                                        self.send_message(NetworkMessageC2S::MoveItem {
                                            from: slot,
                                            to: target_slot,
                                            mode: ItemMoveMode::Stack,
                                        });
                                    }
                                    if self.game.buttons.is_just_down(MouseButton::Right) {
                                        self.send_message(NetworkMessageC2S::MoveItem {
                                            from: slot,
                                            to: target_slot,
                                            mode: ItemMoveMode::Single,
                                        });
                                    }
                                }
                                MouseButton::Right => {
                                    if self.game.buttons.is_just_up(MouseButton::Right) {
                                        self.send_message(NetworkMessageC2S::MoveItem {
                                            from: slot,
                                            to: target_slot,
                                            mode: ItemMoveMode::Half,
                                        });
                                    }
                                }
                                _ => unreachable!(),
                            },
                            None => {
                                for button in [MouseButton::Left, MouseButton::Right] {
                                    if self.game.buttons.is_just_down(button) {
                                        self.game.selected_slot = Some((target_slot, button));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some((_, button)) = self.game.selected_slot.as_ref() {
                    if !self.game.buttons.is_down(*button) {
                        self.game.selected_slot = None;
                    }
                }
                if let Some(held_item) = self.game.held_item() {
                    if self.game.keys.is_just_down(KeyCode::KeyG) {
                        let variation_count = match &held_item.item.data().action {
                            ItemAction::Ignore => 1,
                            ItemAction::Place(item_block_placements) => item_block_placements.len(),
                        };
                        let variation = self.game.item_variation.entry(held_item.item).or_insert(0);
                        *variation += 1;
                        *variation %= variation_count;
                    }
                }
                let render_state = self.render_state.as_mut().unwrap();

                if self.game.screen.is_none() {
                    let crosshair_size = Vec3 {
                        x: 0.02,
                        y: 0.02,
                        z: 0.,
                    };
                    let crosshair_texture = TextureKey::id("crosshair").unwrap().tex_coords();
                    gui_mesh.add_quad(
                        -crosshair_size / 2.,
                        crosshair_size,
                        crosshair_texture,
                        Color::WHITE,
                    );

                    if let Some(tooltip) = match self.camera.raycast(&self.game) {
                        RayCastResult::Empty => None,
                        RayCastResult::Block(pos, face) => {
                            let (chunk, offset) = pos.to_chunk_pos_offset();
                            self.game
                                .chunks
                                .get(&chunk)
                                .unwrap()
                                .blocks
                                .get(offset.index())
                                .unwrap()
                                .block
                                .data()
                                .interact_action
                                .tooltip()
                        }
                        RayCastResult::Entity(uuid) => self
                            .game
                            .entities
                            .get(&uuid)
                            .unwrap()
                            .key
                            .data()
                            .interact_action
                            .tooltip(),
                    } {
                        let text = format!("[E]{}", translate(tooltip));
                        let size = 0.05;
                        let Pos {
                            x: width,
                            y: height,
                            ..
                        } = text_renderer().get_size(&text, size);
                        text_renderer().draw(
                            Pos {
                                x: -width / 2.,
                                y: -height - size,
                                z: 0.,
                            },
                            &text,
                            size,
                            Color::WHITE,
                            &mut gui_mesh,
                        );
                    }
                }
                match render_state.render(
                    &self.camera,
                    &self.game,
                    aspect_ratio,
                    entity_mesh,
                    gui_mesh,
                    viewmodel_mesh,
                    damage_mesh,
                ) {
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
                    if self.game.buttons.is_down(MouseButton::Left)
                        && self.game.hit_timer.is_none()
                        && self.game.screen.is_none()
                    {
                        self.game.hit_timer = Some(0.);
                    }
                    self.camera.update_position(
                        dt,
                        &self.game,
                        &self.player_abilities,
                        self.game.get_player_data(),
                    );
                    self.game.player_position = self.camera.position;
                    self.send_message(NetworkMessageC2S::PlayerPosition {
                        position: self.camera.position,
                        teleport_id: self.teleport_id,
                        direction: self.camera.direction,
                    });
                    if let Some(hit_timer) = self.game.hit_timer {
                        let new_hit_timer = hit_timer + dt;
                        let active_tool = self.game.active_tool();
                        if hit_timer < active_tool.hit_time && new_hit_timer >= active_tool.hit_time
                        {
                            match self.camera.raycast(&self.game) {
                                RayCastResult::Block(position, _) => {
                                    self.send_message(NetworkMessageC2S::AttackBlock { position });
                                }
                                RayCastResult::Entity(entity) => {
                                    self.send_message(NetworkMessageC2S::AttackEntity { entity });
                                }
                                RayCastResult::Empty => {}
                            }
                        }
                        self.game.hit_timer = Some(new_hit_timer);
                        if new_hit_timer > active_tool.swing_time {
                            self.game.hit_timer = None;
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
                                self.game.chunks.insert(
                                    position,
                                    ClientChunk {
                                        blocks,
                                        buffer: None,
                                        position,
                                        components,
                                    },
                                );
                                self.game.modified_chunks.insert(position);
                                for face in Face::all() {
                                    self.game
                                        .modified_chunks
                                        .insert(position + face.get_chunk_offset());
                                }
                            }
                            NetworkMessageS2C::UnloadChunk { position } => {
                                self.game.chunks.remove(&position);
                            }
                            NetworkMessageS2C::SetBlock { position, block } => {
                                let (chunk, offset) = position.to_chunk_pos_offset();
                                {
                                    let chunk = self.game.chunks.get_mut(&chunk).unwrap();
                                    chunk.blocks.set(offset.index(), &block);
                                }
                                self.game.modified_chunks.insert(chunk);
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
                                        self.game
                                            .modified_chunks
                                            .insert(chunk + face.get_chunk_offset());
                                    }
                                }
                            }
                            NetworkMessageS2C::GameTick {
                                ticks_passed,
                                dt,
                                mspt,
                            } => {
                                self.game.tick_server(dt);
                                self.mspt = mspt;
                            }
                            NetworkMessageS2C::AddEntity {
                                uuid,
                                key,
                                position,
                                direction,
                                hand_item,
                            } => {
                                self.game.entities.insert(
                                    uuid,
                                    ClientEntity {
                                        key,
                                        position,
                                        direction,
                                        previous_position: position,
                                        previous_direction: direction,
                                        update_timestamp: Instant::now(),
                                        hand_item,
                                    },
                                );
                            }
                            NetworkMessageS2C::MoveEntity {
                                uuid,
                                position,
                                direction,
                            } => {
                                if let Some(entity) = self.game.entities.get_mut(&uuid) {
                                    entity.previous_position = entity.position;
                                    entity.previous_direction = entity.direction;
                                    entity.update_timestamp = Instant::now();
                                    entity.position = position;
                                    entity.direction = direction;
                                }
                            }
                            NetworkMessageS2C::RemoveEntity { uuid } => {
                                self.game.entities.remove(&uuid);
                            }
                            NetworkMessageS2C::UpdateBlockComponents {
                                chunk,
                                offset,
                                data,
                            } => {
                                if let Some(chunk) = self.game.chunks.get_mut(&chunk) {
                                    data.update(offset, &mut chunk.components);
                                }
                            }
                            NetworkMessageS2C::SetPlayerEntity { uuid } => {
                                self.game.player_entity = uuid;
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
                                self.game.screen = Some(ScreenData {
                                    screen,
                                    slots,
                                    properties: PropertyMap(HashMap::new()),
                                });
                                let render_state = self.render_state.as_ref().unwrap();
                                render_state.window().set_cursor_visible(true);
                                let size = render_state.size();
                                render_state
                                    .window()
                                    .set_cursor_position(PhysicalPosition::new(
                                        size.width / 2,
                                        size.height / 2,
                                    ));
                            }
                            NetworkMessageS2C::UISetSlot { slot, item } => {
                                if let Some(screen) = &mut self.game.screen {
                                    if slot < screen.slots.len() {
                                        screen.slots[slot] = item;
                                    }
                                }
                            }
                            NetworkMessageS2C::UIClose => {
                                self.game.screen = None;
                                self.render_state
                                    .as_ref()
                                    .unwrap()
                                    .window()
                                    .set_cursor_visible(false);
                            }
                            NetworkMessageS2C::HUDSlot { slot, item } => {
                                self.game.hud.slots[slot] = item;
                                /*match (&self.game.held_item, &held_item) {
                                    (None, None) => {}
                                    (Some(first), Some(second)) => {
                                        if first.item != second.item {
                                            self.game.hit_timer = None;
                                        }
                                    }
                                    _ => {
                                        self.game.hit_timer = None;
                                    }
                                }*/
                            }
                            NetworkMessageS2C::EntityHandItem { uuid, item } => {
                                if let Some(entity) = self.game.entities.get_mut(&uuid) {
                                    entity.hand_item = item;
                                }
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
                self.game.buttons.frame_clear();
                self.game.keys.frame_clear();
            }
            _ => (),
        }
    }
}
pub struct InputContainer<T> {
    pub down: HashSet<T>,
    pub just_down: HashSet<T>,
    pub just_up: HashSet<T>,
}
impl<T> Default for InputContainer<T> {
    fn default() -> Self {
        Self {
            down: HashSet::new(),
            just_down: HashSet::new(),
            just_up: HashSet::new(),
        }
    }
}
impl<T: Hash + Eq + Copy> InputContainer<T> {
    pub fn press(&mut self, input: T) {
        self.down.insert(input);
        self.just_down.insert(input);
    }
    pub fn release(&mut self, input: T) {
        self.down.remove(&input);
        self.just_up.insert(input);
    }
    pub fn frame_clear(&mut self) {
        self.just_down.clear();
        self.just_up.clear();
    }
    pub fn is_down(&self, input: T) -> bool {
        self.down.contains(&input)
    }
    pub fn is_just_down(&self, input: T) -> bool {
        self.just_down.contains(&input)
    }
    pub fn is_just_up(&self, input: T) -> bool {
        self.just_up.contains(&input)
    }
}

pub struct TextureAtlas {
    textures: Vec<TexCoords>,
    models: Vec<Vec<(TexCoords, f32, f32)>>,
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
                force_max_dimensions: true,
            });
        for (i, texture) in texture_registry.data_entries().enumerate() {
            packer
                .pack_ref(TextureAtlasEntry::Texture(i), &*texture.texture)
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
        let exporter = ImageExporter::export(&packer, None).unwrap();
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
                                (
                                    TexCoords {
                                        u1: frame.frame.x as f32 / packer.width() as f32,
                                        v1: frame.frame.y as f32 / packer.height() as f32,
                                        u2: (frame.frame.x + frame.frame.w) as f32
                                            / packer.width() as f32,
                                        v2: (frame.frame.y + frame.frame.h) as f32
                                            / packer.height() as f32,
                                    },
                                    frame.frame.w as f32,
                                    frame.frame.h as f32,
                                )
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
            position: Pos::ZERO,
            velocity: Pos::ZERO,
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
    pub fn raycast(&self, world: &ClientGame) -> RayCastResult {
        let ray = Ray {
            position: self.get_eye(world.get_player_data()),
            direction: self.direction.make_front() * 10.,
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
                .block
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
        delta_time: f32,
        game: &ClientGame,
        abilities: &PlayerAbilities,
        player_entity_data: Option<&EntityData>,
    ) {
        let mut forward =
            cgmath::Vector3::new(self.direction.yaw.sin(), 0., -self.direction.yaw.cos());
        use cgmath::InnerSpace;
        let cross_normalized = forward.cross(Self::UP).normalize();
        let move_vector = game.keys.down.iter().copied().fold(
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
                if game.keys.is_down(KeyCode::ShiftLeft) {
                    move_vector /= 2.;
                }
                if game.keys.is_down(KeyCode::Space) && self.on_ground {
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
                    game,
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
                    game,
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
                    game,
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
        world: &ClientGame,
        player_entity_data: Option<&EntityData>,
    ) -> bool {
        match player_entity_data {
            Some(player_entity_data) => {
                let collider = player_entity_data.hitbox().offset(position).to_block();
                for block in collider {
                    let (chunk, offset) = block.to_chunk_pos_offset();
                    match world.chunks.get(&chunk) {
                        Some(chunk) => {
                            if !chunk
                                .blocks
                                .get(offset.index())
                                .unwrap()
                                .block
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
        let front = self.direction.make_front();
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
    pub fn create_projection_matrix(aspect: f32, fov: f32) -> cgmath::Matrix4<f32> {
        cgmath::perspective(cgmath::Deg(fov), aspect, 0.05, 500.)
    }
}
pub struct ClientGame {
    pub player_position: Pos,
    pub chunks: HashMap<ChunkPos, ClientChunk>,
    pub modified_chunks: HashSet<ChunkPos>,
    pub entities: HashMap<Uuid, ClientEntity>,
    pub player_entity: Option<Uuid>,
    pub screen: Option<ScreenData>,
    pub hud: ScreenData,
    pub hit_timer: Option<f32>,
    pub keys: InputContainer<KeyCode>,
    pub buttons: InputContainer<MouseButton>,
    pub cursor_position: PhysicalPosition<f64>,
    pub selected_slot: Option<(usize, MouseButton)>,
    pub hotbar_slot: usize,
    pub item_variation: HashMap<ItemKey, usize>,
    pub build_animation: f32,
}
impl Default for ClientGame {
    fn default() -> Self {
        Self {
            player_position: Pos::ZERO,
            chunks: Default::default(),
            modified_chunks: Default::default(),
            entities: Default::default(),
            player_entity: None,
            screen: None,
            hud: ScreenData {
                screen: Key::id("hud").unwrap(),
                slots: vec![None; 10],
                properties: PropertyMap(HashMap::new()),
            },
            hit_timer: None,
            keys: InputContainer::default(),
            buttons: InputContainer::default(),
            cursor_position: PhysicalPosition::new(0., 0.),
            selected_slot: None,
            hotbar_slot: 0,
            item_variation: HashMap::new(),
            build_animation: 0.,
        }
    }
}
impl ClientGame {
    pub fn held_item(&self) -> &Option<ClientItem> {
        &self.hud.slots[self.hotbar_slot]
    }
    pub fn active_tool(&self) -> ToolData {
        self.held_item()
            .as_ref()
            .and_then(|item| item.item.data().tool)
            .unwrap_or(ToolData::hand())
    }
    pub fn tick_client(
        &mut self,
        device: &Device,
        camera: &ClientPlayer,
        entity_mesh: &mut BaseMesh,
        gui_mesh: &mut GUIMesh,
        viewmodel_mesh: &mut BaseMesh,
        damage_mesh: &mut DamageMesh,
    ) {
        let (viewmodel_animation, viewmodel_time) = if self.build_animation > 0. {
            (Some("place"), self.build_animation)
        } else if let Some(hit_timer) = self.hit_timer {
            (Some("hit"), hit_timer / self.active_tool().swing_time)
        } else {
            (None, 0.)
        };
        render::draw_model(
            ModelKey::id("viewmodel").unwrap(),
            Matrix4::identity(),
            &mut viewmodel_mesh.vertex_consumer(),
            viewmodel_animation,
            viewmodel_time,
            |binding| match binding {
                "hand" => self.held_item().clone(),
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
            let rotation = -block_byte_common::coord::lerp_number(
                entity.previous_direction.yaw,
                entity.direction.yaw,
                lerp_time,
            );
            render::draw_model(
                entity.key.data().model,
                Matrix4::from_translation(Vector3::new(position.x, position.y, position.z))
                    * Matrix4::from_angle_y(Rad(rotation)),
                &mut entity_mesh.vertex_consumer(),
                None,
                0.,
                |slot| entity.hand_item.clone(),
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
        let view_distance = 4;
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
                    let block = chunk.blocks.get(offset.index()).unwrap();
                    let block_data = block.block.data();
                    let progress = (damage.damage
                        / block_data
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
                    match &block_data.render_data {
                        BlockRenderData::Air => {}
                        BlockRenderData::Full { .. } => {
                            for face in Face::all() {
                                face.add_vertices(
                                    TexCoords {
                                        u1: 0.,
                                        v1: 0.,
                                        u2: 16.,
                                        v2: 16.,
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
                        BlockRenderData::Model(model_key) => {
                            let orientation: Orientation = block.rotation.into();
                            let right = orientation.right.get_offset();
                            let up = orientation.up.get_offset();
                            let front = orientation.forward.get_offset();
                            let model = &model_key.data().model;
                            let textures =
                                &TEXTURE_ATLAS.get().unwrap().models[model_key.numeric_id()];
                            model.draw(
                                Matrix4::from_translation(Vector3::new(
                                    base_position.x + 0.5,
                                    base_position.y + 0.5,
                                    base_position.z + 0.5,
                                )) * Matrix4::from_cols(
                                    Vector4::new(right.x, right.y, right.z, 0.),
                                    Vector4::new(up.x, up.y, up.z, 0.),
                                    Vector4::new(-front.x, -front.y, -front.z, 0.),
                                    Vector4::new(0., 0., 0., 1.),
                                ) * Matrix4::from_translation(Vector3::new(0., -0.5, 0.)),
                                None,
                                0.,
                                |position, normal, uv, texture| {
                                    let (_, width, height) = textures[texture];
                                    damage_mesh.vertices.push(DamageVertex {
                                        position: [position.x, position.y, position.z],
                                        tex_coords: [uv.0 * width, uv.1 * height],
                                        progress,
                                    });
                                },
                                |matrix, binding| {},
                            );
                        }
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
                    for (plant, stage) in &plants.plants {
                        let plant = plant.data();
                        let position = base_position
                            + Pos {
                                x: 0.5,
                                y: 1.,
                                z: 0.5,
                            };
                        let texture = plant.stages[*stage as usize].tex_coords();
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
                                    position: [second.x, second.y + plant.height, second.z],
                                    tex_coords: [texture.u2, texture.v1],
                                    normals: [0., 1., 0.],
                                    color: Color::WHITE.into(),
                                },
                                Vertex {
                                    position: [first.x, first.y + plant.height, first.z],
                                    tex_coords: [texture.u1, texture.v1],
                                    normals: [0., 1., 0.],
                                    color: Color::WHITE.into(),
                                },
                            ];
                            entity_mesh.vertices.push(vertices[0]);
                            entity_mesh.vertices.push(vertices[1]);
                            entity_mesh.vertices.push(vertices[2]);
                            entity_mesh.vertices.push(vertices[2]);
                            entity_mesh.vertices.push(vertices[3]);
                            entity_mesh.vertices.push(vertices[0]);
                        }
                    }
                }
            }
        }

        if let Some(held_item) = self.held_item() {
            match &held_item.item.data().action {
                ItemAction::Place(place_block) => match camera.raycast(self) {
                    RayCastResult::Block(position, face) => {
                        let place_block = &place_block[self
                            .item_variation
                            .get(&held_item.item)
                            .cloned()
                            .unwrap_or(0)];
                        let block_position = position + face.get_block_offset();
                        let mut blocked = place_block.use_count > held_item.count;
                        for entity in self.entities.values() {
                            if entity
                                .key
                                .data()
                                .hitbox()
                                .offset(entity.position)
                                .to_block()
                                .contains(block_position)
                            {
                                blocked = true;
                                break;
                            }
                        }
                        let (chunk, offset) = block_position.to_chunk_pos_offset();
                        if let Some(chunk) = self.chunks.get(&chunk) {
                            let block = chunk.blocks.get(offset.index()).unwrap().block;
                            if block == air_block() {
                                let rotation = place_block
                                    .block
                                    .data()
                                    .rotation
                                    .get_nearest_valid(BlockRotation::from(camera.direction));
                                let orientation: Orientation = rotation.into();
                                let right = orientation.right.get_offset();
                                let up = orientation.up.get_offset();
                                let front = orientation.forward.get_offset();
                                render::draw_block_model(
                                    place_block.block,
                                    Matrix4::from_translation(Vector3::new(
                                        block_position.x as f32 + 0.5,
                                        block_position.y as f32 + 0.5,
                                        block_position.z as f32 + 0.5,
                                    )) * Matrix4::from_cols(
                                        Vector4::new(right.x, right.y, right.z, 0.),
                                        Vector4::new(up.x, up.y, up.z, 0.),
                                        Vector4::new(-front.x, -front.y, -front.z, 0.),
                                        Vector4::new(0., 0., 0., 1.),
                                    ) * Matrix4::from_translation(Vector3::new(0., -0.5, 0.)),
                                    &mut |position, tex_coords, normals| {
                                        entity_mesh.vertices.push(Vertex {
                                            position,
                                            tex_coords,
                                            normals,
                                            color: if blocked {
                                                Color {
                                                    r: 255,
                                                    g: 100,
                                                    b: 100,
                                                    a: 100,
                                                }
                                            } else {
                                                Color {
                                                    r: 255,
                                                    g: 255,
                                                    b: 255,
                                                    a: 100,
                                                }
                                            }
                                            .into(),
                                        });
                                    },
                                );
                            }
                        }
                    }
                    RayCastResult::Empty | RayCastResult::Entity(_) => {}
                },
                ItemAction::Ignore => {}
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
                    let data = chunk.blocks.get(offset.index()).unwrap().block.data();
                    if let Some(health_data) = &data.health {
                        health.damage -= dt
                            * chunk
                                .blocks
                                .get(offset.index())
                                .unwrap()
                                .block
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
    hand_item: Option<ClientItem>,
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

impl BaseMesh {
    pub fn vertex_consumer(&mut self) -> impl FnMut([f32; 3], [f32; 2], [f32; 3]) {
        move |position, tex_coords, normals| {
            self.vertices.push(Vertex {
                position,
                tex_coords,
                normals,
                color: Color::WHITE.into(),
            });
        }
    }
}

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

        if self.blocks.unique_values() == 1 && self.blocks.get(0).unwrap().block == air_block() {
            return mesh;
        }

        for x in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    let block = *self.blocks.get(ChunkOffset::new(x, y, z).index()).unwrap();
                    let block_data = block.block.data();
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
                                        .block
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
                                y: (self.position.y as f32 * CHUNK_SIZE as f32) + y as f32 + 0.5,
                                z: (self.position.z as f32 * CHUNK_SIZE as f32) + z as f32 + 0.5,
                            };
                            let orientation: Orientation = block.rotation.into();
                            let right = orientation.right.get_offset();
                            let up = orientation.up.get_offset();
                            let front = orientation.forward.get_offset();
                            render::draw_model(
                                *model,
                                Matrix4::from_translation(Vector3::new(
                                    position.x, position.y, position.z,
                                )) * Matrix4::from_cols(
                                    Vector4::new(right.x, right.y, right.z, 0.),
                                    Vector4::new(up.x, up.y, up.z, 0.),
                                    Vector4::new(-front.x, -front.y, -front.z, 0.),
                                    Vector4::new(0., 0., 0., 1.),
                                ) * Matrix4::from_translation(Vector3::new(0., -0.5, 0.)),
                                &mut |position, tex_coords, normals| {
                                    mesh.vertices.push(Vertex {
                                        position,
                                        tex_coords,
                                        normals,
                                        color: Color::WHITE.into(),
                                    });
                                },
                                None,
                                0.,
                                |_| None,
                            );
                        }
                    }
                }
            }
        }

        mesh
    }
}

pub fn translate<'a>(key: &'a str) -> &'a str {
    Key::<TranslationLanguage>::id("en")
        .unwrap()
        .data()
        .translate(key)
}
