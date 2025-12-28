use std::{
    fmt::Debug,
    marker::PhantomData,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign},
};

use num_integer::{Integer, Roots};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const CHUNK_SIZE_BITS: u8 = 5;
pub const CHUNK_SIZE: u8 = 32;

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
        (self.x * self.y) + (self.y * self.y) + (self.z * self.z)
    }
    pub fn distance_squared(self, other: Self) -> T {
        (self - other).length_squared()
    }
}
impl<T: Copy + Add<Output = T> + Mul<Output = T> + Sub<Output = T> + Roots> Vec3<T> {
    pub fn length(self) -> T {
        self.length_squared().sqrt()
    }
    pub fn distance(self, other: Self) -> T {
        self.distance_squared(other).sqrt()
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

pub type Pos = Vec3<f32>;
pub type BlockPos = Vec3<i32>;
pub type ChunkPos = Vec3<i16>;

impl Pos {
    pub fn to_block_pos(self) -> BlockPos {
        BlockPos {
            x: self.x.floor() as i32,
            y: self.y.floor() as i32,
            z: self.z.floor() as i32,
        }
    }
    pub fn to_chunk_pos(self) -> ChunkPos {
        self.to_block_pos().to_chunk_pos()
    }
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
        debug_assert!(x < CHUNK_SIZE);
        debug_assert!(y < CHUNK_SIZE);
        debug_assert!(z < CHUNK_SIZE);
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

        let max_distance = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z).sqrt();
        if max_distance == 0.0 {
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
        let mut t_max = max_distance;

        let mut hit_face = None;

        // X slab
        {
            let mut t1 = (aabb.min.x - origin.x) * inv_dir.x;
            let mut t2 = (aabb.max.x - origin.x) * inv_dir.x;

            let face1 = if inv_dir.x >= 0.0 {
                Face::Left
            } else {
                Face::Right
            };

            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }

            if t1 > t_min {
                t_min = t1;
                hit_face = Some(face1);
            }

            t_max = t_max.min(t2);
            if t_min > t_max {
                return None;
            }
        }

        // Y slab
        {
            let mut t1 = (aabb.min.y - origin.y) * inv_dir.y;
            let mut t2 = (aabb.max.y - origin.y) * inv_dir.y;

            let face1 = if inv_dir.y >= 0.0 {
                Face::Down
            } else {
                Face::Up
            };

            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }

            if t1 > t_min {
                t_min = t1;
                hit_face = Some(face1);
            }

            t_max = t_max.min(t2);
            if t_min > t_max {
                return None;
            }
        }

        // Z slab
        {
            let mut t1 = (aabb.min.z - origin.z) * inv_dir.z;
            let mut t2 = (aabb.max.z - origin.z) * inv_dir.z;

            let face1 = if inv_dir.z >= 0.0 {
                Face::Front
            } else {
                Face::Back
            };

            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }

            if t1 > t_min {
                t_min = t1;
                hit_face = Some(face1);
            }

            t_max = t_max.min(t2);
            if t_min > t_max {
                return None;
            }
        }

        if t_min < 0.0 || t_min > max_distance {
            return None;
        }

        let hit_pos = Pos {
            x: origin.x + dir.x * (t_min / max_distance),
            y: origin.y + dir.y * (t_min / max_distance),
            z: origin.z + dir.z * (t_min / max_distance),
        };

        Some(AABBRaycastResult {
            position: hit_pos,
            face: hit_face?,
        })
    }
}
pub struct AABBRaycastResult {
    pub position: Pos,
    pub face: Face,
}
