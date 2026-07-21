use core::f32;
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{BinaryHeap, HashMap, HashSet},
    net::{SocketAddr, UdpSocket},
    sync::{Arc, atomic::AtomicU64},
    time::{Duration, Instant, SystemTime},
    u32,
};

use ahash::AHashMap;
use block_byte_common::{
    ACCELERATION_COEFFICIENT, CharacterController, ClientItem, Color, EntityAction, EntityPose,
    EntityStats, HitTimer, InternString, LookDirection, MoveMode, NORMAL_SPEED, SERVER_DT,
    TexCoords,
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, Pos, Ray, Vec3},
    model::{DrawAnimation, LoopMode, ModelGeometry},
    net::{ItemInteractTarget, NetworkMessageC2S, NetworkMessageS2C, make_connection_config},
    number_approach_smooth,
    registry::{
        BlockColor, BlockEntry, BlockInteractAction, BlockPalette, BlockRenderData, EntityData,
        EntityInteractAction, EntityKey, ItemAction, ItemKey, ItemModel, Key, ResearchKey,
        TextureKey, ToolData, TranslationLanguageData, air_block,
    },
    ui::PropertyMap,
    world::{ClientBlockComponentUpdate, ClientChunkBlockComponents},
};
use bytemuck::Pod;
use cgmath::{Matrix4, Rad, Vector3};
use parking_lot::{Mutex, RwLock};
use renet::RenetClient;
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};
use smallvec::SmallVec;
use uuid::Uuid;
use wgpu::{CommandEncoder, Device, Queue, util::StagingBelt};
use winit::{dpi::PhysicalPosition, event::MouseButton, keyboard::KeyCode};

use crate::{
    InputManager,
    atlas::{TexCoordsExt, TexCoordsIndexExt},
    game::clipping::Frustum,
    render::{
        self, BaseMesh, CameraUniform, ChunkMesh, DamageMesh, GPUMesh, GUIMesh, Mesh, MeshVertex,
        MeshVertexConsumer, RenderState, SurfaceError, draw_model, get_block_matrix,
        get_block_rotation_face_vertices,
    },
    ui::{ScreenData, UIMessage, UIPos, UIRect, render_screen, text_renderer},
};

use crate::GameScreen;

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
    pub walking: bool,
    pub crouching: bool,
    pub height_animation: f32,
}
impl Default for ClientPlayer {
    fn default() -> Self {
        ClientPlayer {
            position: Pos::ZERO,
            direction: LookDirection { pitch: 0., yaw: 0. },
            running: false,
            walking: false,
            controller: CharacterController::new(),
            crouching: false,
            height_animation: 0.,
        }
    }
}
impl ClientPlayer {
    pub const UP: cgmath::Vector3<f32> = cgmath::Vector3 {
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

        if let Some((block, position, face)) = ray.block_raycast(|block, _, _| {
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
                ray.aabb_raycast(entity_data.hitbox(entity.pose).offset(entity.position))
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
pub struct ModifiedChunkEntry {
    pub distance: usize,
    pub chunk: ChunkPos,
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
    pub hotbar_slot: usize,
    pub item_variation: HashMap<ItemKey, usize>,
    pub viewmodel_player: AnimationPlayer,
    pub current_local_action: Option<EntityAction>,
    pub swap_hand_item: Option<(ItemKey, usize)>,
    pub chunk_mesh_channels: (
        std::sync::mpsc::Sender<(ChunkPos, ChunkMesh, ChunkMesh, u64)>,
        std::sync::mpsc::Receiver<(ChunkPos, ChunkMesh, ChunkMesh, u64)>,
    ),
    pub researched: HashSet<ResearchKey>,
    pub stamina: f32,
    pub chunk_mesh_queue_size: usize,
    pub server_ticks_passed: u64,
    pub chunk_buffer_pool: ChunkBufferPool,
    pub is_attack_queued: bool,
    pub placement_visualize_toggled: bool,
    pub needs_equip: bool,
    pub camera: ClientPlayer,
    pub connection: ClientConnection,
    pub teleport_id: u32,
    pub mspt: f32,
    pub delta_time_average: f32,
}
impl GameScreen for ClientGame {
    fn render(
        &mut self,
        input: &crate::InputManager,
        renderer: &mut crate::render::RenderState,
        dt: f32,
        _screen_transition: &mut Option<Box<dyn GameScreen>>,
    ) {
        let mut local_player_mesh = Mesh::default();
        let mut entity_mesh = Mesh::default();
        let mut gui_mesh = Mesh::default();
        let mut viewmodel_mesh = Mesh::default();
        let mut damage_mesh = Mesh::default();
        let aspect_ratio = renderer.size().width as f32 / renderer.size().height as f32;
        if self.screen.is_none() {
            let sensitivity = 0.6;
            self.camera.update_orientation(
                -input.mouse_delta.y as f32 * sensitivity * 0.022,
                input.mouse_delta.x as f32 * sensitivity * 0.022,
            );
            let is_variant_selection = input.keys.is_down(KeyCode::KeyG);
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
            .into_iter()
            .enumerate()
            {
                if input.keys.is_just_down(key) {
                    if is_variant_selection {
                        if let Some(held_item) = self.held_item() {
                            let variation_count = held_item.item.data().action.variation_count();
                            if slot < variation_count {
                                let variation =
                                    self.item_variation.entry(held_item.item).or_insert(0);
                                if *variation != slot {
                                    self.needs_equip = true;
                                }
                                *variation = slot;
                            }
                        }
                    } else {
                        if self.hotbar_slot != slot {
                            self.hotbar_slot = slot;
                            self.send_message(NetworkMessageC2S::HotbarSelect { slot });
                            if self.held_item().is_some() || self.swap_hand_item.is_some() {
                                self.needs_equip = true;
                            }
                        }
                    }
                }
            }
            if input.keys.is_just_down(KeyCode::KeyE) {
                match self.camera.raycast(self, false) {
                    RayCastResult::Empty => {}
                    RayCastResult::Block(position, _) => {
                        let block = self.get_block(position).unwrap().block.data();
                        match &block.interact_action {
                            BlockInteractAction::Ignore => {}
                            _ => {
                                self.current_local_action = Some(EntityAction::Interact);
                                self.send_message(NetworkMessageC2S::InteractBlock { position });
                            }
                        }
                    }
                    RayCastResult::Entity(entity) => {
                        let entity_data = self.entities.get(&entity).unwrap().key.data();
                        match &entity_data.interact_action {
                            EntityInteractAction::Ignore => {}
                            _ => {
                                self.current_local_action = Some(EntityAction::Interact);
                                self.send_message(NetworkMessageC2S::InteractEntity { entity });
                            }
                        }
                    }
                    RayCastResult::Plant(position, index) => {
                        self.send_message(NetworkMessageC2S::HarvestPlant { position, index });
                    }
                }
            }
            if input.keys.is_just_down(KeyCode::KeyQ) {
                self.send_message(NetworkMessageC2S::DropItem {
                    stack: input.keys.is_down(KeyCode::ControlLeft),
                });
            }
            let scroll = input.wheel_scroll_delta.y;
            if scroll != 0. {
                if is_variant_selection {
                    if let Some(held_item) = self.held_item() {
                        let variation_count =
                            held_item.item.data().action.variation_count() as isize;
                        if variation_count > 1 {
                            let mut new_variant =
                                *self.item_variation.get(&held_item.item).unwrap_or(&0) as isize;
                            new_variant -= scroll as isize;
                            new_variant = ((new_variant % variation_count) + variation_count)
                                % variation_count;
                            self.item_variation
                                .insert(held_item.item, new_variant as usize);
                            self.needs_equip = true;
                        }
                    }
                } else {
                    let mut new_slot = self.hotbar_slot as isize;
                    new_slot += -scroll as isize;
                    new_slot = ((new_slot % 10) + 10) % 10;
                    self.hotbar_slot = new_slot as usize;
                    self.send_message(NetworkMessageC2S::HotbarSelect {
                        slot: new_slot as usize,
                    });
                    if self.held_item().is_some() || self.swap_hand_item.is_some() {
                        self.needs_equip = true;
                    }
                }
            }
            let crosshair_color = match self.camera.raycast(self, true) {
                RayCastResult::Empty => Color::grayscale(200),
                RayCastResult::Block(_, _) => Color::grayscale(255),
                RayCastResult::Entity(_) => Color::grayscale(255),
                RayCastResult::Plant(_, _) => unreachable!(),
            };
            let crosshair_texture = TextureKey::id("crosshair").unwrap();
            let crosshair_data = &*crosshair_texture.data().texture;
            let crosshair_size = 2.;
            let crosshair_size = UIPos {
                x: crosshair_data.width() as f32 / renderer.size().width as f32
                    * crosshair_size
                    * aspect_ratio,
                y: crosshair_data.height() as f32 / renderer.size().height as f32 * crosshair_size,
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
                crosshair_color,
            );

            if let Some(tooltip) = match self.camera.raycast(self, false) {
                RayCastResult::Empty => None,
                RayCastResult::Block(pos, _face) => self
                    .get_block(pos)
                    .unwrap()
                    .block
                    .data()
                    .interact_action
                    .tooltip(),
                RayCastResult::Entity(uuid) => self
                    .entities
                    .get(&uuid)
                    .unwrap()
                    .key
                    .data()
                    .interact_action
                    .tooltip(),
                RayCastResult::Plant(_, _) => Some("harvest"),
            } {
                let text = format!("[E]{}", language().translate(tooltip));
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
            if input.buttons.is_just_down(MouseButton::Left)
                && match &self.hit_timer {
                    Some(hit_timer) => hit_timer.current_time > hit_timer.current_time * 0.5,
                    None => true,
                }
            {
                self.is_attack_queued = true;
            }
        }
        if input.keys.is_just_down(KeyCode::Escape) {
            if let Some(screen) = &mut self.screen
                && screen.selected_slot.is_some()
            {
                screen.selected_slot = None;
            } else {
                self.send_message(NetworkMessageC2S::CloseUI);
            }
        }
        if input.keys.is_just_down(KeyCode::Tab) {
            self.send_message(if self.screen.is_some() {
                NetworkMessageC2S::CloseUI
            } else {
                NetworkMessageC2S::OpenPlayerInventory
            });
        }
        self.hud.properties.0.insert(
            InternString::intern("stamina_action"),
            match &self.hit_timer {
                Some(hit_timer) => {
                    let tool_data = self.active_tool();
                    let progress = hit_timer.progress();
                    tool_data.stamina * (1. - progress)
                }
                None => 0.,
            },
        );
        self.hud
            .properties
            .0
            .insert(InternString::intern("stamina"), self.stamina);
        self.hud
            .properties
            .0
            .insert(InternString::intern("hotbar_slot"), self.hotbar_slot as f32);
        self.chunk_buffer_pool.tick(&renderer.device);
        {
            let weight = 0.05;
            self.delta_time_average = weight * dt + (1. - weight) * self.delta_time_average;
        }
        let frustum = clipping::Frustum::from_matrix(
            CameraUniform::OPENGL_TO_WGPU_MATRIX
                * ClientPlayer::create_projection_matrix(
                    renderer.size().width as f32 / renderer.size().height as f32,
                    90.,
                )
                * self.camera.create_view_matrix(self.get_player_data()),
        );
        text_renderer().draw(
            UIPos {
                x: -aspect_ratio + 0.1,
                y: 0.9,
            },
            &format!(
                "{:.2} {:.2} {:.2} fps: {:.0}, queue: {}, pool: {}, mspt: {:.2} {}",
                self.player_position.x,
                self.player_position.y,
                self.player_position.z,
                1. / self.delta_time_average,
                self.chunk_mesh_queue_size,
                self.chunk_buffer_pool
                    .buffers
                    .iter()
                    .map(|p| p.len().to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                self.mspt,
                match self.camera.raycast(&self, true) {
                    RayCastResult::Block(position, face) =>
                        format!("looking at {:?} {:?} ", position, face),
                    _ => String::new(),
                },
            ),
            0.05,
            Color::WHITE,
            &mut gui_mesh,
        );
        render_screen(
            &mut self.hud,
            None,
            renderer.size(),
            &mut gui_mesh,
            dt,
            |_| {},
        );
        if let Some(screen) = &mut self.screen {
            render_screen(
                screen,
                Some(&input),
                renderer.size(),
                &mut gui_mesh,
                dt,
                |event| match event {
                    UIMessage::ServerMessage(message) => {
                        let _ = self.connection.tx.send(message);
                    }
                },
            );
        }
        if self.needs_equip {
            self.needs_equip = false;
            self.current_local_action = Some(EntityAction::Equip);
        }
        if self.current_local_action.is_none() {
            if self.is_attack_queued && self.hit_timer.is_none() {
                self.is_attack_queued = false;
                let stamina_cost = self.active_tool().stamina;
                if self.stamina >= stamina_cost {
                    self.stamina -= stamina_cost;
                    self.current_local_action = Some(EntityAction::Attack);
                    self.hit_timer = Some(HitTimer {
                        current_time: 0.,
                        swing_time: self.active_tool().swing_time
                            / (self.player_stats.haste() / 100.),
                    });
                }
            }
        }
        self.tick_camera(dt, input, self.screen.is_none());
        self.player_position = self.camera.position;
        self.send_message(NetworkMessageC2S::PlayerPosition {
            position: self.camera.position,
            teleport_id: self.teleport_id,
            direction: self.camera.direction,
            pose: match (
                self.camera.crouching,
                self.camera.walking,
                self.camera.running,
            ) {
                (false, _, true) => EntityPose::Run,
                (false, true, _) => EntityPose::Walk,
                (false, false, _) => EntityPose::Stand,
                (true, true, _) => EntityPose::CrouchWalk,
                (true, false, _) => EntityPose::Crouch,
            },
        });
        if let Some(hit_timer) = &mut self.hit_timer {
            if hit_timer.tick(dt) {
                match self.camera.raycast(self, true) {
                    RayCastResult::Block(position, face) => {
                        self.send_message(NetworkMessageC2S::AttackBlock { position, face });
                    }
                    RayCastResult::Entity(entity) => {
                        self.send_message(NetworkMessageC2S::AttackEntity { entity });
                    }
                    RayCastResult::Empty => {}
                    RayCastResult::Plant(_position, _index) => {
                        /*self.send_message(NetworkMessageC2S::HarvestPlant {
                            position,
                            index,
                            cut: true,
                        });*/
                    }
                }
            } else {
                if hit_timer.is_finished() {
                    self.hit_timer = None;
                }
            }
        }
        self.process_messages(renderer);
        self.tick_client(
            &renderer.device,
            &renderer.queue,
            &mut local_player_mesh,
            &mut entity_mesh,
            &mut gui_mesh,
            &mut viewmodel_mesh,
            &mut damage_mesh,
            &frustum,
            dt,
            renderer.animation_time,
            input,
        );
        let ps = profiler::profiler_scope("render");
        match renderer.render(
            self,
            entity_mesh,
            local_player_mesh,
            gui_mesh,
            viewmodel_mesh,
            damage_mesh,
            &frustum,
        ) {
            Ok(_) => {}
            Err(SurfaceError::Recreate) => renderer.resize(renderer.size()),
            Err(SurfaceError::Crash) => {
                panic!("crash");
            }
        }
        ps.end();
    }
    fn exit(&mut self) {
        *self.connection.state.lock() = ClientConnectionState::Disconnect;
    }
}
impl ClientGame {
    pub fn send_message(&mut self, message: NetworkMessageC2S) {
        let _ = self.connection.tx.send(message);
    }
    pub fn tick_camera(&mut self, dt: f32, input: &InputManager, can_move: bool) {
        let move_mode = if self.player_stats.flight() > 0. {
            MoveMode::Fly
        } else {
            MoveMode::Normal
        };
        self.camera.height_animation = number_approach_smooth(
            self.camera.height_animation,
            self.camera.position.y
                - if self.camera.crouching {
                    self.get_player_data()
                        .map(|data| data.crouch_height_difference)
                        .unwrap_or(0.)
                } else {
                    0.
                },
            40.,
            0.5,
            dt,
        );
        let Some(player_entity_data) = self.get_player_data() else {
            return;
        };
        let forward = cgmath::Vector3::new(
            self.camera.direction.yaw.sin(),
            0.,
            -self.camera.direction.yaw.cos(),
        );
        use cgmath::InnerSpace;
        let cross_normalized = forward.cross(ClientPlayer::UP).normalize();
        let move_vector = input.keys.down.iter().copied().fold(
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
                KeyCode::Space => vec + ClientPlayer::UP,
                KeyCode::ShiftLeft => vec - ClientPlayer::UP,
                _ => vec,
            },
        );
        let mut move_vector = Pos {
            x: move_vector.x,
            y: move_vector.y,
            z: move_vector.z,
        };
        if !can_move {
            move_vector = Pos::all(0.);
        }
        if !(move_vector.x == 0.0 && move_vector.z == 0.0) {
            let xz_mag = (move_vector.x.powi(2) + move_vector.z.powi(2)).sqrt();
            move_vector.x /= xz_mag;
            move_vector.z /= xz_mag;
        }
        move_vector *= self.player_stats.speed() / 100. * NORMAL_SPEED;
        //move_vector *= player_entity_data.speed;
        self.camera.running = input.keys.is_down(KeyCode::ControlLeft);
        self.camera.walking = move_vector.length_squared() > 0.;
        if move_vector.length_squared() == 0. {
            self.camera.running = false;
        }
        if self.camera.running {
            self.stamina -= dt * 15.;
            if self.stamina <= 0. {
                self.stamina = -1.;
                self.camera.running = false;
            }
        }
        move_vector *= if self.camera.running { 1.35 } else { 1. };
        self.camera.crouching = input.keys.is_down(KeyCode::ShiftLeft) && can_move;
        match move_mode {
            MoveMode::Normal | MoveMode::Fly => {
                if !self.camera.crouching
                    && CharacterController::collides_at(
                        self.camera.position,
                        &|block| {
                            let (chunk, offset) = block.to_chunk_pos_offset();
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
                        },
                        player_entity_data.hitbox(EntityPose::Stand),
                    )
                    .is_some()
                {
                    self.camera.crouching = true;
                }
            }
            MoveMode::NoClip => {}
        }
        match move_mode {
            MoveMode::Normal => {
                if self.camera.crouching {
                    move_vector /= 2.;
                }
                if input.keys.is_down(KeyCode::Space)
                    && self.camera.controller.on_ground
                    && can_move
                {
                    self.camera.controller.velocity.y += self.player_stats.jump_velocity();
                }
            }
            MoveMode::Fly | MoveMode::NoClip => {}
        }
        self.camera.controller.tick(
            &mut self.camera.position,
            dt,
            |block| {
                let (chunk, offset) = block.to_chunk_pos_offset();
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
            },
            move_vector,
            move_mode,
            player_entity_data.hitbox(if self.camera.crouching {
                EntityPose::Crouch
            } else {
                EntityPose::Stand
            }),
            ACCELERATION_COEFFICIENT * self.player_stats.speed() / 100. * NORMAL_SPEED,
            0.5,
            input.keys.is_down(KeyCode::ShiftLeft),
        );
    }
    pub fn process_messages(&mut self, renderer: &mut RenderState) {
        while let Ok((message, time)) = self.connection.rx.try_recv() {
            match message {
                NetworkMessageS2C::LoadChunk {
                    position,
                    blocks,
                    components,
                } => {
                    self.chunks.insert(
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
                    self.mark_modified(position);
                    for face in Face::all() {
                        self.mark_modified(position + face.get_chunk_offset());
                    }
                }
                NetworkMessageS2C::UnloadChunk { position } => {
                    if let Some(chunk) = self.chunks.remove(&position) {
                        self.chunk_buffer_pool.reclaim(chunk.gpu_mesh);
                        self.chunk_buffer_pool.reclaim(chunk.gpu_mesh_high_res);
                    }
                }
                NetworkMessageS2C::SetBlock { position, block } => {
                    let (chunk, offset) = position.to_chunk_pos_offset();
                    {
                        let chunk = self.chunks.get(&chunk).unwrap();
                        chunk
                            .mesh_build_data
                            .blocks
                            .write()
                            .set(offset.index(), &block);
                    }
                    self.mark_modified(chunk);
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
                            self.mark_modified(chunk + face.get_chunk_offset());
                        }
                    }
                }
                NetworkMessageS2C::GameTick { ticks_passed, mspt } => {
                    self.server_ticks_passed = ticks_passed;
                    self.mspt = mspt;
                    self.tick_server();
                }
                NetworkMessageS2C::AddEntity {
                    uuid,
                    key,
                    position,
                    direction,
                    hand_item,
                    pose,
                } => {
                    self.entities.insert(
                        uuid,
                        ClientEntity {
                            key,
                            position,
                            direction,
                            previous_position: position,
                            previous_direction: direction,
                            update_timestamp: Instant::now(),
                            hand_item,
                            pose,
                            pose_player: AnimationPlayer::new(pose.base_animation()),
                            action_player: AnimationPlayer::new("empty"),
                        },
                    );
                }
                NetworkMessageS2C::MoveEntity {
                    uuid,
                    position,
                    direction,
                    pose,
                } => {
                    if let Some(entity) = self.entities.get_mut(&uuid) {
                        entity.previous_position = entity.position;
                        entity.previous_direction = entity.direction;
                        entity.update_timestamp = time;
                        entity.position = position;
                        entity.direction = direction;
                        if entity.pose != pose {
                            entity.pose = pose;
                            entity
                                .pose_player
                                .play_animation(pose.base_animation(), 0.1);
                        }
                    }
                }
                NetworkMessageS2C::RemoveEntity { uuid } => {
                    self.entities.remove(&uuid);
                }
                NetworkMessageS2C::UpdateBlockComponents {
                    chunk: chunk_position,
                    offset,
                    update: data,
                } => {
                    match data {
                        ClientBlockComponentUpdate::ClientBlockPlants(..) => {
                            self.mark_modified(chunk_position);
                        }
                        _ => {}
                    }
                    if let Some(chunk) = self.chunks.get_mut(&chunk_position) {
                        data.update(offset, &mut *chunk.mesh_build_data.components.write());
                    }
                }
                NetworkMessageS2C::SetPlayerEntity { uuid } => {
                    self.player_entity = uuid;
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
                    self.player_stats = stats;
                }
                NetworkMessageS2C::UIOpen {
                    screen,
                    slots,
                    properties,
                } => {
                    self.screen = Some(ScreenData {
                        screen,
                        slots,
                        properties,
                        selected_slot: None,
                        slot_action_prediction: HashMap::new(),
                        element_data: HashMap::new(),
                        time: 0.,
                    });
                    renderer.window().set_cursor_visible(true);
                    let size = renderer.size();
                    let _ = renderer.window().set_cursor_position(PhysicalPosition::new(
                        size.width / 2,
                        size.height / 2,
                    ));
                }
                NetworkMessageS2C::UISetSlot { slot, item } => {
                    if let Some(screen) = &mut self.screen {
                        if slot < screen.slots.len() {
                            screen.slots[slot] = item;
                            screen.slot_action_prediction.remove(&slot);
                        }
                    }
                }
                NetworkMessageS2C::UIClose => {
                    self.screen = None;
                    renderer.window().set_cursor_visible(false);
                }
                NetworkMessageS2C::HUDSlot { slot, item } => {
                    if self.hotbar_slot == slot
                        && self.hud.slots[slot].as_ref().map(|item| item.item)
                            != item.as_ref().map(|item| item.item)
                    {
                        self.needs_equip = true;
                    }
                    self.hud.slots[slot] = item;
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
                    if let Some(entity) = self.entities.get_mut(&uuid) {
                        entity.hand_item = item;
                    }
                }
                NetworkMessageS2C::Knockback { velocity } => {
                    self.camera.controller.velocity += velocity;
                }
                NetworkMessageS2C::UpdateResearch { research } => {
                    self.researched = research;
                }
                NetworkMessageS2C::HudBarUpdate { health } => {
                    self.hud
                        .properties
                        .0
                        .insert(InternString::intern("health"), health);
                }
                NetworkMessageS2C::UISetProperty { property, value } => {
                    if let Some(screen) = &mut self.screen {
                        screen
                            .properties
                            .0
                            .insert(InternString::intern(&property), value);
                    }
                }
                NetworkMessageS2C::EntityAction { entity, action } => {
                    if let Some(entity) = self.entities.get_mut(&entity) {
                        entity.action_player.play_animation(action.animation(), 0.1);
                    }
                }
            }
        }
        if self.connection.state() == ClientConnectionState::Disconnected {
            panic!("disconnect");
        }
    }
}
#[derive(Default)]
pub struct ChunkBufferPool {
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
        let bucket = self.get_bucket(bucket);
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
impl ClientGame {
    pub fn new(connection: ClientConnection) -> ClientGame {
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
                element_data: HashMap::new(),
                time: 0.,
            },
            hit_timer: None,
            hotbar_slot: 0,
            item_variation: HashMap::new(),
            chunk_mesh_channels: std::sync::mpsc::channel(),
            viewmodel_player: AnimationPlayer::new("idle"),
            swap_hand_item: None,
            researched: HashSet::new(),
            stamina: 0.,
            chunk_mesh_queue_size: 0,
            server_ticks_passed: 0,
            chunk_buffer_pool: ChunkBufferPool::default(),
            is_attack_queued: false,
            placement_visualize_toggled: false,
            current_local_action: None,
            needs_equip: false,
            camera: ClientPlayer::default(),
            connection,
            delta_time_average: 0.,
            mspt: 0.,
            teleport_id: 0,
        }
    }
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
    pub fn active_tool(&self) -> &ToolData {
        self.held_item()
            .as_ref()
            .and_then(|item| item.item.data().tool.as_ref())
            .unwrap_or(&ToolData::HAND)
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
        _device: &Device,
        _queue: &Queue,
        _local_player_mesh: &mut BaseMesh,
        entity_mesh: &mut BaseMesh,
        _gui_mesh: &mut GUIMesh,
        viewmodel_mesh: &mut BaseMesh,
        damage_mesh: &mut DamageMesh,
        frustum: &Frustum,
        dt: f32,
        world_animation_time: f32,
        input: &InputManager,
    ) {
        let viewmodel = self
            .get_player_data()
            .and_then(|data| data.viewmodel.as_ref());
        let mut viewmodel_animations: Vec<DrawAnimation<'static>> = Vec::new();
        let (animation, time) = self.viewmodel_player.get_animation();
        let idle_or_run = if self.camera.running {
            "running"
        } else {
            "idle"
        };
        if let Some(action) = self.current_local_action {
            match animation {
                "idle" | "running" => {
                    self.viewmodel_player
                        .play_animation(action.animation(), 0.1);
                }
                "hit" => {
                    let swing_time = self
                        .hit_timer
                        .as_ref()
                        .map(|hit_timer| hit_timer.swing_time)
                        .unwrap_or(0.);
                    if time >= swing_time {
                        match self.current_local_action {
                            Some(EntityAction::Attack) => {
                                self.current_local_action = None;
                            }
                            _ => {}
                        }
                        self.viewmodel_player.play_animation(idle_or_run, 0.1);
                    }
                }
                _ => {
                    if time >= 0.25 {
                        self.current_local_action = None;
                        self.viewmodel_player.play_animation(idle_or_run, 0.1);
                    }
                    if time >= 0.1 {
                        self.swap_hand_item = match self.held_item() {
                            Some(item) => {
                                let item = item.item;
                                Some((item, self.item_variation.get(&item).cloned().unwrap_or(0)))
                            }
                            None => None,
                        };
                    }
                }
            }
        } else {
            match animation {
                "idle" => {
                    if self.camera.running {
                        self.viewmodel_player.play_animation("running", 0.1);
                    } else if time > 4. {
                        self.viewmodel_player.restart_animation();
                    }
                }
                "running" => {
                    if !self.camera.running {
                        self.viewmodel_player.play_animation("idle", 0.1);
                    } else if time > 1. {
                        self.viewmodel_player.restart_animation();
                    }
                }
                _ => {}
            }
        }
        for entry in &mut viewmodel_animations {
            if entry.animation == "hit" {
                let swing_time = self
                    .hit_timer
                    .as_ref()
                    .map(|hit_timer| hit_timer.swing_time)
                    .unwrap_or(1.);
                entry.time = entry.time / swing_time * 0.58;
            }
        }

        self.viewmodel_player.tick(dt, &mut viewmodel_animations);

        if let Some(viewmodel) = viewmodel {
            render::draw_model(
                viewmodel,
                Matrix4::from_translation(Vector3::from(
                    self.camera.get_eye(self.get_player_data()).into_array(),
                )) * Matrix4::from_angle_y(Rad(-self.camera.direction.yaw))
                    * Matrix4::from_angle_x(Rad(self.camera.direction.pitch)),
                &mut viewmodel_mesh.consumer(Color::WHITE),
                &viewmodel_animations[..],
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
                                Some(Cow::Owned(ItemModel::Block(placement.block)))
                            }
                            _ => Some(Cow::Borrowed(&item_data.model)),
                        }
                    }
                    _ => None,
                },
            );
        }

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
                        tx.send((modified_chunk, mesh, mesh_high_res, version))
                            .unwrap();
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
        for (id, entity) in &mut self.entities {
            let mut animations: SmallVec<[DrawAnimation<'static>; 8]> = SmallVec::new();
            entity.action_player.tick(dt, &mut animations);
            match entity.action_player.get_animation() {
                ("empty", _) => {}
                (_, time) => {
                    if time > 1. {
                        entity.action_player.play_animation("empty", 0.1);
                    }
                }
            }
            let model = &entity.key.data().model;
            let (pose_animation, pose_time) = entity.pose_player.get_animation();
            match model.model.data().model.get_animation_info(pose_animation) {
                Some(info) => {
                    if pose_time >= info.length {
                        entity.pose_player.restart_animation();
                    }
                }
                None => {}
            }
            entity.pose_player.tick(dt, &mut animations);
            if Some(*id) == self.player_entity {
                /*render::draw_model(
                    model,
                    Matrix4::from_translation(Vector3::new(
                        self.camera.position.x,
                        self.camera.position.y,
                        self.camera.position.z,
                    )) * Matrix4::from_angle_y(Rad(-self.camera.direction.yaw)),
                    &mut local_player_mesh.consumer(Color::WHITE),
                    &animations[..],
                    |_, _| {
                        entity
                            .hand_item
                            .as_ref()
                            .map(|item| Cow::Borrowed(&item.item.data().model))
                    },
                );*/
            } else {
                let lerp_time =
                    (entity.update_timestamp.elapsed().as_secs_f32() / SERVER_DT).min(1.);
                let position = entity.previous_position.lerp(entity.position, lerp_time);
                let rotation = -block_byte_common::coord::lerp_number(
                    entity.previous_direction.yaw,
                    entity.direction.yaw,
                    lerp_time,
                );
                render::draw_model(
                    model,
                    Matrix4::from_translation(Vector3::new(position.x, position.y, position.z))
                        * Matrix4::from_angle_y(Rad(rotation)),
                    &mut entity_mesh.consumer(Color::WHITE),
                    &animations[..],
                    |_, _| {
                        entity
                            .hand_item
                            .as_ref()
                            .map(|item| Cow::Borrowed(&item.item.data().model))
                    },
                );
            }
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
                    let base_block_position = chunk_position.to_block_pos() + offset.xyz();
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
                                        let position = base_position + position;
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
                                    ModelGeometry::Triangle(_vertices, _texture) => todo!(),
                                },
                                |_matrix, _binding| {},
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
                                false => {
                                    let mut animation = machine_data.model_animations
                                        [machine.animation as usize]
                                        .as_str();
                                    let mut time = animation_time;
                                    let model_data = &machine_model.model.data().model;
                                    let animation_info =
                                        model_data.get_animation_info(animation).unwrap();

                                    if time > animation_info.length {
                                        match animation_info.loop_mode {
                                            LoopMode::Once => {
                                                animation =
                                                    machine_data.model_animations[0].as_str();
                                                time -= animation_info.length;
                                                time %= model_data
                                                    .get_animation_info(animation)
                                                    .unwrap()
                                                    .length;
                                            }
                                            LoopMode::Hold => {
                                                time = animation_info.length;
                                            }
                                            LoopMode::Loop => {
                                                time %= animation_info.length;
                                            }
                                        }
                                    }
                                    Some(DrawAnimation {
                                        animation,
                                        time,
                                        weight: 1.,
                                    })
                                }
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

        if input.keys.is_just_down(KeyCode::AltLeft) {
            self.placement_visualize_toggled ^= true;
        }

        if let Some(held_item) = self.held_item()
            && self.screen.is_none()
        {
            let variant_id = self
                .item_variation
                .get(&held_item.item)
                .cloned()
                .unwrap_or(0);
            let raycast = self.camera.raycast(self, true);
            match &held_item.item.data().action {
                ItemAction::Place(place_block) => match raycast {
                    RayCastResult::Block(position, face) => {
                        let place_block = &place_block[variant_id];
                        let block_position = position + face.get_block_offset();
                        let block_data = place_block.block.data();
                        let mut blocked = place_block.use_count > held_item.count;
                        let rotation = block_data
                            .rotation
                            .from_look_direction(self.camera.direction, face);
                        let fake_block_entry = BlockEntry {
                            block: place_block.block,
                            rotation,
                            color: BlockColor::default(),
                        };
                        for entity in self.entities.values() {
                            let entity_hitbox = entity
                                .key
                                .data()
                                .hitbox(entity.pose)
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
                            match self.get_block(block_position + world_hanging.get_block_offset())
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
                                if self.placement_visualize_toggled {
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
                                if input.buttons.is_just_down(MouseButton::Right)
                                    && self.screen.is_none()
                                    && !blocked
                                    && self.hit_timer.is_none()
                                {
                                    self.current_local_action = Some(EntityAction::Place);
                                    self.viewmodel_player
                                        .play_animation(EntityAction::Place.animation(), 0.1);
                                    self.send_message(NetworkMessageC2S::ItemInteraction {
                                        target: ItemInteractTarget::Block { position, face },
                                        variant: variant_id,
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                },
                ItemAction::Plant(_) | ItemAction::RotateBlock | ItemAction::SpawnEntity(_) => {
                    match raycast {
                        RayCastResult::Block(position, face) => {
                            if input.buttons.is_just_down(MouseButton::Right) {
                                self.current_local_action = Some(EntityAction::Interact);
                                self.send_message(NetworkMessageC2S::ItemInteraction {
                                    target: ItemInteractTarget::Block { position, face },
                                    variant: variant_id,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                ItemAction::Ignore => {}
                ItemAction::Consume { .. } => {
                    if input.buttons.is_just_down(MouseButton::Right) {
                        self.current_local_action = Some(EntityAction::Interact);
                        self.send_message(NetworkMessageC2S::ItemInteraction {
                            target: match raycast {
                                RayCastResult::Empty => ItemInteractTarget::Empty,
                                RayCastResult::Block(position, face) => {
                                    ItemInteractTarget::Block { position, face }
                                }
                                RayCastResult::Entity(entity) => {
                                    ItemInteractTarget::Entity { entity }
                                }
                                RayCastResult::Plant(_, _) => unreachable!(),
                            },
                            variant: variant_id,
                        });
                    }
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
    pose: EntityPose,
    action_player: AnimationPlayer,
    pose_player: AnimationPlayer,
}
pub struct ChunkMeshBuildData {
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
                    let get_neighbor = |neighbor_position: BlockPos| -> Option<BlockEntry> {
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
                                        BlockRenderData::Full { .. } => {
                                            continue;
                                        }
                                    }
                                }
                                let (vertices, local_face) =
                                    get_block_rotation_face_vertices(block.rotation, face);
                                let face_texture = faces.by_face(*local_face);
                                let tex_index = if face_texture.variant_count() > 1 {
                                    let hash = (base_position.x as i32 * 94839)
                                        ^ (base_position.y as i32 * 532)
                                        ^ (base_position.z as i32 * 5473);
                                    let hash = hash * hash * 957548 + hash * 344;
                                    (hash >> 6) as usize
                                } else {
                                    0
                                };
                                let texture = face_texture.tex_coords(tex_index);
                                mesh_consumer.add_quad(vertices.map(|vertex| MeshVertex {
                                    position: vertex.position + base_position,
                                    normal: vertex.normal,
                                    uv: texture.map(vertex.uv),
                                }));
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
pub fn language() -> &'static TranslationLanguageData {
    Key::<TranslationLanguageData>::id("en").unwrap().data()
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

pub struct ClientConnection {
    rx: std::sync::mpsc::Receiver<(NetworkMessageS2C, Instant)>,
    tx: std::sync::mpsc::Sender<NetworkMessageC2S>,
    pub state: Arc<Mutex<ClientConnectionState>>,
}
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ClientConnectionState {
    Connecting,
    Connected,
    Disconnect,
    Disconnected,
}
impl ClientConnection {
    pub fn connect(server_addr: SocketAddr) -> ClientConnection {
        let (s2c_tx, s2c_rx) = std::sync::mpsc::channel();
        let (c2s_tx, c2s_rx) = std::sync::mpsc::channel();
        let state = Arc::new(Mutex::new(ClientConnectionState::Connecting));
        {
            let state = state.clone();
            std::thread::spawn(move || {
                let mut client = RenetClient::new(make_connection_config());
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
                            for message in
                                std::iter::from_fn(|| client.borrow_mut().receive_message(0)).chain(
                                    std::iter::from_fn(|| client.borrow_mut().receive_message(1)),
                                )
                            {
                                let mut message = &message[..];
                                let mut rdr = lz4_flex::frame::FrameDecoder::new(&mut message);
                                let message: NetworkMessageS2C =
                                    serde_cbor::from_reader(&mut rdr).unwrap();
                                s2c_tx.send((message, Instant::now())).unwrap();
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
pub mod profiler {
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
pub struct AnimationPlayer {
    current: AnimationPlayerEntry,
    previous_animations: Vec<AnimationPlayerEntry>,
}
struct AnimationPlayerEntry {
    animation: &'static str,
    animation_time: f32,
    interpolation_time: f32,
    total_interpolation_time: f32,
}
impl AnimationPlayerEntry {
    pub fn interpolation_progress(&self) -> f32 {
        if self.total_interpolation_time == 0. {
            1.
        } else {
            self.interpolation_time / self.total_interpolation_time
        }
    }
}
impl AnimationPlayer {
    pub fn new(current_animation: &'static str) -> AnimationPlayer {
        AnimationPlayer {
            current: AnimationPlayerEntry {
                animation: current_animation,
                animation_time: 0.,
                interpolation_time: 0.,
                total_interpolation_time: 0.,
            },
            previous_animations: Vec::new(),
        }
    }
    pub fn play_animation(&mut self, animation: &'static str, interpolation_time: f32) {
        let mut new_entry = AnimationPlayerEntry {
            animation,
            animation_time: 0.,
            interpolation_time: 0.,
            total_interpolation_time: interpolation_time,
        };
        std::mem::swap(&mut new_entry, &mut self.current);

        new_entry.interpolation_time = interpolation_time * new_entry.interpolation_progress();
        new_entry.total_interpolation_time = interpolation_time;

        self.previous_animations.push(new_entry);
    }
    pub fn get_animation(&self) -> (&'static str, f32) {
        (self.current.animation, self.current.animation_time)
    }
    pub fn restart_animation(&mut self) {
        self.current.animation_time = 0.;
    }
    pub fn tick(&mut self, dt: f32, output: &mut impl Extend<DrawAnimation<'static>>) {
        self.current.animation_time += dt;
        self.current.interpolation_time =
            (self.current.interpolation_time + dt).min(self.current.total_interpolation_time);
        self.previous_animations.retain_mut(|entry| {
            entry.animation_time += dt;
            entry.interpolation_time -= dt;
            entry.interpolation_time > 0.
        });
        output.extend(
            std::iter::once(&self.current)
                .chain(self.previous_animations.iter())
                .map(|entry| DrawAnimation {
                    animation: entry.animation,
                    time: entry.animation_time,
                    weight: entry.interpolation_progress(),
                }),
        );
    }
}
