use std::{
    fmt::Debug,
    marker::PhantomData,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign},
    sync::OnceLock,
};

use num_integer::{Integer, Roots};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::TexCoords;

pub const CHUNK_SIZE_BITS: u8 = 5;
pub const CHUNK_SIZE: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Vec3<T: Copy> {
    pub x: T,
    pub y: T,
    pub z: T,
}
impl<T: Copy + Serialize> Serialize for Vec3<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeTuple;
        let mut tup = serializer.serialize_tuple(3)?;
        tup.serialize_element(&self.x)?;
        tup.serialize_element(&self.y)?;
        tup.serialize_element(&self.z)?;
        tup.end()
    }
}
impl<'de, T: Copy + Deserialize<'de>> Deserialize<'de> for Vec3<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, SeqAccess, Visitor};
        use std::fmt;
        struct Vec3Visitor<T>(PhantomData<T>);

        impl<'de, T: Copy + Deserialize<'de>> Visitor<'de> for Vec3Visitor<T> {
            type Value = Vec3<T>;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a tuple of three floats (x, y, z)")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Vec3<T>, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let x: T = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let y: T = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let z: T = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(2, &self))?;

                Ok(Vec3 { x, y, z })
            }
        }
        deserializer.deserialize_tuple(3, Vec3Visitor::<T>(PhantomData))
    }
}
impl<T: Debug + Copy> Debug for Vec3<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?},{:?},{:?}]", self.x, self.y, self.z)
    }
}
impl<T: Copy + Add<Output = T> + Mul<Output = T> + Sub<Output = T>> Vec3<T> {
    pub fn length_squared(self) -> T {
        (self.x * self.x) + (self.y * self.y) + (self.z * self.z)
    }
    pub fn distance_squared(self, other: Self) -> T {
        (self - other).length_squared()
    }
    pub fn dot(self, other: Self) -> T {
        self.x * other.x + self.y * other.y + self.z * other.z
    }
}
trait Sqrtable {
    fn square_root(self) -> f32;
}
impl Sqrtable for i32 {
    fn square_root(self) -> f32 {
        (self as f32).sqrt()
    }
}
impl Sqrtable for i16 {
    fn square_root(self) -> f32 {
        (self as f32).sqrt()
    }
}
impl Sqrtable for f32 {
    fn square_root(self) -> f32 {
        self.sqrt()
    }
}
impl<T: Copy + Add<Output = T> + Mul<Output = T> + Sub<Output = T> + Sqrtable> Vec3<T> {
    pub fn length(self) -> f32 {
        self.length_squared().square_root()
    }
    pub fn distance(self, other: Self) -> f32 {
        self.distance_squared(other).square_root()
    }
}

impl<T: Copy + Add<Output = T>> Add for Vec3<T> {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}
impl<T: Copy + AddAssign> AddAssign for Vec3<T> {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
        self.z += rhs.z;
    }
}
impl<T: Copy + Sub<Output = T>> Sub for Vec3<T> {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}
impl<T: Copy + SubAssign> SubAssign for Vec3<T> {
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
        self.z -= rhs.z;
    }
}
impl<T: Copy + Neg<Output = T>> Neg for Vec3<T> {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}
impl<T: Copy + Mul<T, Output = T>> Mul<T> for Vec3<T> {
    type Output = Self;
    fn mul(self, rhs: T) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
            z: self.z * rhs,
        }
    }
}
impl<T: Copy + MulAssign> MulAssign<T> for Vec3<T> {
    fn mul_assign(&mut self, rhs: T) {
        self.x *= rhs;
        self.y *= rhs;
        self.z *= rhs;
    }
}
impl<T: Copy + Div<T, Output = T>> Div<T> for Vec3<T> {
    type Output = Self;
    fn div(self, rhs: T) -> Self {
        Self {
            x: self.x / rhs,
            y: self.y / rhs,
            z: self.z / rhs,
        }
    }
}
impl<T: Copy + DivAssign> DivAssign<T> for Vec3<T> {
    fn div_assign(&mut self, rhs: T) {
        self.x /= rhs;
        self.y /= rhs;
        self.z /= rhs;
    }
}

impl<T: Copy> Vec3<T> {
    pub const fn all(value: T) -> Vec3<T> {
        Vec3 {
            x: value,
            y: value,
            z: value,
        }
    }
}

pub type Pos = Vec3<f32>;
pub type BlockPos = Vec3<i32>;
pub type ChunkPos = Vec3<i16>;
impl BlockPos {
    pub const ZERO: BlockPos = Pos::ZERO.to_block_pos();
}
impl Pos {
    pub const X: Pos = Pos {
        x: 1.,
        y: 0.,
        z: 0.,
    };
    pub const Y: Pos = Pos {
        x: 0.,
        y: 1.,
        z: 0.,
    };
    pub const Z: Pos = Pos {
        x: 0.,
        y: 0.,
        z: 1.,
    };
    pub const ZERO: Pos = Pos::all(0.);
    pub const fn to_block_pos(self) -> BlockPos {
        BlockPos {
            x: self.x.floor() as i32,
            y: self.y.floor() as i32,
            z: self.z.floor() as i32,
        }
    }
    pub fn to_chunk_pos(self) -> ChunkPos {
        self.to_block_pos().to_chunk_pos()
    }
    pub fn lerp(self, other: Pos, v: f32) -> Pos {
        Pos {
            x: lerp_number(self.x, other.x, v),
            y: lerp_number(self.y, other.y, v),
            z: lerp_number(self.z, other.z, v),
        }
    }
    pub fn normalize(self) -> Pos {
        self / self.length()
    }
}
pub fn lerp_number(a: f32, b: f32, v: f32) -> f32 {
    a * (1. - v) + b * v
}
impl BlockPos {
    pub fn to_chunk_pos(self) -> ChunkPos {
        use num_integer::Integer;
        ChunkPos {
            x: Integer::div_floor(&self.x, &(CHUNK_SIZE as i32)) as i16,
            y: Integer::div_floor(&self.y, &(CHUNK_SIZE as i32)) as i16,
            z: Integer::div_floor(&self.z, &(CHUNK_SIZE as i32)) as i16,
        }
    }
    pub fn to_chunk_offset(self) -> ChunkOffset {
        ChunkOffset::new(
            self.x.mod_floor(&(CHUNK_SIZE as i32)) as u8,
            self.y.mod_floor(&(CHUNK_SIZE as i32)) as u8,
            self.z.mod_floor(&(CHUNK_SIZE as i32)) as u8,
        )
    }
    pub fn to_chunk_pos_offset(self) -> (ChunkPos, ChunkOffset) {
        let (x, mx) = self.x.div_mod_floor(&(CHUNK_SIZE as i32));
        let (y, my) = self.y.div_mod_floor(&(CHUNK_SIZE as i32));
        let (z, mz) = self.z.div_mod_floor(&(CHUNK_SIZE as i32));
        (
            ChunkPos {
                x: x as i16,
                y: y as i16,
                z: z as i16,
            },
            ChunkOffset::new(mx as u8, my as u8, mz as u8),
        )
    }
    pub fn to_pos(self) -> Pos {
        Pos {
            x: self.x as f32,
            y: self.y as f32,
            z: self.z as f32,
        }
    }
}
impl ChunkPos {
    pub fn to_block_pos(self) -> BlockPos {
        BlockPos {
            x: self.x as i32 * CHUNK_SIZE as i32,
            y: self.y as i32 * CHUNK_SIZE as i32,
            z: self.z as i32 * CHUNK_SIZE as i32,
        }
    }
}
#[derive(Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkOffset(pub u16);
impl ChunkOffset {
    pub fn new(x: u8, y: u8, z: u8) -> Self {
        Self((x as u16) | (y as u16) << CHUNK_SIZE_BITS | (z as u16) << CHUNK_SIZE_BITS * 2)
    }
    pub fn xyz(self) -> BlockPos {
        let mask = CHUNK_SIZE as u16 - 1;
        BlockPos {
            x: (self.0 & mask) as i32,
            y: ((self.0 >> CHUNK_SIZE_BITS) & mask) as i32,
            z: ((self.0 >> CHUNK_SIZE_BITS * 2) & mask) as i32,
        }
    }
    pub fn index(self) -> usize {
        self.0 as usize
    }
}
#[derive(Clone, Copy, Deserialize)]
pub struct AABB<T: Copy> {
    pub min: Vec3<T>,
    pub max: Vec3<T>,
}
impl<T: Copy + Ord> AABB<T> {
    pub fn new(a: Vec3<T>, b: Vec3<T>) -> AABB<T> {
        AABB {
            min: Vec3 {
                x: Ord::min(a.x, b.x),
                y: Ord::min(a.y, b.y),
                z: Ord::min(a.z, b.z),
            },
            max: Vec3 {
                x: Ord::max(a.x, b.x),
                y: Ord::max(a.y, b.y),
                z: Ord::max(a.z, b.z),
            },
        }
    }
    pub fn contains(self, point: Vec3<T>) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }
    pub fn intersects(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }
}
impl<T: Copy + Add<T, Output = T>> AABB<T> {
    pub fn offset(self, offset: Vec3<T>) -> Self {
        AABB {
            min: self.min + offset,
            max: self.max + offset,
        }
    }
}
impl<T: Copy + Ord + AABBWalkable> IntoIterator for AABB<T> {
    type Item = Vec3<T>;
    type IntoIter = AABBIterator<T>;
    fn into_iter(self) -> AABBIterator<T> {
        AABBIterator {
            bb: self,
            head: self.min,
        }
    }
}
pub struct AABBIterator<T: Copy + Ord> {
    bb: AABB<T>,
    head: Vec3<T>,
}
impl<T: Copy + AABBWalkable> Iterator for AABBIterator<T> {
    type Item = Vec3<T>;
    fn next(&mut self) -> Option<Vec3<T>> {
        if self.head.z > self.bb.max.z {
            return None;
        }
        let previous_head = self.head;
        if self.head.x.aabb_walk(self.bb.min.x, self.bb.max.x, true) {
            if self.head.y.aabb_walk(self.bb.min.y, self.bb.max.y, true) {
                self.head.z.aabb_walk(self.bb.min.z, self.bb.max.z, false);
            }
        }
        Some(previous_head)
    }
}
trait AABBWalkable: Ord {
    fn aabb_walk(&mut self, min: Self, max: Self, reset: bool) -> bool;
}
macro_rules! implement_aabb_walkable {
    ($type:tt) => {
        impl AABBWalkable for $type {
            fn aabb_walk(&mut self, min: Self, max: Self, reset: bool) -> bool {
                *self += 1;
                if *self > max {
                    if reset{
                        *self = min;
                    }
                    return true;
                } else {
                    return false;
                }
            }
        }
    };
    ($($type:tt),+) => {
        $(implement_aabb_walkable!($type);)*
    }
}
implement_aabb_walkable!(i32, i16);
impl AABB<f32> {
    pub fn to_block(self) -> AABB<i32> {
        AABB {
            min: self.min.to_block_pos(),
            max: self.max.to_block_pos(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Face {
    Front,
    Back,
    Up,
    Down,
    Left,
    Right,
}
impl Face {
    pub fn all() -> &'static [Face; 6] {
        &[
            Face::Front,
            Face::Back,
            Face::Up,
            Face::Down,
            Face::Left,
            Face::Right,
        ]
    }
    pub fn horizontal() -> &'static [Face; 4] {
        &[Face::Front, Face::Back, Face::Left, Face::Right]
    }
    pub fn get_block_offset(&self) -> BlockPos {
        match self {
            Self::Front => BlockPos { x: 0, y: 0, z: -1 },
            Self::Back => BlockPos { x: 0, y: 0, z: 1 },
            Self::Left => BlockPos { x: -1, y: 0, z: 0 },
            Self::Right => BlockPos { x: 1, y: 0, z: 0 },
            Self::Up => BlockPos { x: 0, y: 1, z: 0 },
            Self::Down => BlockPos { x: 0, y: -1, z: 0 },
        }
    }
    pub fn get_offset(&self) -> Pos {
        self.get_block_offset().to_pos()
    }
    pub fn get_chunk_offset(&self) -> ChunkPos {
        let block_offset = self.get_block_offset();
        ChunkPos {
            x: block_offset.x as i16,
            y: block_offset.y as i16,
            z: block_offset.z as i16,
        }
    }
    pub fn opposite(&self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Front => Self::Back,
            Self::Back => Self::Front,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FaceMap<T> {
    pub front: T,
    pub back: T,
    pub left: T,
    pub right: T,
    pub up: T,
    pub down: T,
}
impl<T> FaceMap<T> {
    pub fn init(mut initializer: impl FnMut(Face) -> T) -> FaceMap<T> {
        FaceMap {
            front: initializer(Face::Front),
            back: initializer(Face::Back),
            left: initializer(Face::Left),
            right: initializer(Face::Right),
            up: initializer(Face::Up),
            down: initializer(Face::Down),
        }
    }
    pub fn by_face(&self, face: Face) -> &T {
        match face {
            Face::Front => &self.front,
            Face::Back => &self.back,
            Face::Left => &self.left,
            Face::Right => &self.right,
            Face::Up => &self.up,
            Face::Down => &self.down,
        }
    }
    pub fn by_face_mut(&mut self, face: Face) -> &mut T {
        match face {
            Face::Front => &mut self.front,
            Face::Back => &mut self.back,
            Face::Left => &mut self.left,
            Face::Right => &mut self.right,
            Face::Up => &mut self.up,
            Face::Down => &mut self.down,
        }
    }
    pub fn map<'a, U>(&'a self, mut mapper: impl FnMut(&'a T) -> U) -> FaceMap<U> {
        FaceMap::init(|face| mapper(self.by_face(face)))
    }
}
#[derive(Clone, Copy)]
pub struct Ray {
    pub position: Pos,
    pub direction: Vec3<f32>,
}
impl Ray {
    pub fn block_raycast<T>(
        self,
        mut f: impl FnMut(BlockPos, Pos, Face) -> Option<T>,
    ) -> Option<T> {
        let origin = self.position;

        let max_distance = (self.direction.x * self.direction.x
            + self.direction.y * self.direction.y
            + self.direction.z * self.direction.z)
            .sqrt();

        if max_distance == 0.0 {
            return None;
        }

        let dir = Vec3 {
            x: self.direction.x / max_distance,
            y: self.direction.y / max_distance,
            z: self.direction.z / max_distance,
        };

        let mut voxel = BlockPos {
            x: origin.x.floor() as i32,
            y: origin.y.floor() as i32,
            z: origin.z.floor() as i32,
        };

        let step_x = if dir.x > 0.0 { 1 } else { -1 };
        let step_y = if dir.y > 0.0 { 1 } else { -1 };
        let step_z = if dir.z > 0.0 { 1 } else { -1 };

        let next_x = if step_x > 0 {
            voxel.x as f32 + 1.0
        } else {
            voxel.x as f32
        };
        let next_y = if step_y > 0 {
            voxel.y as f32 + 1.0
        } else {
            voxel.y as f32
        };
        let next_z = if step_z > 0 {
            voxel.z as f32 + 1.0
        } else {
            voxel.z as f32
        };

        let mut t_max_x = if dir.x != 0.0 {
            (next_x - origin.x) / dir.x
        } else {
            f32::INFINITY
        };
        let mut t_max_y = if dir.y != 0.0 {
            (next_y - origin.y) / dir.y
        } else {
            f32::INFINITY
        };
        let mut t_max_z = if dir.z != 0.0 {
            (next_z - origin.z) / dir.z
        } else {
            f32::INFINITY
        };

        let t_delta_x = if dir.x != 0.0 {
            (1.0 / dir.x).abs()
        } else {
            f32::INFINITY
        };
        let t_delta_y = if dir.y != 0.0 {
            (1.0 / dir.y).abs()
        } else {
            f32::INFINITY
        };
        let t_delta_z = if dir.z != 0.0 {
            (1.0 / dir.z).abs()
        } else {
            f32::INFINITY
        };

        let mut distance = 0.0;
        let mut hit_face = Face::Front;

        while distance <= max_distance {
            let hit_pos = Pos {
                x: origin.x + dir.x * distance,
                y: origin.y + dir.y * distance,
                z: origin.z + dir.z * distance,
            };
            if let Some(hit) = f(voxel, hit_pos, hit_face) {
                return Some(hit);
            }

            if t_max_x < t_max_y && t_max_x < t_max_z {
                voxel.x += step_x;
                distance = t_max_x;
                t_max_x += t_delta_x;
                hit_face = if step_x > 0 { Face::Left } else { Face::Right };
            } else if t_max_y < t_max_z {
                voxel.y += step_y;
                distance = t_max_y;
                t_max_y += t_delta_y;
                hit_face = if step_y > 0 { Face::Down } else { Face::Up };
            } else {
                voxel.z += step_z;
                distance = t_max_z;
                t_max_z += t_delta_z;
                hit_face = if step_z > 0 { Face::Front } else { Face::Back };
            }
        }
        None
    }
    pub fn aabb_raycast(self, aabb: AABB<f32>) -> Option<AABBRaycastResult> {
        let origin = self.position;
        let dir = self.direction;

        // Max ray length comes from direction magnitude
        let max_dist = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z).sqrt();
        if max_dist == 0.0 {
            return None;
        }

        let inv_dir = Vec3 {
            x: if dir.x != 0.0 {
                1.0 / dir.x
            } else {
                f32::INFINITY
            },
            y: if dir.y != 0.0 {
                1.0 / dir.y
            } else {
                f32::INFINITY
            },
            z: if dir.z != 0.0 {
                1.0 / dir.z
            } else {
                f32::INFINITY
            },
        };

        let mut t_min = 0.0;
        let mut t_max = max_dist;
        let mut enter_face: Option<Face> = None;
        let mut exit_face: Option<Face> = None;

        // --- X slab ---
        {
            let (t1, t2, face_enter, face_exit) = if inv_dir.x >= 0.0 {
                (
                    (aabb.min.x - origin.x) * inv_dir.x,
                    (aabb.max.x - origin.x) * inv_dir.x,
                    Face::Left,
                    Face::Right,
                )
            } else {
                (
                    (aabb.max.x - origin.x) * inv_dir.x,
                    (aabb.min.x - origin.x) * inv_dir.x,
                    Face::Right,
                    Face::Left,
                )
            };

            if t1 > t_min {
                t_min = t1;
                enter_face = Some(face_enter);
            }

            if t2 < t_max {
                t_max = t2;
                exit_face = Some(face_exit);
            }

            if t_min > t_max {
                return None;
            }
        }

        // --- Y slab ---
        {
            let (t1, t2, face_enter, face_exit) = if inv_dir.y >= 0.0 {
                (
                    (aabb.min.y - origin.y) * inv_dir.y,
                    (aabb.max.y - origin.y) * inv_dir.y,
                    Face::Down,
                    Face::Up,
                )
            } else {
                (
                    (aabb.max.y - origin.y) * inv_dir.y,
                    (aabb.min.y - origin.y) * inv_dir.y,
                    Face::Up,
                    Face::Down,
                )
            };

            if t1 > t_min {
                t_min = t1;
                enter_face = Some(face_enter);
            }

            if t2 < t_max {
                t_max = t2;
                exit_face = Some(face_exit);
            }

            if t_min > t_max {
                return None;
            }
        }

        // --- Z slab ---
        {
            let (t1, t2, face_enter, face_exit) = if inv_dir.z >= 0.0 {
                (
                    (aabb.min.z - origin.z) * inv_dir.z,
                    (aabb.max.z - origin.z) * inv_dir.z,
                    Face::Front,
                    Face::Back,
                )
            } else {
                (
                    (aabb.max.z - origin.z) * inv_dir.z,
                    (aabb.min.z - origin.z) * inv_dir.z,
                    Face::Back,
                    Face::Front,
                )
            };

            if t1 > t_min {
                t_min = t1;
                enter_face = Some(face_enter);
            }

            if t2 < t_max {
                t_max = t2;
                exit_face = Some(face_exit);
            }

            if t_min > t_max {
                return None;
            }
        }

        // Determine whether we hit entering or exiting
        let (t_hit, face) = if t_min >= 0.0 {
            // Ray starts outside → first hit is entering
            (t_min, enter_face)
        } else {
            // Ray starts inside → first hit is exiting
            (t_max, exit_face)
        };

        // Clamp to ray length
        if t_hit < 0.0 || t_hit > max_dist {
            return None;
        }

        let hit_pos = Pos {
            x: origin.x + dir.x * t_hit,
            y: origin.y + dir.y * t_hit,
            z: origin.z + dir.z * t_hit,
        };

        Some(AABBRaycastResult {
            position: hit_pos,
            face: face?,
        })
    }
}
pub struct AABBRaycastResult {
    pub position: Pos,
    pub face: Face,
}
impl Face {
    pub fn add_vertices(
        self,
        coords: TexCoords,
        rotation: u8,
        mut vertex_consumer: impl FnMut(Pos, (f32, f32)),
    ) {
        let (first, second, third, fourth) = match self {
            Face::Front => (
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 0.,
                },
            ),
            Face::Back => (
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 1.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 1.,
                },
            ),
            Face::Up => (
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 1.,
                },
            ),
            Face::Down => (
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 1.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 1.,
                },
            ),
            Face::Left => (
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 0.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 1.,
                },
                Pos {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
            ),
            Face::Right => (
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 1.,
                },
                Pos {
                    x: 1.,
                    y: 1.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 0.,
                },
                Pos {
                    x: 1.,
                    y: 0.,
                    z: 1.,
                },
            ),
        };
        let get_uv = |id: u8| match (id + rotation) % 4 {
            0 => (coords.u1, coords.v1),
            1 => (coords.u2, coords.v1),
            2 => (coords.u2, coords.v2),
            3 => (coords.u1, coords.v2),
            _ => unreachable!(),
        };

        vertex_consumer(first, get_uv(0));
        vertex_consumer(fourth, get_uv(3));
        vertex_consumer(third, get_uv(2));

        vertex_consumer(third, get_uv(2));
        vertex_consumer(second, get_uv(1));
        vertex_consumer(first, get_uv(0));
    }
}
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}
impl Face {
    pub fn axis_direction(self) -> (Axis, bool) {
        match self {
            Face::Front => (Axis::Z, true),
            Face::Back => (Axis::Z, false),
            Face::Up => (Axis::Y, false),
            Face::Down => (Axis::Y, true),
            Face::Left => (Axis::X, true),
            Face::Right => (Axis::X, false),
        }
    }
    pub fn cross(self, other: Face) -> Face {
        let (a_axis, a_dir) = self.axis_direction();
        let (b_axis, b_dir) = other.axis_direction();

        let (axis, dir) = match (a_axis, b_axis) {
            (Axis::X, Axis::Y) => (Axis::Z, false),
            (Axis::Y, Axis::Z) => (Axis::X, false),
            (Axis::Z, Axis::X) => (Axis::Y, false),
            (Axis::Y, Axis::X) => (Axis::Z, true),
            (Axis::Z, Axis::Y) => (Axis::X, true),
            (Axis::X, Axis::Z) => (Axis::Y, true),
            _ => panic!(),
        };
        let dir = dir ^ a_dir ^ b_dir;
        *Face::all()
            .into_iter()
            .find(|face| {
                let (f_axis, f_dir) = face.axis_direction();
                f_axis == axis && f_dir == dir
            })
            .unwrap()
    }
}
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Orientation {
    pub right: Face,
    pub up: Face,
    pub forward: Face,
}
static ALL_ORIENTATIONS: OnceLock<[Orientation; 24]> = OnceLock::new();
impl Orientation {
    pub const IDENTITY: Self = Self {
        right: Face::Right,
        up: Face::Up,
        forward: Face::Front,
    };
    pub fn compose(self, other: Orientation) -> Orientation {
        Orientation {
            right: self.apply(other.right),
            up: self.apply(other.up),
            forward: self.apply(other.forward),
        }
    }
    pub fn apply(self, face: Face) -> Face {
        let (axis, dir) = face.axis_direction();
        let base = match axis {
            Axis::X => self.right,
            Axis::Y => self.up,
            Axis::Z => self.forward.opposite(),
        };

        if dir { base.opposite() } else { base }
    }
    pub fn inverse_apply(self, face: Face) -> Face {
        for test in Face::all() {
            if self.apply(*test) == face {
                return *test;
            }
        }
        unreachable!()
    }
    pub fn rotate_pos(self, v: Pos) -> Pos {
        let mut out = Pos::ZERO;
        out += self.right.get_offset() * v.x;
        out += self.up.get_offset() * v.y;
        out += -self.forward.get_offset() * v.z;
        out
    }
    pub fn rotate_block_pos(self, v: BlockPos) -> BlockPos {
        let mut out = BlockPos::ZERO;
        out += self.right.get_block_offset() * v.x;
        out += self.up.get_block_offset() * v.y;
        out += -self.forward.get_block_offset() * v.z;
        out
    }
    pub fn from_front_up(front: Face, up: Face) -> Option<Self> {
        if front == up || front == up.opposite() {
            return None;
        }
        let right = front.cross(up);
        Some(Self {
            right,
            up,
            forward: front,
        })
    }
    pub fn from_front_right(front: Face, right: Face) -> Option<Self> {
        if front == right || front == right.opposite() {
            return None;
        }
        let up = right.cross(front);
        Some(Self {
            right,
            up,
            forward: front,
        })
    }
    pub fn all() -> &'static [Orientation; 24] {
        ALL_ORIENTATIONS.get_or_init(|| {
            let mut result = Vec::with_capacity(24);
            for first in Face::all() {
                for second in Face::all() {
                    if let Some(orientation) = Orientation::from_front_up(*first, *second) {
                        result.push(orientation);
                    }
                }
            }
            result.try_into().unwrap()
        })
    }
}
