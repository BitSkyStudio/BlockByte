mod render;
mod ui;

use core::f32;
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{BinaryHeap, HashMap, HashSet},
    fmt::format,
    hash::Hash,
    net::{SocketAddr, UdpSocket},
    ops::ControlFlow,
    path::Path,
    sync::{Arc, OnceLock, atomic::AtomicU64},
    time::{Duration, Instant, SystemTime},
    u32,
};

use ahash::{AHashMap, AHashSet};
use base64::{Engine, prelude::BASE64_STANDARD};
use block_byte_common::{
    CharacterController, ClientItem, Color, ItemMoveMode, LookDirection, MoveMode, PlayerAbilities,
    TexCoords,
    coord::{
        AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, Orientation, Pos, Ray,
        Vec3,
    },
    model::{DrawAnimation, ModelGeometry, ModelTexture},
    net::{NetworkMessageC2S, NetworkMessageS2C, make_connection_config},
    registry::{
        self, BlockColor, BlockEntry, BlockInteractAction, BlockPalette, BlockRenderData,
        BlockRotation, EntityData, EntityInteractAction, EntityKey, ItemAction, ItemKey, ItemModel,
        Key, KeyGroup, ModelData, ModelInstance, ModelKey, Registry, ResearchKey, TextureData,
        TextureKey, ToolData, TranslationLanguageData, air_block, load_registries,
    },
    ui::PropertyMap,
    world::{self, ClientChunkBlockComponents},
};
use cgmath::{Matrix4, Rad, SquareMatrix, Transform, Vector3, Vector4};
use image::{DynamicImage, GenericImage, RgbaImage};
use parking_lot::{Mutex, RwLock};
use rand::{Rng, rngs::StdRng};
use rand_seeder::Seeder;
use rayon::{
    ThreadPoolBuilder,
    iter::{IntoParallelIterator, ParallelIterator},
};
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
    clipping::Frustum,
    render::{
        BaseMesh, CameraUniform, ChunkMesh, ChunkVertex, DamageMesh, DamageVertex, GPUMesh,
        GUIMesh, GUIVertex, Mesh, MeshVertex, MeshVertexConsumer, RenderState, Vertex,
    },
    ui::{
        HoveredElement, ScreenData, TEXT_RENDERER, TextRenderer, UIPos, UIRect, render_screen,
        text_renderer,
    },
};

static START_TIMER: OnceLock<Instant> = OnceLock::new();
fn secs_since_start() -> f32 {
    START_TIMER
        .get_or_init(|| Instant::now())
        .elapsed()
        .as_secs_f32()
}

fn main() {
    load_registries(&[&Path::new("assets"), &Path::new("assets_generated")]);
    use block_byte_common::registry::RegistryProvider;
    let (atlas, text_renderer, image) = TextureAtlas::pack();
    TEXTURE_ATLAS.set(atlas);
    TEXT_RENDERER.set(text_renderer);

    let connection = ClientConnection::connect("127.0.0.1:5000".parse().unwrap());

    let event_loop = EventLoop::new().unwrap();
    event_loop
        .run_app(&mut App {
            camera: ClientPlayer::default(),
            render_state: None,
            texture_image: Some(image),
            game: ClientGame::default(),
            connection,
            teleport_id: 0,
            last_update: Instant::now(),
            player_abilities: PlayerAbilities {
                move_mode: MoveMode::Normal,
                speed: 1.,
                max_stamina: 100.,
            },
            mspt: 0.,
        })
        .unwrap();
}

struct App {
    texture_image: Option<Vec<RgbaImage>>,
    render_state: Option<RenderState>,
    game: ClientGame,
    camera: ClientPlayer,
    connection: ClientConnection,
    player_abilities: PlayerAbilities,
    last_update: Instant,
    teleport_id: u32,
    mspt: f32,
}
impl App {
    pub fn send_message(&mut self, message: NetworkMessageC2S) {
        self.connection.tx.send(message);
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
                    let sensitivity = 0.6;
                    self.camera.update_orientation(
                        -delta.1 as f32 * sensitivity * 0.022,
                        delta.0 as f32 * sensitivity * 0.022,
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
                                        let variation_count =
                                            held_item.item.data().action.variation_count();
                                        if slot < variation_count {
                                            let variation = self
                                                .game
                                                .item_variation
                                                .entry(held_item.item)
                                                .or_insert(0);
                                            if *variation != slot {
                                                self.game.viewmodel_player.trigger("equip");
                                            }
                                            *variation = slot;
                                        }
                                    }
                                } else {
                                    if self.game.hotbar_slot != slot {
                                        self.game.hotbar_slot = slot;
                                        self.send_message(NetworkMessageC2S::HotbarSelect { slot });
                                        if self.game.held_item().is_some()
                                            || self.game.swap_hand_item.is_some()
                                        {
                                            self.game.viewmodel_player.trigger("equip");
                                        }
                                    }
                                }
                            }
                        }
                        if key_code == KeyCode::KeyE && self.game.screen.is_none() {
                            match self.camera.raycast(&self.game, false) {
                                RayCastResult::Empty => {}
                                RayCastResult::Block(position, _) => {
                                    let block = self.game.get_block(position).unwrap().block.data();
                                    match &block.interact_action {
                                        BlockInteractAction::Ignore => {}
                                        _ => {
                                            self.game.viewmodel_player.trigger("interact");
                                            self.send_message(NetworkMessageC2S::InteractBlock {
                                                position,
                                            });
                                        }
                                    }
                                }
                                RayCastResult::Entity(entity) => {
                                    let entity_data =
                                        self.game.entities.get(&entity).unwrap().key.data();
                                    match &entity_data.interact_action {
                                        EntityInteractAction::Ignore => {}
                                        _ => {
                                            self.game.viewmodel_player.trigger("interact");
                                            self.send_message(NetworkMessageC2S::InteractEntity {
                                                entity,
                                            });
                                        }
                                    }
                                }
                                RayCastResult::Plant(position, index) => {
                                    self.send_message(NetworkMessageC2S::HarvestPlant {
                                        position,
                                        index,
                                    });
                                }
                            }
                        }
                        if key_code == KeyCode::Escape {
                            if let Some(screen) = &mut self.game.screen
                                && screen.selected_slot.is_some()
                            {
                                screen.selected_slot = None;
                            } else {
                                self.send_message(NetworkMessageC2S::CloseUI);
                            }
                        }
                        if key_code == KeyCode::Tab {
                            self.send_message(if self.game.screen.is_some() {
                                NetworkMessageC2S::CloseUI
                            } else {
                                NetworkMessageC2S::OpenPlayerInventory
                            });
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
                *self.connection.state.lock() = ClientConnectionState::Disconnect;
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
                        if self.game.keys.is_down(KeyCode::KeyG) {
                            if let Some(held_item) = self.game.held_item() {
                                let variation_count =
                                    held_item.item.data().action.variation_count() as isize;
                                if variation_count > 1 {
                                    let mut new_variant = *self
                                        .game
                                        .item_variation
                                        .get(&held_item.item)
                                        .unwrap_or(&0)
                                        as isize;
                                    new_variant -= scroll as isize;
                                    new_variant = ((new_variant % variation_count)
                                        + variation_count)
                                        % variation_count;
                                    self.game
                                        .item_variation
                                        .insert(held_item.item, new_variant as usize);
                                    self.game.viewmodel_player.trigger("equip");
                                }
                            }
                        } else {
                            let mut new_slot = self.game.hotbar_slot as isize;
                            new_slot += -scroll as isize;
                            new_slot = ((new_slot % 10) + 10) % 10;
                            self.game.hotbar_slot = new_slot as usize;
                            self.send_message(NetworkMessageC2S::HotbarSelect {
                                slot: new_slot as usize,
                            });
                            if self.game.held_item().is_some() || self.game.swap_hand_item.is_some()
                            {
                                self.game.viewmodel_player.trigger("equip");
                            }
                        }
                    }
                }
                winit::event::MouseScrollDelta::PixelDelta(physical_position) => {}
            },
            WindowEvent::MouseInput {
                device_id,
                state,
                button,
            } => match state {
                ElementState::Pressed => {
                    self.game.buttons.press(button);
                }
                ElementState::Released => {
                    self.game.buttons.release(button);
                }
            },
            WindowEvent::RedrawRequested => {
                let dt = self.last_update.elapsed().as_secs_f32();
                self.last_update = Instant::now();
                //println!("{}", 1. / dt);
                // Ref the application.
                //
                // It's preferable for applications that do not render continuously to render in
                // this event rather than in AboutToWait, since rendering in here allows
                // the program to gracefully handle redraws requested by the OS.

                self.game
                    .hud
                    .properties
                    .0
                    .insert("stamina".to_string(), self.game.stamina);
                self.game.stamina = self.game.stamina.min(self.player_abilities.max_stamina);

                self.render_state
                    .as_ref()
                    .unwrap()
                    .window()
                    .pre_present_notify();

                let render_state = self.render_state.as_mut().unwrap();
                let frustum = crate::clipping::Frustum::from_matrix(
                    CameraUniform::OPENGL_TO_WGPU_MATRIX
                        * ClientPlayer::create_projection_matrix(
                            render_state.size().width as f32 / render_state.size().height as f32,
                            90.,
                        )
                        * self.camera.create_view_matrix(self.game.get_player_data()),
                );
                let mut entity_mesh = Mesh::default();
                let mut gui_mesh = Mesh::default();
                let mut viewmodel_mesh = Mesh::default();
                let mut damage_mesh = Mesh::default();
                self.game.tick_client(
                    &render_state.device,
                    &self.camera,
                    &mut entity_mesh,
                    &mut gui_mesh,
                    &mut viewmodel_mesh,
                    &mut damage_mesh,
                    &frustum,
                    dt,
                    &self.connection,
                );
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
                    UIPos {
                        x: -aspect_ratio + 0.1,
                        y: 0.9,
                    },
                    &format!(
                        "{:.2} {:.2} {:.2} fps: {:.0}, mspt: {:.2} {}",
                        self.game.player_position.x,
                        self.game.player_position.y,
                        self.game.player_position.z,
                        1. / dt,
                        self.mspt,
                        match self.camera.raycast(&self.game, true) {
                            RayCastResult::Block(position, face) =>
                                format!("looking at {:?} {:?} ", position, face),
                            _ => String::new(),
                        },
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
                    self.game.cursor_position,
                    &mut gui_mesh,
                    false,
                );

                if let Some(screen) = &mut self.game.screen {
                    if let Some(hovered) = render_screen(
                        screen,
                        render_state.size(),
                        self.game.cursor_position,
                        &mut gui_mesh,
                        true,
                    ) {
                        match hovered {
                            HoveredElement::Slot(target_slot) => match screen.selected_slot {
                                Some((slot, button)) => match button {
                                    MouseButton::Left => {
                                        if self.game.buttons.is_just_up(MouseButton::Left) {
                                            self.connection.tx.send(NetworkMessageC2S::MoveItem {
                                                from: slot,
                                                to: target_slot,
                                                mode: ItemMoveMode::Stack,
                                            });
                                        }
                                        if self.game.buttons.is_just_down(MouseButton::Right) {
                                            self.connection.tx.send(NetworkMessageC2S::MoveItem {
                                                from: slot,
                                                to: target_slot,
                                                mode: ItemMoveMode::Single,
                                            });
                                        }
                                    }
                                    MouseButton::Right => {
                                        if self.game.buttons.is_just_up(MouseButton::Right) {
                                            self.connection.tx.send(NetworkMessageC2S::MoveItem {
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
                                            screen.selected_slot = Some((target_slot, button));
                                            break;
                                        }
                                    }
                                }
                            },
                            HoveredElement::Craft(key) => {
                                for (button, count) in
                                    [(MouseButton::Left, 1), (MouseButton::Right, 5)]
                                {
                                    if self.game.buttons.is_just_down(button) {
                                        self.connection
                                            .tx
                                            .send(NetworkMessageC2S::Craft { recipe: key, count });
                                    }
                                }
                            }
                            HoveredElement::Research(key) => {
                                if self.game.buttons.is_just_down(MouseButton::Left) {
                                    self.connection
                                        .tx
                                        .send(NetworkMessageC2S::Research { research: key });
                                }
                            }
                        }
                    }
                    if let Some((_, button)) = screen.selected_slot.as_ref() {
                        if !self.game.buttons.is_down(*button) {
                            screen.selected_slot = None;
                        }
                    }
                }

                if self.game.buttons.is_just_down(MouseButton::Right) && self.game.screen.is_none()
                {
                    match self.camera.raycast(&self.game, true) {
                        RayCastResult::Block(position, face) => {
                            if let Some(hand_item) =
                                self.game.held_item().as_ref().map(|item| item.item)
                            {
                                match &hand_item.data().action {
                                    ItemAction::Ignore => {}
                                    _ => {
                                        self.game.viewmodel_player.trigger("build");
                                        self.send_message(NetworkMessageC2S::PlaceBlock {
                                            position,
                                            face,
                                            variant: self
                                                .game
                                                .item_variation
                                                .get(&hand_item)
                                                .cloned()
                                                .unwrap_or(0),
                                        });
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }

                let render_state = self.render_state.as_mut().unwrap();

                if self.game.screen.is_none() {
                    let crosshair_texture = TextureKey::id("crosshair").unwrap();
                    let crosshair_data = &*crosshair_texture.data().texture;
                    let crosshair_size = 2.;
                    let crosshair_size = UIPos {
                        x: crosshair_data.width() as f32 / render_state.size().width as f32
                            * crosshair_size
                            * aspect_ratio,
                        y: crosshair_data.height() as f32 / render_state.size().height as f32
                            * crosshair_size,
                    };
                    gui_mesh.add_quad(
                        UIRect {
                            pos: UIPos {
                                x: crosshair_size.x * -0.5,
                                y: crosshair_size.y * -0.5,
                            },
                            size: crosshair_size,
                        },
                        crosshair_texture.tex_coords(),
                        Color::WHITE,
                    );

                    if let Some(tooltip) = match self.camera.raycast(&self.game, false) {
                        RayCastResult::Empty => None,
                        RayCastResult::Block(pos, face) => self
                            .game
                            .get_block(pos)
                            .unwrap()
                            .block
                            .data()
                            .interact_action
                            .tooltip(),
                        RayCastResult::Entity(uuid) => self
                            .game
                            .entities
                            .get(&uuid)
                            .unwrap()
                            .key
                            .data()
                            .interact_action
                            .tooltip(),
                        RayCastResult::Plant(_, _) => Some("harvest"),
                    } {
                        let text = format!("[E]{}", translate(tooltip));
                        let size = 0.05;
                        let Pos {
                            x: width,
                            y: height,
                            ..
                        } = text_renderer().get_size(&text, size);
                        text_renderer().draw(
                            UIPos {
                                x: -width / 2.,
                                y: -height - size,
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
                    &frustum,
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
                if self.connection.state() == ClientConnectionState::Connected {
                    if self.game.buttons.is_down(MouseButton::Left)
                        && self.game.hit_timer.is_none()
                        && self.game.screen.is_none()
                    {
                        let stamina_cost = self.game.active_tool().stamina;
                        if self.game.stamina >= stamina_cost {
                            self.game.stamina -= stamina_cost;
                            self.game.hit_timer = Some(0.);
                        }
                    }
                    self.camera
                        .update_position(dt, &mut self.game, &self.player_abilities);
                    self.game.player_position = self.camera.position;
                    self.send_message(NetworkMessageC2S::PlayerPosition {
                        position: self.camera.position,
                        teleport_id: self.teleport_id,
                        direction: self.camera.direction,
                    });
                    if let Some(hit_timer) = self.game.hit_timer {
                        let new_hit_timer = hit_timer + dt;
                        let active_tool = self.game.active_tool();
                        if hit_timer < active_tool.swing_time * 0.5
                            && new_hit_timer >= active_tool.swing_time * 0.5
                        {
                            match self.camera.raycast(&self.game, true) {
                                RayCastResult::Block(position, face) => {
                                    self.send_message(NetworkMessageC2S::AttackBlock {
                                        position,
                                        face,
                                    });
                                }
                                RayCastResult::Entity(entity) => {
                                    self.send_message(NetworkMessageC2S::AttackEntity { entity });
                                }
                                RayCastResult::Empty => {}
                                RayCastResult::Plant(position, index) => {
                                    /*self.send_message(NetworkMessageC2S::HarvestPlant {
                                        position,
                                        index,
                                        cut: true,
                                    });*/
                                }
                            }
                        }
                        self.game.hit_timer = Some(new_hit_timer);
                        if new_hit_timer > active_tool.swing_time {
                            self.game.hit_timer = None;
                        }
                    }
                    while let Ok((mut message, time)) = self.connection.rx.try_recv() {
                        match message {
                            NetworkMessageS2C::LoadChunk {
                                position,
                                blocks,
                                components,
                            } => {
                                self.game.chunks.insert(
                                    position,
                                    ClientChunk {
                                        mesh_build_data: Arc::new(ChunkMeshBuildData {
                                            blocks: RwLock::new(blocks),
                                            version: AtomicU64::new(0),
                                        }),
                                        buffer: None,
                                        position,
                                        components,
                                        modified: false,
                                        scheduled: false,
                                    },
                                );
                                self.game.mark_modified(position);
                                for face in Face::all() {
                                    self.game.mark_modified(position + face.get_chunk_offset());
                                }
                            }
                            NetworkMessageS2C::UnloadChunk { position } => {
                                self.game.chunks.remove(&position);
                            }
                            NetworkMessageS2C::SetBlock { position, block } => {
                                let (chunk, offset) = position.to_chunk_pos_offset();
                                {
                                    let chunk = self.game.chunks.get(&chunk).unwrap();
                                    chunk
                                        .mesh_build_data
                                        .blocks
                                        .write()
                                        .set(offset.index(), &block);
                                }
                                self.game.mark_modified(chunk);
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
                                        self.game.mark_modified(chunk + face.get_chunk_offset());
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
                                    entity.update_timestamp = time;
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
                                    selected_slot: None,
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
                            NetworkMessageS2C::Knockback { velocity } => {
                                self.camera.controller.velocity += velocity;
                            }
                            NetworkMessageS2C::UpdateResearch { research } => {
                                self.game.researched = research;
                            }
                            NetworkMessageS2C::HudBarUpdate { health } => {
                                self.game
                                    .hud
                                    .properties
                                    .0
                                    .insert("health".to_string(), health);
                            }
                        }
                    }
                } else if self.connection.state() == ClientConnectionState::Disconnected {
                    println!("disconnected");
                    event_loop.exit();
                    return;
                }

                self.game.buttons.frame_clear(dt);
                self.game.keys.frame_clear(dt);
            }
            _ => (),
        }
    }
}
pub struct InputContainer<T> {
    pub down: HashSet<T>,
    pub just_down: HashSet<T>,
    pub just_up: HashSet<T>,
    pub held_for: HashMap<T, f32>,
}
impl<T> Default for InputContainer<T> {
    fn default() -> Self {
        Self {
            down: HashSet::new(),
            just_down: HashSet::new(),
            just_up: HashSet::new(),
            held_for: HashMap::new(),
        }
    }
}
impl<T: Hash + Eq + Copy> InputContainer<T> {
    pub fn press(&mut self, input: T) {
        self.down.insert(input);
        self.just_down.insert(input);
        self.held_for.insert(input, 0.);
    }
    pub fn release(&mut self, input: T) {
        self.down.remove(&input);
        self.just_up.insert(input);
        self.held_for.remove(&input);
    }
    pub fn frame_clear(&mut self, dt: f32) {
        self.just_down.clear();
        self.just_up.clear();
        for time in self.held_for.values_mut() {
            *time += dt;
        }
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
    pub fn held_time(&self, input: T) -> f32 {
        self.held_for.get(&input).cloned().unwrap_or(0.)
    }
}

pub struct TextureAtlas {
    textures: Vec<Option<TexCoords>>,
    models: Vec<Vec<TexCoords>>,
}

impl TextureAtlas {
    pub fn pack() -> (Self, TextRenderer, Vec<RgbaImage>) {
        #[derive(Hash, PartialEq, Eq, Clone, Copy)]
        enum TextureAtlasKey {
            Texture(usize),
            Model(usize, usize),
            Glyph(usize),
        }
        let texture_dimensions = 2048;
        let mut packer =
            texture_packer::TexturePacker::new_skyline(texture_packer::TexturePackerConfig {
                max_width: texture_dimensions,
                max_height: texture_dimensions,
                allow_rotation: false,
                texture_outlines: false,
                border_padding: 0,
                texture_padding: 0,
                trim: false,
                texture_extrusion: 0,
                force_max_dimensions: true,
            });
        fn get_nearest_texture_multiple(size: u32) -> u32 {
            let mut current = 16;
            loop {
                if size <= current {
                    return current;
                }
                current <<= 1;
                if current == 0 {
                    panic!()
                }
            }
        }
        struct TextureAtlasEntry {
            width_multiple: f32,
            height_multiple: f32,
        }
        let mut textures = HashMap::new();
        let mut add_texture = |key: TextureAtlasKey, image: &DynamicImage| {
            let new_width = get_nearest_texture_multiple(image.width());
            let new_height = get_nearest_texture_multiple(image.height());
            let mut new_image = DynamicImage::new_rgba8(new_width, new_height);
            new_image.copy_from(image, 0, 0).unwrap();
            textures.insert(
                key,
                TextureAtlasEntry {
                    width_multiple: image.width() as f32 / new_width as f32,
                    height_multiple: image.height() as f32 / new_height as f32,
                },
            );
            packer.pack_own(key, new_image);
        };
        let skip_list = KeyGroup::<TextureData>::parse("#skip");
        for (i, texture) in TextureKey::entries().enumerate() {
            if let Some(skip_list) = &skip_list {
                if skip_list.contains(texture) {
                    continue;
                }
            }
            let texture = texture.data();
            add_texture(TextureAtlasKey::Texture(i), &*texture.texture);
        }
        for (i, model) in ModelKey::entries().enumerate() {
            let model = model.data();
            for (j, (texture, _, _)) in model.model.textures.iter().enumerate() {
                match texture {
                    ModelTexture::Embed(texture, _) => {
                        let image = image::load_from_memory_with_format(
                            BASE64_STANDARD
                                .decode(&texture["data:image/png;base64,".len()..])
                                .unwrap()
                                .as_slice(),
                            image::ImageFormat::Png,
                        )
                        .unwrap();

                        add_texture(TextureAtlasKey::Model(i, j), &image);
                    }
                    _ => {}
                }
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
                    add_texture(TextureAtlasKey::Glyph(i), &font_texture);
                }
            }
        }
        let get_texture = |key: TextureAtlasKey| {
            let entry = textures.get(&key)?;
            let frame = packer.get_frame(&key)?;
            Some(TexCoords {
                u1: frame.frame.x as f32 / packer.width() as f32,
                v1: frame.frame.y as f32 / packer.height() as f32,
                u2: (frame.frame.x as f32 + frame.frame.w as f32 * entry.width_multiple)
                    / packer.width() as f32,
                v2: (frame.frame.y as f32 + frame.frame.h as f32 * entry.height_multiple)
                    / packer.height() as f32,
            })
        };
        use texture_packer::exporter::ImageExporter;
        use texture_packer::texture::Texture;
        let exporter: DynamicImage = ImageExporter::export(&packer, None).unwrap();
        if false {
            exporter.save(Path::new("textureatlasdump.png")).unwrap();
        }
        let mut texture_atlas_mips = vec![exporter.to_rgba8()];
        for _ in 0..4 {
            let mut last_level = texture_atlas_mips.last().unwrap();
            let mut new_image =
                DynamicImage::new_rgba8(last_level.width() / 2, last_level.height() / 2);
            let mut image_buffer = new_image.as_mut_rgba8().unwrap();
            for x in 0..image_buffer.width() {
                for y in 0..image_buffer.height() {
                    let mut color_sums = [0.; 3];
                    let mut alpha_sum = 0.;
                    for ux in 0..2 {
                        for uy in 0..2 {
                            let pixel = last_level.get_pixel(x * 2 + ux, y * 2 + uy);
                            let alpha = pixel.0[3] as f32 / 255.;
                            for i in 0..3 {
                                color_sums[i] += pixel.0[i] as f32 / 255. * alpha;
                            }
                            alpha_sum += alpha;
                        }
                    }
                    let mut color_sums = color_sums.map(|c| (c / alpha_sum * 255.) as u8);
                    image_buffer.put_pixel(
                        x,
                        y,
                        image::Rgba::<u8>([
                            color_sums[0],
                            color_sums[1],
                            color_sums[2],
                            (alpha_sum * 255.) as u8,
                        ]),
                    );
                }
            }
            texture_atlas_mips.push(new_image.to_rgba8());
        }
        (
            TextureAtlas {
                textures: TextureKey::entries()
                    .enumerate()
                    .map(|(i, _)| get_texture(TextureAtlasKey::Texture(i)))
                    .collect(),
                models: ModelKey::entries()
                    .enumerate()
                    .map(|(i, model)| {
                        let model = model.data();
                        model
                            .model
                            .textures
                            .iter()
                            .enumerate()
                            .filter_map(|(j, _)| get_texture(TextureAtlasKey::Model(i, j)))
                            .collect()
                    })
                    .collect(),
            },
            TextRenderer {
                glyphs: (0..font.glyph_count())
                    .map(|i| {
                        get_texture(TextureAtlasKey::Glyph(i)).unwrap_or(TexCoords {
                            u1: 0.,
                            v1: 0.,
                            u2: 0.,
                            v2: 0.,
                        })
                    })
                    .collect(),
                font,
            },
            texture_atlas_mips,
        )
    }
}
impl std::ops::Index<TextureKey> for TextureAtlas {
    type Output = TexCoords;
    fn index(&self, texture: TextureKey) -> &Self::Output {
        self.textures[texture.numeric_id()]
            .as_ref()
            .expect("this texture is not included in atlas")
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
trait TexCoordsIndexExt {
    fn tex_coords(self, index: usize) -> TexCoords;
}
impl TexCoordsIndexExt for KeyGroup<TextureData> {
    fn tex_coords(self, index: usize) -> TexCoords {
        self.list()[index % self.list().len()].tex_coords()
    }
}
pub enum RayCastResult {
    Empty,
    Block(BlockPos, Face),
    Entity(Uuid),
    Plant(BlockPos, usize),
}
pub struct ClientPlayer {
    pub position: Pos,
    pub direction: LookDirection,
    pub controller: CharacterController,
    pub running: bool,
}
impl Default for ClientPlayer {
    fn default() -> Self {
        ClientPlayer {
            position: Pos::ZERO,
            direction: LookDirection { pitch: 0., yaw: 0. },
            running: false,
            controller: CharacterController::new(),
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
    pub fn raycast(&self, world: &ClientGame, ignore_plants: bool) -> RayCastResult {
        let ray = Ray {
            position: self.get_eye(world.get_player_data()),
            direction: self.direction.make_front() * world.active_tool().reach,
        };
        let mut min_distance = f32::INFINITY;
        let mut raycast_result = RayCastResult::Empty;

        if let Some((block, position, face)) = ray.block_raycast(|block, position, face| {
            let block_entry = world.get_block(block)?;
            let selection = &block_entry.block.data().selection;
            for selection in selection {
                if let Some(result) = ray.aabb_raycast(
                    block_entry
                        .rotation
                        .rotate_aabb(*selection)
                        .offset(block.to_pos()),
                ) {
                    return Some((block, result.position, result.face));
                }
            }
            None
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
        if !ignore_plants {
            //todo: this lookup should probably be smarter
            for chunk_position in (AABB {
                min: ChunkPos::all(-1),
                max: ChunkPos::all(1),
            })
            .offset(ray.position.to_chunk_pos())
            {
                if let Some(chunk) = world.chunks.get(&chunk_position) {
                    for (offset, plants) in chunk.components.plant.iter() {
                        for (i, plant) in plants.plants.iter().enumerate() {
                            let plant_data = plant.0.data();
                            if i != plant_data.stages.len() - 1 {
                                continue;
                            }
                            let block_position = chunk_position.to_block_pos() + offset.xyz();
                            let aabb = AABB {
                                min: Pos {
                                    x: -plant_data.size / 2.,
                                    y: 0.,
                                    z: -plant_data.size / 2.,
                                },
                                max: Pos {
                                    x: plant_data.size / 2.,
                                    y: plant_data.height,
                                    z: plant_data.size / 2.,
                                },
                            }
                            .offset(
                                block_position.to_pos()
                                    + Pos {
                                        x: 0.5,
                                        y: 1.,
                                        z: 0.5,
                                    },
                            );
                            if let Some(result) = ray.aabb_raycast(aabb) {
                                let distance = result.position.distance(ray.position);
                                if distance < min_distance {
                                    min_distance = distance;
                                    raycast_result = RayCastResult::Plant(block_position, i);
                                }
                            }
                        }
                    }
                }
            }
        }
        raycast_result
    }
    pub fn update_position(
        &mut self,
        delta_time: f32,
        game: &mut ClientGame,
        abilities: &PlayerAbilities,
    ) {
        let player_entity_data = game.get_player_data();
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
        move_vector *= 5.;
        self.running = game.keys.is_down(KeyCode::ControlLeft);
        if move_vector.length_squared() == 0. {
            self.running = false;
        }
        if self.running {
            game.stamina -= delta_time * 15.;
            if game.stamina <= 0. {
                game.stamina = 0.;
                self.running = false;
            }
        }
        move_vector *= if self.running { 1.35 } else { 1. };
        match abilities.move_mode {
            MoveMode::Normal => {
                if game.keys.is_down(KeyCode::ShiftLeft) {
                    move_vector /= 2.;
                }
                if game.keys.is_down(KeyCode::Space) && self.controller.on_ground {
                    self.controller.velocity.y += 8.2;
                }
            }
            MoveMode::Fly | MoveMode::NoClip => {}
        }
        self.controller.tick(
            &mut self.position,
            delta_time,
            |block| game.get_block(block),
            move_vector,
            abilities.move_mode,
            player_entity_data
                .map(|entity_data| entity_data.hitbox())
                .unwrap_or(AABB {
                    min: Pos::ZERO,
                    max: Pos::ZERO,
                }),
        );
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
#[derive(Copy, Clone, Eq, PartialEq)]
struct ModifiedChunkEntry {
    distance: usize,
    chunk: ChunkPos,
}
impl Ord for ModifiedChunkEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.distance
            .cmp(&other.distance)
            .reverse()
            .then_with(|| self.chunk.x.cmp(&other.chunk.x))
            .then_with(|| self.chunk.y.cmp(&other.chunk.y))
            .then_with(|| self.chunk.z.cmp(&other.chunk.z))
    }
}
impl PartialOrd for ModifiedChunkEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
pub struct ClientGame {
    pub player_position: Pos,
    pub chunks: AHashMap<ChunkPos, ClientChunk>,
    pub modified_chunks: BinaryHeap<ModifiedChunkEntry>,
    pub entities: AHashMap<Uuid, ClientEntity>,
    pub player_entity: Option<Uuid>,
    pub screen: Option<ScreenData>,
    pub hud: ScreenData,
    pub hit_timer: Option<f32>,
    pub keys: InputContainer<KeyCode>,
    pub buttons: InputContainer<MouseButton>,
    pub cursor_position: PhysicalPosition<f64>,
    pub hotbar_slot: usize,
    pub item_variation: HashMap<ItemKey, usize>,
    pub viewmodel_player: AnimationPlayer,
    pub swap_hand_item: Option<(ItemKey, usize)>,
    pub chunk_mesh_channels: (
        std::sync::mpsc::Sender<(ChunkPos, Option<GPUMesh>, u64)>,
        std::sync::mpsc::Receiver<(ChunkPos, Option<GPUMesh>, u64)>,
    ),
    pub researched: HashSet<ResearchKey>,
    pub stamina: f32,
    pub place_message: Option<NetworkMessageC2S>,
}
impl Default for ClientGame {
    fn default() -> Self {
        Self {
            player_position: Pos::ZERO,
            chunks: AHashMap::with_capacity(1000), //capacity
            modified_chunks: Default::default(),
            entities: Default::default(),
            player_entity: None,
            screen: None,
            hud: ScreenData {
                screen: Key::id("hud").unwrap(),
                slots: vec![None; 10],
                properties: PropertyMap(HashMap::new()),
                selected_slot: None,
            },
            hit_timer: None,
            keys: InputContainer::default(),
            buttons: InputContainer::default(),
            cursor_position: PhysicalPosition::new(0., 0.),
            hotbar_slot: 0,
            item_variation: HashMap::new(),
            chunk_mesh_channels: std::sync::mpsc::channel(),
            viewmodel_player: AnimationPlayer::new(viewmodel_graph()),
            swap_hand_item: None,
            researched: HashSet::new(),
            stamina: 0.,
            place_message: None,
        }
    }
}
impl ClientGame {
    pub fn mark_modified(&mut self, chunk_position: ChunkPos) {
        if let Some(chunk) = self.chunks.get_mut(&chunk_position) {
            chunk
                .mesh_build_data
                .version
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if !chunk.modified {
                chunk.modified = true;
                self.modified_chunks.push(ModifiedChunkEntry {
                    distance: self
                        .player_position
                        .to_chunk_pos()
                        .distance_squared(chunk_position) as usize,
                    chunk: chunk_position,
                });
            }
        }
    }
    pub fn held_item(&self) -> &Option<ClientItem> {
        &self.hud.slots[self.hotbar_slot]
    }
    pub fn active_tool(&self) -> ToolData {
        self.held_item()
            .as_ref()
            .and_then(|item| item.item.data().tool)
            .unwrap_or(ToolData::hand())
    }
    pub fn get_block(&self, position: BlockPos) -> Option<BlockEntry> {
        let (chunk, offset) = position.to_chunk_pos_offset();
        Some(
            *self
                .chunks
                .get(&chunk)?
                .mesh_build_data
                .blocks
                .read()
                .get(offset.index())
                .unwrap(),
        )
    }
    pub fn tick_client(
        &mut self,
        device: &Arc<Device>,
        camera: &ClientPlayer,
        entity_mesh: &mut BaseMesh,
        gui_mesh: &mut GUIMesh,
        viewmodel_mesh: &mut BaseMesh,
        damage_mesh: &mut DamageMesh,
        frustum: &Frustum,
        dt: f32,
        connection: &ClientConnection,
    ) {
        if self.held_item().is_none() {
            self.viewmodel_player.trigger("empty_hand");
        }
        if camera.running {
            self.viewmodel_player.trigger("run");
        }
        let (mut animation, observers) = self.viewmodel_player.evaluate(viewmodel_graph(), dt);
        if let Some(hit_timer) = self.hit_timer {
            let time = hit_timer / self.active_tool().swing_time;
            let weight = (((time - 0.5) * 2.).abs() - 0.6).max(0.) * 1.5;
            animation.iter_mut().for_each(|a| {
                a.weight *= weight;
            });
            animation.push(DrawAnimation {
                animation: "hit",
                time: time / 2.,
                weight: 1. - weight,
            });
        }
        let mut update_hand_item = false;
        for observer in observers {
            if observer == "swap_hand_item" {
                update_hand_item = true;
            }
            if observer == "place" {
                if let Some(message) = self.place_message.take() {
                    connection.tx.send(message);
                }
            }
        }
        if (self.viewmodel_player.current_animation == "idle"
            || self.viewmodel_player.current_animation == "running")
            && self.viewmodel_player.transition.is_none()
            || self.viewmodel_player.current_animation == "down"
            || update_hand_item
        {
            self.swap_hand_item = match self.held_item() {
                Some(item) => {
                    let item = item.item;
                    Some((item, self.item_variation.get(&item).cloned().unwrap_or(0)))
                }
                None => None,
            };
        }
        render::draw_model(
            &ModelInstance {
                model: ModelKey::id("viewmodel").unwrap(),
                textures: vec![],
            },
            Matrix4::identity(),
            &mut viewmodel_mesh.consumer(Color::WHITE),
            &animation,
            |binding| match binding {
                "hand" => {
                    let (item, variant) = match self.swap_hand_item.as_ref() {
                        Some(item) => item,
                        None => return None,
                    };
                    let item_data = item.data();
                    match &item_data.action {
                        ItemAction::Place(item_block_placements) => {
                            let placement = &item_block_placements[*variant];
                            Some(Cow::Owned(ItemModel::Block((placement.block))))
                        }
                        _ => Some(Cow::Borrowed(&item_data.model)),
                    }
                }
                _ => None,
            },
        );
        while let Ok((position, buffer, version)) = self.chunk_mesh_channels.1.try_recv() {
            if let Some(chunk) = self.chunks.get_mut(&position) {
                chunk.scheduled = false;
                chunk.buffer = buffer;
                if version
                    < chunk
                        .mesh_build_data
                        .version
                        .load(std::sync::atomic::Ordering::Relaxed)
                {
                    if !chunk.modified {
                        chunk.modified = true;

                        self.modified_chunks.push(ModifiedChunkEntry {
                            distance: self
                                .player_position
                                .to_chunk_pos()
                                .distance_squared(chunk.position)
                                as usize,
                            chunk: chunk.position,
                        });
                    }
                }
            }
        }
        let mut i = 0;
        while let Some(modified_chunk) = self.modified_chunks.pop() {
            let modified_chunk = modified_chunk.chunk;
            if let Some(chunk) = self.chunks.get_mut(&modified_chunk) {
                chunk.modified = false;
                if !chunk.scheduled {
                    chunk.scheduled = true;
                    let tx = self.chunk_mesh_channels.0.clone();
                    let build_data = chunk.mesh_build_data.clone();
                    let neighbor_chunks = FaceMap::init(|face| {
                        Some(
                            self.chunks
                                .get(&(face.get_chunk_offset() + modified_chunk))?
                                .mesh_build_data
                                .clone(),
                        )
                    });
                    let device = device.clone();
                    rayon::spawn(move || {
                        let (mesh, version) = ClientChunk::build_chunk_mesh(
                            modified_chunk,
                            build_data,
                            neighbor_chunks,
                        );
                        tx.send((modified_chunk, GPUMesh::allocate(&mesh, &device), version));
                    });
                    i += 1;
                    if i > 10 {
                        break;
                    }
                }
            }
        }
        for (id, entity) in &self.entities {
            if Some(*id) == self.player_entity {
                continue;
            }
            let lerp_time = (entity.update_timestamp.elapsed().as_secs_f32() / (1. / 40.)).min(1.);
            let position = entity
                .previous_position
                .lerp(entity.position, lerp_time + 1.);
            let rotation = -block_byte_common::coord::lerp_number(
                entity.previous_direction.yaw,
                entity.direction.yaw,
                lerp_time,
            );
            render::draw_model(
                &entity.key.data().model,
                Matrix4::from_translation(Vector3::new(position.x, position.y, position.z))
                    * Matrix4::from_angle_y(Rad(rotation)),
                &mut entity_mesh.consumer(Color::WHITE),
                &[],
                |slot| {
                    entity
                        .hand_item
                        .as_ref()
                        .map(|item| Cow::Borrowed(&item.item.data().model))
                },
            );
        }
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
            if !frustum.intersects_aabb(
                &(AABB {
                    min: Pos::all(0.),
                    max: Pos::all(CHUNK_SIZE as f32),
                })
                .offset(chunk_position.to_block_pos().to_pos()),
            ) {
                continue;
            }
            if let Some(chunk) = self.chunks.get(&chunk_position) {
                for (offset, damage) in chunk.components.damage.iter() {
                    let base_position = (chunk_position.to_block_pos() + offset.xyz()).to_pos();
                    if base_position.distance_squared(self.player_position)
                        > (view_distance * view_distance) as f32
                            * CHUNK_SIZE as f32
                            * CHUNK_SIZE as f32
                    {
                        continue;
                    }
                    let blocks = chunk.mesh_build_data.blocks.read();
                    let block = blocks.get(offset.index()).unwrap();
                    let block_data = block.block.data();
                    let progress = (damage.damage
                        / block_data
                            .health
                            .as_ref()
                            .map(|health| health.health)
                            .unwrap_or(1.));
                    let mut mesh_vertex_consumer = damage_mesh.consumer(progress);
                    match &block_data.render_data {
                        BlockRenderData::Air => {}
                        BlockRenderData::Full { .. } => {
                            for face in Face::all() {
                                mesh_vertex_consumer.add_quad(
                                    face.get_vertices(
                                        TexCoords {
                                            u1: 0.,
                                            v1: 0.,
                                            u2: 16.,
                                            v2: 16.,
                                        },
                                        0,
                                    )
                                    .map(|(position, uv)| {
                                        MeshVertex {
                                            position: base_position + position,
                                            normal: face.get_offset(),
                                            uv,
                                        }
                                    }),
                                );
                            }
                        }
                        BlockRenderData::Model(model) => {
                            let orientation = Orientation::from_block_rotation(block.rotation);
                            let right = orientation.right.get_offset();
                            let up = orientation.up.get_offset();
                            let front = orientation.forward.get_offset();
                            let model_data = &model.model.data();
                            model_data.model.draw(
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
                                &[],
                                |geometry| match geometry {
                                    ModelGeometry::Quad(vertices, texture) => {
                                        let (_, width, height) = model_data.model.textures[texture];
                                        mesh_vertex_consumer.add_quad(vertices.map(|vertex| {
                                            MeshVertex {
                                                position: vertex.position,
                                                normal: vertex.normal,
                                                uv: [vertex.uv[0] * width, vertex.uv[1] * height],
                                            }
                                        }));
                                    }
                                    ModelGeometry::Triangle(vertices, texture) => todo!(),
                                },
                                |matrix, binding| {},
                            );
                        }
                    }
                }
                for (offset, plants) in chunk.components.plant.iter() {
                    let base_position = (chunk_position.to_block_pos() + offset.xyz()).to_pos();
                    if base_position.distance_squared(self.player_position)
                        > (view_distance * view_distance) as f32
                            * CHUNK_SIZE as f32
                            * CHUNK_SIZE as f32
                    {
                        continue;
                    }
                    let mut mesh_vertex_consumer = entity_mesh.consumer(Color::WHITE);
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
                            let up_height_vector = Pos::Y * plant.height;
                            let vertices = [
                                MeshVertex {
                                    position: first,
                                    uv: [texture.u1, texture.v2],
                                    normal: Pos::Y,
                                },
                                MeshVertex {
                                    position: second,
                                    uv: [texture.u2, texture.v2],
                                    normal: Pos::Y,
                                },
                                MeshVertex {
                                    position: second + up_height_vector,
                                    uv: [texture.u2, texture.v1],
                                    normal: Pos::Y,
                                },
                                MeshVertex {
                                    position: first + up_height_vector,
                                    uv: [texture.u1, texture.v1],
                                    normal: Pos::Y,
                                },
                            ];
                            mesh_vertex_consumer.add_quad(vertices);
                        }
                    }
                }
            }
        }

        if let Some(held_item) = self.held_item() {
            if self.keys.is_down(KeyCode::AltLeft) {
                match &held_item.item.data().action {
                    ItemAction::Place(place_block) => match camera.raycast(self, true) {
                        RayCastResult::Block(position, face) => {
                            let place_block = &place_block[self
                                .item_variation
                                .get(&held_item.item)
                                .cloned()
                                .unwrap_or(0)];
                            let block_position = position + face.get_block_offset();
                            let block_data = place_block.block.data();
                            let mut blocked = place_block.use_count > held_item.count;
                            let rotation = block_data
                                .rotation
                                .from_look_direction(camera.direction, face);
                            let fake_block_entry = BlockEntry {
                                block: place_block.block,
                                rotation,
                                color: BlockColor::default(),
                            };
                            for entity in self.entities.values() {
                                let entity_hitbox =
                                    entity.key.data().hitbox().offset(entity.position);
                                if fake_block_entry
                                    .colliders(block_position)
                                    .any(|collider| entity_hitbox.intersects(collider))
                                {
                                    blocked = true;
                                    break;
                                }
                            }
                            if let Some(hanging) = block_data.hanging {
                                let world_hanging = rotation.rotate_face(hanging);
                                match self
                                    .get_block(block_position + world_hanging.get_block_offset())
                                {
                                    Some(hanging_block) => {
                                        if !hanging_block.supports(world_hanging.opposite()) {
                                            blocked = true;
                                        }
                                    }
                                    None => {
                                        blocked = true;
                                    }
                                }
                            }
                            let (chunk, offset) = block_position.to_chunk_pos_offset();
                            if let Some(chunk) = self.chunks.get(&chunk) {
                                let block = chunk
                                    .mesh_build_data
                                    .blocks
                                    .read()
                                    .get(offset.index())
                                    .unwrap()
                                    .block;
                                if block == air_block() {
                                    let orientation = Orientation::from_block_rotation(rotation);
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
                                        &mut entity_mesh.consumer(if blocked {
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
                                                a: 150,
                                            }
                                        }),
                                    );
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
    pub fn tick_server(&mut self, dt: f32) {
        self.stamina += dt * 10.;
        for (_, chunk) in &mut self.chunks {
            let mut damage_to_clear = Vec::new();
            let blocks = chunk.mesh_build_data.blocks.read();
            for (offset, health) in chunk.components.damage.iter_mut() {
                let data = blocks.get(offset.index()).unwrap().block.data();
                if let Some(health_data) = &data.health {
                    health.damage -= dt
                        * blocks
                            .get(offset.index())
                            .unwrap()
                            .block
                            .data()
                            .health
                            .as_ref()
                            .map(|health| health.health_regen)
                            .unwrap_or(1.);
                    if health.damage <= 0. {
                        damage_to_clear.push(offset);
                    }
                }
            }
            for block in damage_to_clear {
                chunk.components.damage.remove(block);
            }
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
struct ChunkMeshBuildData {
    pub blocks: RwLock<BlockPalette>,
    pub version: AtomicU64,
}
pub struct ClientChunk {
    pub position: ChunkPos,
    pub mesh_build_data: Arc<ChunkMeshBuildData>,
    pub components: ClientChunkBlockComponents,
    pub buffer: Option<GPUMesh>,
    pub modified: bool,
    pub scheduled: bool,
}

impl ClientChunk {
    pub fn build_chunk_mesh(
        position: ChunkPos,
        chunk_data: Arc<ChunkMeshBuildData>,
        neighbor_chunks: FaceMap<Option<Arc<ChunkMeshBuildData>>>,
    ) -> (ChunkMesh, u64) {
        let mut mesh: ChunkMesh = ChunkMesh::default();
        let chunk_blocks = chunk_data.blocks.read();
        let neighbor_chunks =
            neighbor_chunks.map(|chunk| chunk.as_ref().map(|chunk| chunk.blocks.read()));

        if chunk_blocks.unique_values() == 1 && chunk_blocks.get(0).unwrap().block == air_block() {
            return (
                mesh,
                chunk_data
                    .version
                    .load(std::sync::atomic::Ordering::Relaxed),
            );
        }

        for x in 0..CHUNK_SIZE as u8 {
            for y in 0..CHUNK_SIZE as u8 {
                for z in 0..CHUNK_SIZE as u8 {
                    let block = *chunk_blocks.get(ChunkOffset::new(x, y, z).index()).unwrap();
                    let block_data = block.block.data();
                    let mut mesh_consumer = mesh.consumer(block.color, 0);
                    match &block_data.render_data {
                        BlockRenderData::Air => {}
                        BlockRenderData::Full { faces } => {
                            let base_position = Pos {
                                x: (position.x as f32 * CHUNK_SIZE as f32) + x as f32,
                                y: (position.y as f32 * CHUNK_SIZE as f32) + y as f32,
                                z: (position.z as f32 * CHUNK_SIZE as f32) + z as f32,
                            };
                            let mut rng = StdRng::from_seed(
                                Seeder::from((
                                    base_position.x as i32,
                                    base_position.y as i32,
                                    base_position.z as i32,
                                ))
                                .make_seed(),
                            );
                            use rand::SeedableRng;
                            for face in Face::all() {
                                let neighbor_position = BlockPos {
                                    x: x as i32,
                                    y: y as i32,
                                    z: z as i32,
                                } + face.get_block_offset();
                                let (neighbor_chunk, neighbor_offset) =
                                    neighbor_position.to_chunk_pos_offset();
                                /*let neighbor_chunk = match Face::all()
                                    .iter()
                                    .find(|f| f.get_chunk_offset() == neighbor_chunk)
                                {
                                    Some(face) => neighbor_chunks.by_face(*face).as_ref(),
                                    None => Some(&chunk_blocks),
                                };*/
                                let neighbor_chunk =
                                    match (neighbor_chunk.x, neighbor_chunk.y, neighbor_chunk.z) {
                                        (0, 0, 0) => Some(&chunk_blocks),
                                        (-1, 0, 0) => neighbor_chunks.left.as_ref(),
                                        (1, 0, 0) => neighbor_chunks.right.as_ref(),
                                        (0, -1, 0) => neighbor_chunks.down.as_ref(),
                                        (0, 1, 0) => neighbor_chunks.up.as_ref(),
                                        (0, 0, -1) => neighbor_chunks.front.as_ref(),
                                        (0, 0, 1) => neighbor_chunks.back.as_ref(),
                                        _ => unreachable!(),
                                    };
                                if let Some(neighbor_chunk) = neighbor_chunk {
                                    let neighbor_block_data = neighbor_chunk
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
                                let texture = faces
                                    .by_face(*face)
                                    .tex_coords(rng.random::<u32>() as usize);
                                mesh_consumer.add_quad(face.get_vertices(texture, 0).map(
                                    |(position, uv)| MeshVertex {
                                        position: base_position + position,
                                        normal: face.get_offset(),
                                        uv,
                                    },
                                ));
                            }
                        }
                        BlockRenderData::Model(model) => {
                            let position = Pos {
                                x: (position.x as f32 * CHUNK_SIZE as f32) + x as f32 + 0.5,
                                y: (position.y as f32 * CHUNK_SIZE as f32) + y as f32 + 0.5,
                                z: (position.z as f32 * CHUNK_SIZE as f32) + z as f32 + 0.5,
                            };
                            let orientation = Orientation::from_block_rotation(block.rotation);
                            let right = orientation.right.get_offset();
                            let up = orientation.up.get_offset();
                            let front = orientation.forward.get_offset();
                            render::draw_model(
                                model,
                                Matrix4::from_translation(Vector3::new(
                                    position.x, position.y, position.z,
                                )) * Matrix4::from_cols(
                                    Vector4::new(right.x, right.y, right.z, 0.),
                                    Vector4::new(up.x, up.y, up.z, 0.),
                                    Vector4::new(-front.x, -front.y, -front.z, 0.),
                                    Vector4::new(0., 0., 0., 1.),
                                ) * Matrix4::from_translation(Vector3::new(0., -0.5, 0.)),
                                &mut mesh_consumer,
                                &[],
                                |_| None,
                            );
                        }
                    }
                }
            }
        }

        (
            mesh,
            chunk_data
                .version
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

pub fn translate<'a>(key: &'a str) -> &'a str {
    Key::<TranslationLanguageData>::id("en")
        .unwrap()
        .data()
        .translate(key)
}
pub mod clipping {
    use block_byte_common::coord::AABB;
    use cgmath::InnerSpace;
    use cgmath::Vector3;

    #[derive(Clone, Copy)]
    pub struct Plane {
        pub normal: Vector3<f32>,
        pub d: f32,
    }
    pub struct Frustum {
        pub planes: [Plane; 6],
    }
    impl Plane {
        fn normalize(self) -> Plane {
            let len = self.normal.magnitude();
            Plane {
                normal: self.normal / len,
                d: self.d / len,
            }
        }
        pub fn distance(&self, p: Vector3<f32>) -> f32 {
            self.normal.dot(p) + self.d
        }
    }
    impl Frustum {
        pub fn from_matrix(m: cgmath::Matrix4<f32>) -> Self {
            use cgmath::Matrix;
            //let m = m.transpose(); // cgmath is column-major

            let rows = [m.row(0), m.row(1), m.row(2), m.row(3)];

            let planes = [
                // Left
                Self::plane_from_row(rows[3] + rows[0]),
                // Right
                Self::plane_from_row(rows[3] - rows[0]),
                // Bottom
                Self::plane_from_row(rows[3] + rows[1]),
                // Top
                Self::plane_from_row(rows[3] - rows[1]),
                // Near
                Self::plane_from_row(/*rows[3] + */ rows[2]),
                // Far
                Self::plane_from_row(rows[3] - rows[2]),
            ];

            Frustum { planes }
        }
        fn plane_from_row(v: cgmath::Vector4<f32>) -> Plane {
            Plane {
                normal: Vector3::new(v.x, v.y, v.z),
                d: v.w,
            }
            .normalize()
        }
        pub fn intersects_aabb(&self, aabb: &AABB<f32>) -> bool {
            for plane in &self.planes {
                let center = (aabb.min + aabb.max) * 0.5;
                let extents = aabb.max - center;

                let r = extents.x * plane.normal.x.abs()
                    + extents.y * plane.normal.y.abs()
                    + extents.z * plane.normal.z.abs();

                if plane.normal.dot(Vector3 {
                    x: center.x,
                    y: center.y,
                    z: center.z,
                }) + plane.d
                    < -r
                {
                    return false;
                }
            }
            true
        }
    }
}

pub struct AnimationTransition {
    pub to: String,
    pub condition: String,
    pub inverted: bool,
    pub reset: bool,
    pub time: f32,
}
pub struct AnimationNode {
    pub next: String,
    pub model_animation: String,
    pub length: f32,
    pub speed: f32,
    pub transitions: Vec<AnimationTransition>,
    pub observers: Vec<(f32, String)>,
}
pub struct AnimationGraph {
    pub default_animation: String,
    pub animations: HashMap<String, AnimationNode>,
}
struct AnimationPlayerTransition {
    pub time: f32,
    pub length: f32,
    pub to: String,
}
pub struct AnimationPlayer {
    pub current_animation: String,
    pub time: f32,
    pub triggers: HashSet<String>,
    pub transition: Option<AnimationPlayerTransition>,
}
impl AnimationPlayer {
    pub fn new(graph: &AnimationGraph) -> AnimationPlayer {
        AnimationPlayer {
            current_animation: graph.default_animation.clone(),
            time: 0.,
            triggers: HashSet::new(),
            transition: None,
        }
    }
    pub fn trigger(&mut self, name: impl ToString) {
        self.triggers.insert(name.to_string());
    }
    pub fn evaluate<'a>(
        &mut self,
        graph: &'a AnimationGraph,
        dt: f32,
    ) -> (Vec<DrawAnimation<'a>>, Vec<String>) {
        let mut activated_observers = Vec::new();
        match &mut self.transition {
            Some(transition) => {
                transition.time += dt;
                if transition.time > transition.length {
                    self.current_animation = self.transition.take().unwrap().to;
                    self.time = 0.;
                }
            }
            None => {
                let current_node = graph.animations.get(&self.current_animation).unwrap();
                let previous_time = self.time;
                self.time += dt * current_node.speed;
                for (time, observer) in &current_node.observers {
                    if previous_time < *time * current_node.speed
                        && self.time >= *time * current_node.speed
                    {
                        activated_observers.push(observer.to_string());
                    }
                }
                if self.time > current_node.length * current_node.speed {
                    self.current_animation = current_node.next.clone();
                    self.time = 0.;
                }
                for trigger in &current_node.transitions {
                    if self.triggers.contains(&trigger.condition) ^ trigger.inverted {
                        self.transition = Some(AnimationPlayerTransition {
                            time: 0.,
                            length: trigger.time,
                            to: trigger.to.clone(),
                        });
                    }
                }
                self.triggers.clear();
            }
        }
        match &self.transition {
            Some(transition) => {
                let progress = transition.time / transition.length;
                (
                    vec![
                        DrawAnimation {
                            animation: &graph
                                .animations
                                .get(&self.current_animation)
                                .unwrap()
                                .model_animation,
                            time: self.time,
                            weight: 1. - progress,
                        },
                        DrawAnimation {
                            animation: &graph
                                .animations
                                .get(&transition.to)
                                .unwrap()
                                .model_animation,
                            time: 0.,
                            weight: progress,
                        },
                    ],
                    Vec::new(),
                )
            }
            None => (
                vec![DrawAnimation {
                    animation: &graph
                        .animations
                        .get(&self.current_animation)
                        .unwrap()
                        .model_animation,
                    time: self.time,
                    weight: 1.,
                }],
                activated_observers,
            ),
        }
    }
}
static VIEWMODEL_GRAPH: OnceLock<AnimationGraph> = OnceLock::new();
pub fn viewmodel_graph() -> &'static AnimationGraph {
    VIEWMODEL_GRAPH.get_or_init(|| {
        let mut animations = HashMap::new();
        animations.insert(
            "idle".to_string(),
            AnimationNode {
                next: "idle".to_string(),
                model_animation: "idle".to_string(),
                length: 8.,
                speed: 0.25,
                transitions: vec![
                    AnimationTransition {
                        condition: "build".to_string(),
                        inverted: false,
                        reset: true,
                        to: "place".to_string(),
                        time: 0.0,
                    },
                    AnimationTransition {
                        condition: "interact".to_string(),
                        inverted: false,
                        reset: true,
                        to: "interact".to_string(),
                        time: 0.1,
                    },
                    AnimationTransition {
                        condition: "equip".to_string(),
                        inverted: false,
                        reset: true,
                        to: "equip".to_string(),
                        time: 0.,
                    },
                    AnimationTransition {
                        condition: "run".to_string(),
                        inverted: false,
                        reset: true,
                        to: "running".to_string(),
                        time: 0.1,
                    },
                ],
                observers: vec![],
            },
        );
        animations.insert(
            "place".to_string(),
            AnimationNode {
                next: "idle".to_string(),
                model_animation: "place".to_string(),
                length: 0.38,
                speed: 2.,
                transitions: vec![AnimationTransition {
                    condition: "build".to_string(),
                    inverted: false,
                    reset: true,
                    to: "place".to_string(),
                    time: 0.05,
                }],
                observers: vec![
                    //(0.1, "place".to_string()),
                    (0.07, "swap_hand_item".to_string()),
                ],
            },
        );
        animations.insert(
            "interact".to_string(),
            AnimationNode {
                next: "idle".to_string(),
                model_animation: "place".to_string(),
                length: 0.38,
                speed: 1.,
                transitions: vec![],
                observers: vec![(0.1, "swap_hand_item".to_string())],
            },
        );
        animations.insert(
            "equip".to_string(),
            AnimationNode {
                next: "idle".to_string(),
                model_animation: "equip".to_string(),
                length: 0.5,
                speed: 1.,
                transitions: vec![AnimationTransition {
                    condition: "equip".to_string(),
                    reset: true,
                    inverted: false,
                    to: "equip".to_string(),
                    time: 0.,
                }],
                observers: vec![(0.25, "swap_hand_item".to_string())],
            },
        );
        animations.insert(
            "running".to_string(),
            AnimationNode {
                next: "running".to_string(),
                model_animation: "running".to_string(),
                length: 1.,
                speed: 1.,
                transitions: vec![
                    AnimationTransition {
                        condition: "run".to_string(),
                        reset: true,
                        inverted: true,
                        to: "idle".to_string(),
                        time: 0.1,
                    },
                    AnimationTransition {
                        condition: "build".to_string(),
                        inverted: false,
                        reset: true,
                        to: "place".to_string(),
                        time: 0.,
                    },
                    AnimationTransition {
                        condition: "interact".to_string(),
                        inverted: false,
                        reset: true,
                        to: "interact".to_string(),
                        time: 0.1,
                    },
                    AnimationTransition {
                        condition: "equip".to_string(),
                        inverted: false,
                        reset: true,
                        to: "equip".to_string(),
                        time: 0.,
                    },
                ],
                observers: vec![],
            },
        );
        AnimationGraph {
            default_animation: "idle".to_string(),
            animations,
        }
    })
}
pub struct ClientConnection {
    rx: std::sync::mpsc::Receiver<(NetworkMessageS2C, Instant)>,
    tx: std::sync::mpsc::Sender<NetworkMessageC2S>,
    state: Arc<Mutex<ClientConnectionState>>,
}
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ClientConnectionState {
    Connecting,
    Connected,
    Disconnect,
    Disconnected,
}
impl ClientConnection {
    pub fn connect(addr: SocketAddr) -> ClientConnection {
        let (s2c_tx, s2c_rx) = std::sync::mpsc::channel();
        let (c2s_tx, c2s_rx) = std::sync::mpsc::channel();
        let state = Arc::new(Mutex::new(ClientConnectionState::Connecting));
        {
            let state = state.clone();
            std::thread::spawn(move || {
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

                let mut transport =
                    NetcodeClientTransport::new(current_time, authentication, socket).unwrap();
                loop {
                    let delta_time_duration = Duration::from_secs_f32(1. / 250.);
                    match { *state.lock() } {
                        ClientConnectionState::Connecting => {
                            if client.is_connected() {
                                *state.lock() = ClientConnectionState::Connected;
                            }
                        }
                        ClientConnectionState::Connected => {
                            while let Ok(message) = c2s_rx.try_recv() {
                                client.send_message(0, serde_cbor::to_vec(&message).unwrap());
                            }
                            client.update(delta_time_duration);
                            let client = RefCell::new(&mut client);
                            for mut message in
                                std::iter::from_fn(|| client.borrow_mut().receive_message(0)).chain(
                                    std::iter::from_fn(|| client.borrow_mut().receive_message(1)),
                                )
                            {
                                let mut message = &message[..];
                                let mut rdr = lz4_flex::frame::FrameDecoder::new(&mut message);
                                let message: NetworkMessageS2C =
                                    serde_cbor::from_reader(&mut rdr).unwrap();
                                s2c_tx.send((message, Instant::now()));
                            }
                        }
                        ClientConnectionState::Disconnect => break,
                        ClientConnectionState::Disconnected => unreachable!(),
                    }
                    if !client.is_disconnected() {
                        transport.update(delta_time_duration, &mut client).unwrap();
                        transport.send_packets(&mut client).unwrap();
                    } else {
                        println!("disconnect {:?}", client.disconnect_reason());
                        *state.lock() = ClientConnectionState::Disconnected;
                        return;
                    }
                    std::thread::sleep(delta_time_duration);
                }
                transport.disconnect();
                client.disconnect();
            });
        }
        ClientConnection {
            rx: s2c_rx,
            tx: c2s_tx,
            state,
        }
    }
    pub fn state(&self) -> ClientConnectionState {
        *self.state.lock()
    }
}
