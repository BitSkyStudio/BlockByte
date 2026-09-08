use std::collections::{BinaryHeap, HashMap, HashSet, hash_map::Entry};

use block_byte_common::{
    ActiveEffect, CharacterController, DamageTable, EntityAction, EntityPose,
    EntityResearchProgress, EntityStats, HitTimer, LookDirection, NORMAL_SPEED, PassiveAbility,
    SERVER_DT, SERVER_TPS,
    coord::{self, AABB, BlockPos, Pos},
    net::NetworkMessageS2C,
    registry::{EffectKey, EntityKey, ToolData},
};
use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    UserIndex,
    inventory::{Inventory, ItemStack, LootGenerationContext, generate_loot_table},
    world::{WorldAccess, compute_tool_damage_and_knockback},
};

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
    pub goal: Option<(Pos, f32)>,
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
        if let Some((goal, _)) = self.goal {
            let goal_block = goal.to_block_pos();
            if goal_block != position.to_block_pos() {
                let solution = astar_block_closest(position.to_block_pos(), goal_block, |node| {
                    let entity_block_position = position.to_block_pos();
                    [
                        (0, 1),
                        (0, -1),
                        (1, 0),
                        (-1, 0),
                        (1, 1),
                        (1, -1),
                        (-1, 1),
                        (-1, -1),
                    ]
                    .into_iter()
                    .filter_map(move |offset| {
                        let block_position = node
                            + BlockPos {
                                x: offset.0,
                                y: 0,
                                z: offset.1,
                            };
                        if block_position.distance_squared(entity_block_position) > (24i32).pow(2) {
                            return None;
                        }
                        let is_block_empty = |block: BlockPos| match world.get_block(block) {
                            Some(block) => block.block.data().collision.is_empty(),
                            None => false,
                        };
                        let can_fit_in = |block: BlockPos| {
                            (0..eye_height.ceil() as i32)
                                .all(|i| is_block_empty(block + BlockPos::Y * i))
                        };
                        if !is_block_empty(block_position) {
                            if can_fit_in(block_position + BlockPos::Y) {
                                return Some(block_position + BlockPos::Y);
                            }
                        } else {
                            if is_block_empty(block_position - BlockPos::Y) {
                                if !is_block_empty(block_position - BlockPos::Y * 2) {
                                    return Some(block_position - BlockPos::Y);
                                }
                            } else {
                                if can_fit_in(block_position) {
                                    return Some(block_position);
                                }
                            }
                        }
                        None
                    })
                });
                self.path = solution
                    .into_iter()
                    .rev()
                    .map(|p| p.to_pos() + Pos::XZ_HALF)
                    .collect();
                if !self.path.is_empty() && self.path[0].to_block_pos() == goal_block {
                    self.path[0].x = goal.x;
                    self.path[0].z = goal.z;
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
                        let guard_distance =
                            target.get_eye().distance(brain.guard_position.to_pos());
                        if guard_distance > 28. {
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
                            if let Some(current_target) = &brain.target {
                                if current_target.id == target.uuid {
                                    brain.target = None;
                                }
                            }
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

                        if brain.path.first().unwrap().distance(self.position)
                            < brain.goal.as_ref().map(|goal| goal.1).unwrap_or(0.)
                        {
                            *move_vector = Pos {
                                x: 0.,
                                y: 0.,
                                z: 0.,
                            };
                        }
                    }
                }
                if let Some(target) = &mut brain.target {
                    target.score -= 5. * SERVER_DT;
                    if target.score < 0. {
                        brain.target = None;
                    }
                }
                if move_vector.length_squared() > 0. {
                    self.direction.yaw = -move_vector.x.atan2(move_vector.z) + std::f32::consts::PI;
                } else {
                    if let Some(target) = &brain.target {
                        let offset = target.last_seen_position - entity_eye_position;
                        self.direction.yaw = -offset.x.atan2(offset.z) + std::f32::consts::PI;
                    }
                }
                if let Some(target) = &brain.target {
                    brain.goal = Some((target.last_seen_position, 1.2));
                    let hand_item = self.inventory.get_slot_raw(self.hand_slot);
                    let tool = hand_item
                        .and_then(|item| item.item.data().tool.as_ref())
                        .unwrap_or(ToolData::hand());
                    let reach_distance = tool.reach * 0.6;
                    //todo: eye height
                    if let Some(mut target_entity) = world.get_entity(target.id) {
                        if target_entity.position.distance(entity_eye_position) <= reach_distance {
                            if let Some(timer) = &mut brain.hit_timer {
                                if timer.is_finished() {
                                    brain.hit_timer = None;
                                } else if timer.tick(SERVER_DT) {
                                    let (damage_table, knockback) =
                                        compute_tool_damage_and_knockback(
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
                                        action: EntityAction::Attack(tool.hit_animation),
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
                    brain.goal = Some((brain.guard_position.to_pos(), 0.2));
                    brain.hit_timer = None;
                }
            }
            None => {}
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

fn astar_block_closest<F: Fn(BlockPos) -> I, I: Iterator<Item = BlockPos>>(
    start: BlockPos,
    end: BlockPos,
    neighbors: F,
) -> Vec<BlockPos> {
    let mut queue = BinaryHeap::new();
    let mut came_from = HashMap::new();
    let mut lowest_score = HashMap::new();
    let mut closest_yet = start;
    #[derive(PartialEq)]
    struct QueueEntry {
        position: BlockPos,
        real_cost: f32,
        heuristic: f32,
    }
    impl Eq for QueueEntry {}
    impl PartialOrd for QueueEntry {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(
                (self.real_cost + self.heuristic)
                    .total_cmp(&(other.real_cost + other.heuristic))
                    .reverse(),
            )
        }
    }
    impl Ord for QueueEntry {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.partial_cmp(other).unwrap()
        }
    }
    lowest_score.insert(start, 0.0);
    queue.push(QueueEntry {
        position: start,
        real_cost: 0.,
        heuristic: 0.,
    });
    while let Some(entry) = queue.pop() {
        if entry.position.distance_squared(end) < closest_yet.distance_squared(end) {
            closest_yet = entry.position;
        }
        if entry.position == end {
            break;
        }
        for succesor in neighbors(entry.position) {
            let next_entry = QueueEntry {
                position: succesor,
                real_cost: entry.real_cost + entry.position.distance(succesor),
                heuristic: succesor.distance(end),
            };
            match lowest_score.entry(succesor) {
                Entry::Occupied(mut lowest) => {
                    if *lowest.get() > next_entry.real_cost {
                        lowest.insert(next_entry.real_cost);
                    } else {
                        continue;
                    }
                }
                Entry::Vacant(lowest) => {
                    lowest.insert(next_entry.real_cost);
                }
            }
            came_from.insert(succesor, entry.position);
            queue.push(next_entry);
        }
    }
    let mut path = Vec::new();
    path.push(closest_yet);
    while let Some(next) = came_from.get(&closest_yet) {
        path.push(*next);
        closest_yet = *next;
    }
    path.reverse();
    path
}
