use std::{
    cell::{RefCell, UnsafeCell},
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
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
        BlockMachineFace, BlockPalette, BlockRotation, EntityInteractAction, EntityKey, ItemAction,
        KeyGroup, MachineInstrution, PlantKey, PrefabKey, ResearchKey, ToolData, air_block,
    },
    scripts::{self, CallbackResult, ExternalScriptByteCode, RunResult, ScriptState, ScriptValue},
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
use slotmap::{SlotMap, new_key_type};
use smallvec::SmallVec;
use splines::{Interpolation, Spline};
use uuid::Uuid;

use crate::{
    InventoryProvider, MessageQueue, Server, User, UserIndex, UserScreenState,
    inventory::{Inventory, ItemQuality, ItemStack, generate_loot_table, lock_inventories},
    registry::{Key, RegistryConfigLoadable},
};
#[derive(Serialize, Deserialize)]
pub struct ChunkSaveData {
    pub blocks: BlockPalette,
    pub block_events: VecDeque<WorldEvent>,
    pub components: ChunkBlockComponents,
    pub entities: Vec<Entity>,
}
pub struct Chunk {
    pub position: ChunkPos,
    pub blocks: RefCell<BlockPalette>,
    pub viewers: HashSet<UserIndex>,
    pub events: RefCell<VecDeque<WorldEvent>>,
    pub components: ChunkBlockComponents,
    pub entities: BTreeMap<Uuid, WorldAccessCell<Entity>>,
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
                    block_damage.damage += damage_dealt;
                    if block_damage.damage >= block_data.health.health {
                        true
                    } else {
                        drop(block_damage);
                        world.sync_block_component::<BlockDamage>(block_position);
                        false
                    }
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
                            machine.blocked = false;
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
                            machine.blocked = false;
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
                let block_data = block.block.data();
                if let Some(support_face) = block_data.hanging {
                    let face = block.rotation.inverse_rotate_face(world_face);
                    if face == support_face {
                        world.drop_items(
                            world.break_block(block_position).unwrap().into_iter(),
                            block_position.to_pos() + Pos::all(0.5),
                        );
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
                    machine.blocked = false;
                    if inventory_updated {
                        for to_wakeup in machine.inventory_observers.drain(..) {
                            let (target_chunk, target_offset) = to_wakeup.to_chunk_pos_offset();
                            world.schedule_event(
                                target_chunk,
                                WorldEvent::BlockWakeup {
                                    block: target_offset,
                                    inventory_updated: false,
                                },
                            );
                        }
                    }
                }
            }
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
            WorldEvent::EntityKnockback {
                entity: entity_id,
                knockback,
            } => {
                let Some(mut entity) = world.get_entity(entity_id) else {
                    continue;
                };
                entity.state.character_controller.velocity += knockback;
            }
        }
    }
    for mut entity in world.iter_entities(&[], true) {
        let entity_data = entity.key.data();
        let mut entity = &mut *entity;
        if let Some(controlling_user) = entity.controlling_user {
            let mut velocity = Pos::ZERO;
            std::mem::swap(
                &mut velocity,
                &mut entity.state.character_controller.velocity,
            );
            if velocity.length_squared() > 0. {
                world.send(controlling_user, NetworkMessageS2C::Knockback { velocity });
            }
            {
                let inventory: Vec<_> = entity
                    .inventory
                    .items
                    .iter()
                    .map(|item| item.as_ref().map(|item| item.client()))
                    .collect();
                for (i, item) in inventory.into_iter().enumerate() {
                    world.send(
                        controlling_user,
                        NetworkMessageS2C::HUDSlot { slot: i, item },
                    );
                }
                world.send(
                    controlling_user,
                    NetworkMessageS2C::HudBarUpdate {
                        health: entity.state.health,
                    },
                );
                //println!("entity {:?}", world.center_chunk);
                let Some(user) = world.users.get(controlling_user) else {
                    continue;
                };
                let mut message_queue = user.message_queue.lock();
                while let Some(message) = message_queue.pop_back() {
                    match message {
                        NetworkMessageC2S::PlayerPosition {
                            position,
                            direction,
                            teleport_id,
                            crouching,
                        } => {
                            let user = world.users.get(entity.controlling_user.unwrap()).unwrap();
                            if user.teleport_id.load(Ordering::SeqCst) != teleport_id {
                                continue;
                            }
                            *user.view_position.lock() = position.to_chunk_pos();
                            entity.state.direction = direction;
                            entity.state.crouching = crouching;
                            world.teleport_entity(entity, position);
                        }
                        NetworkMessageC2S::AttackBlock { position, face } => {
                            let item = &entity.inventory.items[entity.state.hand_slot];
                            let tool = item
                                .as_ref()
                                .and_then(|item| item.item.data().tool.clone())
                                .unwrap_or(ToolData::hand());
                            let quality_multiplier = match item {
                                Some(item) => item
                                    .components
                                    .get_component::<ItemQuality>()
                                    .map(|quality| quality.factor())
                                    .unwrap_or(1.),
                                None => 1.,
                            };
                            let mut damage_table = DamageTable::default();
                            damage_table[tool.damage_type] = Some(tool.damage * quality_multiplier);
                            let (chunk, offset) = position.to_chunk_pos_offset();
                            let _ = world.schedule_event(
                                chunk,
                                WorldEvent::BlockDamage {
                                    block: offset,
                                    damage: damage_table,
                                },
                            );
                        }
                        NetworkMessageC2S::PlaceBlock {
                            position,
                            face,
                            variant,
                        } => {
                            let mut item_stack =
                                &mut entity.inventory.items[entity.state.hand_slot];
                            println!("place");

                            if let Some(item) = &mut item_stack {
                                match &item.item.data().action {
                                    ItemAction::Ignore => {}
                                    ItemAction::Place(placements) => {
                                        println!("place2");

                                        let Some(place) = placements.get(variant) else {
                                            continue;
                                        };
                                        if item.count < place.use_count {
                                            continue;
                                        }
                                        if let Some(research) = place.research {
                                            if !entity.state.research.contains(&research) {
                                                continue;
                                            }
                                        }
                                        //todo: aabb collision check
                                        if let Ok(_) = world.place_block(
                                            position + face.get_block_offset(),
                                            BlockEntry {
                                                block: place.block,
                                                color: BlockColor::default(),
                                                rotation: place
                                                    .block
                                                    .data()
                                                    .rotation
                                                    .from_look_direction(
                                                        entity.state.direction,
                                                        face,
                                                    ),
                                            },
                                        ) {
                                            item.count -= place.use_count;
                                        }
                                    }
                                    ItemAction::SpawnEntity(key) => {
                                        world
                                            .spawn_entity(Entity::new(
                                                *key,
                                                position.to_pos() + Pos::Y * 0.5,
                                            ))
                                            .unwrap();
                                    }
                                    ItemAction::Plant(key) => todo!(),
                                }
                                if item.count == 0 {
                                    *item_stack = None;
                                }
                            }
                        }
                        NetworkMessageC2S::CloseUI => {
                            if let Some(user) = entity.controlling_user {
                                let Some(user) = world.users.get(user) else {
                                    continue;
                                };
                                if let Some(screen) = user.screen.lock().as_mut() {
                                    screen.state = UserScreenState::Close;
                                }
                            }
                        }
                        NetworkMessageC2S::HotbarSelect { slot } => {
                            entity.state.hand_slot = slot;
                        }
                        NetworkMessageC2S::InteractBlock { position } => {
                            let Some(block) = world.get_block(position) else {
                                continue;
                            };
                            println!("{}", block.block.text_id());
                            let block_data = block.block.data();
                            match &block_data.interact_action {
                                BlockInteractAction::Ignore => {}
                                BlockInteractAction::OpenInventory {
                                    screen: screen_key,
                                    view,
                                } => {
                                    if let Some(user) = entity.controlling_user {
                                        let Some(user) = world.users.get(user) else {
                                            continue;
                                        };
                                        user.set_screen(
                                            *screen_key,
                                            vec![
                                                (
                                                    InventoryProvider::Entity(entity.uuid),
                                                    InventoryView::from_range(0..10),
                                                ),
                                                (InventoryProvider::Block(position), view.clone()),
                                            ],
                                        );
                                    }
                                }
                                BlockInteractAction::Pickup => {
                                    println!(
                                        "{:?} - {:?}",
                                        world.center_chunk,
                                        position.to_chunk_pos()
                                    );
                                    let Ok(drops) = world.break_block(position) else {
                                        continue;
                                    };
                                    let view = entity.inventory.full_view();
                                    for item in drops {
                                        if let Some(rest) = entity.inventory.add_item(&view, item) {
                                            world.drop_items(
                                                std::iter::once(rest),
                                                position.to_pos() + Pos::all(0.5),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        NetworkMessageC2S::InteractEntity {
                            entity: other_entity_uuid,
                        } => {
                            let Some(mut other_entity) = world.get_entity(other_entity_uuid) else {
                                continue;
                            };
                            let other_entity_data = other_entity.key.data();
                            match other_entity_data.interact_action {
                                EntityInteractAction::Ignore => {}
                                EntityInteractAction::Pickup => {
                                    let mut items_present = false;
                                    let player_view = entity.inventory.full_view();
                                    for slot in &mut other_entity.inventory.items {
                                        if let Some(item) = &slot {
                                            *slot = entity
                                                .inventory
                                                .add_item(&player_view, item.clone());
                                            if slot.is_some() {
                                                items_present = true;
                                            }
                                        }
                                    }
                                    if !items_present {
                                        drop(other_entity);
                                        world.remove_entity(other_entity_uuid).unwrap();
                                    }
                                }
                            }
                        }
                        NetworkMessageC2S::AttackEntity {
                            entity: other_entity_id,
                        } => {
                            let item = &entity.inventory.items[entity.state.hand_slot];
                            let tool = item
                                .as_ref()
                                .and_then(|item| item.item.data().tool.clone())
                                .unwrap_or(ToolData::hand());
                            let quality_multiplier = match item {
                                Some(item) => item
                                    .components
                                    .get_component::<ItemQuality>()
                                    .map(|quality| quality.factor())
                                    .unwrap_or(1.),
                                None => 1.,
                            };
                            let mut damage_table = DamageTable::default();
                            damage_table[tool.damage_type] = Some(tool.damage * quality_multiplier);
                            world
                                .schedule_event(
                                    world.center_chunk,
                                    WorldEvent::EntityDamage {
                                        entity: other_entity_id,
                                        damage: damage_table,
                                    },
                                )
                                .unwrap();
                            let knockback_direction = entity.state.direction.make_front();
                            world
                                .schedule_event(
                                    world.center_chunk,
                                    WorldEvent::EntityKnockback {
                                        entity: other_entity_id,
                                        knockback: (knockback_direction + Pos::Y * 0.3)
                                            * tool.knockback
                                            * 4.,
                                    },
                                )
                                .unwrap();
                        }
                        NetworkMessageC2S::DropItem { stack } => {
                            let slot = entity.inventory.get_slot_mut_raw(entity.state.hand_slot);
                            let drop_item = if let Some(item) = slot {
                                if item.count == 1 || stack {
                                    slot.take().unwrap()
                                } else {
                                    item.count -= 1;
                                    item.copy(1)
                                }
                            } else {
                                continue;
                            };
                            let mut item_entity = Entity::new(
                                EntityKey::id("item").unwrap(),
                                entity.position + Pos::Y * entity.key.data().eye_height,
                            );
                            item_entity.state.direction = LookDirection {
                                pitch: 0.,
                                yaw: entity.state.direction.yaw,
                            };
                            let throw_force = 10.;
                            let mut throw_velocity =
                                entity.state.direction.make_front() * throw_force;
                            throw_velocity.y = 0.1;
                            item_entity.state.character_controller.velocity = throw_velocity;
                            item_entity.inventory.items[0] = Some(drop_item);
                            world.spawn_entity(item_entity);
                        }
                        NetworkMessageC2S::MoveItem { from, to, mode } => todo!(),
                        NetworkMessageC2S::Research { research } => todo!(),
                        NetworkMessageC2S::Craft { recipe, count } => todo!(),
                        NetworkMessageC2S::OpenPlayerInventory => {
                            if let Some(user) = entity.controlling_user {
                                let Some(user) = world.users.get(user) else {
                                    continue;
                                };
                                user.set_screen(
                                    Key::id("player").unwrap(),
                                    vec![(
                                        InventoryProvider::Entity(entity.uuid),
                                        InventoryView::from_range(0..10),
                                    )],
                                );
                            }
                        }
                        NetworkMessageC2S::HarvestPlant { position, index } => todo!(),
                        NetworkMessageC2S::UIButtonPress {
                            property,
                            value,
                            modify_mode,
                        } => todo!(),
                    }
                }
            }
        } else {
            let hitbox = entity_data.hitbox(entity.state.crouching);
            let mut move_vector = Pos::ZERO;
            match &entity_data.ai {
                Some(_) => {
                    if entity.state.character_controller.on_ground {
                        entity.state.character_controller.velocity.y += 15.;
                    }
                    let front = entity.state.direction.make_front();
                    move_vector.x = front.x * 1.;
                    move_vector.z = front.z * 1.;
                    if rand::random_bool(1. / 40. / 5.) {
                        entity.state.direction.yaw =
                            rand::random_range((0.)..(std::f32::consts::PI * 2.))
                    }
                }
                None => {}
            }
            let mut new_position = entity.position;
            entity.state.character_controller.tick(
                &mut new_position,
                SERVER_DT,
                |block| world.get_block(block),
                move_vector,
                MoveMode::Normal,
                hitbox,
                40.,
                0.5,
                false,
            );
            if new_position != entity.position {
                world.teleport_entity(entity, new_position);
            }
        }
        {
            let new_hand_item = entity
                .inventory
                .items
                .get(entity.state.hand_slot)
                .cloned()
                .flatten();
            if new_hand_item != entity.state.last_hand_item {
                world.send_viewers(
                    entity.position.to_chunk_pos(),
                    NetworkMessageS2C::EntityHandItem {
                        uuid: entity.uuid,
                        item: new_hand_item.as_ref().map(|item| item.client()),
                    },
                );
                entity.state.last_hand_item = new_hand_item;
            }
        }
    }
    if world.grid.iter().all(Option::is_some) {
        for mut machine in world.iter_block_components::<BlockMachine>(&[], true) {
            let block_position = machine.lock_key;
            let block = world.get_block(block_position).unwrap();
            let block_data = block.block.data();
            let machine_data = block_data.machine.as_ref().unwrap();
            let mut machine = &mut *machine;
            if machine.sleep_cooldown == 0 {
                if !machine.blocked {
                    match machine.script_state.run(
                        &machine_data.script,
                        |state, instruction| match instruction {
                            MachineInstrution::Yield => CallbackResult::Suspend,
                            MachineInstrution::Sleep { time } => {
                                machine.sleep_cooldown = (*time * SERVER_TPS as f32).round() as u32;
                                CallbackResult::Suspend
                            }
                            MachineInstrution::Block => {
                                machine.blocked = true;
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
                                let target_position =
                                    block_position + block.rotation.rotate_block_pos(*push_offset);
                                let Some(target_block) = world.get_block(target_position) else {
                                    return CallbackResult::Continue;
                                };
                                let target_block_data = target_block.block.data();
                                if let Some(target_machine_data) = &target_block_data.machine {
                                    let mut target_machine = world
                                        .get_block_component::<BlockMachine>(target_position)
                                        .unwrap();
                                    let face_rotated = target_block.rotation.inverse_rotate_face(
                                        block.rotation.rotate_face(*other_face),
                                    );
                                    let face_data = target_machine_data.faces.by_face(face_rotated);
                                    match face_data {
                                        BlockMachineFace::InventoryAccess { input, output } => {
                                            let other_view = if *pull { output } else { input };
                                            let mut first_inventory = &mut machine.inventory;
                                            let mut second_inventory =
                                                &mut target_machine.inventory;
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
                                                        let (target_chunk, target_offset) =
                                                            target_position.to_chunk_pos_offset();
                                                        world.schedule_event(
                                                            target_chunk,
                                                            WorldEvent::BlockWakeup {
                                                                block: target_offset,
                                                                inventory_updated: true,
                                                            },
                                                        );
                                                        item.count -= 1;
                                                        if item.count == 0 {
                                                            first_inventory.items[slot.slot] = None;
                                                        }
                                                        state.pc = *success;
                                                        return CallbackResult::Continue;
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
                                match machine.logic_state.by_face_mut(*face).take() {
                                    Some(value) => {
                                        state.registers[*register] = value;
                                        state.pc = *success;
                                    }
                                    None => {}
                                }
                                CallbackResult::Continue
                            }
                            MachineInstrution::AddWakeupObserver { other } => {
                                let target_position =
                                    block_position + block.rotation.rotate_block_pos(*other);
                                let Some(target_block) = world.get_block(target_position) else {
                                    return CallbackResult::Continue;
                                };
                                let target_position =
                                    block_position + block.rotation.rotate_block_pos(*other);
                                let Some(mut other_machine) =
                                    world.get_block_component::<BlockMachine>(target_position)
                                else {
                                    return CallbackResult::Continue;
                                };
                                other_machine.inventory_observers.push(target_position);
                                CallbackResult::Continue
                            }
                            MachineInstrution::ReadLogic { face, register } => {
                                state.registers[*register] =
                                    machine.logic_state.by_face(*face).unwrap_or(0);
                                CallbackResult::Continue
                            }
                            MachineInstrution::WriteSignal { face, value } => {
                                let value = state.resolve_value(value);
                                let world_face = block.rotation.rotate_face(*face);
                                let target_position =
                                    block_position + world_face.get_block_offset();
                                let (target_chunk, target_offset) =
                                    target_position.to_chunk_pos_offset();
                                world.schedule_event(
                                    target_chunk,
                                    WorldEvent::BlockLogicSignal {
                                        block: target_offset,
                                        value,
                                        world_face: world_face.opposite(),
                                    },
                                );
                                CallbackResult::Continue
                            }
                            MachineInstrution::WriteLogic { face, value } => {
                                let value = state.resolve_value(value);
                                let mut logic_state = machine.logic_state.by_face_mut(*face);
                                if let Some(previous) = logic_state {
                                    if *previous == value {
                                        return CallbackResult::Continue;
                                    }
                                }
                                *logic_state = Some(value);
                                let world_face = block.rotation.rotate_face(*face);
                                let target_position =
                                    block_position + world_face.get_block_offset();
                                let (target_chunk, target_offset) =
                                    target_position.to_chunk_pos_offset();
                                world.schedule_event(
                                    target_chunk,
                                    WorldEvent::BlockLogicState {
                                        block: target_offset,
                                        value,
                                        world_face: world_face.opposite(),
                                    },
                                );
                                CallbackResult::Continue
                            }
                            MachineInstrution::GetSlotItemCount { slot, register } => {
                                if let Some(item) = machine
                                    .inventory
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
                                for slot in &from_view.slots {
                                    if let Some(item) = machine.inventory.items[slot.slot]
                                        .as_ref()
                                        .map(|item| item.copy(1))
                                    {
                                        if machine.inventory.add_item(to_view, item).is_none() {
                                            let item = machine
                                                .inventory
                                                .get_slot_mut_raw(slot.slot)
                                                .as_mut()
                                                .unwrap();
                                            item.count -= 1;
                                            if item.count == 0 {
                                                machine.inventory.items[slot.slot] = None;
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
                                for recipe in recipes.list() {
                                    let recipe = recipe.data();
                                    let mut failed = false;
                                    for (input, count) in &recipe.inputs {
                                        if machine.inventory.count_item(input_view, *input) < *count
                                        {
                                            failed = true;
                                            break;
                                        }
                                    }
                                    if failed {
                                        continue;
                                    }
                                    for (input, count) in &recipe.inputs {
                                        machine.inventory.remove_item(input_view, *input, *count);
                                    }
                                    for output in generate_loot_table(recipe.outputs.data()) {
                                        machine.inventory.add_item(output_view, output);
                                    }
                                    machine.sleep_cooldown =
                                        (recipe.craft_time * speed * SERVER_TPS as f32).round()
                                            as u32;
                                    state.pc = *success;
                                    return CallbackResult::Suspend;
                                }
                                CallbackResult::Continue
                            }
                        },
                        1000,
                    ) {
                        RunResult::Suspended => {}
                        RunResult::TimedOut => {
                            println!("timed out");
                        }
                    }
                }
            } else {
                machine.sleep_cooldown -= 1;
            }
        }
    }
    for mut damage in world.iter_block_components::<BlockDamage>(&[], true) {
        let block_position = damage.lock_key;
        let block = world.get_block(block_position).unwrap();
        let block_data = block.block.data();
        damage.damage -= block_data.health.health_regen * SERVER_DT;
        if damage.damage <= 0. {
            drop(damage);
            world
                .remove_block_component::<BlockDamage>(block_position)
                .unwrap();
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
}

#[derive(Serialize, Deserialize)]
pub struct Entity {
    pub key: EntityKey,
    pub uuid: Uuid,
    pub position: Pos,
    pub inventory: Inventory,
    #[serde(default, skip_serializing, skip_deserializing)]
    pub controlling_user: Option<UserIndex>,
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
            inventory: Inventory::new(entity_data.inventory_size),
            controlling_user: None,
            state: InternalEntityState {
                character_controller: CharacterController::new(),
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
    pub fn get_hitbox(&self) -> AABB<f32> {
        let entity_data = self.key.data();
        entity_data
            .hitbox(self.state.crouching)
            .offset(self.position)
    }
}
impl Entity {
    pub fn create_add_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::AddEntity {
            uuid: self.uuid,
            key: self.key,
            position: self.position,
            direction: self.state.direction,
            crouching: self.state.crouching,
            hand_item: self
                .inventory
                .items
                .get(self.state.hand_slot)
                .cloned()
                .flatten()
                .map(|item| item.client()),
        }
    }
    pub fn create_move_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::MoveEntity {
            uuid: self.uuid,
            position: self.position,
            direction: self.state.direction,
            crouching: self.state.crouching,
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
                    .extract_if(.., |a, b| true)
                {
                    let (chunk, offset) = position.to_chunk_pos_offset();
                    let chunk = WorldAccess::get_grid_index_center(world.center_chunk, chunk).unwrap();
                    world.grid[chunk].as_mut()
                        .unwrap()
                        .components
                        .$id
                        .set(offset, *component);
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
trait BlockComponentUpdater {
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

create_chunk_block_components!(BlockDamage, damage; BlockPlants, plant; BlockMachine, machine);
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
pub struct WorldAccess<'a> {
    ticks_passed: u64,
    center_chunk: ChunkPos,
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
                .borrow()
                .get(offset.index())
                .unwrap()
                .clone(),
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
    pub fn remove_block_component<C>(&self, position: BlockPos) -> Result<(), ()>
    where
        ChunkBlockComponents:
            ComponentTypeAccess<C, Item = BlockComponentStorage<WorldAccessCell<C>>>,
        ChunkBlockComponentsAccess: ComponentTypeAccess<C, Item = WorldAccessComponentStorage<C>>,
        C: BlockComponentUpdater,
    {
        if self.get_block_component::<C>(position).is_none() {
            return Err(());
        }
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
        Ok(())
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
    pub fn sync_block_component<C>(&self, position: BlockPos) -> Result<(), ()>
    where
        ChunkBlockComponents:
            ComponentTypeAccess<C, Item = BlockComponentStorage<WorldAccessCell<C>>>,
        ChunkBlockComponentsAccess: ComponentTypeAccess<C, Item = WorldAccessComponentStorage<C>>,
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
        let chunk_position = match self.get_entity(uuid) {
            Some(entity) => entity.position.to_chunk_pos(),
            None => return Err(()),
        };
        self.send_viewers(chunk_position, NetworkMessageS2C::RemoveEntity { uuid });
        self.entities_removed.borrow_mut().push(uuid);
        self.entities_added.borrow_mut().remove(&uuid);
        self.entity_teleports.borrow_mut().remove(&uuid);
        Ok(())
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
            let entity = self.grid[chunk_id]
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
        (unsafe { &*self.0.get() })
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
