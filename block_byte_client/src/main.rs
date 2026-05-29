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
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU32, AtomicU64},
    },
    time::{Duration, Instant, SystemTime},
    u32,
};

use ahash::{AHashMap, AHashSet};
use base64::{Engine, prelude::BASE64_STANDARD};
use block_byte_common::{
    ACCELERATION_COEFFICIENT, CharacterController, ClientItem, Color, EntityStats, HitTimer,
    ItemMoveMode, LookDirection, MoveMode, NORMAL_SPEED, SERVER_DT, TexCoords,
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, Pos, Ray, Vec3},
    model::{DrawAnimation, ModelGeometry, ModelTexture},
    net::{NetworkMessageC2S, NetworkMessageS2C, make_connection_config},
    number_approach_smooth,
    registry::{
        self, BlockColor, BlockEntry, BlockInteractAction, BlockPalette, BlockRenderData,
        EntityData, EntityInteractAction, EntityKey, ItemAction, ItemKey, ItemModel, Key, KeyGroup,
        ModelData, ModelInstance, ModelKey, Registry, ResearchKey, TextureData, TextureKey,
        ToolData, TranslationLanguageData, air_block, load_registries,
    },
    ui::{PropertyMap, SlotId},
    world::{self, ClientBlockComponentUpdate, ClientChunkBlockComponents},
};
use bytemuck::Pod;
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
use wgpu::{
    Buffer, CommandEncoder, Device, Queue,
    util::{DeviceExt, StagingBelt},
};
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
        GUIMesh, GUIVertex, Mesh, MeshVertex, MeshVertexConsumer, RenderState, SurfaceError,
        Vertex, draw_model, get_block_matrix,
    },
    ui::{ScreenData, TextRenderer, UIInput, UIPos, UIRect, render_screen, text_renderer},
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
    let texture_atlas = TextureAtlas::pack();
    TEXTURE_ATLAS.set(texture_atlas);

    rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build_global();

    let connection = ClientConnection::connect("127.0.0.1:5000".parse().unwrap());

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    event_loop
        .run_app(&mut App {
            camera: ClientPlayer::default(),
            render_state: None,
            game: ClientGame::default(),
            connection,
            teleport_id: 0,
            last_update: Instant::now(),
            mspt: 0.,
            delta_time_average: 0.,
        })
        .unwrap();
}

struct App {
    render_state: Option<RenderState>,
    game: ClientGame,
    camera: ClientPlayer,
    connection: ClientConnection,
    last_update: Instant,
    teleport_id: u32,
    mspt: f32,
    delta_time_average: f32,
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
                winit::event::MouseScrollDelta::LineDelta(scroll_x, scroll_y) => {
                    self.game.wheel_scroll_delta.x += scroll_x;
                    self.game.wheel_scroll_delta.y += scroll_y;
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
                                    new_variant -= scroll_y as isize;
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
                            new_slot += -scroll_y as isize;
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

                self.game.hud.properties.0.insert(
                    "stamina_action".to_string(),
                    match &self.game.hit_timer {
                        Some(hit_timer) => {
                            let tool_data = self.game.active_tool();
                            let progress = hit_timer.progress();
                            tool_data.stamina * (1. - progress)
                        }
                        None => 0.,
                    },
                );

                self.game
                    .hud
                    .properties
                    .0
                    .insert("stamina".to_string(), self.game.stamina);

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
                    &render_state.queue,
                    &self.camera,
                    &mut entity_mesh,
                    &mut gui_mesh,
                    &mut viewmodel_mesh,
                    &mut damage_mesh,
                    &frustum,
                    dt,
                    &self.connection,
                    render_state.animation_time,
                );
                self.game.chunk_buffer_pool.tick(&render_state.device);
                {
                    let weight = 0.05;
                    self.delta_time_average = weight * dt + (1. - weight) * self.delta_time_average;
                }
                let aspect_ratio =
                    render_state.size().width as f32 / render_state.size().height as f32;
                text_renderer().draw(
                    UIPos {
                        x: -aspect_ratio + 0.1,
                        y: 0.9,
                    },
                    &format!(
                        "{:.2} {:.2} {:.2} fps: {:.0}, queue: {}, pool: {}, mspt: {:.2} {}",
                        self.game.player_position.x,
                        self.game.player_position.y,
                        self.game.player_position.z,
                        1. / self.delta_time_average,
                        self.game.chunk_mesh_queue_size,
                        self.game
                            .chunk_buffer_pool
                            .buffers
                            .iter()
                            .map(|p| p.len().to_string())
                            .collect::<Vec<_>>()
                            .join(","),
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
                let mut message_queue = Vec::new();
                render_screen(
                    &mut self.game.hud,
                    None,
                    render_state.size(),
                    &mut gui_mesh,
                    dt,
                    &mut message_queue,
                );

                if let Some(screen) = &mut self.game.screen {
                    render_screen(
                        screen,
                        Some(
                            &(UIInput {
                                mouse_position: UIPos {
                                    x: self.game.cursor_position.x as f32,
                                    y: self.game.cursor_position.y as f32,
                                },
                                last_mouse_position: UIPos {
                                    x: self.game.last_cursor_position.x as f32,
                                    y: self.game.last_cursor_position.y as f32,
                                },
                                last_scroll: self.game.wheel_scroll_delta,
                                buttons: &self.game.buttons,
                                keys: &self.game.keys,
                            }),
                        ),
                        render_state.size(),
                        &mut gui_mesh,
                        dt,
                        &mut message_queue,
                    );
                }

                for message in message_queue {
                    self.send_message(message);
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

                let ps = profiler::profiler_scope("render");
                match render_state.render(
                    &self.camera,
                    &mut self.game,
                    aspect_ratio,
                    entity_mesh,
                    gui_mesh,
                    viewmodel_mesh,
                    damage_mesh,
                    &frustum,
                    dt,
                ) {
                    Ok(_) => {}
                    Err(SurfaceError::Recreate) => render_state.resize(render_state.size()),
                    Err(SurfaceError::Crash) => {
                        println!("error");
                        event_loop.exit();
                    }
                }
                ps.end();

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
                            self.game.hit_timer = Some(HitTimer {
                                current_time: 0.,
                                swing_time: self.game.active_tool().swing_time
                                    * (self.game.player_stats.haste() / 100.),
                            });
                        }
                    }
                    self.camera.update_position(dt, &mut self.game);
                    self.game.player_position = self.camera.position;
                    self.send_message(NetworkMessageC2S::PlayerPosition {
                        position: self.camera.position,
                        teleport_id: self.teleport_id,
                        direction: self.camera.direction,
                        crouching: self.camera.crouching,
                    });
                    if let Some(hit_timer) = &mut self.game.hit_timer {
                        if hit_timer.tick(dt) {
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
                        } else {
                            if hit_timer.is_finished() {
                                self.game.hit_timer = None;
                            }
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
                                            components: RwLock::new(components),
                                            version: AtomicU64::new(0),
                                        }),
                                        gpu_mesh: GPUMesh::empty(),
                                        gpu_mesh_high_res: GPUMesh::empty(),
                                        position,
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
                                if let Some(chunk) = self.game.chunks.remove(&position) {
                                    self.game.chunk_buffer_pool.reclaim(chunk.gpu_mesh);
                                    self.game.chunk_buffer_pool.reclaim(chunk.gpu_mesh_high_res);
                                }
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
                            NetworkMessageS2C::GameTick { ticks_passed, mspt } => {
                                self.game.server_ticks_passed = ticks_passed;
                                self.mspt = mspt;
                                self.game.tick_server();
                            }
                            NetworkMessageS2C::AddEntity {
                                uuid,
                                key,
                                position,
                                direction,
                                hand_item,
                                crouching,
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
                                        crouching,
                                    },
                                );
                            }
                            NetworkMessageS2C::MoveEntity {
                                uuid,
                                position,
                                direction,
                                crouching,
                            } => {
                                if let Some(entity) = self.game.entities.get_mut(&uuid) {
                                    entity.previous_position = entity.position;
                                    entity.previous_direction = entity.direction;
                                    entity.update_timestamp = time;
                                    entity.position = position;
                                    entity.direction = direction;
                                    entity.crouching = crouching;
                                }
                            }
                            NetworkMessageS2C::RemoveEntity { uuid } => {
                                self.game.entities.remove(&uuid);
                            }
                            NetworkMessageS2C::UpdateBlockComponents {
                                chunk: chunk_position,
                                offset,
                                update: data,
                            } => {
                                match data {
                                    ClientBlockComponentUpdate::ClientBlockPlants(..) => {
                                        self.game.mark_modified(chunk_position);
                                    }
                                    _ => {}
                                }
                                if let Some(chunk) = self.game.chunks.get_mut(&chunk_position) {
                                    data.update(
                                        offset,
                                        &mut *chunk.mesh_build_data.components.write(),
                                    );
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
                                self.camera.height_animation = position.y;
                                self.teleport_id = teleport_id;
                            }
                            NetworkMessageS2C::UpdatePlayerStats { stats } => {
                                self.game.player_stats = stats;
                            }
                            NetworkMessageS2C::UIOpen {
                                screen,
                                slots,
                                properties,
                            } => {
                                self.game.screen = Some(ScreenData {
                                    screen,
                                    slots,
                                    properties,
                                    selected_slot: None,
                                    slot_action_prediction: HashMap::new(),
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
                                        screen.slot_action_prediction.remove(&slot);
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
                            NetworkMessageS2C::UISetProperty { property, value } => {
                                if let Some(screen) = &mut self.game.screen {
                                    screen.properties.0.insert(property, value);
                                }
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
                self.game.last_cursor_position = self.game.cursor_position;
                self.game.wheel_scroll_delta = UIPos { x: 0., y: 0. };

                let profiler_data = profiler::profiler_consume();
                if self.last_update.elapsed().as_millis() > 18 && false {
                    println!(
                        "slow frame: {}",
                        self.last_update.elapsed().as_secs_f32() * 1000.
                    );
                    for (name, duration) in profiler_data {
                        println!("\t{} - {}", name, duration.as_micros());
                    }
                }
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
    text_renderer: TextRenderer,
    texture_mips: Vec<RgbaImage>,
    texture_material: RgbaImage,
}

impl TextureAtlas {
    pub fn pack() -> Self {
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
        let mut material_texture = RgbaImage::new(texture_dimensions, texture_dimensions);
        for texture in TextureKey::entries() {
            let texture_data = texture.data();
            if let Some(tex_coords) = get_texture(TextureAtlasKey::Texture(texture.numeric_id())) {
                let [color_mask, emissive] = [
                    texture_data.color_mask.as_ref(),
                    texture_data.emissive.as_ref(),
                ]
                .map(|t| t.map(|t| t.grayscale().into_luma8()));
                let start_x = (tex_coords.u1 * texture_dimensions as f32) as u32;
                let start_y = (tex_coords.v1 * texture_dimensions as f32) as u32;
                for x in 0..texture_data.texture.width() {
                    for y in 0..texture_data.texture.height() {
                        let [color_mask, emissive] = [color_mask.as_ref(), emissive.as_ref()]
                            .map(|t| t.map(|t| t.get_pixel(x, y).0[0]).unwrap_or(0));
                        material_texture.get_pixel_mut(start_x + x, start_y + y).0 =
                            [color_mask, emissive, 0, 0];
                    }
                }
            }
        }

        TextureAtlas {
            textures: TextureKey::entries()
                .map(|texture| get_texture(TextureAtlasKey::Texture(texture.numeric_id())))
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
            text_renderer: TextRenderer {
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
            texture_material: material_texture,
            texture_mips: texture_atlas_mips,
        }
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
    pub crouching: bool,
    pub height_animation: f32,
}
impl Default for ClientPlayer {
    fn default() -> Self {
        ClientPlayer {
            position: Pos::ZERO,
            direction: LookDirection { pitch: 0., yaw: 0. },
            running: false,
            controller: CharacterController::new(),
            crouching: false,
            height_animation: 0.,
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
        Pos {
            x: self.position.x,
            y: self.height_animation + player_entity_data.map(|data| data.eye_height).unwrap_or(0.),
            z: self.position.z,
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
            if let Some(result) =
                ray.aabb_raycast(entity_data.hitbox(entity.crouching).offset(entity.position))
            {
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
                    for (offset, plants) in chunk.mesh_build_data.components.read().plant.iter() {
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
    pub fn update_position(&mut self, delta_time: f32, game: &mut ClientGame) {
        let move_mode = MoveMode::Normal;
        self.height_animation = number_approach_smooth(
            self.height_animation,
            self.position.y
                - if self.crouching {
                    game.get_player_data()
                        .map(|data| data.crouch_height_difference)
                        .unwrap_or(0.)
                } else {
                    0.
                },
            40.,
            0.5,
            delta_time,
        );
        let Some(player_entity_data) = game.get_player_data() else {
            return;
        };
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
        move_vector *= game.player_stats.speed() / 100. * NORMAL_SPEED;
        //move_vector *= player_entity_data.speed;
        self.running = game.keys.is_down(KeyCode::ControlLeft);
        if move_vector.length_squared() == 0. {
            self.running = false;
        }
        if self.running {
            game.stamina -= delta_time * 15.;
            if game.stamina <= 0. {
                game.stamina = -1.;
                self.running = false;
            }
        }
        move_vector *= if self.running { 1.35 } else { 1. };
        self.crouching = game.keys.is_down(KeyCode::ShiftLeft);
        match move_mode {
            MoveMode::Normal | MoveMode::Fly => {
                if !self.crouching
                    && CharacterController::collides_at(
                        self.position,
                        &|block| game.get_block(block),
                        player_entity_data.hitbox(false),
                    )
                    .is_some()
                {
                    self.crouching = true;
                }
            }
            MoveMode::NoClip => {}
        }
        match move_mode {
            MoveMode::Normal => {
                if self.crouching {
                    move_vector /= 2.;
                }
                if game.keys.is_down(KeyCode::Space) && self.controller.on_ground {
                    self.controller.velocity.y += player_entity_data.base_stats.jump_velocity();
                }
            }
            MoveMode::Fly | MoveMode::NoClip => {}
        }
        self.controller.tick(
            &mut self.position,
            delta_time,
            |block| game.get_block(block),
            move_vector,
            move_mode,
            player_entity_data.hitbox(self.crouching),
            ACCELERATION_COEFFICIENT * game.player_stats.speed() / 100. * NORMAL_SPEED,
            0.5,
            game.keys.is_down(KeyCode::ShiftLeft),
        );
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
    pub player_stats: EntityStats,
    pub player_position: Pos,
    pub chunks: AHashMap<ChunkPos, ClientChunk>,
    pub modified_chunks: BinaryHeap<ModifiedChunkEntry>,
    pub entities: AHashMap<Uuid, ClientEntity>,
    pub player_entity: Option<Uuid>,
    pub screen: Option<ScreenData>,
    pub hud: ScreenData,
    pub hit_timer: Option<HitTimer>,
    pub keys: InputContainer<KeyCode>,
    pub buttons: InputContainer<MouseButton>,
    pub cursor_position: PhysicalPosition<f64>,
    pub last_cursor_position: PhysicalPosition<f64>,
    pub wheel_scroll_delta: UIPos,
    pub hotbar_slot: usize,
    pub item_variation: HashMap<ItemKey, usize>,
    pub viewmodel_player: AnimationPlayer,
    pub swap_hand_item: Option<(ItemKey, usize)>,
    pub chunk_mesh_channels: (
        std::sync::mpsc::Sender<(ChunkPos, ChunkMesh, ChunkMesh, u64)>,
        std::sync::mpsc::Receiver<(ChunkPos, ChunkMesh, ChunkMesh, u64)>,
    ),
    pub researched: HashSet<ResearchKey>,
    pub stamina: f32,
    pub place_message: Option<NetworkMessageC2S>,
    pub chunk_mesh_queue_size: usize,
    pub server_ticks_passed: u64,
    pub chunk_buffer_pool: ChunkBufferPool,
}
#[derive(Default)]
struct ChunkBufferPool {
    pub buffers: Vec<Vec<GPUMesh>>,
}
impl ChunkBufferPool {
    const MIN_BUFFER_SIZE: usize = 64 * 1024;
    pub fn tick(&mut self, device: &Device) {
        let preallocated_buckets = [40, 40, 20, 4];
        self.buffers
            .resize_with(preallocated_buckets.len(), Vec::new);
        /*println!(
            "{}",
            self.buffers
                .iter()
                .map(|b| b.len().to_string())
                .collect::<Vec<String>>()
                .join(",")
        );*/
        for (bucket_id, size) in preallocated_buckets.iter().enumerate() {
            let bucket = &mut self.buffers[bucket_id];
            if bucket.len() < *size {
                bucket.push(GPUMesh::allocate(
                    &BaseMesh::default(),
                    Self::MIN_BUFFER_SIZE * 4_usize.pow(bucket_id as u32),
                    device,
                ));
                return;
            }
        }
    }
    pub fn reclaim(&mut self, mesh: GPUMesh) {
        if let Some(buffer) = &mesh.buffer {
            let bucket = Self::get_data_index(buffer.size() as usize).0;
            self.get_bucket(bucket).push(mesh);
        }
    }
    pub fn allocate_or_reuse<T: Pod>(
        &mut self,
        mesh: &Mesh<T>,
        mut gpu_buffer: GPUMesh,
        staging_belt: &mut StagingBelt,
        command_encoder: &mut CommandEncoder,
        device: &Device,
    ) -> GPUMesh {
        let (vertex_size, index_size, _) = mesh.get_data_size();
        let total_size = vertex_size + index_size;
        if let Some(buffer) = &gpu_buffer.buffer {
            if total_size <= buffer.size() as usize {
                gpu_buffer.upload(mesh, device, staging_belt, command_encoder);
                return gpu_buffer;
            }
        }
        self.reclaim(gpu_buffer);
        self.allocate(mesh, staging_belt, device, command_encoder)
    }
    pub fn allocate<T: Pod>(
        &mut self,
        mesh: &Mesh<T>,
        staging_belt: &mut StagingBelt,
        device: &Device,
        command_encoder: &mut CommandEncoder,
    ) -> GPUMesh {
        if mesh.is_empty() {
            return GPUMesh::empty();
        }
        let (vertex_size, index_size, _) = mesh.get_data_size();
        let total_size = vertex_size + index_size;
        let (bucket, size) = Self::get_data_index(total_size);
        let mut bucket = self.get_bucket(bucket);
        if let Some(mut buffer) = bucket.pop() {
            buffer.upload(mesh, device, staging_belt, command_encoder);
            buffer
        } else {
            /*println!(
                "had to allocate new buffer of size {}, for {}",
                size, total_size
            );*/
            GPUMesh::allocate(mesh, size, device)
        }
    }
    fn get_bucket(&mut self, id: usize) -> &mut Vec<GPUMesh> {
        if id >= self.buffers.len() {
            self.buffers.resize_with(id + 1, || Vec::new());
        }
        &mut self.buffers[id]
    }
    fn get_data_index(buffer_size: usize) -> (usize, usize) {
        let mut current_size = Self::MIN_BUFFER_SIZE;
        let mut i = 0;
        loop {
            if buffer_size <= current_size {
                return (i, current_size);
            }
            i += 1;
            current_size *= 4;
            if i > 20 {
                panic!("failsafe");
            }
        }
    }
}
impl Default for ClientGame {
    fn default() -> Self {
        Self {
            player_stats: EntityStats::default(),
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
                slot_action_prediction: HashMap::new(),
            },
            hit_timer: None,
            keys: InputContainer::default(),
            buttons: InputContainer::default(),
            cursor_position: PhysicalPosition::new(0., 0.),
            last_cursor_position: PhysicalPosition::new(0., 0.),
            wheel_scroll_delta: UIPos { x: 0., y: 0. },
            hotbar_slot: 0,
            item_variation: HashMap::new(),
            chunk_mesh_channels: std::sync::mpsc::channel(),
            viewmodel_player: AnimationPlayer::new(viewmodel_graph()),
            swap_hand_item: None,
            researched: HashSet::new(),
            stamina: 0.,
            place_message: None,
            chunk_mesh_queue_size: 0,
            server_ticks_passed: 0,
            chunk_buffer_pool: ChunkBufferPool::default(),
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
        device: &Device,
        queue: &Queue,
        camera: &ClientPlayer,
        entity_mesh: &mut BaseMesh,
        gui_mesh: &mut GUIMesh,
        viewmodel_mesh: &mut BaseMesh,
        damage_mesh: &mut DamageMesh,
        frustum: &Frustum,
        dt: f32,
        connection: &ClientConnection,
        world_animation_time: f32,
    ) {
        if self.held_item().is_none() {
            self.viewmodel_player.trigger("empty_hand");
        }
        if camera.running {
            self.viewmodel_player.trigger("run");
        }
        let (mut animation, observers) = self.viewmodel_player.evaluate(viewmodel_graph(), dt);
        if let Some(hit_timer) = &self.hit_timer {
            let weight = (((hit_timer.progress() - 0.5) * 2.).abs() - 0.6).max(0.) * 1.5;
            animation.iter_mut().for_each(|a| {
                a.weight *= weight;
            });
            animation.push(DrawAnimation {
                animation: "hit",
                time: hit_timer.progress() / 2.,
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
            Matrix4::from_translation(Vector3::from(
                camera.get_eye(self.get_player_data()).into_array(),
            )) * Matrix4::from_angle_y(Rad(-camera.direction.yaw))
                * Matrix4::from_angle_x(Rad(camera.direction.pitch)),
            &mut viewmodel_mesh.consumer(Color::WHITE),
            &animation,
            |binding, vc| match binding {
                "hand" => {
                    let (item, variant) = match self.swap_hand_item.as_ref() {
                        Some(item) => item,
                        None => return None,
                    };
                    let item_data = item.data();
                    match &item_data.action {
                        ItemAction::Place(item_block_placements) => {
                            let placement = &item_block_placements[*variant]; //todo: flash red when not enough items
                            if let Some(held_item) = self.held_item() {
                                if held_item.count < placement.use_count {
                                    vc.color = Color {
                                        r: 255,
                                        g: 200,
                                        b: 200,
                                        a: 255,
                                    };
                                }
                            }
                            Some(Cow::Owned(ItemModel::Block((placement.block))))
                        }
                        _ => Some(Cow::Borrowed(&item_data.model)),
                    }
                }
                _ => None,
            },
        );

        let mut i = 0;
        let ps = profiler::profiler_scope("upload chunks");
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
                    rayon::spawn(move || {
                        let (mesh, mesh_high_res, version) = ClientChunk::build_chunk_mesh(
                            modified_chunk,
                            build_data,
                            neighbor_chunks,
                        );
                        tx.send((modified_chunk, mesh, mesh_high_res, version));
                    });
                    self.chunk_mesh_queue_size += 1;
                    i += 1;
                    if i > 5 {
                        //break;
                    }
                }
            }
        }
        ps.end();
        for (id, entity) in &self.entities {
            if Some(*id) == self.player_entity {
                continue;
            }
            let lerp_time = (entity.update_timestamp.elapsed().as_secs_f32() / SERVER_DT).min(1.);
            let position = entity.previous_position.lerp(entity.position, lerp_time);
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
                |slot, _| {
                    entity
                        .hand_item
                        .as_ref()
                        .map(|item| Cow::Borrowed(&item.item.data().model))
                },
            );
        }

        let ps = profiler::profiler_scope("draw block component");
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
                for (offset, damage) in chunk.mesh_build_data.components.read().damage.iter() {
                    let base_block_position = (chunk_position.to_block_pos() + offset.xyz());
                    let base_position = base_block_position.to_pos();
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
                    let progress = damage.damage / block_data.health.health;
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
                                        let mut position = base_position + position;
                                        MeshVertex {
                                            position,
                                            normal: face.get_offset(),
                                            uv,
                                        }
                                    }),
                                );
                            }
                        }
                        BlockRenderData::Model {
                            model,
                            render_flags,
                            ..
                        } => {
                            let model_data = &model.model.data();
                            model_data.model.draw(
                                get_block_matrix(base_block_position, block.rotation),
                                &[],
                                |geometry| match geometry {
                                    ModelGeometry::Quad(vertices, texture) => {
                                        let (_, width, height) = model_data.model.textures[texture];
                                        mesh_vertex_consumer.add_quad(vertices.map(|vertex| {
                                            let mut position = vertex.position;
                                            if *render_flags & 1 != 0 {
                                                position.x += (world_animation_time * 0.8
                                                    + position.x * 0.2)
                                                    .sin()
                                                    * 0.1;
                                                position.z += (world_animation_time * 0.8
                                                    + 10.
                                                    + position.z * 0.2)
                                                    .sin()
                                                    * 0.1;
                                            }
                                            MeshVertex {
                                                position,
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
                for (offset, machine) in chunk.mesh_build_data.components.read().machine.iter() {
                    let blocks = chunk.mesh_build_data.blocks.read();
                    let block = blocks.get(offset.index()).unwrap();
                    let block_data = block.block.data();
                    if let Some(machine_data) = block_data.machine.as_ref() {
                        if let Some(machine_model) = machine_data.model.as_ref() {
                            let animation_time =
                                (self.server_ticks_passed - machine.animation_start_time) as f32
                                    * SERVER_DT;
                            let animation = match machine_data.model_animations.is_empty() {
                                true => None,
                                false => Some(DrawAnimation {
                                    animation: machine_data.model_animations
                                        [machine.animation as usize]
                                        .as_str(),
                                    time: animation_time,
                                    weight: 1.,
                                }),
                            };

                            draw_model(
                                machine_model,
                                get_block_matrix(
                                    chunk.position.to_block_pos() + offset.xyz(),
                                    block.rotation,
                                ),
                                &mut entity_mesh.consumer(Color::WHITE),
                                match animation.as_ref() {
                                    Some(animation) => std::slice::from_ref(animation),
                                    None => &[],
                                },
                                |_, _| None,
                            );
                        }
                    }
                }
            }
        }
        ps.end();

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
                                let entity_hitbox = entity
                                    .key
                                    .data()
                                    .hitbox(entity.crouching)
                                    .offset(entity.position);
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
                                    render::draw_block_model(
                                        place_block.block,
                                        get_block_matrix(block_position, rotation),
                                        &mut entity_mesh.consumer(if blocked {
                                            Color {
                                                r: 255,
                                                g: 100,
                                                b: 100,
                                                a: 150,
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
    pub fn tick_server(&mut self) {
        let dt = SERVER_DT;
        self.stamina += dt * self.player_stats.stamina_regen();
        self.stamina = self.stamina.min(self.player_stats.stamina());
        for (_, chunk) in &mut self.chunks {
            let blocks = chunk.mesh_build_data.blocks.read();
            for (offset, health) in chunk.mesh_build_data.components.write().damage.iter_mut() {
                let data = blocks.get(offset.index()).unwrap().block.data();
                health.damage -= dt * data.health.health_regen;
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
    crouching: bool,
}
struct ChunkMeshBuildData {
    pub blocks: RwLock<BlockPalette>,
    pub components: RwLock<ClientChunkBlockComponents>,
    pub version: AtomicU64,
}
pub struct ClientChunk {
    pub position: ChunkPos,
    pub mesh_build_data: Arc<ChunkMeshBuildData>,
    pub gpu_mesh: GPUMesh,
    pub gpu_mesh_high_res: GPUMesh,
    pub modified: bool,
    pub scheduled: bool,
}

impl ClientChunk {
    pub fn build_chunk_mesh(
        position: ChunkPos,
        chunk_data: Arc<ChunkMeshBuildData>,
        neighbor_chunks: FaceMap<Option<Arc<ChunkMeshBuildData>>>,
    ) -> (ChunkMesh, ChunkMesh, u64) {
        let mut mesh = ChunkMesh::default();
        let mut mesh_high_res = ChunkMesh::default();
        let chunk_blocks = chunk_data.blocks.read();
        let chunk_components = chunk_data.components.read();
        let neighbor_chunks =
            neighbor_chunks.map(|chunk| chunk.as_ref().map(|chunk| chunk.blocks.read()));

        for (offset, plants) in chunk_components.plant.iter() {
            let base_position = (position.to_block_pos() + offset.xyz()).to_pos();
            let mut mesh_vertex_consumer = mesh_high_res.consumer(BlockColor::default(), 0);
            for (plant, stage) in &plants.plants {
                let plant = plant.data();
                let position = base_position
                    + Pos {
                        x: 0.5,
                        y: 1.,
                        z: 0.5,
                    };
                let texture = plant.stages[*stage as usize].tex_coords();
                for blade in 0..plant.blades {
                    let first_angle =
                        f32::consts::PI * 2. * (blade as f32 / plant.blades as f32 + 0.25);
                    let second_angle = f32::consts::PI + first_angle;
                    let perpendicular = f32::consts::PI / 2. + first_angle;
                    let center_offset = Pos {
                        x: perpendicular.cos(),
                        y: 0.,
                        z: perpendicular.sin(),
                    };
                    /*let normal = Pos {
                        x: center_offset.x,
                        y: 1.,
                        z: center_offset.z,
                    }
                    .normalize();*/
                    let normal = Pos::Y;
                    let first = Pos {
                        x: first_angle.cos(),
                        y: 0.,
                        z: first_angle.sin(),
                    } * (plant.size / 2.)
                        + position
                        + center_offset * plant.center_offset;
                    let second = Pos {
                        x: second_angle.cos(),
                        y: 0.,
                        z: second_angle.sin(),
                    } * (plant.size / 2.)
                        + position
                        + center_offset * plant.center_offset;

                    let mut vertices = [
                        MeshVertex {
                            position: first,
                            uv: [texture.u1, texture.v2],
                            normal,
                        },
                        MeshVertex {
                            position: second,
                            uv: [texture.u2, texture.v2],
                            normal,
                        },
                        MeshVertex {
                            position: second + Pos::Y,
                            uv: [texture.u2, texture.v1],
                            normal,
                        },
                        MeshVertex {
                            position: first + Pos::Y,
                            uv: [texture.u1, texture.v1],
                            normal,
                        },
                    ];
                    for first in [true, false] {
                        mesh_vertex_consumer.add_quad(vertices.clone());
                        let vertex_count = mesh_vertex_consumer.mesh.vertices.len();
                        let start_index = (vertex_count - 2) - if first { 0 } else { 2 };
                        for vertex in
                            &mut mesh_vertex_consumer.mesh.vertices[start_index..start_index + 2]
                        {
                            vertex.flags = 1;
                        }
                        if first {
                            vertices.reverse();
                            for vertex in &mut vertices {
                                vertex.normal *= -1.;
                                vertex.normal.y *= -1.;
                            }
                        }
                    }
                }
            }
        }

        if chunk_blocks.unique_values() == 1 && chunk_blocks.get(0).unwrap().block == air_block() {
            return (
                mesh,
                mesh_high_res,
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
                    let guarantee_inside = x > 0
                        && x < CHUNK_SIZE as u8 - 1
                        && y > 0
                        && y < CHUNK_SIZE as u8 - 1
                        && z > 0
                        && z < CHUNK_SIZE as u8 - 1;
                    let mut get_neighbor = |neighbor_position: BlockPos| -> Option<BlockEntry> {
                        let (neighbor_chunk, neighbor_offset) = if guarantee_inside {
                            (
                                ChunkPos::all(0),
                                ChunkOffset::new(
                                    neighbor_position.x as u8,
                                    neighbor_position.y as u8,
                                    neighbor_position.z as u8,
                                ),
                            )
                        } else {
                            neighbor_position.to_chunk_pos_offset()
                        };
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
                            Some(*neighbor_chunk.get(neighbor_offset.index()).unwrap())
                        } else {
                            None
                        }
                    };
                    match &block_data.render_data {
                        BlockRenderData::Air => {}
                        BlockRenderData::Full { faces, .. } => {
                            let mut mesh_consumer = mesh.consumer(block.color, 0);
                            let base_position = Pos {
                                x: (position.x as f32 * CHUNK_SIZE as f32) + x as f32,
                                y: (position.y as f32 * CHUNK_SIZE as f32) + y as f32,
                                z: (position.z as f32 * CHUNK_SIZE as f32) + z as f32,
                            };
                            for face in Face::all() {
                                let neighbor_position = BlockPos {
                                    x: x as i32,
                                    y: y as i32,
                                    z: z as i32,
                                } + face.get_block_offset();
                                if let Some(neighbor_block) = get_neighbor(neighbor_position) {
                                    let neighbor_block_data = neighbor_block.block.data();
                                    match &neighbor_block_data.render_data {
                                        BlockRenderData::Air | BlockRenderData::Model { .. } => {}
                                        BlockRenderData::Full { faces, .. } => {
                                            continue;
                                        }
                                    }
                                }
                                let texture = faces
                                    .by_face(face)
                                    .tex_coords(f32::to_bits(
                                        base_position.x * base_position.y * base_position.z,
                                    ) as usize);
                                mesh_consumer.add_quad(face.get_vertices(texture, 0).map(
                                    |(position, uv)| MeshVertex {
                                        position: base_position + position,
                                        normal: face.get_offset(),
                                        uv,
                                    },
                                ));
                            }
                        }
                        BlockRenderData::Model {
                            model,
                            lod_hidden,
                            render_flags,
                            render_connections,
                            ..
                        } => {
                            render::draw_model(
                                model,
                                get_block_matrix(
                                    position.to_block_pos()
                                        + BlockPos {
                                            x: x as i32,
                                            y: y as i32,
                                            z: z as i32,
                                        },
                                    block.rotation,
                                ),
                                &mut {
                                    if *lod_hidden {
                                        &mut mesh_high_res
                                    } else {
                                        &mut mesh
                                    }
                                    .consumer(block.color, *render_flags)
                                },
                                &[],
                                |_, _| None,
                            );
                            for connection in render_connections {
                                for rotation in &connection.rotations {
                                    let final_rotation = block.rotation.compose(*rotation);
                                    let rotated_offset_face =
                                        final_rotation.rotate_face(connection.offset);
                                    let neighbor_position = BlockPos {
                                        x: x as i32,
                                        y: y as i32,
                                        z: z as i32,
                                    } + rotated_offset_face
                                        .get_block_offset();
                                    if let Some(neighbor_block) = get_neighbor(neighbor_position) {
                                        let neighbor_block_data = neighbor_block.block.data();
                                        let connectors = match &neighbor_block_data.render_data {
                                            BlockRenderData::Air => continue,
                                            BlockRenderData::Model {
                                                render_connectors, ..
                                            }
                                            | BlockRenderData::Full {
                                                render_connectors, ..
                                            } => render_connectors,
                                        };
                                        let connectors = connectors.by_face(
                                            neighbor_block.rotation.inverse_rotate_face(
                                                rotated_offset_face.opposite(),
                                            ),
                                        );
                                        if connection.contain.is_subset(connectors)
                                            && connection.deny.is_disjoint(connectors)
                                        {
                                            render::draw_model(
                                                &connection.model,
                                                get_block_matrix(
                                                    position.to_block_pos()
                                                        + BlockPos {
                                                            x: x as i32,
                                                            y: y as i32,
                                                            z: z as i32,
                                                        },
                                                    final_rotation,
                                                ),
                                                &mut {
                                                    if connection.lod_hidden {
                                                        &mut mesh_high_res
                                                    } else {
                                                        &mut mesh
                                                    }
                                                    .consumer(block.color, 0)
                                                },
                                                &[],
                                                |_, _| None,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        (
            mesh,
            mesh_high_res,
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

                    transport.update(delta_time_duration, &mut client).ok();
                    transport.send_packets(&mut client).ok();
                    if client.is_disconnected() {
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
mod profiler {
    use std::{
        cell::RefCell,
        time::{Duration, Instant},
    };

    thread_local! {
        pub static PROFILER: RefCell<Profiler> = const {RefCell::new(Profiler{entries: Vec::new()})};
    }
    struct Profiler {
        entries: Vec<(String, Duration)>,
    }
    pub struct ProfilerScope {
        name: String,
        time: Instant,
    }
    pub fn profiler_scope(name: impl ToString) -> ProfilerScope {
        ProfilerScope {
            name: name.to_string(),
            time: Instant::now(),
        }
    }
    pub fn profiler_consume() -> Vec<(String, Duration)> {
        let mut to_return = Vec::new();
        PROFILER.with_borrow_mut(|profiler| {
            std::mem::swap(&mut to_return, &mut profiler.entries);
        });
        to_return
    }
    impl ProfilerScope {
        pub fn end(self) {
            PROFILER.with_borrow_mut(|profiler| {
                profiler.entries.push((self.name, self.time.elapsed()));
            });
        }
    }
}
