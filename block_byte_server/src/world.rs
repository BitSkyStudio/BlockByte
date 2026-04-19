use std::{
    collections::{HashMap, HashSet},
    mem::MaybeUninit,
    ops::Deref,
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use block_byte_common::{
    CharacterController, Color, DamageType, InventoryView, LookDirection, MoveMode, SERVER_DT,
    SERVER_TPS,
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, Orientation, Pos},
    net::NetworkMessageS2C,
    registry::{
        BiomeKey, BlockColor, BlockData, BlockEntry, BlockInteractAction, BlockKey,
        BlockMachineFace, BlockPalette, BlockRotation, EntityInteractAction, EntityKey, KeyGroup,
        MachineInstrution, PlantKey, PrefabKey, ResearchKey, air_block,
    },
    scripts::{self, CallbackResult, ExternalScriptByteCode, ScriptState, ScriptValue},
    world::{
        BlockComponentStorage, ClientBlockComponentUpdate, ClientBlockDamage, ClientBlockMachine,
        ClientBlockPlants, ClientChunkBlockComponents,
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
    pub fn generate(position: ChunkPos, generator: &WorldGenerator) -> Chunk {
        /*use noise::MultiFractal;
        let height_noise: BasicMulti<Perlin> = BasicMulti::new(seed)
            .set_octaves(4)
            .set_frequency(1.0)
            .set_lacunarity(2.0)
            .set_persistence(0.5);*/

        let column_data = generator.get_column_generation(position.x, position.z);

        let mut blocks = BlockPalette::filled(
            BlockEntry::simple(air_block()),
            CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE,
        );
        let mut components = ChunkBlockComponents::default();

        use rand::SeedableRng;
        let mut rng =
            StdRng::from_seed(Seeder::from((generator.seed as u32, position)).make_seed());
        for z in 0..CHUNK_SIZE as u8 {
            for y in 0..CHUNK_SIZE as u8 {
                for x in 0..CHUNK_SIZE as u8 {
                    let offset = ChunkOffset::new(x, y, z);
                    let y_pos = y as i32 + position.y as i32 * CHUNK_SIZE as i32;
                    let biome = column_data.biomes[x as usize][y as usize].data();
                    let height = column_data.height[x as usize][z as usize] as i32;
                    /*let holes = hole_spline.clamped_sample(height as f32).unwrap();
                    let density = density_noise.get([
                        (position.x as f64 * CHUNK_SIZE as f64 + x as f64) / 10.,
                        (position.y as f64 * CHUNK_SIZE as f64 + y as f64) / 10.,
                        (position.z as f64 * CHUNK_SIZE as f64 + z as f64) / 10.,
                    ]) as f32
                        * holes
                        + (height - y_pos) as f32 / 20.;
                    if density > 0. {
                        blocks.set(
                            offset.index(),
                            &BlockEntry {
                                block: biome.bottom_block,
                                color: Color::WHITE,
                                rotation: BlockRotation::default(),
                            },
                        );
                    }*/
                    if y_pos == height {
                        blocks.set(offset.index(), &BlockEntry::simple(biome.top_block));
                        let spawned_plants: SmallVec<_> = biome
                            .plants
                            .iter()
                            .filter_map(|spawner| {
                                if rng.random_bool(spawner.chance as f64) {
                                    let plant_data = spawner.plant.data();
                                    Some((
                                        spawner.plant,
                                        rng.random::<f32>() * plant_data.growth_length,
                                    ))
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
                        blocks.set(offset.index(), &BlockEntry::simple(biome.bottom_block));
                    } else if y_pos < height {
                        blocks.set(offset.index(), &BlockEntry::simple(biome.middle_block));
                    }
                }
            }
        }
        let replacable_tag = KeyGroup::parse("#prefab_replacable").unwrap();
        for neighbor_chunk in (AABB {
            min: ChunkPos { x: -1, y: 0, z: -1 },
            max: ChunkPos { x: 1, y: 0, z: 1 },
        })
        .offset(position)
        {
            let block_offset = neighbor_chunk.to_block_pos();
            let column = generator.get_column_generation(neighbor_chunk.x, neighbor_chunk.z);
            for placed_decoration in column.get_legal_decorations(generator) {
                let height = column.height[placed_decoration.x as usize]
                    [placed_decoration.z as usize] as i32
                    + 1;
                let block_position = BlockPos {
                    x: block_offset.x + placed_decoration.x as i32,
                    y: height,
                    z: block_offset.z + placed_decoration.z as i32,
                };
                //todo: do bounding box calculations to cull
                if
                /*height >= block_offset.y && height < block_offset.y + CHUNK_SIZE as i32*/
                true {
                    let prefab = placed_decoration.key.data();
                    let rotation = [Face::Front, Face::Back, Face::Left, Face::Right]
                        [placed_decoration.rotation as usize];
                    let rotation = Orientation::from_front_up(rotation, Face::Up).unwrap();
                    for part in &prefab.parts {
                        //todo
                        /*if !rng.random_bool(part.chance as f64) {
                            continue;
                        }*/
                        for (offset, block) in &part.blocks {
                            let offset = rotation.rotate_block_pos(*offset);
                            let mut block = *block;
                            block.rotation = block.block.data().rotation.get_nearest_valid(
                                rotation
                                    .compose(Orientation::from_block_rotation(block.rotation))
                                    .into_block_rotation(),
                            );
                            let place_position = block_position + offset;
                            let (place_chunk, place_chunk_offset) =
                                place_position.to_chunk_pos_offset();
                            if place_chunk == position {
                                if replacable_tag
                                    .contains(blocks.get(place_chunk_offset.index()).unwrap().block)
                                {
                                    blocks.set(place_chunk_offset.index(), &block);
                                }
                            }
                        }
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
                BlockEvent::Damage {
                    damage,
                    damage_type,
                } => {
                    let block_data = self.blocks.read().get(block.index()).unwrap().block.data();
                    let damage = damage * block_data.health.table[damage_type].unwrap_or(1.);
                    let mut damage_component = self.components.damage.write();
                    let mut destroy = false;

                    if damage >= block_data.health.health {
                        destroy = true;
                    } else if let Some(block_damage) = damage_component.get_mut(block) {
                        block_damage.damage += damage;
                        if block_damage.damage >= block_data.health.health {
                            destroy = true;
                        }
                    } else {
                        damage_component.set(block, BlockDamage { damage });
                    }
                    if destroy {
                        drop(damage_component);
                        let block_pos = self.position.to_block_pos() + block.xyz();
                        let drops = server.destroy(block_pos);
                        if block_data.health.transform_block != air_block() {
                            //todo: maybe apply overflow damage?
                            server.place(
                                block_pos,
                                BlockEntry::simple(block_data.health.transform_block),
                            );
                        }
                        for item in drops {
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
                    } else {
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
                BlockEvent::PlayerInteract { player } => {
                    let block_data = self.blocks.read().get(block.index()).unwrap().block.data();
                    let block_position = self.position.to_block_pos() + block.xyz();
                    match &block_data.interact_action {
                        BlockInteractAction::Ignore => {}
                        BlockInteractAction::OpenInventory { screen, view } => {
                            if let Some(player_entity) = server.get_entity(player.0) {
                                if let Some(user) = player_entity.controller {
                                    if let Some(user) = server.get_user(user) {
                                        user.set_screen(
                                            *screen,
                                            vec![
                                                (
                                                    InventoryProvider::Entity(player.0),
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
                        }
                        BlockInteractAction::Pickup => {
                            if let Some(entity) = server.get_entity(player.0) {
                                let block_pos = self.position.to_block_pos() + block.xyz();
                                let mut drops = server.destroy(block_pos);
                                if drops.is_empty() {
                                    continue;
                                }
                                let mut entity_inventory = entity.inventory.write();
                                let view = entity_inventory.full_view();
                                for item in drops {
                                    if let Some(rest) = entity_inventory.add_item(&view, item) {
                                        server.spawn_item(
                                            rest,
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
                        }
                    }
                }
                BlockEvent::PlantHarvest { player } => {}
                BlockEvent::LogicSignal { value, world_face } => {
                    let block_state = *self.blocks.read().get(block.index()).unwrap();
                    let block_data = block_state.block.data();
                    let mut machines = self.components.machine.write();
                    if let Some(machine) = machines.get_mut(block) {
                        let own_face = block_state.rotation.inverse_rotate_face(world_face);
                        match block_data.machine.as_ref().unwrap().faces.by_face(own_face) {
                            BlockMachineFace::SignalInput => {
                                machine.blocked.store(false, Ordering::Relaxed);
                                *machine.logic_state.lock().by_face_mut(own_face) = Some(value);
                            }
                            _ => {}
                        }
                    }
                }
                BlockEvent::UpdateLogicState { value, world_face } => {
                    let block_state = *self.blocks.read().get(block.index()).unwrap();
                    let block_data = block_state.block.data();
                    let mut machines = self.components.machine.write();
                    if let Some(machine) = machines.get_mut(block) {
                        let own_face = block_state.rotation.inverse_rotate_face(world_face);
                        match block_data.machine.as_ref().unwrap().faces.by_face(own_face) {
                            BlockMachineFace::LogicInput => {
                                machine.blocked.store(false, Ordering::Relaxed);
                                *machine.logic_state.lock().by_face_mut(own_face) = Some(value);
                            }
                            _ => {}
                        }
                    }
                }
                BlockEvent::NeighborDestroyed { world_face } => {
                    let block_pos = self.position.to_block_pos() + block.xyz();
                    let block = *self.blocks.read().get(block.index()).unwrap();
                    let block_data = block.block.data();
                    let face = block.rotation.inverse_rotate_face(world_face);
                    if let Some(support_face) = block_data.hanging {
                        if face == support_face {
                            let drops = server.destroy(block_pos);
                            for item in drops {
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
                }
                BlockEvent::Wakeup { inventory_updated } => {
                    let mut machines = self.components.machine.write();
                    if let Some(machine) = machines.get_mut(block) {
                        machine.blocked.store(false, Ordering::Relaxed);
                        if inventory_updated {
                            for to_wakeup in machine.inventory_observers.get_mut().drain(..) {
                                server.schedule_block_event(
                                    to_wakeup,
                                    BlockEvent::Wakeup {
                                        inventory_updated: false,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        for (offset, machine) in self.components.machine.read().iter() {
            let block = *self.blocks.read().get(offset.index()).unwrap();
            let block_data = block.block.data();
            let machine_data = block_data.machine.as_ref().unwrap();
            let mut cooldown = machine.sleep_cooldown.lock();
            if *cooldown == 0 {
                if machine.blocked.load(Ordering::Relaxed) {
                    continue;
                }
                match machine.script_state.lock().run(
                    &machine_data.script,
                    |state, instruction| match instruction {
                        MachineInstrution::Next => CallbackResult::Suspend,
                        MachineInstrution::Sleep { time } => {
                            *cooldown = (*time * SERVER_TPS as f32).round() as u32;
                            CallbackResult::Suspend
                        }
                        MachineInstrution::Suspend => {
                            machine.blocked.store(true, Ordering::Relaxed);
                            CallbackResult::Suspend
                        }
                        MachineInstrution::TranferItem {
                            self_view: view,
                            other: push_offset,
                            other_face,
                            pull,
                            success,
                        } => {
                            let view = &machine_data.script_views[*view];
                            let other_position = block.rotation.rotate_block_pos(*push_offset)
                                + offset.xyz()
                                + self.position.to_block_pos();
                            let Some(other_block) = server.get_block(other_position) else {
                                return CallbackResult::Continue;
                            };
                            let other_block_data = other_block.block.data();
                            if let Some(other_machine_data) = &other_block_data.machine {
                                let (other_chunk, other_offset) =
                                    other_position.to_chunk_pos_offset();
                                let other_chunk_machines = server
                                    .get_chunk(other_chunk)
                                    .unwrap()
                                    .components
                                    .machine
                                    .read();
                                let other_machine = other_chunk_machines.get(other_offset).unwrap();
                                let face_rotated = other_block
                                    .rotation
                                    .inverse_rotate_face(block.rotation.rotate_face(*other_face));
                                let face_data = other_machine_data.faces.by_face(face_rotated);
                                match face_data {
                                    BlockMachineFace::InventoryAccess { input, output } => {
                                        let other_view = if *pull { output } else { input };
                                        let (mut first_inventory, mut second_inventory) =
                                            lock_inventories(
                                                &machine.inventory,
                                                &other_machine.inventory,
                                            );
                                        if *pull {
                                            std::mem::swap(
                                                &mut first_inventory,
                                                &mut second_inventory,
                                            );
                                        }
                                        let mut exit = false;
                                        for slot in &view.slots {
                                            if let Some(item) =
                                                first_inventory.get_slot_mut_raw(slot.slot)
                                            {
                                                if second_inventory
                                                    .add_item(other_view, item.copy(1))
                                                    .is_none()
                                                {
                                                    other_machine
                                                        .blocked
                                                        .store(false, Ordering::Relaxed);
                                                    item.count -= 1;
                                                    if item.count == 0 {
                                                        first_inventory.items[slot.slot] = None;
                                                    }
                                                    state.pc = *success;
                                                    exit = true;
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            CallbackResult::Continue
                        }
                        MachineInstrution::ReadSignal {
                            face,
                            register,
                            success,
                        } => {
                            match machine.logic_state.lock().by_face_mut(*face).take() {
                                Some(value) => {
                                    state.registers[*register] = value;
                                    state.pc = *success;
                                }
                                None => {}
                            }
                            CallbackResult::Continue
                        }
                        MachineInstrution::ReadSignalBlock { face, register } => {
                            match machine.logic_state.lock().by_face_mut(*face).take() {
                                Some(value) => {
                                    state.registers[*register] = value;
                                    CallbackResult::Continue
                                }
                                None => CallbackResult::Wait,
                            }
                        }
                        MachineInstrution::ReadLogic { face, register } => {
                            state.registers[*register] =
                                machine.logic_state.lock().by_face(*face).unwrap_or(0);
                            CallbackResult::Continue
                        }
                        MachineInstrution::WriteSignal { face, value } => {
                            let value = state.resolve_value(value);
                            let world_face = block.rotation.rotate_face(*face);
                            let target_position = self.position.to_block_pos()
                                + offset.xyz()
                                + world_face.get_block_offset();
                            server.schedule_block_event(
                                target_position,
                                BlockEvent::LogicSignal {
                                    value,
                                    world_face: world_face.opposite(),
                                },
                            );
                            CallbackResult::Continue
                        }
                        MachineInstrution::WriteValue { face, value } => {
                            let value = state.resolve_value(value);
                            let mut logic_state = machine.logic_state.lock();
                            let mut logic_state = logic_state.by_face_mut(*face);
                            if let Some(previous) = logic_state {
                                if *previous == value {
                                    return CallbackResult::Continue;
                                }
                            }
                            *logic_state = Some(value);
                            let world_face = block.rotation.rotate_face(*face);
                            let target_position = self.position.to_block_pos()
                                + offset.xyz()
                                + world_face.get_block_offset();
                            server.schedule_block_event(
                                target_position,
                                BlockEvent::UpdateLogicState {
                                    value,
                                    world_face: world_face.opposite(),
                                },
                            );
                            CallbackResult::Continue
                        }
                        MachineInstrution::GetSlotItemCount { slot, register } => {
                            if let Some(item) = machine
                                .inventory
                                .read()
                                .items
                                .get(state.resolve_value(slot) as usize)
                            {
                                state.registers[*register] =
                                    item.as_ref().map(|item| item.count).unwrap_or(0);
                            }
                            CallbackResult::Continue
                        }
                        MachineInstrution::MoveItem {
                            from_view,
                            to_view,
                            success,
                        } => {
                            let from_view = &machine_data.script_views[*from_view];
                            let to_view = &machine_data.script_views[*to_view];
                            let mut inventory = machine.inventory.write();
                            for slot in &from_view.slots {
                                if let Some(item) =
                                    inventory.items[slot.slot].as_ref().map(|item| item.copy(1))
                                {
                                    if inventory.add_item(to_view, item).is_none() {
                                        let item =
                                            inventory.get_slot_mut_raw(slot.slot).as_mut().unwrap();
                                        item.count -= 1;
                                        if item.count == 0 {
                                            inventory.items[slot.slot] = None;
                                        }
                                        state.pc = *success;
                                        break;
                                    }
                                }
                            }
                            CallbackResult::Continue
                        }
                        MachineInstrution::Craft {
                            recipes,
                            input_view,
                            output_view,
                            speed,
                            success,
                        } => {
                            let input_view = &machine_data.script_views[*input_view];
                            let output_view = &machine_data.script_views[*output_view];
                            let mut inventory = machine.inventory.write();
                            for recipe in recipes.list() {
                                let recipe = recipe.data();
                                let mut failed = false;
                                for (input, count) in &recipe.inputs {
                                    if inventory.count_item(input_view, *input) < *count {
                                        failed = true;
                                        break;
                                    }
                                }
                                if failed {
                                    continue;
                                }
                                for (input, count) in &recipe.inputs {
                                    inventory.remove_item(input_view, *input, *count);
                                }
                                for output in generate_loot_table(recipe.outputs.data()) {
                                    inventory.add_item(output_view, output);
                                }
                                *cooldown =
                                    (recipe.craft_time * speed * SERVER_TPS as f32).round() as u32;
                                state.pc = *success;
                                return CallbackResult::Suspend;
                            }
                            CallbackResult::Continue
                        }
                    },
                    500,
                ) {
                    scripts::RunResult::Suspended => {}
                    scripts::RunResult::TimedOut => {
                        eprintln!("script of block {} timed out", block.block.text_id());
                    }
                }
            } else {
                *cooldown -= 1;
            }
        }
        /*let chunk_id =
            (self.position.x * 5823 + self.position.y * 9547 + self.position.z * 12782) as u64;
        if (chunk_id + server.ticks_passed) % (server.tps * 10) == 0 {
            let blocks = self.blocks.read();
            let mut plants = self.components.plant.write();
            for plant in &mut plants.components {

            }
        }*/
        for entity in &self.entities {
            let entity = server.entities.get(*entity).unwrap();
            entity.tick(server);
        }
        {
            let blocks = self.blocks.read();
            let mut damage = self.components.damage.write();
            let mut damage_to_clear = Vec::new();
            for (offset, damage) in damage.iter_mut() {
                damage.damage -= 1.
                    * SERVER_DT
                    * blocks
                        .get(offset.index())
                        .unwrap()
                        .block
                        .data()
                        .health
                        .health_regen;
                if damage.damage <= 0. {
                    damage_to_clear.push(offset);
                }
            }
            for block in damage_to_clear {
                damage.remove(block);
            }
        }
    }
}

pub struct EntityIndexSave(pub EntityIndex);
impl Serialize for EntityIndexSave {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_unit()
    }
}
impl<'de> Deserialize<'de> for EntityIndexSave {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DefaultVisitor;

        impl<'de> serde::de::Visitor<'de> for DefaultVisitor {
            type Value = EntityIndexSave;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("unit")
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(EntityIndexSave(slotmap::Key::null()))
            }
        }
        deserializer.deserialize_unit(DefaultVisitor)
    }
}
impl Into<EntityIndex> for EntityIndexSave {
    fn into(self) -> EntityIndex {
        self.0
    }
}
impl From<EntityIndex> for EntityIndexSave {
    fn from(value: EntityIndex) -> Self {
        Self(value)
    }
}

#[derive(Serialize, Deserialize)]
pub enum BlockEvent {
    Damage {
        damage: f32,
        damage_type: DamageType,
    },
    PlayerInteract {
        player: EntityIndexSave,
    },
    PlantHarvest {
        player: EntityIndexSave,
    },
    LogicSignal {
        value: ScriptValue,
        world_face: Face,
    },
    UpdateLogicState {
        value: ScriptValue,
        world_face: Face,
    },
    NeighborDestroyed {
        world_face: Face,
    },
    Wakeup {
        inventory_updated: bool,
    },
}

#[derive(Serialize, Deserialize)]
pub enum EntityEvent {
    Damage {
        damage: f32,
        damage_type: DamageType,
    },
    PlayerInteract {
        user: EntityIndexSave,
    },
    Teleport {
        position: Pos,
    },
    Knockback {
        knockback: Pos,
    },
    Remove,
}

new_key_type! {pub struct EntityIndex;}
#[derive(Serialize, Deserialize)]
pub struct Entity {
    pub key: EntityKey,
    pub uuid: Uuid,
    pub position: Pos,
    pub inventory: RwLock<Inventory>,
    pub events: Mutex<SmallVec<[EntityEvent; 4]>>,
    #[serde(default, skip_serializing, skip_deserializing)]
    pub controller: Option<UserIndex>,
    pub state: Mutex<InternalEntityState>,
}
#[derive(Serialize, Deserialize)]
pub struct InternalEntityState {
    pub character_controller: CharacterController,
    pub teleport: Option<Pos>,
    pub removed: bool,
    pub hand_slot: usize,
    #[serde(skip_serializing, skip_deserializing)]
    pub last_hand_item: Option<ItemStack>,
    pub health: f32,
    pub research: HashSet<ResearchKey>,
    pub brain: Option<MobBrain>,
    pub direction: LookDirection,
}
#[derive(Serialize, Deserialize)]
pub struct MobBrain {}
impl MobBrain {
    pub fn new() -> Self {
        Self {}
    }
}
impl Entity {
    pub fn new(key: EntityKey, position: Pos) -> Entity {
        let entity_data = key.data();
        Entity {
            key,
            uuid: Uuid::new_v4(),
            position,
            inventory: RwLock::new(Inventory::new(entity_data.inventory_size)),
            events: Mutex::new(SmallVec::new()),
            controller: None,
            state: Mutex::new(InternalEntityState {
                character_controller: CharacterController::new(),
                removed: false,
                teleport: None,
                hand_slot: 0,
                last_hand_item: None,
                health: entity_data.health,
                research: HashSet::new(),
                brain: match &entity_data.ai {
                    Some(_) => Some(MobBrain::new()),
                    None => None,
                },
                direction: LookDirection { pitch: 0., yaw: 0. },
            }),
        }
    }
    pub fn schedule_event(&self, event: EntityEvent) {
        self.events.lock().push(event);
    }
    pub fn tick(&self, server: &Server) {
        let entity_data = self.key.data();
        let mut processing_events = SmallVec::new();
        std::mem::swap(&mut processing_events, &mut *self.events.lock());
        let mut state = self.state.lock();
        for event in processing_events {
            match event {
                EntityEvent::Damage {
                    damage,
                    damage_type,
                } => {
                    let damage = damage * entity_data.damage_table[damage_type].unwrap_or(1.);
                    state.health -= damage;
                    if state.health <= 0. {
                        if self.key.text_id() != "player" {
                            state.removed = true;
                        }
                    }
                }
                EntityEvent::PlayerInteract { user } => match self.key.data().interact_action {
                    EntityInteractAction::Ignore => {}
                    EntityInteractAction::Pickup => {
                        if let Some(player_entity) = server.get_entity(user.0) {
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
                },
                EntityEvent::Teleport { position } => {
                    state.teleport = Some(position);
                }
                EntityEvent::Remove => {
                    state.removed = true;
                }
                EntityEvent::Knockback { knockback } => {
                    state.character_controller.velocity += knockback;
                }
            }
        }
        if self.controller.is_none() {
            let hitbox = self.key.data().hitbox();
            let mut move_vector = Pos::ZERO;
            match &self.key.data().ai {
                Some(_) => {
                    if state.character_controller.on_ground {
                        state.character_controller.velocity.y += 15.;
                    }
                    let front = state.direction.make_front();
                    move_vector.x = front.x * 1.;
                    move_vector.z = front.z * 1.;
                    if rand::random_bool(1. / 40. / 5.) {
                        state.direction.yaw = rand::random_range((0.)..(std::f32::consts::PI * 2.))
                    }
                }
                None => {}
            }
            let mut new_position = self.position;
            state.character_controller.tick(
                &mut new_position,
                SERVER_DT,
                |block| server.get_block(block),
                move_vector,
                MoveMode::Normal,
                hitbox,
                40.,
                0.5,
                false,
            );
            if new_position != self.position && state.teleport.is_none() {
                state.teleport = Some(new_position);
            }
            /*let mut movement = state.velocity;
            movement.y -= 10. * server.delta_time();

            let mut friction = 0.;

            let mut on_ground = false;

            if movement.x != 0.
                && server.hitbox_block_collides(hitbox.offset(
                    self.position
                        + Pos {
                            x: movement.x,
                            y: 0.,
                            z: 0.,
                        } * server.delta_time(),
                ))
            {
                movement.x = 0.;
                friction += 1.;
            }
            if movement.y != 0.
                && server.hitbox_block_collides(hitbox.offset(
                    self.position
                        + Pos {
                            x: movement.x,
                            y: movement.y,
                            z: 0.,
                        } * server.delta_time(),
                ))
            {
                on_ground = movement.y < 0.;
                movement.y = 0.;
                friction += 1.;
            }
            if movement.z != 0.
                && server.hitbox_block_collides(hitbox.offset(
                    self.position
                        + Pos {
                            x: movement.x,
                            y: movement.y,
                            z: movement.z,
                        } * server.delta_time(),
                ))
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

            match self.key.data().ai_tasks.get(0) {
                Some(task) => match task {
                    block_byte_common::registry::MobAiTask::Attack {
                        targets,
                        damage,
                        damage_type,
                    } => todo!(),
                    block_byte_common::registry::MobAiTask::Wander => {
                        if on_ground {
                            movement.y += 10.;
                        }
                        let front = state.direction.make_front();
                        movement.x = front.x * 3.;
                        movement.z = front.z * 3.;
                        if rand::random_bool(1. / 40. / 5.) {
                            state.direction.yaw =
                                rand::random_range((0.)..(std::f32::consts::PI * 2.))
                        }
                    }
                },
                None => {}
            }

            state.velocity = movement;*/
        } else {
            if state.character_controller.velocity.length_squared() > 0. {
                server.send_message(
                    self.controller.unwrap(),
                    NetworkMessageS2C::Knockback {
                        velocity: state.character_controller.velocity,
                    },
                );
                state.character_controller.velocity = Pos::ZERO;
            }
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
        let hand_slot = self.state.lock().hand_slot;
        NetworkMessageS2C::AddEntity {
            uuid: self.uuid,
            key: self.key,
            position: self.position,
            direction: self.state.lock().direction,
            hand_item: self
                .inventory
                .read()
                .items
                .get(hand_slot)
                .cloned()
                .flatten()
                .map(|item| item.client()),
        }
    }
    pub fn create_move_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::MoveEntity {
            uuid: self.uuid,
            position: self.position,
            direction: self.state.lock().direction,
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
    components.read().iter().count() == 0
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
create_chunk_block_components_client_mapping!(damage, plant, machine);

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
    pub plants: SmallVec<[(PlantKey, f32); 1]>,
}
impl Into<ClientBlockPlants> for &BlockPlants {
    fn into(self) -> ClientBlockPlants {
        ClientBlockPlants {
            plants: self
                .plants
                .iter()
                .map(|(plant, growth)| {
                    let plant_data = plant.data();
                    let stage = (((*growth / plant_data.growth_length)
                        * (plant_data.stages.len() - 1) as f32)
                        as usize);
                    (*plant, stage as u8)
                })
                .collect(),
        }
    }
}
#[derive(Serialize, Deserialize)]
pub struct BlockMachine {
    pub inventory: RwLock<Inventory>,
    pub sleep_cooldown: Mutex<u32>,
    pub script_state: Mutex<ScriptState>,
    pub logic_state: Mutex<FaceMap<Option<ScriptValue>>>,
    pub blocked: AtomicBool,
    pub inventory_observers: Mutex<SmallVec<[BlockPos; 1]>>,
    pub current_animation: Mutex<u16>,
    pub animation_start_time: Mutex<u64>,
}

impl Into<ClientBlockMachine> for &BlockMachine {
    fn into(self) -> ClientBlockMachine {
        ClientBlockMachine {
            animation: *self.current_animation.lock(),
            animation_start_time: *self.animation_start_time.lock(),
        }
    }
}
pub struct RegionGeneration {}
pub struct ChunkColumnGeneration {
    pub x: i16,
    pub z: i16,
    pub biomes: [[BiomeKey; CHUNK_SIZE as usize]; CHUNK_SIZE as usize],
    pub height: [[u16; CHUNK_SIZE as usize]; CHUNK_SIZE as usize],
    pub unique_biomes: Vec<BiomeKey>,
    pub decorations: Vec<ChunkColumnDecoration>,
}
impl ChunkColumnGeneration {
    pub fn is_blocked(&self, x: i32, z: i32, exclusion_radius: u8) -> bool {
        for decoration in &self.decorations {
            let decoration_x = (decoration.x as i32 + self.x as i32 * CHUNK_SIZE as i32);
            let decoration_z = (decoration.z as i32 + self.z as i32 * CHUNK_SIZE as i32);
            let distance = (decoration_x - x).pow(2) + (decoration_z - z).pow(2);
            let decoration_data = decoration.key.data();
            if distance <= (decoration.exclusion_zone as i32 + exclusion_radius as i32).pow(2) {
                return true;
            }
        }
        false
    }
    const NEIGHBOR_CHUNK_BLOCKERS: [(i8, i8); 4] = [(0, -1), (-1, -1), (-1, 0), (-1, 1)];
    pub fn get_legal_decorations<'a>(
        &'a self,
        world_generator: &WorldGenerator,
    ) -> impl Iterator<Item = &'a ChunkColumnDecoration> {
        let blocking_neighbors = Self::NEIGHBOR_CHUNK_BLOCKERS.map(|(x, z)| {
            world_generator.get_column_generation(self.x + x as i16, self.z + z as i16)
        });
        self.decorations.iter().filter(move |decoration| {
            !blocking_neighbors.iter().any(|neighbor| {
                neighbor.is_blocked(
                    decoration.x as i32 + self.x as i32 * CHUNK_SIZE as i32,
                    decoration.z as i32 + self.z as i32 * CHUNK_SIZE as i32,
                    decoration.exclusion_zone,
                )
            })
        })
    }
}
struct ChunkColumnDecoration {
    key: PrefabKey,
    x: u8,
    z: u8,
    exclusion_zone: u8,
    rotation: u8,
}
#[derive(Deserialize)]
pub struct WorldGeneratorConfig {}
pub struct WorldGenerator {
    pub seed: u64,
    pub config: WorldGeneratorConfig,
    pub biome_height_cache: moka::sync::Cache<(i16, i16), Arc<ChunkColumnGeneration>>,
}
impl WorldGenerator {
    pub fn new(config: WorldGeneratorConfig, seed: u64) -> WorldGenerator {
        WorldGenerator {
            seed,
            config,
            biome_height_cache: moka::sync::Cache::new(1024),
        }
    }
    pub fn get_column_generation(&self, chunk_x: i16, chunk_z: i16) -> Arc<ChunkColumnGeneration> {
        self.biome_height_cache.get_with((chunk_x, chunk_z), || {
            let height_noise = Perlin::new(self.seed as u32);
            let density_noise = Perlin::new(self.seed as u32 ^ 583279234);
            let mut height_map = [[0; CHUNK_SIZE as usize]; CHUNK_SIZE as usize];
            let forest = BiomeKey::id("forest").unwrap();
            let mut biome_map = [[forest; CHUNK_SIZE as usize]; CHUNK_SIZE as usize];
            let mountain_spline = Spline::from_vec(vec![
                splines::Key::new(-1., 60., Interpolation::Linear),
                splines::Key::new(0., 80., Interpolation::Linear),
                splines::Key::new(0.5, 100., Interpolation::Cosine),
                splines::Key::new(1., 200., Interpolation::Linear),
            ]);
            let small_spline = Spline::from_vec(vec![
                splines::Key::new(-1., -3., Interpolation::Linear),
                splines::Key::new(1., 3., Interpolation::Linear),
            ]);
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let block_x = x as i32 + chunk_x as i32 * CHUNK_SIZE as i32;
                    let block_z = z as i32 + chunk_z as i32 * CHUNK_SIZE as i32;

                    let mountain_height = height_noise
                        .get([block_x as f64 / 500., block_z as f64 / 500.])
                        .clamp(-0.99, 0.99);

                    let small_noise = height_noise
                        .get([block_x as f64 / 30., block_z as f64 / 30.])
                        .clamp(-0.99, 0.99);
                    let height = (mountain_spline.sample(mountain_height).unwrap()
                        + small_spline.sample(small_noise).unwrap());
                    height_map[x as usize][z as usize] = height as u16;
                    //biome_map[x as usize][z as usize] = biome;
                }
            }
            let hole_spline = Spline::from_vec(vec![
                splines::Key::new(100., 0., Interpolation::Linear),
                splines::Key::new(120., 0.8, Interpolation::Linear),
                splines::Key::new(200., 0., Interpolation::Linear),
            ]);
            //todo: this is probably broken between runs
            let unique_biomes = vec![forest];
            use rand::SeedableRng;
            let mut rng =
                StdRng::from_seed(Seeder::from((self.seed as u32, chunk_x, chunk_z)).make_seed());
            let mut chunk_column_generation = ChunkColumnGeneration {
                x: chunk_x,
                z: chunk_z,
                biomes: biome_map,
                height: height_map,
                unique_biomes,
                decorations: Vec::new(),
            };
            for biome in &chunk_column_generation.unique_biomes {
                for decorator in &biome.data().decorators {
                    for i in 0..decorator.count {
                        if !rng.random_bool(decorator.chance as f64) {
                            continue;
                        }
                        for _ in 0..10 {
                            let rotation = rng.random_range(0..4) as u8;
                            let offset_x = rng.random_range(0..CHUNK_SIZE) as u8;
                            let offset_z = rng.random_range(0..CHUNK_SIZE) as u8;
                            if biome_map[offset_x as usize][offset_z as usize] != *biome {
                                continue;
                            }
                            if !chunk_column_generation.is_blocked(
                                offset_x as i32 + chunk_x as i32 * CHUNK_SIZE as i32,
                                offset_z as i32 + chunk_z as i32 * CHUNK_SIZE as i32,
                                decorator.exclusion_zone,
                            ) {
                                chunk_column_generation
                                    .decorations
                                    .push(ChunkColumnDecoration {
                                        key: decorator.prefab,
                                        x: offset_x,
                                        z: offset_z,
                                        exclusion_zone: decorator.exclusion_zone,
                                        rotation,
                                    });
                                break;
                            }
                        }
                    }
                }
            }
            Arc::new(chunk_column_generation)
        })
    }
}
