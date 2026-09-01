
use block_byte_common::{
    InternString,
    coord::{
        BlockPos, FaceMap, FaceSet,
    },
    net::PropertyModifyMode,
    registry::{
        BlockEntry, BlockMachineData, BlockMachineFace,
        MachineInstrution,
    },
    scripts::{CallbackResult, RunResult, ScriptState, ScriptValue},
    time_to_ticks,
    world::ClientBlockMachine,
};
use serde::{Deserialize, Serialize};

use crate::{
    inventory::{
        Inventory,
        LootGenerationContext, generate_loot_table,
    },
    world::{WorldAccess, WorldEvent},
};

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
            logic_state: FaceMap::init(|face| match &machine_data.faces[face] {
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
                        let face_data = &target_machine_data.faces[face_rotated];
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
                } => match self.logic_state[*face].take() {
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
                } => match self.logic_state[*face] {
                    Some(value) => {
                        state.registers[*register] = value;
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
                    let logic_state = &mut self.logic_state[*face];
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
