use serde::{Deserialize, Serialize};

use crate::{
    coord::{AABB, BlockPos, Pos},
    registry::{BlockEntry, ItemData, ItemKey, KeyGroup},
};
use serde_default_utils::*;

pub mod coord;
pub mod model;
pub mod net;
pub mod registry;
pub mod scripts;
pub mod ui;
pub mod world;

pub const SERVER_TPS: u32 = 40;
pub const SERVER_DT: f32 = 1. / (SERVER_TPS as f32);
pub const GRAVITY_ACCELERATION: f32 = 25.;

#[derive(Copy, Clone, Serialize, Deserialize)]
pub enum MoveMode {
    Normal,
    Fly,
    NoClip,
}
#[derive(Copy, Clone, Serialize, Deserialize)]
pub struct PlayerAbilities {
    pub move_mode: MoveMode,
    pub speed: f32,
    pub max_stamina: f32,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientItem {
    pub item: ItemKey,
    pub count: u16,
    pub description: String,
}
#[derive(Clone, Copy)]
pub struct TexCoords {
    pub u1: f32,
    pub v1: f32,
    pub u2: f32,
    pub v2: f32,
}
impl TexCoords {
    pub fn map(self, coords: [f32; 2]) -> [f32; 2] {
        let self_w = self.u2 - self.u1;
        let self_h = self.v2 - self.v1;
        [
            self.u1 + (coords[0] * self_w),
            self.v1 + (coords[1] * self_h),
        ]
    }
    pub fn map_sub(self, inner: TexCoords) -> TexCoords {
        let self_w = self.u2 - self.u1;
        let self_h = self.v2 - self.v1;
        TexCoords {
            u1: self.u1 + (inner.u1 * self_w),
            v1: self.v1 + (inner.v1 * self_h),
            u2: self.u1 + (inner.u2 * self_w),
            v2: self.v1 + (inner.v2 * self_h),
        }
    }
}

#[derive(Copy, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    #[serde(default = "default_u8::<255>")]
    pub a: u8,
}
impl Color {
    pub const WHITE: Color = Self::grayscale(255);
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }
    pub const fn grayscale(v: u8) -> Color {
        Color {
            r: v,
            g: v,
            b: v,
            a: 255,
        }
    }
}
impl Default for Color {
    fn default() -> Self {
        Color::WHITE
    }
}
impl Into<[u8; 4]> for Color {
    fn into(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct LookDirection {
    pub pitch: f32,
    pub yaw: f32,
}
impl LookDirection {
    pub fn make_front(self) -> Pos {
        Pos {
            x: self.yaw.sin() * self.pitch.cos(),
            y: self.pitch.sin(),
            z: -self.yaw.cos() * self.pitch.cos(),
        }
    }
    pub fn make_right(self) -> Pos {
        Pos {
            x: self.yaw.cos(),
            y: 0.,
            z: self.yaw.sin(),
        }
    }
}
#[derive(Serialize, Deserialize, Copy, Clone)]
pub enum ItemMoveMode {
    Stack,
    Single,
    Half,
    KeepOne,
}
impl ItemMoveMode {
    pub fn get_count(self, count: u16) -> u16 {
        match self {
            ItemMoveMode::Stack => count,
            ItemMoveMode::Single => 1,
            ItemMoveMode::Half => count.div_ceil(2),
            ItemMoveMode::KeepOne => {
                if count > 1 {
                    count - 1
                } else {
                    0
                }
            }
        }
    }
    pub fn can_swap(self) -> bool {
        match self {
            ItemMoveMode::Stack => true,
            _ => false,
        }
    }
}
#[derive(Clone, Deserialize)]
pub struct ViewSlot {
    pub slot: usize,
    #[serde(default)]
    pub stack_size_override: Option<u16>,
    #[serde(default)]
    pub filter: Option<KeyGroup<ItemData>>,
}

pub static DEFAULT_VIEWSLOT: ViewSlot = ViewSlot {
    slot: 0,
    filter: None,
    stack_size_override: None,
};
#[derive(Default, Clone)]
pub struct InventoryView {
    pub slots: Vec<ViewSlot>,
}
impl InventoryView {
    pub fn from_range(range: std::ops::Range<usize>) -> Self {
        InventoryView {
            slots: range
                .map(|slot| ViewSlot {
                    slot,
                    stack_size_override: None,
                    filter: None,
                })
                .collect(),
        }
    }
    pub fn size(&self) -> usize {
        self.slots.len()
    }
}
impl<'de> Deserialize<'de> for InventoryView {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(InventoryView {
            slots: Vec::<ViewSlot>::deserialize(deserializer)?,
        })
    }
}

macro_rules! create_damage_types {
    ($($id:ident),*) => {
        #[derive(Copy, Clone, Deserialize, Serialize, PartialEq)]
        pub enum DamageType{
            $($id,)*
        }
        fn default_damage_vulnerability() -> f32{1.}
        #[allow(non_snake_case)]
        #[derive(Default, Serialize, Deserialize)]
        pub struct DamageTable{
            $(
                #[serde(default)]
                $id: Option<f32>,
            )*
        }
        impl DamageTable {
            pub fn iter(&self) -> impl Iterator<Item=(DamageType, f32)>{
                [$(DamageType::$id,)*].into_iter().filter_map(|damage_type|self[damage_type].map(|value|(damage_type, value)))
            }
        }
        impl std::ops::Index<DamageType> for DamageTable {
            type Output = Option<f32>;

            fn index(&self, damage_type: DamageType) -> &Self::Output {
                match damage_type {
                    $(DamageType::$id => &self.$id,)*
                }
            }
        }
        impl std::ops::IndexMut<DamageType> for DamageTable {
            fn index_mut(&mut self, damage_type: DamageType) -> &mut Self::Output {
                match damage_type {
                    $(DamageType::$id => &mut self.$id,)*
                }
            }
        }
    };
}
create_damage_types!(Blunt, Pierce, Slash, Cut);

#[derive(Serialize, Deserialize)]
pub struct CharacterController {
    pub velocity: Pos,
    pub on_ground: bool,
}
impl CharacterController {
    pub fn new() -> CharacterController {
        CharacterController {
            velocity: Pos::ZERO,
            on_ground: false,
        }
    }
    pub fn tick(
        &mut self,
        position: &mut Pos,
        delta_time: f32,
        block_query: impl Fn(BlockPos) -> Option<BlockEntry>,
        move_vector: Pos,
        move_mode: MoveMode,
        hitbox: AABB<f32>,
        acceleration: f32,
        step_height: f32,
        holding_ledge: bool,
    ) {
        match move_mode {
            MoveMode::Normal => {
                self.velocity.y -= GRAVITY_ACCELERATION * delta_time;
            }
            MoveMode::Fly | MoveMode::NoClip => {}
        }
        let ground_multiplier = if self.on_ground { 1. } else { 0.2 };
        let acceleration = ground_multiplier * acceleration;
        let mut error = (move_vector - self.velocity);
        match move_mode {
            MoveMode::Normal => {
                error.y = 0.;
            }
            MoveMode::Fly | MoveMode::NoClip => {}
        }
        if error.length() > 0. {
            self.velocity += (error.normalize() * (error.length().min(acceleration * delta_time)));
        }
        self.velocity *= ((1_f32 - 0.1 * ground_multiplier).powf(delta_time));
        let total_move = self.velocity * delta_time;
        match move_mode {
            MoveMode::Normal | MoveMode::Fly => {
                if Self::collides_at(
                    *position
                        + Pos {
                            x: 0.,
                            y: total_move.y,
                            z: 0.,
                        },
                    &block_query,
                    hitbox,
                )
                .is_none()
                {
                    position.y += total_move.y;
                    self.on_ground = false;
                } else {
                    self.on_ground = self.velocity.y < 0.;
                    self.velocity.y = 0.;
                }
                if let Some(highest_point) = Self::collides_at(
                    *position
                        + Pos {
                            x: total_move.x,
                            y: 0.,
                            z: 0.,
                        },
                    &block_query,
                    hitbox,
                ) {
                    let step_difference = highest_point - position.y;
                    if step_difference <= step_height + 0.01 && self.on_ground {
                        let snap = Pos {
                            x: total_move.x,
                            y: step_difference + 0.02,
                            z: 0.,
                        };
                        if Self::collides_at(*position + snap, &block_query, hitbox).is_none() {
                            *position += snap;
                        } else {
                            self.velocity.x = 0.;
                        }
                    } else {
                        self.velocity.x = 0.;
                    }
                } else {
                    if !self.on_ground
                        || !holding_ledge
                        || Self::collides_at(
                            *position
                                + Pos {
                                    x: total_move.x,
                                    y: -0.01,
                                    z: 0.,
                                },
                            &block_query,
                            hitbox,
                        )
                        .is_some()
                    {
                        position.x += total_move.x;
                    }
                }
                if let Some(highest_point) = Self::collides_at(
                    *position
                        + Pos {
                            x: 0.,
                            y: 0.,
                            z: total_move.z,
                        },
                    &block_query,
                    hitbox,
                ) {
                    let step_difference = highest_point - position.y;
                    if step_difference <= step_height + 0.01 && self.on_ground {
                        let snap = Pos {
                            x: 0.,
                            y: step_difference + 0.02,
                            z: total_move.z,
                        };
                        if Self::collides_at(*position + snap, &block_query, hitbox).is_none() {
                            *position += snap;
                        } else {
                            self.velocity.z = 0.;
                        }
                    } else {
                        self.velocity.z = 0.;
                    }
                } else {
                    if !self.on_ground
                        || !holding_ledge
                        || Self::collides_at(
                            *position
                                + Pos {
                                    x: 0.,
                                    y: -0.01,
                                    z: total_move.z,
                                },
                            &block_query,
                            hitbox,
                        )
                        .is_some()
                    {
                        position.z += total_move.z;
                    }
                }
            }
            MoveMode::NoClip => {
                *position += total_move;
            }
        }
    }
    pub fn collides_at(
        position: Pos,
        block_query: &impl Fn(BlockPos) -> Option<BlockEntry>,
        hitbox: AABB<f32>,
    ) -> Option<f32> {
        let player_collider = hitbox.offset(position);
        let player_block_collider = player_collider.to_block();
        let mut max_height = None;
        for block in player_block_collider {
            match block_query(block) {
                Some(block_entry) => {
                    let block_collider = &block_entry.block.data().collision;
                    for block_collider in block_collider {
                        let block_collider = block_entry
                            .rotation
                            .rotate_aabb(*block_collider)
                            .offset(block.to_pos());
                        if player_collider.intersects(block_collider) {
                            match max_height {
                                Some(current_height) => {
                                    if current_height < block_collider.max.y {
                                        max_height = Some(block_collider.max.y);
                                    }
                                }
                                None => {
                                    max_height = Some(block_collider.max.y);
                                }
                            }
                        }
                    }
                }
                None => return Some(0.),
            }
        }
        max_height
    }
}
pub fn number_approach_smooth(
    current: f32,
    target: f32,
    smooth: f32,
    min_speed: f32,
    dt: f32,
) -> f32 {
    let diff = target - current;
    if diff.abs() < f32::EPSILON {
        return target;
    }
    let t = 1.0 - (-smooth * dt).exp();
    let mut step = diff * t;
    let min_step = min_speed * dt;
    if step.abs() < min_step {
        step = min_step * diff.signum();
    }
    if step.abs() > diff.abs() {
        return target;
    }
    current + step
}
