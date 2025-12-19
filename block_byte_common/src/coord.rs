use std::{
    fmt::Debug,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign},
};

use num_integer::{Integer, Roots};
use serde::{Deserialize, Serialize};

pub const CHUNK_SIZE_BITS: u8 = 5;
pub const CHUNK_SIZE: u8 = 32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Vec3<T: Copy> {
    pub x: T,
    pub y: T,
    pub z: T,
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
#[derive(Copy, Clone)]
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
}
#[derive(Clone, Copy)]
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
