use std::{
    cell::{RefCell, UnsafeCell},
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    marker::PhantomData,
    ptr::NonNull,
};

use block_byte_common::{
    ACCELERATION_COEFFICIENT, ActiveEffect, CharacterController, DamageTable, DamageType,
    EntityAction, EntityPose, EntityResearchProgress, EntityStats, HitTimer, InternString,
    LookDirection, MoveMode, NORMAL_SPEED, PassiveAbility, SERVER_DT, SERVER_TPS,
    coord::{
        self, AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, FaceSet,
        HorizontalFace, Pos, Ray,
    },
    net::{NetworkMessageS2C, PropertyModifyMode},
    registry::{
        BlockEntry, BlockMachineData, BlockMachineFace, BlockPalette, EffectKey, EntityKey,
        MachineInstrution, PlantKey, ToolData, air_block,
    },
    scripts::{CallbackResult, RunResult, ScriptState, ScriptValue},
    time_to_ticks,
    ui::PropertyMap,
    world::{
        BlockComponentStorage, ClientBlockComponentUpdate, ClientBlockDamage, ClientBlockMachine,
        ClientBlockPlants, ClientChunkBlockComponents, ComponentTypeAccess,
    },
};
use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use smallvec::SmallVec;
use uuid::Uuid;

use crate::{
    InventoryProvider, MessageQueue, User, UserIndex, UserScreenState,
    inventory::{
        Inventory, ItemComponentPassiveAbility, ItemCraftStats, ItemQuality, ItemStack,
        LootGenerationContext, generate_loot_table,
    },
};
#[derive(Serialize, Deserialize)]
pub struct ChunkSaveData {
    pub blocks: BlockPalette,
    pub block_events: VecDeque<WorldEvent>,
    pub components: ChunkBlockComponents,
    pub entities: Vec<Entity>,
}
pub struct ChunkBlocks(UnsafeCell<BlockPalette>);
impl ChunkBlocks {
    pub fn empty() -> ChunkBlocks {
        ChunkBlocks::new(BlockPalette::filled(
            BlockEntry::simple(air_block()),
            CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE,
        ))
    }
    pub fn new(palette: BlockPalette) -> ChunkBlocks {
        ChunkBlocks(UnsafeCell::new(palette))
    }
    pub fn get(&self, offset: ChunkOffset) -> BlockEntry {
        let blocks = unsafe { &mut *self.0.get() };
        *blocks.get(offset.index()).unwrap()
    }
    pub fn set(&self, offset: ChunkOffset, block: BlockEntry) {
        let blocks = unsafe { &mut *self.0.get() };
        blocks.set(offset.index(), &block);
    }
    pub fn get_mut(&mut self) -> &mut BlockPalette {
        self.0.get_mut()
    }
    pub fn into_inner(self) -> BlockPalette {
        self.0.into_inner()
    }
}
pub struct Chunk {
    pub position: ChunkPos,
    pub blocks: ChunkBlocks,
    pub viewers: HashSet<UserIndex>,
    pub events: RefCell<VecDeque<WorldEvent>>,
    pub components: ChunkBlockComponents,
    pub entities: BTreeMap<Uuid, WorldAccessCell<Entity>>,
}
pub fn tick_chunk(world: &WorldAccess) {
    for _ in 0..world.get_event_queue_length() {
        let event = world.pop_event().unwrap();
        match event {
            WorldEvent::BlockDamage {
                block,
                damage,
                source_entity,
            } => {
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
                    block_damage.damage += damage_dealt;
                    if block_damage.damage >= block_data.health.health {
                        true
                    } else {
                        world.sync_block_component(&block_damage);
                        false
                    }
                };
                if should_break_block {
                    let mut items = world.break_block(block_position).unwrap();
                    if let Some(source_entity) = source_entity {
                        if let Some(mut source_entity) = world.get_entity(source_entity) {
                            let view = source_entity.key.data().pickup_view();
                            items.retain_mut(|item| {
                                match source_entity.inventory.add_item(view, item.clone()) {
                                    Some(overflow) => {
                                        *item = overflow;
                                        true
                                    }
                                    None => false,
                                }
                            });
                        }
                    }
                    world.drop_items(items.into_iter(), block_position.to_pos() + Pos::all(0.5));
                }
            }
            WorldEvent::BlockLogicSignal {
                block,
                value,
                world_face,
            } => {
                let block_position = world.center_chunk.to_block_pos() + block.xyz();
                let Some(block) = world.get_block(block_position) else {
                    continue;
                };
                let block_data = block.block.data();
                if let Some(machine_data) = &block_data.machine {
                    let mut machine = world
                        .get_block_component::<BlockMachine>(block_position)
                        .unwrap();
                    let own_face = block.rotation.inverse_rotate_face(world_face);
                    match machine_data.faces.by_face(own_face) {
                        BlockMachineFace::SignalInput => {
                            world
                                .wakeup_component::<BlockMachine>(block_position)
                                .unwrap();
                            *machine.logic_state.by_face_mut(own_face) = Some(value);
                        }
                        _ => {}
                    }
                }
            }
            WorldEvent::BlockLogicState {
                block,
                value,
                world_face,
            } => {
                let block_position = world.center_chunk.to_block_pos() + block.xyz();
                let Some(block) = world.get_block(block_position) else {
                    continue;
                };
                let block_data = block.block.data();
                if let Some(machine_data) = &block_data.machine {
                    let mut machine = world
                        .get_block_component::<BlockMachine>(block_position)
                        .unwrap();
                    let own_face = block.rotation.inverse_rotate_face(world_face);
                    match machine_data.faces.by_face(own_face) {
                        BlockMachineFace::LogicInput => {
                            world
                                .wakeup_component::<BlockMachine>(block_position)
                                .unwrap();
                            *machine.logic_state.by_face_mut(own_face) = Some(value);
                        }
                        _ => {}
                    }
                }
            }
            WorldEvent::BlockNeighborDestroyed { block, world_face } => {
                let block_position = world.center_chunk.to_block_pos() + block.xyz();
                let Some(block) = world.get_block(block_position) else {
                    continue;
                };
                let face = block.rotation.inverse_rotate_face(world_face);
                let block_data = block.block.data();
                if let Some(support_face) = block_data.hanging {
                    if face == support_face {
                        world.drop_items(
                            world.break_block(block_position).unwrap().into_iter(),
                            block_position.to_pos() + Pos::all(0.5),
                        );
                        continue;
                    }
                }
                if let Some(machine_data) = &block_data.machine {
                    match machine_data.faces.by_face(face) {
                        BlockMachineFace::LogicInput => {
                            let mut machine = world
                                .get_block_component::<BlockMachine>(block_position)
                                .unwrap();
                            *machine.logic_state.by_face_mut(face) = None;
                            world
                                .wakeup_component::<BlockMachine>(block_position)
                                .unwrap();
                        }
                        _ => {}
                    }
                }
            }
            WorldEvent::BlockWakeup {
                block,
                inventory_updated,
            } => {
                let block_position = world.center_chunk.to_block_pos() + block.xyz();
                if let Some(mut machine) = world.get_block_component::<BlockMachine>(block_position)
                {
                    world
                        .wakeup_component::<BlockMachine>(block_position)
                        .unwrap();
                    if inventory_updated {
                        for face in machine.inventory_observers.iter_set() {
                            let (target_chunk, target_offset) =
                                (face.get_block_offset() + block_position).to_chunk_pos_offset();
                            world
                                .schedule_event(
                                    target_chunk,
                                    WorldEvent::BlockWakeup {
                                        block: target_offset,
                                        inventory_updated: false,
                                    },
                                )
                                .unwrap();
                        }
                        machine.inventory_observers.clear();
                    }
                }
            }
        }
    }
    for mut entity_ref in world.iter_entities(&[], true) {
        let entity_data = entity_ref.key.data();
        let entity = &mut *entity_ref;
        let mut effects_modified = false;
        entity
            .effects
            .extract_if(|_, instance| {
                effects_modified |= instance.tick();
                instance.is_empty()
            })
            .count();
        if entity.inventory.modified || effects_modified {
            entity.inventory.modified = false;
            entity.current_passives.clear();
            let mut stats = entity.key.data().base_stats.clone();
            for (effect, instance) in &entity.effects {
                stats.apply(&effect.data().stats, instance.get_level());
            }
            let mut apply_item = |item: &ItemStack| {
                let quality_multiplier = item
                    .components
                    .get_component::<ItemQuality>()
                    .map(|quality| quality.factor())
                    .unwrap_or(1.);
                stats.apply(&item.item.data().equip_stats, quality_multiplier);
                if let Some(craft_stats) = item.components.get_component::<ItemCraftStats>() {
                    stats.apply(&craft_stats.0, quality_multiplier);
                }
                if let Some(passive) = item
                    .components
                    .get_component::<ItemComponentPassiveAbility>()
                {
                    entity.current_passives.insert(passive.0);
                }
            };
            for equipment in &entity.key.data().equipment_view.slots {
                if let Some(item) = &entity.inventory.get_slot_raw(equipment.slot) {
                    apply_item(item);
                }
            }
            if let Some(item) = &entity.inventory.get_slot_raw(entity.hand_slot) {
                apply_item(item);
            }
            if *entity.current_stats != stats {
                if let Some(controlling_user) = entity.controlling_user {
                    world.send(
                        controlling_user,
                        NetworkMessageS2C::UpdatePlayerStats {
                            stats: stats.clone(),
                        },
                    );
                }
                *entity.current_stats = stats;
            }
        }
        if world.ticks_passed % SERVER_TPS as u64 == 0 {
            entity.health += entity.current_stats.regen();
        }
        entity.health = entity.health.min(entity.current_stats.vitality());
        if let Some(controlling_user) = entity.controlling_user {
            let mut velocity = Pos::ZERO;
            std::mem::swap(&mut velocity, &mut entity.character_controller.velocity);
            if velocity.length_squared() > 0. {
                world.send(controlling_user, NetworkMessageS2C::Knockback { velocity });
            }
            {
                //println!("entity {:?}", world.center_chunk);
                let Some(user) = world.users.get(controlling_user) else {
                    continue;
                };
                {
                    let mut last_update = user.player_sync_items.lock();
                    last_update.resize(entity.inventory.size(), None);
                    for (i, item) in entity.inventory.iter().enumerate() {
                        let item = item.as_ref().cloned().map(ItemStack::client);
                        if item != last_update[i] {
                            last_update[i] = item.clone();
                            world.send(
                                controlling_user,
                                NetworkMessageS2C::PlayerSetSlot { slot: i, item },
                            );
                        }
                    }
                    world.send(
                        controlling_user,
                        NetworkMessageS2C::HudBarUpdate {
                            health: entity.health,
                        },
                    );
                    let mut screen_lock = user.screen.lock();
                    if let Some(screen) = &mut *screen_lock {
                        let mut items = Vec::new();
                        let mut should_close = false;
                        let mut properties = PropertyMap(HashMap::new());
                        let screen_data = screen.screen.data();
                        let mut load_inventory =
                            |inventory: &Inventory| {
                                items.extend(screen.view.slots.iter().map(|i| {
                                    inventory.get_slot_raw(i.slot).map(|item| item.client())
                                }));
                            };
                        let access_distance = 10.;
                        match screen.provider {
                            InventoryProvider::Entity(uuid) => {
                                if let Some(other_entity) = world.get_entity(uuid) {
                                    if other_entity.position.distance(entity.position)
                                        > access_distance
                                    {
                                        should_close = true;
                                    }
                                    load_inventory(&other_entity.inventory);
                                } else {
                                    should_close = true;
                                }
                            }
                            InventoryProvider::Block(position) => {
                                if let Some(machine) =
                                    world.get_block_component::<BlockMachine>(position)
                                {
                                    if (position.to_pos() + Pos::all(0.5)).distance(entity.position)
                                        > access_distance
                                    {
                                        should_close = true;
                                    }
                                    load_inventory(&machine.inventory);
                                    let machine_data = world
                                        .get_block(position)
                                        .unwrap()
                                        .block
                                        .data()
                                        .machine
                                        .as_ref()
                                        .unwrap();
                                    for property in &screen_data.display_properties {
                                        if let Some(register_id) = machine_data
                                            .script
                                            .named_registers
                                            .iter()
                                            .position(|p| p == property)
                                        {
                                            properties.0.insert(
                                                property.clone(),
                                                machine.script_state.registers[register_id] as f32,
                                            );
                                        }
                                    }
                                } else {
                                    should_close = true;
                                };
                            }
                            InventoryProvider::None => {}
                        }
                        if !should_close {
                            match screen.state {
                                UserScreenState::Open => {
                                    world.send(
                                        controlling_user,
                                        NetworkMessageS2C::UIOpen {
                                            screen: screen.screen,
                                            slots: items.clone(),
                                            properties,
                                        },
                                    );
                                    screen.previous_items = items;
                                    screen.state = UserScreenState::Normal;
                                }
                                UserScreenState::Normal => {
                                    for (property, value) in properties.0 {
                                        let old_value = screen
                                            .previous_properties
                                            .0
                                            .entry(property.clone())
                                            .or_insert(0.);
                                        if *old_value != value {
                                            *old_value = value;
                                            world.send(
                                                controlling_user,
                                                NetworkMessageS2C::UISetProperty {
                                                    property,
                                                    value,
                                                },
                                            );
                                        }
                                    }
                                    for (slot, (previous, new)) in
                                        screen.previous_items.iter().zip(items.iter()).enumerate()
                                    {
                                        if previous != new {
                                            world.send(
                                                controlling_user,
                                                NetworkMessageS2C::UISetSlot {
                                                    slot,
                                                    item: new.clone(),
                                                },
                                            );
                                        }
                                    }
                                    screen.previous_items = items;
                                }
                                UserScreenState::Close => {
                                    should_close = true;
                                }
                            }
                        }
                        if should_close {
                            world.send(controlling_user, NetworkMessageS2C::UIClose);
                            *screen_lock = None;
                        }
                    }
                }
                user.tick_controlling_entity(entity, controlling_user, world);
            }
        } else {
            let mut move_vector = Pos::ZERO;
            entity.tick(&mut move_vector, world);
            entity.pose = if move_vector.length_squared() > 0. {
                EntityPose::Walk
            } else {
                EntityPose::Stand
            };
            let mut new_position = entity.position;
            entity.character_controller.tick(
                &mut new_position,
                SERVER_DT,
                |block| world.get_block(block),
                move_vector,
                MoveMode::Normal,
                entity_data.hitbox(entity.pose),
                ACCELERATION_COEFFICIENT * entity_data.base_stats.speed() / 100. * NORMAL_SPEED,
                0.5,
                false,
            );
            if new_position != entity.position {
                world.teleport_entity(entity, new_position).unwrap();
            }
        }
        {
            let new_hand_item = entity.inventory.get_slot_raw(entity.hand_slot).cloned();
            if new_hand_item != entity.last_hand_item {
                world.send_viewers(
                    entity.position.to_chunk_pos(),
                    NetworkMessageS2C::EntityHandItem {
                        uuid: entity.uuid,
                        item: new_hand_item.as_ref().map(|item| item.client()),
                    },
                );
                world.send_viewers(
                    entity.position.to_chunk_pos(),
                    NetworkMessageS2C::EntityAction {
                        entity: entity.uuid,
                        action: EntityAction::Equip,
                    },
                );
                entity.last_hand_item = new_hand_item;
            }
        }
        if entity.health <= 0. {
            world.remove_entity(entity_ref);
        }
    }
    if world.grid.iter().all(Option::is_some) {
        let machine_components = &world.grid[WorldAccess::GRID_CENTER]
            .as_ref()
            .unwrap()
            .components
            .machine;
        let mut tick_list = machine_components.tick_list.lock().unwrap();
        tick_list.process_timer(&machine_components.tree);
        let mut iteration_index = tick_list.start_index();
        while let Some(index) = tick_list.next_index(&mut iteration_index) {
            let mut machine = world.get_center_block_component_by_id::<BlockMachine>(index);
            let block_position = machine.lock_key;
            let block = world.get_block(block_position).unwrap();
            let machine_data = block.block.data().machine.as_ref().unwrap();
            match machine.tick(block_position, block, machine_data, world) {
                MachineRunResult::Continue => {}
                MachineRunResult::Block => {
                    tick_list.set_ticking(index, false);
                }
                MachineRunResult::Sleep(time) => {
                    tick_list.set_ticking(index, false);
                    tick_list.schedule_wakeup(block_position.to_chunk_offset(), time);
                }
            }
            if machine.animation_start_time == world.ticks_passed {
                world.sync_block_component(&machine);
            }
        }
    }

    for mut damage in world.iter_block_components::<BlockDamage>(&[], true) {
        let block_position = damage.lock_key;
        let block = world.get_block(block_position).unwrap();
        let block_data = block.block.data();
        damage.damage -= block_data.health.health_regen * SERVER_DT;
        if damage.damage <= 0. {
            world.remove_block_component(damage);
        }
    }
    if (world.center_chunk.x as u32 * 3278
        + world.center_chunk.y as u32 * 9841
        + world.center_chunk.z as u32 * 87
        + world.ticks_passed as u32)
        % (10 * SERVER_TPS)
        == 0
    {
        for plants in world.iter_block_components::<BlockPlants>(&[], true) {
            let Some(block) = world.get_block(plants.lock_key + BlockPos::Y) else {
                continue;
            };
            if block.block != air_block() {
                //todo: maybe check tag?
                world.remove_block_component(plants);
            } else {
                //grow
            }
        }
    }
    let added_machines: Vec<_> = world
        .block_components
        .machine
        .added
        .borrow()
        .keys()
        .cloned()
        .collect();
    for added_machine in added_machines {
        let block_entry = world.get_block(added_machine).unwrap();
        let machine_data = block_entry.block.data().machine.as_ref().unwrap();
        let mut machine = world
            .get_block_component::<BlockMachine>(added_machine)
            .unwrap();
        for face in Face::all() {
            match machine_data.faces.by_face(face) {
                BlockMachineFace::LogicInput => {
                    let world_face = block_entry.rotation.rotate_face(face);
                    let other_position = added_machine + world_face.get_block_offset();
                    let other_block = world.get_block(other_position).unwrap();
                    let other_machine = world
                        .get_block_component::<BlockMachine>(other_position)
                        .unwrap();
                    let other_machine_data = other_block.block.data().machine.as_ref().unwrap();
                    let other_face = other_block
                        .rotation
                        .inverse_rotate_face(world_face.opposite());
                    match other_machine_data.faces.by_face(other_face) {
                        BlockMachineFace::LogicOutput => {
                            *machine.logic_state.by_face_mut(face) =
                                *other_machine.logic_state.by_face(other_face);
                        }
                        _ => {}
                    }
                }
                BlockMachineFace::Inventory { .. } => {
                    let world_face = block_entry.rotation.rotate_face(face);
                    let other_position = added_machine + world_face.get_block_offset();
                    let _ = world.wakeup_component::<BlockMachine>(other_position);
                }
                _ => {}
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum WorldEvent {
    BlockDamage {
        block: ChunkOffset,
        damage: DamageTable,
        source_entity: Option<Uuid>,
    },
    BlockLogicSignal {
        block: ChunkOffset,
        value: ScriptValue,
        world_face: Face,
    },
    BlockLogicState {
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
}

#[derive(Serialize, Deserialize)]
pub struct Entity {
    pub key: EntityKey,
    pub uuid: Uuid,
    pub position: Pos,
    pub inventory: Inventory,
    #[serde(default, skip_serializing, skip_deserializing)]
    pub controlling_user: Option<UserIndex>,
    pub character_controller: CharacterController,
    pub hand_slot: usize,
    #[serde(skip_serializing, skip_deserializing)]
    pub last_hand_item: Option<ItemStack>,
    pub health: f32,
    pub research: Option<EntityResearchProgress>,
    pub brain: Option<Box<MobBrain>>,
    pub direction: LookDirection,
    pub pose: EntityPose,
    pub effects: HashMap<EffectKey, ActiveEffect>,
    #[serde(skip_serializing, skip_deserializing)]
    pub current_stats: Box<EntityStats>,
    #[serde(skip_serializing, skip_deserializing)]
    pub current_passives: HashSet<PassiveAbility>,
}
#[derive(Serialize, Deserialize)]
pub struct MobBrainTarget {
    pub id: Uuid,
    pub last_seen_position: Pos,
    pub score: f32,
}
#[derive(Serialize, Deserialize)]
pub struct MobBrain {
    pub goal: Option<Pos>,
    pub path: Vec<Pos>,
    pub received_attacks: HashMap<Uuid, f32>,
    pub target: Option<MobBrainTarget>,
    pub hit_timer: Option<HitTimer>,
    pub guard_position: BlockPos,
}
impl MobBrain {
    pub fn new(guard_position: BlockPos) -> Self {
        Self {
            goal: None,
            path: Vec::new(),
            received_attacks: HashMap::new(),
            hit_timer: None,
            target: None,
            guard_position,
        }
    }
    pub fn recalculate_path(&mut self, position: Pos, world: &WorldAccess, eye_height: f32) {
        if let Some(goal) = self.goal {
            let goal_block = goal.to_block_pos();
            if goal_block != position.to_block_pos() {
                let solution = pathfinding::directed::astar::astar(
                    &position.to_block_pos(),
                    |node| {
                        let node = *node;
                        let entity_block_position = position.to_block_pos();
                        HorizontalFace::all().into_iter().filter_map(move |face| {
                            let block_position = node + face.get_block_offset();
                            if block_position.distance_squared(entity_block_position)
                                > (24i32).pow(2)
                            {
                                return None;
                            }
                            let is_block_empty = |block: BlockPos| match world.get_block(block) {
                                Some(block) => block.block.data().collision.is_empty(),
                                None => false,
                            };
                            if !is_block_empty(block_position) {
                                if is_block_empty(block_position + BlockPos::Y) {
                                    return Some((block_position + BlockPos::Y, OrderedFloat(1.)));
                                }
                            } else {
                                if is_block_empty(block_position - BlockPos::Y) {
                                    if !is_block_empty(block_position - BlockPos::Y * 2) {
                                        return Some((
                                            block_position - BlockPos::Y,
                                            OrderedFloat(1.),
                                        ));
                                    }
                                } else {
                                    return Some((block_position, OrderedFloat(1.)));
                                }
                            }
                            None
                        })
                    },
                    |node| OrderedFloat(1.3 * node.distance(goal_block)),
                    |node| {
                        node.x == goal_block.x
                            && node.z == goal_block.z
                            && (node.y - goal_block.y).abs() <= 1
                    },
                );
                if let Some((solution, _)) = solution {
                    self.path = solution
                        .into_iter()
                        .rev()
                        .map(|p| {
                            p.to_pos()
                                + Pos {
                                    x: 0.5,
                                    y: 0.,
                                    z: 0.5,
                                }
                        })
                        .collect();
                    self.path[0].x = goal.x;
                    self.path[0].z = goal.z;
                    let mut i = 0;
                    while i + 2 < self.path.len() {
                        let first = self.path[i]
                            + Pos {
                                x: 0.,
                                y: eye_height,
                                z: 0.,
                            };
                        let third = self.path[i + 2]
                            + Pos {
                                x: 0.,
                                y: eye_height,
                                z: 0.,
                            };
                        if !world.block_ray_test(Ray::new_line(first, third)) {
                            self.path.remove(i + 1);
                        } else {
                            i += 1;
                        }
                    }
                }
            }
        }
    }
}
impl Entity {
    pub fn new(key: EntityKey, position: Pos) -> Entity {
        let entity_data = key.data();
        Entity {
            key,
            uuid: Uuid::new_v4(),
            position,
            inventory: Inventory::new(entity_data.inventory_size),
            controlling_user: None,
            character_controller: CharacterController::new(),
            hand_slot: 0,
            last_hand_item: None,
            health: entity_data.base_stats.vitality(),
            research: None,
            brain: match &entity_data.ai {
                Some(_) => Some(Box::new(MobBrain::new(position.to_block_pos()))),
                None => None,
            },
            direction: LookDirection { pitch: 0., yaw: 0. },
            pose: EntityPose::Stand,
            effects: HashMap::new(),
            current_stats: Box::new(entity_data.base_stats.clone()),
            current_passives: HashSet::new(),
        }
    }
    pub fn damage(
        &mut self,
        damage: DamageTable,
        source_entity: Option<&mut Entity>,
        world: &WorldAccess,
    ) {
        if self.health <= 0. {
            return;
        }
        //todo: better formula?
        if rand::random_bool(
            1. - 1. / (self.current_stats.evasion().max(0.) as f64 / 100. + 2.).log2(),
        ) {
            return;
        }
        let entity_data = self.key.data();
        let received_damage = damage
            .iter()
            .map(|(damage_type, damage)| {
                damage * entity_data.damage_table[damage_type].unwrap_or(1.)
            })
            .sum::<f32>();
        let received_damage = received_damage * (self.current_stats.vulnerability() / 100.)
            / (1. + self.current_stats.armor().max(0.) / 100.);
        if let Some(source_entity) = &source_entity {
            if let Some(brain) = &mut self.brain {
                *brain
                    .received_attacks
                    .entry(source_entity.uuid)
                    .or_insert(0.) += received_damage;
            }
        }
        self.health -= received_damage;
        if self.health <= 0. {
            let mut items = generate_loot_table(
                entity_data.loot_table.data(),
                LootGenerationContext::new(rand::random()),
            );
            for item in self.inventory.iter_mut() {
                if let Some(item) = item.take() {
                    items.push(item);
                }
            }
            if let Some(source_entity) = source_entity {
                let view = source_entity.key.data().pickup_view();
                items.retain_mut(|item| {
                    match source_entity.inventory.add_item(view, item.clone()) {
                        Some(overflow) => {
                            *item = overflow;
                            true
                        }
                        None => false,
                    }
                });
            }
            world.drop_items(
                items.into_iter(),
                self.position + Pos::all(entity_data.hitbox_size),
            );
        }
    }
    pub fn get_eye(&self) -> Pos {
        let entity_data = self.key.data();
        self.position + Pos::Y * (entity_data.eye_height + self.pose.height_difference(entity_data))
    }
    pub fn get_hitbox(&self) -> AABB<f32> {
        self.key.data().hitbox(self.pose).offset(self.position)
    }
    pub fn tick(&mut self, move_vector: &mut Pos, world: &WorldAccess) {
        let entity_data = self.key.data();
        match &entity_data.ai {
            Some(ai) => {
                let entity_eye_position = self.get_eye();
                let brain = self.brain.as_mut().unwrap();
                brain.received_attacks.retain(|_, damage| {
                    *damage -= 1. * SERVER_DT;
                    *damage > 0.
                });
                let target_entity = world
                    .iter_entities(&[self.uuid], false)
                    .filter_map(|target| {
                        if world.block_ray_test(coord::Ray::new_line(
                            entity_eye_position,
                            target.get_eye(),
                        )) {
                            return None;
                        }
                        let distance = entity_eye_position.distance(target.get_eye());
                        if distance > 32. {
                            return None;
                        }
                        let received_damage = brain
                            .received_attacks
                            .get(&target.uuid)
                            .cloned()
                            .unwrap_or(0.);
                        let aggression =
                            ai.aggression_score.get(&target.key).cloned().unwrap_or(0.);
                        let score =
                            (received_damage * ai.defence_damage_score + aggression) / distance;
                        if score > 0. {
                            Some(MobBrainTarget {
                                id: target.uuid,
                                last_seen_position: target.position,
                                score,
                            })
                        } else {
                            None
                        }
                    })
                    .max_by_key(|target| OrderedFloat(target.score));
                if let Some(target_entity) = target_entity {
                    match &mut brain.target {
                        Some(brain_target) => {
                            if target_entity.score > brain_target.score
                                || brain_target.id == target_entity.id
                            {
                                *brain_target = target_entity;
                            }
                        }
                        None => {
                            brain.target = Some(target_entity);
                        }
                    }
                }
                if (self.uuid.as_u64_pair().0 + world.ticks_passed) % (SERVER_TPS as u64 / 2) == 0 {
                    brain.recalculate_path(
                        self.position,
                        world,
                        entity_data.eye_height + self.pose.height_difference(entity_data),
                    );
                }

                if !brain.path.is_empty() {
                    if brain.path.last().unwrap().to_block_pos() == self.position.to_block_pos() {
                        brain.path.pop();
                    } else {
                        let next_path_point = *brain.path.last().unwrap();
                        *move_vector = next_path_point - self.position;
                        if move_vector.y > 0. {
                            if self.character_controller.on_ground {
                                self.character_controller.velocity.y +=
                                    entity_data.base_stats.jump_velocity();
                            }
                        }
                        move_vector.y = 0.;
                        *move_vector = move_vector.normalize() * entity_data.base_stats.speed()
                            / 100.
                            * NORMAL_SPEED;

                        if brain.path.first().unwrap().distance(self.position) < 1. {
                            *move_vector = Pos {
                                x: 0.,
                                y: 0.,
                                z: 0.,
                            };
                        }
                    }
                }
                if let Some(target) = &brain.target {
                    brain.goal = Some(target.last_seen_position);
                    let hand_item = self.inventory.get_slot_raw(self.hand_slot);
                    let tool = hand_item
                        .and_then(|item| item.item.data().tool.as_ref())
                        .unwrap_or(&ToolData::HAND);
                    let reach_distance = tool.reach * 0.6;
                    //todo: eye height
                    if let Some(mut target_entity) = world.get_entity(target.id) {
                        if target_entity.position.distance(entity_eye_position) <= reach_distance {
                            if let Some(timer) = &mut brain.hit_timer {
                                if timer.is_finished() {
                                    brain.hit_timer = None;
                                } else if timer.tick(SERVER_DT) {
                                    let (damage_table, knockback) = compute_tool_damage_and_knockback(
                                        hand_item,
                                        &self.current_stats,
                                    );
                                    target_entity.damage(damage_table, Some(self), world);
                                    target_entity.character_controller.velocity +=
                                        (self.direction.make_front() + Pos::Y * 0.5) * knockback;
                                }
                            } else {
                                world.send_viewers(
                                    self.position.to_chunk_pos(),
                                    NetworkMessageS2C::EntityAction {
                                        entity: self.uuid,
                                        action: EntityAction::Attack,
                                    },
                                );
                                brain.hit_timer = Some(HitTimer {
                                    current_time: 0.,
                                    swing_time: tool.swing_time * 1.4,
                                });
                            }
                        }
                    } else {
                        brain.target = None;
                    }
                } else {
                    brain.goal = None;
                    brain.path.clear();
                    brain.hit_timer = None;
                }
            }
            None => {}
        }
        if move_vector.length_squared() > 0. {
            self.direction.yaw = -move_vector.x.atan2(move_vector.z) + std::f32::consts::PI;
        }
    }
}
impl Entity {
    pub fn create_add_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::AddEntity {
            uuid: self.uuid,
            key: self.key,
            position: self.position,
            direction: self.direction,
            pose: self.pose,
            hand_item: self
                .inventory
                .get_slot_raw(self.hand_slot)
                .map(|item| item.client()),
            effects: self.effects.clone(),
        }
    }
    pub fn create_move_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::MoveEntity {
            uuid: self.uuid,
            position: self.position,
            direction: self.direction,
            pose: self.pose,
        }
    }
    pub fn create_remove_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::RemoveEntity { uuid: self.uuid }
    }
}

macro_rules! create_chunk_block_components{
    ($($type:tt, $id:ident, $autotick: literal);*) => {
        #[derive(Default, Serialize, Deserialize)]
        pub struct ChunkBlockComponents{
            $(
                //#[serde(skip_serializing_if = std::concat!("BlockComponentStorage::<", std::stringify!($type), ">::is_empty"), default)]
                pub $id: BlockComponentStorage<WorldAccessCell<$type>>,
            )*
        }
        $(
            impl ComponentTypeAccess<$type> for ChunkBlockComponents{
                type Item = BlockComponentStorage<WorldAccessCell<$type>>;
                fn get_component_type(&self) -> &Self::Item{
                    &self.$id
                }
                fn get_component_type_mut(&mut self) -> &mut Self::Item{
                    &mut self.$id
                }
            }
        )*
        pub struct ChunkBlockComponentsAccess{
            $(
                pub $id: WorldAccessComponentStorage<$type>,
            )*
        }
        impl Default for ChunkBlockComponentsAccess{
            fn default() -> Self{
                Self{
                    $($id: Default::default(),)*
                }
            }
        }
        $(
            impl ComponentTypeAccess<$type> for ChunkBlockComponentsAccess{
                type Item = WorldAccessComponentStorage<$type>;
                fn get_component_type(&self) -> &Self::Item{
                    &self.$id
                }
                fn get_component_type_mut(&mut self) -> &mut Self::Item{
                    &mut self.$id
                }
            }
        )*

        fn flush_block_components(world: &mut WorldAccess) {
            $({
                for (position, component) in world
                    .block_components
                    .$id
                    .added
                    .get_mut()
                    .extract_if(.., |_, _| true)
                {
                    let (chunk, offset) = position.to_chunk_pos_offset();
                    let chunk = WorldAccess::get_grid_index_center(world.center_chunk, chunk).unwrap();
                    let components = &mut world.grid[chunk].as_mut().unwrap().components.$id;
                    components.set(offset, *component);
                    if $autotick {
                        components.tick_list.lock().unwrap().set_ticking(components.tree.get(offset).unwrap() as usize, true);
                    }
                }
                for position in world
                    .block_components
                    .$id
                    .removed
                    .get_mut()
                    .extract_if(.., |_| true)
                {
                    let (chunk, offset) = position.to_chunk_pos_offset();
                    let chunk = WorldAccess::get_grid_index_center(world.center_chunk, chunk).unwrap();
                    world.grid[chunk].as_mut().unwrap().components.$id.remove(offset);
                }
            })*
        }
    }
}
pub trait BlockComponentUpdater {
    fn client_update(&self) -> Option<ClientBlockComponentUpdate>;
    fn client_empty_update() -> Option<ClientBlockComponentUpdate>;
}
macro_rules! create_chunk_block_components_client_mapping {
    ($($server: ident, $client: ident, $id:ident);*) => {
        impl ChunkBlockComponents {
            pub fn client(&self) -> ClientChunkBlockComponents {
                ClientChunkBlockComponents {
                    $(
                        $id: self.$id.map(|component|unsafe{component.get_ref()}.into()),
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

create_chunk_block_components!(BlockDamage, damage, false; BlockPlants, plant, false; BlockMachine, machine, true);
create_chunk_block_components_client_mapping!(BlockDamage, ClientBlockDamage, damage; BlockPlants, ClientBlockPlants, plant; BlockMachine, ClientBlockMachine, machine);
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
                    let stage = ((*growth / plant_data.growth_length)
                        * (plant_data.stages.len() - 1) as f32)
                        as usize;
                    (*plant, stage as u8)
                })
                .collect(),
        }
    }
}
pub enum MachineRunResult {
    Continue,
    Block,
    Sleep(u32),
}
#[derive(Serialize, Deserialize)]
pub struct BlockMachine {
    pub inventory: Inventory,
    pub script_state: ScriptState,
    pub logic_state: FaceMap<Option<ScriptValue>>,
    pub inventory_observers: FaceSet,
    pub current_animation: u16,
    pub animation_start_time: u64,
}
impl BlockMachine {
    pub fn new(machine_data: &BlockMachineData, ticks_passed: u64) -> BlockMachine {
        BlockMachine {
            inventory: Inventory::new(machine_data.inventory_size),
            script_state: ScriptState::new(&machine_data.script),
            logic_state: FaceMap::init(|face| match machine_data.faces.by_face(face) {
                BlockMachineFace::LogicOutput => Some(0),
                _ => None,
            }),
            inventory_observers: FaceSet::default(),
            current_animation: 0,
            animation_start_time: ticks_passed,
        }
    }
    pub fn tick(
        &mut self,
        block_position: BlockPos,
        block: BlockEntry,
        machine_data: &BlockMachineData,
        world: &WorldAccess,
    ) -> MachineRunResult {
        let mut run_result = MachineRunResult::Block;
        match self.script_state.run(
            &machine_data.script,
            |state, instruction| match instruction {
                MachineInstrution::Yield => {
                    run_result = MachineRunResult::Continue;
                    CallbackResult::Suspend
                }
                MachineInstrution::Sleep { time } => {
                    run_result = MachineRunResult::Sleep(time_to_ticks(*time));
                    CallbackResult::Suspend
                }
                MachineInstrution::Block => CallbackResult::Suspend,
                MachineInstrution::TranferItem {
                    self_view: view,
                    face,
                    pull,
                    result,
                } => {
                    let push_offset = face.get_block_offset();
                    let other_face = face.opposite();
                    let view = &machine_data.script_views[*view];
                    let target_position =
                        block_position + block.rotation.rotate_block_pos(push_offset);
                    let Some(target_block) = world.get_block(target_position) else {
                        return result.fail();
                    };
                    let target_block_data = target_block.block.data();
                    if let Some(target_machine_data) = &target_block_data.machine {
                        let mut target_machine = world
                            .get_block_component::<BlockMachine>(target_position)
                            .unwrap();
                        let face_rotated = target_block
                            .rotation
                            .inverse_rotate_face(block.rotation.rotate_face(other_face));
                        let face_data = target_machine_data.faces.by_face(face_rotated);
                        match face_data {
                            BlockMachineFace::Inventory(other_view) => {
                                let mut first_inventory = &mut self.inventory;
                                let mut second_inventory = &mut target_machine.inventory;
                                if *pull {
                                    std::mem::swap(&mut first_inventory, &mut second_inventory);
                                }
                                for slot in &view.slots {
                                    if let Some(item) = first_inventory.get_slot_mut_raw(slot.slot)
                                    {
                                        if second_inventory
                                            .add_item(other_view, item.copy(1))
                                            .is_none()
                                        {
                                            let (target_chunk, target_offset) =
                                                target_position.to_chunk_pos_offset();
                                            world
                                                .schedule_event(
                                                    target_chunk,
                                                    WorldEvent::BlockWakeup {
                                                        block: target_offset,
                                                        inventory_updated: true,
                                                    },
                                                )
                                                .unwrap();
                                            item.count -= 1;
                                            if item.count == 0 {
                                                first_inventory.set_slot_raw(slot.slot, None);
                                            }
                                            self.inventory.modified = true;
                                            return result.succeed();
                                        }
                                    }
                                }
                                match result.fail() {
                                    CallbackResult::Wait => {
                                        target_machine
                                            .inventory_observers
                                            .set(face.opposite(), true);
                                        return CallbackResult::Wait;
                                    }
                                    result => return result,
                                }
                            }
                            _ => {}
                        }
                    }
                    result.fail()
                }
                MachineInstrution::ReadSignal {
                    face,
                    register,
                    result,
                } => match self.logic_state.by_face_mut(*face).take() {
                    Some(value) => {
                        state.registers[*register] = value;
                        result.succeed()
                    }
                    None => result.fail(),
                },
                MachineInstrution::AddWakeupObserver { face } => {
                    let target_position = block_position + face.get_block_offset();
                    let Some(mut other_machine) =
                        world.get_block_component::<BlockMachine>(target_position)
                    else {
                        return CallbackResult::Continue;
                    };
                    other_machine.inventory_observers.set(face.opposite(), true);
                    CallbackResult::Continue
                }
                MachineInstrution::ReadLogic {
                    face,
                    register,
                    result,
                } => match self.logic_state.by_face(*face) {
                    Some(value) => {
                        state.registers[*register] = *value;
                        result.succeed()
                    }
                    None => result.fail(),
                },
                MachineInstrution::WriteSignal { face, value } => {
                    let value = state.resolve_value(value);
                    let world_face = block.rotation.rotate_face(*face);
                    let target_position = block_position + world_face.get_block_offset();
                    let (target_chunk, target_offset) = target_position.to_chunk_pos_offset();
                    world
                        .schedule_event(
                            target_chunk,
                            WorldEvent::BlockLogicSignal {
                                block: target_offset,
                                value,
                                world_face: world_face.opposite(),
                            },
                        )
                        .unwrap();
                    CallbackResult::Continue
                }
                MachineInstrution::WriteLogic { face, value } => {
                    let value = state.resolve_value(value);
                    let logic_state = self.logic_state.by_face_mut(*face);
                    if let Some(previous) = logic_state {
                        if *previous == value {
                            return CallbackResult::Continue;
                        }
                    }
                    *logic_state = Some(value);
                    let world_face = block.rotation.rotate_face(*face);
                    let target_position = block_position + world_face.get_block_offset();
                    let (target_chunk, target_offset) = target_position.to_chunk_pos_offset();
                    world
                        .schedule_event(
                            target_chunk,
                            WorldEvent::BlockLogicState {
                                block: target_offset,
                                value,
                                world_face: world_face.opposite(),
                            },
                        )
                        .unwrap();
                    CallbackResult::Continue
                }
                MachineInstrution::GetItemCount { view, register } => {
                    let view = &machine_data.script_views[*view];
                    let mut count = 0;
                    for slot in &view.slots {
                        if let Some(item) = self.inventory.get_slot_raw(slot.slot) {
                            count += item.count;
                        }
                    }
                    state.registers[*register] = count;
                    CallbackResult::Continue
                }
                MachineInstrution::WaitForItems { view } => {
                    let view = &machine_data.script_views[*view];
                    for slot in &view.slots {
                        if let Some(_) = self.inventory.get_slot_raw(slot.slot) {
                            return CallbackResult::Continue;
                        }
                    }
                    run_result = MachineRunResult::Block;
                    CallbackResult::Wait
                }
                MachineInstrution::MoveItem {
                    from_view,
                    to_view,
                    result,
                } => {
                    let from_view = &machine_data.script_views[*from_view];
                    let to_view = &machine_data.script_views[*to_view];
                    for slot in &from_view.slots {
                        if let Some(item) = self
                            .inventory
                            .get_slot_raw(slot.slot)
                            .as_ref()
                            .map(|item| item.copy(1))
                        {
                            if self.inventory.add_item(to_view, item).is_none() {
                                let item =
                                    self.inventory.get_slot_mut_raw(slot.slot).as_mut().unwrap();
                                item.count -= 1;
                                if item.count == 0 {
                                    self.inventory.set_slot_raw(slot.slot, None);
                                }
                                self.inventory.modified = true;
                                return result.succeed();
                            }
                        }
                    }
                    result.fail()
                }
                MachineInstrution::Craft {
                    recipes,
                    view,
                    process_speed_constant,
                    result,
                } => {
                    let view = &machine_data.script_views[*view];
                    for recipe in recipes.list() {
                        let recipe = recipe.data();
                        let mut failed = false;
                        for (input, count) in &recipe.inputs {
                            if self.inventory.count_removeable_items(view, *input) < *count {
                                failed = true;
                                break;
                            }
                        }
                        if failed {
                            continue;
                        }
                        for (input, count) in &recipe.inputs {
                            self.inventory.remove_item(view, *input, *count);
                        }
                        let mut loot_context = LootGenerationContext::new(rand::random());
                        for (catalyst, max_count, variable) in &recipe.catalysts {
                            let not_removed =
                                self.inventory.remove_item(view, *catalyst, *max_count);
                            *loot_context.variables.or_insert_default(*variable) +=
                                (*max_count - not_removed) as f32;
                        }
                        let craft_time = loot_context.generate_number(&recipe.craft_time);
                        for output in generate_loot_table(recipe.outputs.data(), loot_context) {
                            self.inventory.add_item(view, output);
                        }
                        self.inventory.modified = true;
                        run_result = MachineRunResult::Sleep(time_to_ticks(
                            craft_time * process_speed_constant,
                        ));
                        return CallbackResult::Suspend;
                    }
                    result.fail()
                }
                MachineInstrution::PlayAnimation { animation } => {
                    self.current_animation = machine_data
                        .model_animations
                        .iter()
                        .position(|a| a == animation)
                        .unwrap() as u16;
                    self.animation_start_time = world.ticks_passed;
                    CallbackResult::Continue
                }
            },
            1000,
        ) {
            RunResult::Suspended => {}
            RunResult::TimedOut => {
                println!("timed out {:?} {}", block_position, block.block.text_id());
            }
        }
        if self.inventory.modified {
            for face in self.inventory_observers.iter_set() {
                let (target_chunk, target_offset) =
                    (face.get_block_offset() + block_position).to_chunk_pos_offset();
                world
                    .schedule_event(
                        target_chunk,
                        WorldEvent::BlockWakeup {
                            block: target_offset,
                            inventory_updated: false,
                        },
                    )
                    .unwrap();
            }
            self.inventory_observers.clear();
        }
        run_result
    }
    pub fn modify_property(
        &mut self,
        machine_data: &BlockMachineData,
        property: InternString,
        value: u16,
        mode: PropertyModifyMode,
    ) {
        if let Some(property) = machine_data
            .script
            .named_registers
            .iter()
            .position(|r| *r == property)
        {
            let register = &mut self.script_state.registers[property];
            match mode {
                PropertyModifyMode::Add => {
                    *register = register.wrapping_add(value);
                }
                PropertyModifyMode::Set => {
                    *register = value;
                }
            }
        }
    }
}

impl Into<ClientBlockMachine> for &BlockMachine {
    fn into(self) -> ClientBlockMachine {
        ClientBlockMachine {
            animation: self.current_animation,
            animation_start_time: self.animation_start_time,
        }
    }
}
pub struct WorldAccess<'a> {
    ticks_passed: u64,
    pub center_chunk: ChunkPos,
    grid: [Option<&'a mut Chunk>; 27],
    users: &'a SlotMap<UserIndex, User>,
    message_queue: &'a MessageQueue,
    entity_locks: RefCell<Vec<Uuid>>,
    entities_added: RefCell<BTreeMap<Uuid, Box<WorldAccessCell<Entity>>>>,
    entities_removed: RefCell<Vec<Uuid>>,
    entity_teleports: RefCell<BTreeMap<Uuid, ChunkPos>>,
    block_components: ChunkBlockComponentsAccess,
}
pub struct WorldAccessComponentStorage<C> {
    locks: RefCell<Vec<BlockPos>>,
    added: RefCell<BTreeMap<BlockPos, Box<WorldAccessCell<C>>>>,
    removed: RefCell<BTreeSet<BlockPos>>,
}
impl<T> Default for WorldAccessComponentStorage<T> {
    fn default() -> Self {
        Self {
            locks: Default::default(),
            added: Default::default(),
            removed: Default::default(),
        }
    }
}
impl WorldAccess<'_> {
    pub fn lock<'a>(
        center_chunk: ChunkPos,
        chunks: &'a mut ahash::HashMap<ChunkPos, Chunk>,
        ticks_passed: u64,
        users: &'a SlotMap<UserIndex, User>,
        message_queue: &'a MessageQueue,
    ) -> WorldAccess<'a> {
        WorldAccess {
            ticks_passed,
            center_chunk,
            grid: unsafe {
                chunks.get_disjoint_unchecked_mut(
                    core::array::from_fn::<_, 27, _>(|i| {
                        let x = i % 3;
                        let y = (i / 3) % 3;
                        let z = i / 9;
                        ChunkPos {
                            x: center_chunk.x + x as i16 - 1,
                            y: center_chunk.y + y as i16 - 1,
                            z: center_chunk.z + z as i16 - 1,
                        }
                    })
                    .each_ref(),
                )
            },
            users,
            message_queue,
            entity_locks: RefCell::new(Vec::new()),
            entities_added: RefCell::new(BTreeMap::new()),
            entities_removed: RefCell::new(Vec::new()),
            entity_teleports: RefCell::new(BTreeMap::new()),
            block_components: Default::default(),
        }
    }
    const GRID_CENTER: usize = 13;
    fn get_grid_index_center(center_chunk: ChunkPos, chunk: ChunkPos) -> Option<usize> {
        let x_diff = chunk.x - center_chunk.x + 1;
        let y_diff = chunk.y - center_chunk.y + 1;
        let z_diff = chunk.z - center_chunk.z + 1;
        if x_diff < 0 || x_diff > 2 || y_diff < 0 || y_diff > 2 || z_diff < 0 || z_diff > 2 {
            return None;
        }
        Some(x_diff as usize + y_diff as usize * 3 + z_diff as usize * 9)
    }
    fn get_grid_index(&self, chunk: ChunkPos) -> Option<usize> {
        Self::get_grid_index_center(self.center_chunk, chunk)
    }
    pub fn get_block(&self, position: BlockPos) -> Option<BlockEntry> {
        let (chunk, offset) = position.to_chunk_pos_offset();
        Some(
            self.grid[self.get_grid_index(chunk)?]
                .as_ref()?
                .blocks
                .get(offset),
        )
    }
    pub fn replace_block(&self, position: BlockPos, block: BlockEntry) -> Result<BlockEntry, ()> {
        let (chunk, offset) = position.to_chunk_pos_offset();
        self.send_viewers(chunk, NetworkMessageS2C::SetBlock { position, block });
        match self
            .get_grid_index(chunk)
            .and_then(|chunk| self.grid[chunk].as_ref())
        {
            Some(chunk) => {
                let previous = chunk.blocks.get(offset);
                chunk.blocks.set(offset, block);
                Ok(previous)
            }
            None => Err(()),
        }
    }
    pub fn break_block(&self, position: BlockPos) -> Result<Vec<ItemStack>, ()> {
        let previous_block = self.replace_block(position, BlockEntry::simple(air_block()))?;
        let block_data = previous_block.block.data();
        if let Some(damage) = self.get_block_component::<BlockDamage>(position) {
            self.remove_block_component(damage);
        }

        let mut drops = generate_loot_table(
            block_data.loot_table.data(),
            LootGenerationContext::new(rand::random()),
        );

        if let Some(plant) = self.get_block_component::<BlockPlants>(position) {
            //todo: harvest
            self.remove_block_component(plant);
        }

        if let Some(mut machine) = self.get_block_component::<BlockMachine>(position) {
            //todo: this should probably be returned in remove_block_component
            for item in machine.inventory.iter_mut() {
                if let Some(item) = item.take() {
                    drops.push(item);
                }
            }
            self.remove_block_component(machine);
        }
        for face in Face::all() {
            let neighbor_position = position + face.get_block_offset();
            let (chunk, offset) = neighbor_position.to_chunk_pos_offset();
            let _ = self.schedule_event(
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
            self.get_or_create_block_component(position, || {
                BlockMachine::new(machine_data, self.ticks_passed)
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
        ChunkBlockComponents:
            ComponentTypeAccess<C, Item = BlockComponentStorage<WorldAccessCell<C>>>,
        ChunkBlockComponentsAccess: ComponentTypeAccess<C, Item = WorldAccessComponentStorage<C>>,
    {
        let component_access = self.block_components.get_component_type();
        if component_access.removed.borrow().contains(&position) {
            return None;
        }
        if let Some(block) = component_access.added.borrow().get(&position) {
            return Some(block.lock(&component_access.locks, position));
        }
        let (chunk, offset) = position.to_chunk_pos_offset();
        let components = self.grid[self.get_grid_index(chunk)?]
            .as_ref()?
            .components
            .get_component_type();
        return Some(
            components
                .get(offset)?
                .lock(&component_access.locks, position),
        );
    }
    pub fn get_center_block_component_by_id<'a, C>(
        &'a self,
        id: usize,
    ) -> WorldAccessRef<'a, C, BlockPos>
    where
        ChunkBlockComponents:
            ComponentTypeAccess<C, Item = BlockComponentStorage<WorldAccessCell<C>>>,
        ChunkBlockComponentsAccess: ComponentTypeAccess<C, Item = WorldAccessComponentStorage<C>>,
    {
        let component_access = self.block_components.get_component_type();
        let components = self.grid[Self::GRID_CENTER]
            .as_ref()
            .unwrap()
            .components
            .get_component_type();
        let (offset, component) = components.components.get(id).unwrap();
        component.lock(
            &component_access.locks,
            self.center_chunk.to_block_pos() + offset.xyz(),
        )
    }
    pub fn wakeup_component<C>(&self, block: BlockPos) -> Result<(), ()>
    where
        ChunkBlockComponents:
            ComponentTypeAccess<C, Item = BlockComponentStorage<WorldAccessCell<C>>>,
        ChunkBlockComponentsAccess: ComponentTypeAccess<C, Item = WorldAccessComponentStorage<C>>,
    {
        let (chunk, offset) = block.to_chunk_pos_offset();
        let components = self.grid[self.get_grid_index(chunk).ok_or(())?]
            .as_ref()
            .ok_or(())?
            .components
            .get_component_type();
        let Some(index) = components.tree.get(offset) else {
            return Err(());
        };
        let mut tick_list = components.tick_list.lock().unwrap();
        if tick_list.has_wakeup_scheduled(offset) {
            return Ok(());
        }
        tick_list.set_ticking(index as usize, true);
        Ok(())
    }
    pub fn iter_block_components<'a, C: 'a>(
        &'a self,
        exclude: &[BlockPos],
        center_only: bool,
    ) -> impl Iterator<Item = WorldAccessRef<'a, C, BlockPos>>
    where
        ChunkBlockComponents:
            ComponentTypeAccess<C, Item = BlockComponentStorage<WorldAccessCell<C>>>,
        ChunkBlockComponentsAccess: ComponentTypeAccess<C, Item = WorldAccessComponentStorage<C>>,
    {
        self.grid
            .as_ref()
            .iter()
            .enumerate()
            .filter_map(move |(i, chunk): (usize, &Option<&mut Chunk>)| {
                if center_only && i != WorldAccess::GRID_CENTER {
                    return None;
                }
                chunk.as_ref().map(|chunk| {
                    chunk
                        .components
                        .get_component_type()
                        .iter()
                        .map(|(offset, _)| chunk.position.to_block_pos() + offset.xyz())
                })
            })
            .flatten()
            .filter(|position| !exclude.contains(position))
            .filter_map(|position| self.get_block_component::<C>(position))
    }
    pub fn remove_block_component<C>(&self, component: WorldAccessRef<'_, C, BlockPos>)
    where
        ChunkBlockComponents:
            ComponentTypeAccess<C, Item = BlockComponentStorage<WorldAccessCell<C>>>,
        ChunkBlockComponentsAccess: ComponentTypeAccess<C, Item = WorldAccessComponentStorage<C>>,
        C: BlockComponentUpdater,
    {
        let position = component.lock_key;
        let component_access = self.block_components.get_component_type();
        component_access.added.borrow_mut().remove(&position);
        component_access.removed.borrow_mut().insert(position);
        if let Some(update) = C::client_empty_update() {
            let (chunk, offset) = position.to_chunk_pos_offset();
            self.send_viewers(
                position.to_chunk_pos(),
                NetworkMessageS2C::UpdateBlockComponents {
                    chunk,
                    offset,
                    update,
                },
            );
        }
    }
    pub fn get_or_create_block_component<'a, C>(
        &'a self,
        position: BlockPos,
        init: impl FnOnce() -> C,
    ) -> Result<WorldAccessRef<'a, C, BlockPos>, ()>
    where
        ChunkBlockComponents:
            ComponentTypeAccess<C, Item = BlockComponentStorage<WorldAccessCell<C>>>,
        ChunkBlockComponentsAccess: ComponentTypeAccess<C, Item = WorldAccessComponentStorage<C>>,
        C: BlockComponentUpdater,
    {
        if self.grid[self.get_grid_index(position.to_chunk_pos()).ok_or(())?].is_none() {
            return Err(());
        }
        if let Some(component) = self.get_block_component::<C>(position) {
            return Ok(component);
        }
        let components = self.block_components.get_component_type();
        components.removed.borrow_mut().remove(&position);
        let component = init();
        if let Some(update) = component.client_update() {
            let (chunk, offset) = position.to_chunk_pos_offset();
            self.send_viewers(
                chunk,
                NetworkMessageS2C::UpdateBlockComponents {
                    chunk,
                    offset,
                    update,
                },
            );
        }
        components
            .added
            .borrow_mut()
            .insert(position, Box::new(WorldAccessCell::new(component)));
        Ok(self.get_block_component(position).unwrap())
    }
    pub fn sync_block_component<C>(&self, component: &WorldAccessRef<'_, C, BlockPos>)
    where
        ChunkBlockComponents:
            ComponentTypeAccess<C, Item = BlockComponentStorage<WorldAccessCell<C>>>,
        ChunkBlockComponentsAccess: ComponentTypeAccess<C, Item = WorldAccessComponentStorage<C>>,
        C: BlockComponentUpdater,
    {
        let Some(update) = (*component).client_update() else {
            return;
        };
        let position = component.lock_key;
        let (chunk, offset) = position.to_chunk_pos_offset();
        self.send_viewers(
            chunk,
            NetworkMessageS2C::UpdateBlockComponents {
                chunk,
                offset,
                update,
            },
        );
    }
    pub fn schedule_event(&self, chunk: ChunkPos, event: WorldEvent) -> Result<(), ()> {
        match &self.grid[self.get_grid_index(chunk).ok_or(())?] {
            Some(chunk) => {
                chunk.events.borrow_mut().push_front(event);
                Ok(())
            }
            None => Err(()),
        }
    }
    pub fn get_event_queue_length(&self) -> usize {
        self.grid[WorldAccess::GRID_CENTER]
            .as_ref()
            .unwrap()
            .events
            .borrow()
            .len()
    }
    pub fn pop_event(&self) -> Option<WorldEvent> {
        self.grid[WorldAccess::GRID_CENTER]
            .as_ref()
            .unwrap()
            .events
            .borrow_mut()
            .pop_back()
    }
    pub fn send(&self, user: UserIndex, message: NetworkMessageS2C) {
        self.message_queue
            .send_message(std::iter::once(user), message);
    }
    pub fn send_viewers(&self, chunk: ChunkPos, message: NetworkMessageS2C) {
        if let Some(chunk) = self.get_grid_index(chunk) {
            if let Some(chunk) = &self.grid[chunk] {
                self.message_queue
                    .send_message(chunk.viewers.iter(), message);
            }
        }
    }
    pub fn send_self_viewers(&self, message: NetworkMessageS2C) {
        self.send_viewers(self.center_chunk, message);
    }
    pub fn get_entity<'a>(&'a self, uuid: Uuid) -> Option<WorldAccessRef<'a, Entity, Uuid>> {
        if self.entities_removed.borrow().contains(&uuid) {
            return None;
        }
        if let Some(entity) = self.entities_added.borrow().get(&uuid) {
            return Some(entity.lock(&self.entity_locks, uuid));
        }
        for cell in &self.grid {
            if let Some(cell) = cell {
                if let Some(entity) = cell.entities.get(&uuid) {
                    let entity: &WorldAccessCell<Entity> = entity;
                    return Some(entity.lock(&self.entity_locks, uuid));
                }
            }
        }
        None
    }
    pub fn teleport_entity(&self, entity: &mut Entity, new_position: Pos) -> Result<(), ()> {
        let Some(chunk) = self.get_grid_index(new_position.to_chunk_pos()) else {
            return Err(());
        };
        if self.grid[chunk].is_none() {
            return Err(());
        }
        let hitbox = entity.key.data().hitbox(entity.pose).offset(new_position);
        for block_position in hitbox.to_block() {
            let Some(block) = self.get_block(block_position) else {
                return Err(());
            };
            if block
                .colliders(block_position)
                .any(|block_collider| block_collider.intersects(hitbox))
            {
                return Err(());
            }
        }
        let mut entity_teleports = self.entity_teleports.borrow_mut();
        if !entity_teleports.contains_key(&entity.uuid) {
            entity_teleports.insert(entity.uuid, entity.position.to_chunk_pos());
        }
        entity.position = new_position;
        Ok(())
    }
    pub fn iter_entities<'a>(
        &'a self,
        exclude: &[Uuid],
        center_only: bool,
    ) -> impl Iterator<Item = WorldAccessRef<'a, Entity, Uuid>> {
        self.grid
            .as_ref()
            .iter()
            .enumerate()
            .filter_map(move |(i, chunk)| {
                if center_only && i != WorldAccess::GRID_CENTER {
                    return None;
                }
                chunk.as_ref().map(|chunk| chunk.entities.keys())
            })
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
        self.send_viewers(entity.position.to_chunk_pos(), entity.create_add_message());
        let uuid = entity.uuid;
        self.entities_added
            .borrow_mut()
            .insert(entity.uuid, Box::new(WorldAccessCell::new(entity)));
        Ok(self.get_entity(uuid).unwrap())
    }
    pub fn drop_items(&self, items: impl Iterator<Item = ItemStack>, position: Pos) {
        let item_entity_key = EntityKey::id("item").unwrap();
        for item in items {
            let mut item_entity = Entity::new(item_entity_key, position);
            let angle = rand::random::<f32>() * 2. * std::f32::consts::PI;
            item_entity.character_controller.velocity = Pos {
                x: angle.cos(),
                y: rand::random::<f32>() / 2.,
                z: angle.sin(),
            } * 3.;
            item_entity.inventory.set_slot_raw(0, Some(item));
            let _ = self.spawn_entity(item_entity);
        }
    }
    pub fn remove_entity(&self, entity: WorldAccessRef<'_, Entity, Uuid>) {
        let chunk_position = entity.position.to_chunk_pos();
        let uuid = entity.uuid;

        self.send_viewers(chunk_position, NetworkMessageS2C::RemoveEntity { uuid });
        self.entities_removed.borrow_mut().push(uuid);
        self.entities_added.borrow_mut().remove(&uuid);
        self.entity_teleports.borrow_mut().remove(&uuid);
    }
}
impl WorldAccess<'_> {
    pub fn block_ray_test(&self, ray: coord::Ray) -> bool {
        ray.block_raycast(|pos, _, _| match self.get_block(pos) {
            Some(block) => {
                if block
                    .colliders(pos)
                    .any(|collider| ray.aabb_raycast(collider).is_some())
                {
                    Some(())
                } else {
                    None
                }
            }
            None => Some(()),
        })
        .is_some()
    }
}
impl Drop for WorldAccess<'_> {
    fn drop(&mut self) {
        for (id, mut entity) in self.entities_added.get_mut().extract_if(.., |_, _| true) {
            let chunk = entity.get_mut().position.to_chunk_pos();
            let chunk = Self::get_grid_index_center(self.center_chunk, chunk).unwrap();
            assert!(
                self.grid[chunk]
                    .as_mut()
                    .unwrap()
                    .entities
                    .insert(id, *entity)
                    .is_none()
            );
        }
        let mut removals = Vec::new();
        std::mem::swap(&mut removals, self.entities_removed.get_mut());
        for entity in removals.drain(..) {
            let chunk = self.get_entity(entity).unwrap().position.to_chunk_pos();
            let chunk_id = Self::get_grid_index_center(self.center_chunk, chunk).unwrap();
            let _entity = self.grid[chunk_id]
                .as_mut()
                .unwrap()
                .entities
                .remove(&entity)
                .unwrap();
        }
        let mut teleports = BTreeMap::new();
        std::mem::swap(&mut teleports, self.entity_teleports.get_mut());
        for (uuid, old_chunk_position) in teleports.extract_if(.., |_, _| true) {
            let Some(entity) = self.get_entity(uuid) else {
                continue;
            };
            let new_entity_position = entity.position;
            let add_message = entity.create_add_message();
            let move_message = entity.create_move_message();
            let remove_message = entity.create_remove_message();
            drop(entity);

            if old_chunk_position != new_entity_position.to_chunk_pos() {
                let old_chunk =
                    Self::get_grid_index_center(self.center_chunk, old_chunk_position).unwrap();
                let new_chunk = Self::get_grid_index_center(
                    self.center_chunk,
                    new_entity_position.to_chunk_pos(),
                )
                .unwrap();
                let [old_chunk, new_chunk] =
                    self.grid.get_disjoint_mut([old_chunk, new_chunk]).unwrap();
                let old_chunk = old_chunk.as_mut().unwrap();
                let new_chunk = new_chunk.as_mut().unwrap();
                let entity = old_chunk.entities.remove(&uuid).unwrap();
                assert!(new_chunk.entities.insert(uuid, entity).is_none());
                self.message_queue.send_message(
                    new_chunk.viewers.difference(&old_chunk.viewers),
                    add_message,
                );
                self.message_queue.send_message(
                    old_chunk.viewers.difference(&new_chunk.viewers),
                    remove_message,
                );
                self.message_queue
                    .send_message(new_chunk.viewers.union(&old_chunk.viewers), move_message);
            } else {
                let chunk =
                    Self::get_grid_index_center(self.center_chunk, old_chunk_position).unwrap();
                let chunk = self.grid[chunk].as_mut().unwrap();
                self.message_queue
                    .send_message(chunk.viewers.iter(), move_message);
            }
        }
        flush_block_components(self);
    }
}

pub struct WorldAccessCell<T>(UnsafeCell<T>);
impl<T> WorldAccessCell<T> {
    pub fn new(value: T) -> WorldAccessCell<T> {
        WorldAccessCell(UnsafeCell::new(value))
    }
    pub fn into_inner(self) -> T {
        self.0.into_inner()
    }
    pub unsafe fn get_ref(&self) -> &T {
        unsafe { &*self.0.get() }
    }
    pub fn get_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
    pub fn lock<'b, L: PartialEq + Copy>(
        &self,
        lock: &'b RefCell<Vec<L>>,
        key: L,
    ) -> WorldAccessRef<'b, T, L> {
        let mut lock_vec = lock.borrow_mut();
        if lock_vec.contains(&key) {
            panic!("attempted reborrow");
        }
        lock_vec.push(key);
        drop(lock_vec);
        WorldAccessRef {
            value: unsafe { NonNull::new_unchecked(self.0.get()) },
            borrow: lock,
            lock_key: key,
            _marker: PhantomData,
        }
    }
}
impl<T: serde::Serialize> serde::Serialize for WorldAccessCell<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let v = unsafe { self.get_ref() };
        v.serialize(serializer)
    }
}
impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for WorldAccessCell<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(WorldAccessCell(UnsafeCell::new(T::deserialize(
            deserializer,
        )?)))
    }
}

pub struct WorldAccessRef<'b, T: 'b, L: PartialEq> {
    //noalias
    value: NonNull<T>,
    borrow: &'b RefCell<Vec<L>>,
    lock_key: L,
    _marker: PhantomData<&'b mut T>,
}
impl<T, L: PartialEq> Drop for WorldAccessRef<'_, T, L> {
    fn drop(&mut self) {
        let mut locks = self.borrow.borrow_mut();
        let lock_pos = locks
            .iter()
            .position(|value| value == &self.lock_key)
            .unwrap();
        locks.remove(lock_pos);
    }
}
impl<T, L: PartialEq> std::ops::Deref for WorldAccessRef<'_, T, L> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}
impl<T, L: PartialEq> std::ops::DerefMut for WorldAccessRef<'_, T, L> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.value.as_mut() }
    }
}

pub fn compute_tool_damage_and_knockback(
    item: Option<&ItemStack>,
    stats: &EntityStats,
) -> (DamageTable, f32) {
    let tool = item
        .as_ref()
        .and_then(|item| item.item.data().tool.as_ref())
        .unwrap_or(&ToolData::HAND);
    let quality_multiplier = match item {
        Some(item) => item
            .components
            .get_component::<ItemQuality>()
            .map(|quality| quality.factor())
            .unwrap_or(1.),
        None => 1.,
    };
    let strength_multiplier = stats.strength() / 100.;

    let mut damage_table = tool.damage_table.clone();
    for damage_type in DamageType::list() {
        if let Some(value) = &mut damage_table[*damage_type] {
            *value *= quality_multiplier * strength_multiplier;
        }
    }
    (damage_table, tool.knockback)
}
