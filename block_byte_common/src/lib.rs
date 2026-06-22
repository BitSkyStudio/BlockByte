use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::{
    coord::{AABB, BlockPos, Pos},
    registry::{BlockEntry, EntityData, ItemData, ItemKey, KeyGroup},
};
use serde_default_utils::*;

pub mod coord;
pub mod model;
pub mod net;
pub mod registry;
pub mod rotation;
pub mod scripts;
pub mod ui;
pub mod world;

pub const SERVER_TPS: u32 = 40;
pub const SERVER_DT: f32 = 1. / (SERVER_TPS as f32);
pub const GRAVITY_ACCELERATION: f32 = 25.;
pub const NORMAL_SPEED: f32 = 6.;
pub const ACCELERATION_COEFFICIENT: f32 = 8.;

pub const fn time_to_ticks(time: f32) -> u32 {
    (time * SERVER_TPS as f32).round() as u32
}

#[derive(Copy, Clone, Serialize, Deserialize)]
pub enum MoveMode {
    Normal,
    Fly,
    NoClip,
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

#[derive(Copy, Clone, Hash, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}
impl Serialize for Color {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut buffer = [0u8; 8];
        let mut buffer_slice = &mut buffer[..];
        use std::io::Write;
        write!(buffer_slice, "{:2x}{:2x}{:2x}", self.r, self.g, self.b);
        let buf_len = if self.a != 255 {
            write!(buffer_slice, "{:2x}", self.a);
            8
        } else {
            6
        };
        serializer.serialize_str(std::str::from_utf8(&buffer[0..buf_len]).unwrap())
    }
}
impl<'de> Deserialize<'de> for Color {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ColorVisitor;
        use serde::de::Visitor;
        impl<'de> Visitor<'de> for ColorVisitor {
            type Value = Color;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("valid hex color")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                use serde::de::Error;
                if v.len() != 6 && v.len() != 8 {
                    return Err(Error::custom("length of color string must be 6 or 8"));
                }
                let r = u8::from_str_radix(&v[0..2], 16)
                    .map_err(|_| Error::custom("not valid hex integer"))?;
                let g = u8::from_str_radix(&v[2..4], 16)
                    .map_err(|_| Error::custom("not valid hex integer"))?;
                let b = u8::from_str_radix(&v[4..6], 16)
                    .map_err(|_| Error::custom("not valid hex integer"))?;
                let a = if v.len() == 8 {
                    u8::from_str_radix(&v[6..8], 16)
                        .map_err(|_| Error::custom("not valid hex integer"))?
                } else {
                    255
                };
                Ok(Color { r, g, b, a })
            }
        }
        deserializer.deserialize_str(ColorVisitor)
    }
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
    #[serde(default = "default_bool::<true>")]
    pub input: bool,
    #[serde(default = "default_bool::<true>")]
    pub output: bool,
}

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
                    input: true,
                    output: true,
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

macro_rules! create_entity_stats {
    ($($id:ident : $default_val:literal),*) => {
        fn default_mul_identity() -> f32{1.}
        paste::paste! {
        #[derive(Clone, Serialize, Deserialize)]
        pub struct EntityStats{
            $(
                #[serde(default)]
                [<$id _add>]: f32,
                #[serde(default = "default_mul_identity")]
                [<$id _mul>]: f32,
            )*
        }
        impl PartialEq for EntityStats{
            fn eq(&self, other: &Self) -> bool{
                $(
                    if self.[<$id _add>] != other.[<$id _add>] || self.[<$id _mul>] != other.[<$id _mul>] {
                        return false;
                    }
                )*
                true
            }
        }
        impl Eq for EntityStats{}
        }
        impl Default for EntityStats{
            fn default() -> Self{
                paste::paste! {
                Self{
                    $(
                        [<$id _add>]: 0.,
                        [<$id _mul>]: 1.,
                    )*
                }
            }
            }
        }
        impl EntityStats {
            pub fn apply(&mut self, other: &EntityStats, quality: f32){
                $(
                    paste::paste! {
                    self.[<$id _add>] += other.[<$id _add>] * quality;
                    self.[<$id _mul>] *= (other.[<$id _mul>] - 1.) * quality + 1.;
                    }
                )*
            }
            $(
                pub fn $id(&self) -> f32{
                    paste::paste! {($default_val + self.[<$id _add>]) * self.[<$id _mul>]}
                }
            )*
        }
    };
}
impl EntityStats {
    pub fn jump_velocity(&self) -> f32 {
        //s = 1/2at^2
        let t = (2. * self.jump_height() / GRAVITY_ACCELERATION).sqrt();
        //v = a*t
        GRAVITY_ACCELERATION * t
    }
}
create_entity_stats!(strength: 100., speed: 100., haste: 100., evasion: 0., vitality: 100., regen: 5., mana: 100., mana_regen: 5., stamina: 100., stamina_regen: 10., vulnerability: 100., jump_height: 1.3, armor: 0., flight: 0.);

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
        let acceleration = match move_mode {
            MoveMode::Normal => ground_multiplier,
            MoveMode::Fly | MoveMode::NoClip => 1.,
        } * acceleration;
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
#[derive(Serialize, Deserialize)]
pub struct HitTimer {
    pub current_time: f32,
    pub swing_time: f32,
}
impl HitTimer {
    pub fn tick(&mut self, dt: f32) -> bool {
        let old_hit_timer = self.current_time;
        self.current_time += dt;
        old_hit_timer < self.swing_time * 0.5 && self.current_time >= self.swing_time * 0.5
    }
    pub fn is_finished(&self) -> bool {
        self.current_time >= self.swing_time
    }
    pub fn progress(&self) -> f32 {
        self.current_time / self.swing_time
    }
}

#[derive(Copy, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityPose {
    Stand,
    Walk,
    Run,
    Crouch,
    CrouchWalk,
    Slide,
    Levitate,
    Fly,
    Fall,
    Sleeping,
    Mantle,
}
impl EntityPose {
    pub fn height(self, data: &EntityData) -> f32 {
        data.hitbox_height
            - (match self {
                EntityPose::Stand
                | EntityPose::Walk
                | EntityPose::Run
                | EntityPose::Fly
                | EntityPose::Levitate
                | EntityPose::Fall
                | EntityPose::Sleeping
                | EntityPose::Mantle => 0.,
                EntityPose::Crouch | EntityPose::CrouchWalk | EntityPose::Slide => {
                    data.crouch_height_difference
                }
            })
    }
}
pub trait WeightedEntry {
    type WeightModifier: Copy;
    fn get_weight(&self, modifier: Self::WeightModifier) -> f32;
}
pub trait WeightedList {
    type Entry: WeightedEntry;
    fn get_random<'a>(
        &'a self,
        modifier: <Self::Entry as WeightedEntry>::WeightModifier,
        rng: &mut impl Rng,
    ) -> Option<&'a Self::Entry>;
    fn get_random_weighted_list<'a>(
        &'a self,
        modifier: <Self::Entry as WeightedEntry>::WeightModifier,
        rng: &mut impl Rng,
    ) -> impl Iterator<Item = &'a Self::Entry>;
}
impl<T: WeightedEntry> WeightedList for Vec<T> {
    type Entry = T;
    fn get_random<'a>(
        &'a self,
        modifier: T::WeightModifier,
        rng: &mut impl Rng,
    ) -> Option<&'a Self::Entry> {
        let sum_weights = self.iter().map(|e| e.get_weight(modifier)).sum();
        let mut selection = rng.random_range((0.)..sum_weights);
        for entry in self {
            let weight = entry.get_weight(modifier);
            if selection < weight {
                return Some(entry);
            }
            selection -= weight;
        }
        None
    }
    fn get_random_weighted_list<'a>(
        &'a self,
        modifier: T::WeightModifier,
        rng: &mut impl Rng,
    ) -> impl Iterator<Item = &'a Self::Entry> {
        let mut list: Vec<_> = self
            .iter()
            .map(|entry| {
                (
                    entry,
                    rng.random::<f32>().powf(1. / entry.get_weight(modifier)),
                )
            })
            .collect();
        list.sort_by(|(_, a), (_, b)| b.total_cmp(a));
        list.into_iter().map(|(e, _)| e)
    }
}
