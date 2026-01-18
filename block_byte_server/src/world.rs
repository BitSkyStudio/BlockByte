use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    path::Path,
    sync::{OnceLock, atomic::AtomicBool},
};

use block_byte_common::{
    InventoryView, LookDirection,
    coord::{AABB, CHUNK_SIZE, ChunkOffset, ChunkPos, Pos},
    net::NetworkMessageS2C,
    registry::{
        BiomeKey, BlockData, BlockInteractAction, BlockKey, BlockPalette, EntityInteractAction,
        EntityKey, PlantKey, air_block,
    },
    world::{
        BlockComponentStorage, ClientBlockComponentUpdate, ClientBlockDamage, ClientBlockPlants,
        ClientChunkBlockComponents,
    },
};
use noise::{BasicMulti, NoiseFn, Perlin};
use palettevec::{PaletteVec, index_buffer::AlignedIndexBuffer, palette::HybridPalette};
use parking_lot::{Mutex, RwLock};
use rand::{Rng, rngs::StdRng};
use rand_seeder::Seeder;
use serde::{Deserialize, Serialize};
use slotmap::new_key_type;
use smallvec::SmallVec;
use splines::{Interpolation, Spline};
use uuid::Uuid;

use crate::{
    InventoryProvider, Server, UserIndex,
    inventory::{Inventory, ItemStack, generate_loot_table, lock_inventories},
    registry::{Key, RegistryConfigLoadable},
};
#[derive(Serialize, Deserialize)]
pub struct ChunkSaveData {
    pub blocks: BlockPalette,
    pub block_events: Vec<(ChunkOffset, BlockEvent)>,
    pub components: ChunkBlockComponents,
    pub entities: Vec<Entity>,
}
pub struct Chunk {
    pub position: ChunkPos,
    pub blocks: RwLock<BlockPalette>,
    pub viewers: HashSet<UserIndex>,
    pub block_events: Mutex<Vec<(ChunkOffset, BlockEvent)>>,
    pub components: ChunkBlockComponents,
    pub entities: Vec<EntityIndex>,
}
impl Chunk {
    pub fn generate(position: ChunkPos) -> Chunk {
        let seed = 1;

        /*use noise::MultiFractal;
        let height_noise: BasicMulti<Perlin> = BasicMulti::new(seed)
            .set_octaves(4)
            .set_frequency(1.0)
            .set_lacunarity(2.0)
            .set_persistence(0.5);*/
        let height_noise = Perlin::new(seed);

        let mut blocks = BlockPalette::filled(
            air_block(),
            CHUNK_SIZE as usize * CHUNK_SIZE as usize * CHUNK_SIZE as usize,
        );
        let mut components = ChunkBlockComponents::default();
        let mut height_map = [[0; CHUNK_SIZE as usize]; CHUNK_SIZE as usize];
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block_x = x as i32 + position.x as i32 * CHUNK_SIZE as i32;
                let block_z = z as i32 + position.z as i32 * CHUNK_SIZE as i32;
                let mountain_spline = Spline::from_vec(vec![
                    splines::Key::new(-1., 60., Interpolation::Linear),
                    splines::Key::new(0., 80., Interpolation::Linear),
                    splines::Key::new(0.5, 100., Interpolation::Cosine),
                    splines::Key::new(1., 200., Interpolation::Linear),
                ]);
                let mountain_height = height_noise
                    .get([block_x as f64 / 500., block_z as f64 / 500.])
                    .clamp(-0.99, 0.99);
                let small_spline = Spline::from_vec(vec![
                    splines::Key::new(-1., -3., Interpolation::Linear),
                    splines::Key::new(1., 3., Interpolation::Linear),
                ]);
                let small_noise = height_noise
                    .get([block_x as f64 / 30., block_z as f64 / 30.])
                    .clamp(-0.99, 0.99);
                let height = (mountain_spline.sample(mountain_height).unwrap()
                    + small_spline.sample(small_noise).unwrap());
                height_map[x as usize][z as usize] = height as i32;
            }
        }
        let biome = BiomeKey::id("forest").unwrap().data();
        use rand::SeedableRng;
        let mut rng = StdRng::from_seed(Seeder::from((seed, position)).make_seed());
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let offset = ChunkOffset::new(x, y, z);
                    let y_pos = y as i32 + position.y as i32 * CHUNK_SIZE as i32;
                    let height = height_map[x as usize][z as usize];
                    if y_pos == height {
                        blocks.set(offset.index(), &biome.top_block);
                        let spawned_plants: Vec<_> = biome
                            .plants
                            .iter()
                            .filter_map(|spawner| {
                                if rng.random_bool(spawner.chance as f64) {
                                    Some((spawner.plant, 0.))
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if !spawned_plants.is_empty() {
                            components.plant.write().set(
                                offset,
                                BlockPlants {
                                    plants: spawned_plants,
                                },
                            );
                        }
                    } else if y_pos < height - 3 {
                        blocks.set(offset.index(), &biome.bottom_block);
                    } else if y_pos < height {
                        blocks.set(offset.index(), &biome.middle_block);
                    }
                }
            }
        }
        //blocks.set(ChunkOffset::new(16, 16, 16).index(), &grass);
        Chunk {
            position,
            blocks: RwLock::new(blocks),
            viewers: HashSet::new(),
            block_events: Mutex::new(Vec::new()),
            components,
            entities: Vec::new(),
        }
    }
    pub fn tick(&self, server: &Server) {
        let mut processing_events = Vec::new();
        std::mem::swap(&mut processing_events, &mut *self.block_events.lock());
        for (block, event) in processing_events {
            match event {
                BlockEvent::Damage { damage } => {
                    let block_data = self.blocks.read().get(block.index()).unwrap().data();
                    if let Some(health) = &block_data.health {
                        let mut damage_component = self.components.damage.write();
                        let mut destroy = false;

                        if damage >= health.health {
                            destroy = true;
                        } else if let Some(block_damage) = damage_component.get_mut(block) {
                            block_damage.damage += damage;
                            if block_damage.damage >= health.health {
                                destroy = true;
                            }
                        } else {
                            damage_component.set(block, BlockDamage { damage });
                        }
                        if destroy {
                            damage_component.remove(block);
                            self.blocks.write().set(block.index(), &air_block());
                            if block_data.plantable {
                                if self.components.plant.write().remove(block).is_some() {
                                    server.send_message_multiple(
                                        self.viewers.iter(),
                                        NetworkMessageS2C::UpdateBlockComponents {
                                            chunk: self.position,
                                            offset: block,
                                            data: Option::<ClientBlockPlants>::None.into(),
                                        },
                                    );
                                }
                            }
                            let block_pos = self.position.to_block_pos() + block.xyz();
                            for item in generate_loot_table(block_data.loot_table.data()) {
                                server.spawn_item(
                                    item,
                                    block_pos.to_pos()
                                        + Pos {
                                            x: 0.5,
                                            y: 0.5,
                                            z: 0.5,
                                        },
                                );
                            }
                            if let Some(_) = &block_data.machine {
                                let machine =
                                    self.components.machine.write().remove(block).unwrap();
                                for item in machine.inventory.into_inner().items {
                                    if let Some(item) = item {
                                        server.spawn_item(
                                            item,
                                            block_pos.to_pos()
                                                + Pos {
                                                    x: 0.5,
                                                    y: 0.5,
                                                    z: 0.5,
                                                },
                                        );
                                    }
                                }
                            }
                            server.send_message_multiple(
                                self.viewers.iter(),
                                NetworkMessageS2C::SetBlock {
                                    position: block_pos,
                                    block: air_block(),
                                },
                            );
                        }
                        server.send_message_multiple(
                            self.viewers.iter(),
                            NetworkMessageS2C::UpdateBlockComponents {
                                chunk: self.position,
                                offset: block,
                                data: damage_component
                                    .get(block)
                                    .map(|component| Into::<ClientBlockDamage>::into(component))
                                    .into(),
                            },
                        );
                    }
                }
                BlockEvent::PlayerInteract { user } => {
                    let block_data = self.blocks.read().get(block.index()).unwrap().data();
                    let block_position = self.position.to_block_pos() + block.xyz();
                    match &block_data.interact_action {
                        BlockInteractAction::Ignore => {}
                        BlockInteractAction::OpenInventory { screen, view } => {
                            if let Some(user) = server.get_user(user.0) {
                                if let Some(player) = user.entity {
                                    user.set_screen(
                                        *screen,
                                        vec![
                                            (
                                                InventoryProvider::Entity(player),
                                                InventoryView::from_range(0..10),
                                            ),
                                            (
                                                InventoryProvider::Block(block_position),
                                                view.clone(),
                                            ),
                                        ],
                                    );
                                }
                            }
                        }
                        BlockInteractAction::Pickup => {
                            server.schedule_block_event(
                                block_position,
                                BlockEvent::Damage { damage: 10000. },
                            );
                        }
                    }
                }
            }
        }
        for entity in &self.entities {
            let entity = server.entities.get(*entity).unwrap();
            entity.tick(server);
        }
        {
            let blocks = self.blocks.read();
            self.components
                .damage
                .write()
                .components
                .retain_mut(|(offset, damage)| {
                    damage.damage -= 1.
                        * server.delta_time()
                        * blocks
                            .get(offset.index())
                            .unwrap()
                            .data()
                            .health
                            .as_ref()
                            .map(|health| health.health_regen)
                            .unwrap();
                    damage.damage > 0.
                });
        }
    }
}

pub struct UserIndexSave(UserIndex);
impl Serialize for UserIndexSave {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_unit()
    }
}
impl<'de> Deserialize<'de> for UserIndexSave {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DefaultVisitor;

        impl<'de> serde::de::Visitor<'de> for DefaultVisitor {
            type Value = UserIndexSave;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("unit")
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(UserIndexSave(slotmap::Key::null()))
            }
        }
        deserializer.deserialize_unit(DefaultVisitor)
    }
}
impl Into<UserIndex> for UserIndexSave {
    fn into(self) -> UserIndex {
        self.0
    }
}
impl From<UserIndex> for UserIndexSave {
    fn from(value: UserIndex) -> Self {
        Self(value)
    }
}

#[derive(Serialize, Deserialize)]
pub enum BlockEvent {
    Damage { damage: f32 },
    PlayerInteract { user: UserIndexSave },
}

#[derive(Serialize, Deserialize)]
pub enum EntityEvent {
    Damage { damage: f32 },
    PlayerInteract { user: UserIndexSave },
    Teleport { position: Pos },
    Remove,
}

new_key_type! {pub struct EntityIndex;}
#[derive(Serialize, Deserialize)]
pub struct Entity {
    pub key: EntityKey,
    pub uuid: Uuid,
    pub position: Pos,
    pub direction: LookDirection,
    pub inventory: RwLock<Inventory>,
    pub events: Mutex<SmallVec<[EntityEvent; 4]>>,
    pub state: Mutex<InternalEntityState>,
}
#[derive(Serialize, Deserialize)]
pub struct InternalEntityState {
    pub velocity: Pos,
    pub teleport: Option<Pos>,
    pub removed: bool,
    pub hand_slot: usize,
    #[serde(skip_serializing, skip_deserializing)]
    pub last_hand_item: Option<ItemStack>,
}
impl Entity {
    pub fn new(key: EntityKey, position: Pos) -> Entity {
        let entity_data = key.data();
        Entity {
            key,
            uuid: Uuid::new_v4(),
            position,
            direction: LookDirection { pitch: 0., yaw: 0. },
            inventory: RwLock::new(Inventory::new(entity_data.inventory_size)),
            events: Mutex::new(SmallVec::new()),
            state: Mutex::new(InternalEntityState {
                velocity: Pos::ZERO,
                removed: false,
                teleport: None,
                hand_slot: 0,
                last_hand_item: None,
            }),
        }
    }
    pub fn schedule_event(&self, event: EntityEvent) {
        self.events.lock().push(event);
    }
    pub fn tick(&self, server: &Server) {
        let mut processing_events = SmallVec::new();
        std::mem::swap(&mut processing_events, &mut *self.events.lock());
        let mut state = self.state.lock();
        for event in processing_events {
            match event {
                EntityEvent::Damage { damage } => {}
                EntityEvent::PlayerInteract { user } => match self.key.data().interact_action {
                    EntityInteractAction::Ignore => {}
                    EntityInteractAction::Pickup => {
                        if let Some(user) = server.get_user(user.0) {
                            if let Some(player_entity) =
                                user.entity.and_then(|entity| server.get_entity(entity))
                            {
                                let (mut inventory, mut player_inventory) =
                                    lock_inventories(&self.inventory, &player_entity.inventory);
                                let mut items_present = false;
                                for slot in &mut inventory.items {
                                    if let Some(item) = &slot {
                                        let view = player_inventory.full_view();
                                        *slot = player_inventory.add_item(&view, item.clone());
                                        if slot.is_some() {
                                            items_present = true;
                                        }
                                    }
                                }
                                if !items_present {
                                    self.schedule_event(EntityEvent::Remove);
                                }
                            }
                        }
                    }
                },
                EntityEvent::Teleport { position } => {
                    state.teleport = Some(position);
                }
                EntityEvent::Remove => {
                    state.removed = true;
                }
            }
        }
        {
            let hitbox = self.key.data().hitbox();
            let mut movement = state.velocity;
            movement.y -= 10. * server.delta_time();

            let mut friction = 0.;

            if movement.x != 0.
                && server.hitbox_block_collides(
                    hitbox
                        .offset(
                            self.position
                                + Pos {
                                    x: movement.x,
                                    y: 0.,
                                    z: 0.,
                                } * server.delta_time(),
                        )
                        .to_block(),
                )
            {
                movement.x = 0.;
                friction += 1.;
            }
            if movement.y != 0.
                && server.hitbox_block_collides(
                    hitbox
                        .offset(
                            self.position
                                + Pos {
                                    x: movement.x,
                                    y: movement.y,
                                    z: 0.,
                                } * server.delta_time(),
                        )
                        .to_block(),
                )
            {
                movement.y = 0.;
                friction += 1.;
            }
            if movement.z != 0.
                && server.hitbox_block_collides(
                    hitbox
                        .offset(
                            self.position
                                + Pos {
                                    x: movement.x,
                                    y: movement.y,
                                    z: movement.z,
                                } * server.delta_time(),
                        )
                        .to_block(),
                )
            {
                movement.z = 0.;
                friction += 1.;
            }
            if movement.length_squared() != 0. {
                if state.teleport.is_none() {
                    state.teleport = Some(self.position + movement * server.delta_time());
                } else {
                    movement = Pos::ZERO;
                }
            }
            {
                let drag = 0.1;
                movement = movement * (1. - drag * server.delta_time());
            }
            let movement_length = movement.length();
            if movement_length > 0. {
                let friction_constant = 8.;
                let friction_axis = |value: f32| -> f32 {
                    let force = value / movement_length
                        * friction_constant
                        * friction
                        * server.delta_time();
                    if value.abs() < force.abs() {
                        return 0.;
                    }
                    value - force
                };
                movement.x = friction_axis(movement.x);
                movement.y = friction_axis(movement.y);
                movement.z = friction_axis(movement.z);
            }

            state.velocity = movement;
        }
        {
            let new_hand_item = self
                .inventory
                .read()
                .items
                .get(state.hand_slot)
                .cloned()
                .flatten();
            if new_hand_item != state.last_hand_item {
                server.send_message_multiple(
                    server
                        .get_chunk(self.position.to_chunk_pos())
                        .unwrap()
                        .viewers
                        .iter(),
                    NetworkMessageS2C::EntityHandItem {
                        uuid: self.uuid,
                        item: new_hand_item.as_ref().map(|item| item.client()),
                    },
                );
                state.last_hand_item = new_hand_item;
            }
        }
    }
    pub fn get_hitbox(&self) -> AABB<f32> {
        let entity_data = self.key.data();
        entity_data.hitbox().offset(self.position)
    }
}
impl Entity {
    pub fn create_add_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::AddEntity {
            uuid: self.uuid,
            key: self.key,
            position: self.position,
            direction: self.direction,
        }
    }
    pub fn create_move_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::MoveEntity {
            uuid: self.uuid,
            position: self.position,
            direction: self.direction,
        }
    }
    pub fn create_remove_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::RemoveEntity { uuid: self.uuid }
    }
}

macro_rules! create_chunk_block_components{
    ($($type:tt, $id:ident);*) => {
        #[derive(Default, Serialize, Deserialize)]
        pub struct ChunkBlockComponents{
            $(
                #[serde(skip_serializing_if = "skip_serializing_component_storage", default)]
                pub $id: parking_lot::RwLock<BlockComponentStorage<$type>>,
            )*
        }
    }
}
pub fn skip_serializing_component_storage<T>(
    components: &parking_lot::RwLock<BlockComponentStorage<T>>,
) -> bool {
    components.read().components.is_empty()
}

macro_rules! create_chunk_block_components_client_mapping {
    ($($id:ident),*) => {
        impl ChunkBlockComponents {
            pub fn client(&self) -> ClientChunkBlockComponents {
                ClientChunkBlockComponents {
                    $(
                        $id: (&*self.$id.read()).into(),
                    )*
                }
            }
        }
    };
}

create_chunk_block_components!(BlockDamage, damage; BlockPlants, plant; BlockMachine, machine);
create_chunk_block_components_client_mapping!(damage, plant);

#[derive(Serialize, Deserialize)]
pub struct BlockDamage {
    pub damage: f32,
}
impl Into<ClientBlockDamage> for &BlockDamage {
    fn into(self) -> ClientBlockDamage {
        ClientBlockDamage {
            damage: self.damage,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub struct BlockPlants {
    pub plants: Vec<(PlantKey, f32)>,
}
impl Into<ClientBlockPlants> for &BlockPlants {
    fn into(self) -> ClientBlockPlants {
        ClientBlockPlants {
            plants: self.plants.iter().map(|(plant, _)| *plant).collect(),
        }
    }
}
#[derive(Serialize, Deserialize)]
pub struct BlockMachine {
    pub inventory: RwLock<Inventory>,
}
