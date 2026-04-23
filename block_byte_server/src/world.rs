use std::{
    cell::{RefCell, UnsafeCell},
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    marker::PhantomData,
    mem::MaybeUninit,
    num::{NonZero, NonZeroU32},
    ops::Deref,
    path::Path,
    ptr::NonNull,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use block_byte_common::{
    CharacterController, Color, DamageTable, DamageType, InventoryView, LookDirection, MoveMode,
    SERVER_DT, SERVER_TPS,
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, Orientation, Pos},
    net::{NetworkMessageC2S, NetworkMessageS2C},
    registry::{
        BiomeKey, BlockColor, BlockData, BlockEntry, BlockInteractAction, BlockKey,
        BlockMachineFace, BlockPalette, BlockRotation, EntityInteractAction, EntityKey, KeyGroup,
        MachineInstrution, PlantKey, PrefabKey, ResearchKey, air_block,
    },
    scripts::{self, CallbackResult, ExternalScriptByteCode, ScriptState, ScriptValue},
    world::{
        BlockComponentStorage, ClientBlockComponentUpdate, ClientBlockDamage, ClientBlockMachine,
        ClientBlockPlants, ClientChunkBlockComponents, ComponentTypeAccess,
    },
};
use noise::{BasicMulti, NoiseFn, Perlin};
use palettevec::{PaletteVec, index_buffer::AlignedIndexBuffer, palette::HybridPalette};
use parking_lot::{Mutex, RwLock};
use rand::{Rng, RngCore, SeedableRng, rngs::StdRng};
use rand_seeder::Seeder;
use serde::{Deserialize, Serialize};
use slotmap::new_key_type;
use smallvec::SmallVec;
use splines::{Interpolation, Spline};
use uuid::Uuid;

use crate::{
    InventoryProvider, MessageQueue, Server, UserIndex,
    inventory::{Inventory, ItemStack, generate_loot_table, lock_inventories},
    registry::{Key, RegistryConfigLoadable},
};
#[derive(Serialize, Deserialize)]
pub struct ChunkSaveData {
    pub blocks: BlockPalette,
    pub block_events: Vec<(ChunkOffset, WorldEvent)>,
    pub components: ChunkBlockComponents,
    pub entities: Vec<Entity>,
}
pub struct Chunk {
    pub position: ChunkPos,
    pub blocks: RefCell<BlockPalette>,
    pub viewers: HashSet<UserIndex>,
    pub block_events: RefCell<VecDeque<WorldEvent>>,
    pub components: ChunkBlockComponents,
    pub entities: BTreeMap<Uuid, UnsafeCell<Entity>>,
}
pub fn tick_chunk(world: &WorldAccess) {
    for _ in 0..world.get_event_queue_length() {
        let event = world.pop_event().unwrap();
        match event {
            WorldEvent::BlockDamage { block, damage } => {
                let block_position = world.center_chunk.to_block_pos() + block.xyz();
                let Some(block) = world.get_block(block_position) else {
                    continue;
                };
                let block_data = block.block.data();
                let damage_dealt = damage
                    .iter()
                    .map(|(damage_type, damage)| {
                        damage * block_data.health.table[damage_type].unwrap_or(1.)
                    })
                    .sum::<f32>();
                let should_break_block = if damage_dealt >= block_data.health.health {
                    true
                } else {
                    let mut block_damage = world
                        .get_or_create_block_component::<BlockDamage>(block_position, || {
                            BlockDamage { damage: 0. }
                        })
                        .unwrap();
                    block_damage.damage -= damage_dealt;
                    if block_damage.damage >= block_data.health.health {
                        true
                    } else {
                        //sync
                        false
                    };
                };
                if should_break_block {
                    world.drop_items(
                        world.break_block(block_position).unwrap().into_iter(),
                        block_position.to_pos() + Pos::all(0.5),
                    );
                }
            }
            WorldEvent::BlockLogicSignal {
                block,
                value,
                world_face,
            } => todo!(),
            WorldEvent::BlockUpdateLogicState {
                block,
                value,
                world_face,
            } => todo!(),
            WorldEvent::BlockNeighborDestroyed { block, world_face } => todo!(),
            WorldEvent::BlockWakeup {
                block,
                inventory_updated,
            } => todo!(),
            WorldEvent::EntityDamage {
                entity: entity_id,
                damage,
            } => {
                let Some(mut entity) = world.get_entity(entity_id) else {
                    continue;
                };
                let entity_data = entity.key.data();
                for (damage_type, damage) in damage.iter() {
                    entity.state.health -=
                        damage * entity_data.damage_table[damage_type].unwrap_or(1.);
                }
                if entity.state.health <= 0. {
                    drop(entity); //check if necessarry
                    world.remove_entity(entity_id);
                }
            }
            WorldEvent::EntityTeleport { entity, position } => todo!(),
            WorldEvent::EntityKnockback { entity, knockback } => todo!(),
            WorldEvent::EntityClientMessage { entity, message } => todo!(),
        }
    }
    for mut machine in world.iter_block_components::<BlockMachine>() {
        let position = machine.lock_key;
        let mut machine = &mut *machine;
    }
}
impl Chunk {
    pub fn tick(&self, server: &Server) {
        let mut processing_events = Vec::new();
        std::mem::swap(&mut processing_events, &mut *self.block_events.lock());
        for (block, event) in processing_events {
            match event {
                WorldEvent::Damage {
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
                                update: damage_component
                                    .get(block)
                                    .map(|component| Into::<ClientBlockDamage>::into(component))
                                    .into(),
                            },
                        );
                    }
                }
                WorldEvent::PlayerInteract { player } => {
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
                WorldEvent::PlantHarvest { player } => {}
                WorldEvent::LogicSignal { value, world_face } => {
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
                WorldEvent::UpdateLogicState { value, world_face } => {
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
                WorldEvent::NeighborDestroyed { world_face } => {
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
                WorldEvent::Wakeup { inventory_updated } => {
                    let mut machines = self.components.machine.write();
                    if let Some(machine) = machines.get_mut(block) {
                        machine.blocked.store(false, Ordering::Relaxed);
                        if inventory_updated {
                            for to_wakeup in machine.inventory_observers.get_mut().drain(..) {
                                server.schedule_block_event(
                                    to_wakeup,
                                    WorldEvent::Wakeup {
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
                                WorldEvent::LogicSignal {
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
                                WorldEvent::UpdateLogicState {
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

#[derive(Serialize, Deserialize)]
pub enum WorldEvent {
    BlockDamage {
        block: ChunkOffset,
        damage: DamageTable,
    },
    BlockLogicSignal {
        block: ChunkOffset,
        value: ScriptValue,
        world_face: Face,
    },
    BlockUpdateLogicState {
        block: ChunkOffset,
        value: ScriptValue,
        world_face: Face,
    },
    BlockNeighborDestroyed {
        block: ChunkOffset,
        world_face: Face,
    },
    BlockWakeup {
        block: ChunkOffset,
        inventory_updated: bool,
    },
    EntityDamage {
        entity: Uuid,
        damage: DamageTable,
    },
    EntityTeleport {
        entity: Uuid,
        position: Pos,
    },
    EntityKnockback {
        entity: Uuid,
        knockback: Pos,
    },
    EntityClientMessage {
        entity: Uuid,
        message: NetworkMessageC2S,
    },
}

#[derive(Serialize, Deserialize)]
pub struct Entity {
    pub key: EntityKey,
    pub uuid: Uuid,
    pub position: Pos,
    pub inventory: Inventory,
    #[serde(default, skip_serializing, skip_deserializing)]
    pub controller: Option<UserIndex>,
    pub state: InternalEntityState,
}
#[derive(Serialize, Deserialize)]
pub struct InternalEntityState {
    pub character_controller: CharacterController,
    pub teleport: Option<Pos>,
    pub hand_slot: usize,
    #[serde(skip_serializing, skip_deserializing)]
    pub last_hand_item: Option<ItemStack>,
    pub health: f32,
    pub research: HashSet<ResearchKey>,
    pub brain: Option<MobBrain>,
    pub direction: LookDirection,
    pub crouching: bool,
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
            controller: None,
            state: InternalEntityState {
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
                crouching: false,
            },
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
            let hitbox = self.key.data().hitbox(state.crouching);
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
    pub fn get_hitbox(&self, state: &InternalEntityState) -> AABB<f32> {
        let entity_data = self.key.data();
        entity_data.hitbox(state.crouching).offset(self.position)
    }
}
impl Entity {
    pub fn create_add_message(&self) -> NetworkMessageS2C {
        let hand_slot = self.state.lock().hand_slot;
        let state = self.state.lock();
        NetworkMessageS2C::AddEntity {
            uuid: self.uuid,
            key: self.key,
            position: self.position,
            direction: state.direction,
            crouching: state.crouching,
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
        let state = self.state.lock();
        NetworkMessageS2C::MoveEntity {
            uuid: self.uuid,
            position: self.position,
            direction: state.direction,
            crouching: state.crouching,
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
                //#[serde(skip_serializing_if = std::concat!("BlockComponentStorage::<", std::stringify!($type), ">::is_empty"), default)]
                pub $id: BlockComponentStorage<UnsafeCell<$type>>,
            )*
        }
        $(
            impl ComponentTypeAccess<$type> for ChunkBlockComponents{
                type Item = BlockComponentStorage<UnsafeCell<$type>>;
                fn get_component_type(&self) -> &Self::Item{
                    &self.$id
                }
                fn get_component_type_mut(&mut self) -> &mut Self::Item{
                    &mut self.$id
                }
            }
        )*
        pub struct ChunkBlockComponentsMap<T>{
            $(
                pub $id: T,
            )*
        }
        impl<T> ChunkBlockComponentsMap<T>{
            pub fn iter(&self) -> impl Iterator<Item = &T>{
                [$(&$id,)*].into_iter()
            }
        }
        $(
            impl<T> ComponentTypeAccess<$type> for ChunkBlockComponentsMap<T>{
                type Item = T;
                fn get_component_type(&self) -> &Self::Item{
                    &self.$id
                }
                fn get_component_type_mut(&mut self) -> &mut Self::Item{
                    &mut self.$id
                }
            }
        )*
    }
}
trait BlockComponentUpdater {
    fn client_update(&self) -> Option<ClientBlockComponentUpdate>;
    fn client_empty_update() -> Option<ClientBlockComponentUpdate>;
}
macro_rules! create_chunk_block_components_client_mapping {
    ($($client: ident, $server: ident, $id:ident);*) => {
        impl ChunkBlockComponents {
            pub fn client(&self) -> ClientChunkBlockComponents {
                ClientChunkBlockComponents {
                    $(
                        $id: self.$id.into(),
                    )*
                }
            }
        }
        $(
            impl BlockComponentUpdater for $server {
                fn client_update(&self) -> Option<ClientBlockComponentUpdate>{
                    Some(ClientBlockComponentUpdate::$client(Some(self.into())))
                }
                fn client_empty_update() -> Option<ClientBlockComponentUpdate>{
                    Some(ClientBlockComponentUpdate::$client(None))
                }
            }
        )*
    };
}
macro_rules! create_chunk_block_components_server_only {
    ($($server: ident);*) => {
        $(
            impl BlockComponentUpdater for $server {
                fn client_update(&self) -> Option<ClientBlockComponentUpdate>{
                    None
                }
                fn client_empty_update() -> Option<ClientBlockComponentUpdate>{
                    None
                }
            }
        )*
    };
}

create_chunk_block_components!(BlockDamage, damage; BlockPlants, plant; BlockMachine, machine);
create_chunk_block_components_client_mapping!(BlockDamage, ClientBlockDamage, damage; BlockPlant, ClientBlockPlants, plant; BlockMachine, ClientBlockMachine, machine);
create_chunk_block_components_server_only!();

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
    pub inventory: Inventory,
    pub sleep_cooldown: u32,
    pub script_state: ScriptState,
    pub logic_state: FaceMap<Option<ScriptValue>>,
    pub blocked: bool,
    pub inventory_observers: SmallVec<[BlockPos; 1]>,
    pub current_animation: u16,
    pub animation_start_time: u64,
}

impl Into<ClientBlockMachine> for &BlockMachine {
    fn into(self) -> ClientBlockMachine {
        ClientBlockMachine {
            animation: self.current_animation,
            animation_start_time: self.animation_start_time,
        }
    }
}
trait ComponentStorageAccess<C> {
    fn get_component_storage(&self) -> &BlockComponentStorage<UnsafeCell<C>>;
}
impl<C> ComponentStorageAccess<C> for ChunkBlockComponents
where
    ChunkBlockComponents: ComponentTypeAccess<C, Item = BlockComponentStorage<UnsafeCell<C>>>,
    C: BlockComponentUpdater,
{
    fn get_component_storage(&self) -> &BlockComponentStorage<UnsafeCell<C>> {
        <ChunkBlockComponents as ComponentTypeAccess<
            C,
            Item = BlockComponentStorage<UnsafeCell<C>>,
        >>::get_component_type(self)
    }
}
impl ChunkBlockComponents {
    pub fn get_components<C>(&self) -> &BlockComponentStorage<UnsafeCell<C>>
    where
        ChunkBlockComponents: ComponentStorageAccess<C>,
    {
        <ChunkBlockComponents as ComponentStorageAccess<C>>::get_component_storage(&self)
    }
}
pub struct WorldAccess {
    ticks_passed: u64,
    center_chunk: ChunkPos,
    grid: [Option<&mut Chunk>; 27],
    message_queue: &MessageQueue,
    entity_locks: RefCell<Vec<Uuid>>,
    entities_added: RefCell<BTreeMap<Uuid, Box<UnsafeCell<Entity>>>>,
    entities_removed: RefCell<Vec<Uuid>>,
    block_component_locks: ChunkBlockComponentsMap<RefCell<Vec<BlockPos>>>,
    block_components_added: ChunkBlockComponentsMap<RefCell<BTreeMap<BlockPos, Box<UnsafeCell>>>>,
    block_components_removed: ChunkBlockComponentsMap<RefCell<Vec<BlockPos>>>,
}
impl WorldAccess {
    const GRID_CENTER: usize = 1 + 3 + 9;
    fn get_grid_index(&self, chunk: ChunkPos) -> Option<usize> {
        let x_diff = chunk.x - self.center_chunk.x + 1;
        let y_diff = chunk.x - self.center_chunk.x + 1;
        let z_diff = chunk.x - self.center_chunk.x + 1;
        if x_diff < 0 || x_diff > 2 || y_diff < 0 || y_diff > 2 || z_diff < 0 || z_diff > 2 {
            return None;
        }
        Some(x_diff as usize + y_diff as usize * 3 + z_diff as usize * 9)
    }
    pub fn get_block(&self, position: BlockPos) -> Option<BlockEntry> {
        let (chunk, offset) = position.to_chunk_pos_offset();
        Some(
            *self.grid[self.get_grid_index(chunk)?]?
                .blocks
                .borrow()
                .get(offset.index())
                .unwrap(),
        )
    }
    pub fn replace_block(&self, position: BlockPos, block: BlockEntry) -> Result<BlockEntry, ()> {
        let (chunk, offset) = position.to_chunk_pos_offset();
        match self
            .get_grid_index(chunk)
            .and_then(|chunk| self.grid[chunk])
        {
            Some(chunk) => {
                let mut blocks = chunk.blocks.borrow_mut();
                let previous = *blocks.get(offset.index()).unwrap();
                blocks.set(offset.index(), &block);
                Ok(previous)
            }
            None => Err(()),
        }
    }
    pub fn break_block(&self, position: BlockPos) -> Result<Vec<ItemStack>, ()> {
        let previous_block = self.replace_block(position, BlockEntry::simple(air_block()))?;
        let block_data = previous_block.block.data();
        let _ = self.remove_block_component::<BlockDamage>(position);
        let mut drops = generate_loot_table(block_data.loot_table.data());

        //todo: harvest
        let _ = self.remove_block_component::<BlockPlants>(position);

        if let Some(mut machine) = self.get_block_component::<BlockMachine>(position) {
            //todo: this should probably be returned in remove_block_component
            for item in &mut machine.inventory.items {
                if let Some(item) = item.take() {
                    drops.push(item);
                }
            }
            drop(machine);
            self.remove_block_component::<BlockMachine>(position);
        }
        for face in Face::all() {
            let neighbor_position = position + face.get_block_offset();
            let (chunk, offset) = neighbor_position.to_chunk_pos_offset();
            self.schedule_event(
                chunk,
                WorldEvent::BlockNeighborDestroyed {
                    block: offset,
                    world_face: face.opposite(),
                },
            );
        }
        Ok(drops)
    }
    pub fn place_block(&self, position: BlockPos, block: BlockEntry) -> Result<(), ()> {
        let block_data = block.block.data();
        if let Some(hanging) = block_data.hanging {
            let world_hanging = block.rotation.rotate_face(hanging);
            let Some(hanging_block) = self.get_block(position + world_hanging.get_block_offset())
            else {
                return Err(());
            };
            if !hanging_block.supports(world_hanging.opposite()) {
                return Err(());
            }
        }
        match self.get_block(position) {
            Some(block) => {
                if block.block != air_block() {
                    return Err(());
                }
            }
            None => return Err(()),
        }
        self.replace_block(position, block).unwrap();
        if let Some(machine_data) = &block_data.machine {
            self.get_or_create_block_component(position, || BlockMachine {
                inventory: Inventory::new(machine_data.inventory_size),
                sleep_cooldown: 0,
                script_state: ScriptState::new(&machine_data.script),
                logic_state: Default::default(),
                blocked: false,
                inventory_observers: SmallVec::new(),
                current_animation: 0,
                animation_start_time: self.ticks_passed,
            })
            .unwrap();
        }
        Ok(())
    }
    pub fn get_block_component<'a, C>(
        &'a self,
        position: BlockPos,
    ) -> Option<WorldAccessRef<'a, C, BlockPos>>
    where
        ChunkBlockComponents: ComponentStorageAccess<C>,
    {
        if let Some(block) = self.block_components_added.get().borrow().get(&position) {
            self.block_component_locks.borrow_mut().push(position);
            return Some(WorldAccessRef {
                value: unsafe { NonNull::new_unchecked(block.get()) },
                borrow: &self.block_component_locks,
                lock_key: uuid,
                _marker: PhantomData,
            });
        }
        let (chunk, offset) = position.to_chunk_pos_offset();
    }
    pub fn iter_block_components<'a, C>(
        &'a self,
        exclude: &[BlockPos],
    ) -> impl Iterator<Item = WorldAccessRef<'a, C, BlockPos>>
    where
        ChunkBlockComponents: ComponentStorageAccess<C>,
    {
        self.grid
            .as_ref()
            .iter()
            .filter_map(|chunk| {
                chunk.map(|chunk| {
                    chunk
                        .components
                        .get_components::<C>()
                        .iter()
                        .map(|(offset, _)| chunk.position.to_block_pos() + offset.xyz())
                })
            })
            .flatten()
            .filter(|position| !exclude.contains(position))
            .filter_map(|position| self.get_block_component::<C>(position))
    }
    pub fn remove_block_component<C>(&self, position: BlockPos) -> Result<(), ()>
    where
        ChunkBlockComponents: ComponentStorageAccess<C>,
    {
    }
    pub fn get_or_create_block_component<'a, C>(
        &'a self,
        position: BlockPos,
        init: impl FnOnce() -> C,
    ) -> Result<WorldAccessRef<'a, C, BlockPos>, ()>
    where
        ChunkBlockComponents: ComponentStorageAccess<C>,
    {
    }
    pub fn sync_block_component<C>(&self, position: BlockPos) -> Result<(), ()>
    where
        ChunkBlockComponents: ComponentStorageAccess<C>,
        C: BlockComponentUpdater,
    {
        let Some(component) = self.get_block_component(position) else {
            return Err(());
        };
        let Some(update) = (*component).client_update() else {
            return Ok(());
        };
        let (chunk, offset) = position.to_chunk_pos_offset();
        self.send_viewers(
            chunk,
            NetworkMessageS2C::UpdateBlockComponents {
                chunk,
                offset,
                update,
            },
        );
        Ok(())
    }
    pub fn schedule_event(&self, chunk: ChunkPos, event: WorldEvent) -> Result<(), ()> {
        match self.grid[self.get_grid_index(chunk)?] {
            Some(chunk) => {
                chunk.block_events.borrow_mut().push_front(event);
                Ok(())
            }
            None => Err(()),
        }
    }
    pub fn get_event_queue_length(&self) -> usize {
        self.grid[WorldAccess::GRID_CENTER]
            .as_ref()
            .unwrap()
            .block_events
            .borrow()
            .len()
    }
    pub fn pop_event(&self) -> Option<WorldEvent> {
        self.grid[WorldAccess::GRID_CENTER]
            .as_ref()
            .unwrap()
            .block_events
            .borrow_mut()
            .pop_back()
    }
    pub fn send(&self, user: UserIndex, message: NetworkMessageS2C) {
        self.message_queue
            .send_message(std::iter::once(user), message);
    }
    pub fn send_viewers(&self, chunk: ChunkPos, message: NetworkMessageS2C) {
        if let Some(chunk) = self.get_grid_index(chunk) {
            if let Some(chunk) = self.grid[chunk] {
                self.message_queue
                    .send_message(chunk.viewers.iter(), message);
            }
        }
    }
    pub fn send_self_viewers(&self, message: NetworkMessageS2C) {
        self.send_viewers(self.center_chunk, message);
    }
    pub fn get_entity<'a>(&'a self, uuid: Uuid) -> Option<WorldAccessRef<'a, Entity, Uuid>> {
        if self.entity_locks.borrow().contains(&uuid) {
            panic!("attempted reborrow");
        }
        if self.entities_removed.borrow().contains(&uuid) {
            return None;
        }
        if let Some(entity) = self.entities_added.borrow().get(&uuid) {
            self.entity_locks.borrow_mut().push(uuid);
            return Some(WorldAccessRef {
                value: unsafe { NonNull::new_unchecked(entity.get()) },
                borrow: &self.entity_locks,
                lock_key: uuid,
                _marker: PhantomData,
            });
        }
        for cell in &self.grid {
            if let Some(cell) = cell {
                if let Some(entity) = cell.entities.get(&uuid) {
                    self.entity_locks.borrow_mut().push(uuid);
                    return Some(WorldAccessRef {
                        value: unsafe { NonNull::new_unchecked(entity.get()) },
                        borrow: &self.entity_locks,
                        lock_key: uuid,
                        _marker: PhantomData,
                    });
                }
            }
        }
        None
    }
    pub fn iter_entities<'a>(
        &'a self,
        exclude: &[Uuid],
    ) -> impl Iterator<Item = WorldAccessRef<'a, Entity, Uuid>> {
        self.grid
            .as_ref()
            .iter()
            .filter_map(|chunk| chunk.map(|chunk| chunk.entities.keys()))
            .flatten()
            .filter(|uuid| !exclude.contains(*uuid))
            .filter_map(|uuid| self.get_entity(*uuid))
    }
    pub fn spawn_entity<'a>(
        &'a self,
        entity: Entity,
    ) -> Result<WorldAccessRef<'a, Entity, Uuid>, ()> {
        if self.grid[self
            .get_grid_index(entity.position.to_chunk_pos())
            .ok_or(())?]
        .is_none()
        {
            return Err(());
        }
        let uuid = entity.uuid;
        self.entities_added
            .borrow_mut()
            .insert(entity.uuid, Box::new(UnsafeCell::new(entity)));
        Ok(self.get_entity(uuid).unwrap())
    }
    pub fn drop_items(
        &self,
        items: impl Iterator<Item = ItemStack>,
        position: Pos,
    ) -> Result<(), ()> {
        let item_entity_key = EntityKey::id("item").unwrap();
        for item in items {
            let mut item_entity = Entity::new(item_entity_key, position);
            item_entity.state.character_controller.velocity = Pos {
                x: rand::random::<f32>() * 2. - 1.,
                y: rand::random::<f32>(),
                z: rand::random::<f32>() * 2. - 1.,
            };
            item_entity.inventory.items[0] = Some(item);
            let _ = self.spawn_entity(item_entity);
        }
        Ok(())
    }
    pub fn remove_entity(&self, uuid: Uuid) -> Result<(), ()> {
        if self.get_entity(uuid).is_none() {
            return Err(());
        }
        self.entities_removed.borrow_mut().push(uuid);
        Ok(())
    }
}
impl Drop for WorldAccess {
    fn drop(&mut self) {
        for (id, entity) in self.entities_added.borrow_mut().extract_if(.., |_, _| true) {
            let entity = entity.into_inner();
            let chunk = entity.position.to_chunk_pos();
            let chunk = self.get_grid_index(chuk).unwrap();
            assert!(
                self.grid[chunk]
                    .as_mut()
                    .unwrap()
                    .insert(id, *entity)
                    .is_none()
            );
        }
        for entity in self.entities_removed.drain(..) {
            let chunk = self.get_entity(entity).unwrap().position.to_chunk_pos();
            let chunk = entity.position.to_chunk_pos();
            let chunk = self.get_grid_index(chunk).unwrap();
            assert!(self.grid[chunk].as_mut().unwrap().remove(&entity).is_some());
        }
    }
}

pub struct WorldAccessRef<'b, T: 'b, L: PartialEq> {
    //noalias
    value: NonNull<T>,
    borrow: &RefCell<Vec<L>>,
    lock_key: L,
    _marker: PhantomData<&'b mut T>,
}
impl<T, L: PartialEq> Drop for WorldAccessRef<T, L> {
    fn drop(&mut self) {
        let mut locks = self.borrow.borrow_mut();
        locks.remove(
            locks
                .iter()
                .position(|value| value == self.lock_key)
                .unwrap(),
        );
    }
}
impl<T> std::ops::Deref for WorldAccessRef<'_, T, _> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}
impl<T> std::ops::DerefMut for WorldAccessRef<'_, T, _> {
    fn deref_mut(&self) -> &mut Self::Target {
        unsafe { self.value.as_mut() }
    }
}
