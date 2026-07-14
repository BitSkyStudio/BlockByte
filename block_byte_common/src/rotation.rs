use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::coord::{AABB, Axis, BlockPos, Face, HorizontalFace, Pos};

#[derive(Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BlockRotation {
    FrontUp = 0,
    FrontDown = 1,
    FrontRight = 2,
    FrontLeft = 3,
    BackUp = 4,
    BackDown = 5,
    BackRight = 6,
    BackLeft = 7,
    UpFront = 8,
    UpBack = 9,
    UpRight = 10,
    UpLeft = 11,
    DownFront = 12,
    DownBack = 13,
    DownRight = 14,
    DownLeft = 15,
    RightFront = 16,
    RightBack = 17,
    RightUp = 18,
    RightDown = 19,
    LeftFront = 20,
    LeftBack = 21,
    LeftUp = 22,
    LeftDown = 23,
}
impl BlockRotation {
    pub fn front_face(self) -> Face {
        match self {
            BlockRotation::FrontUp => Face::Front,
            BlockRotation::FrontDown => Face::Front,
            BlockRotation::FrontRight => Face::Front,
            BlockRotation::FrontLeft => Face::Front,
            BlockRotation::BackUp => Face::Back,
            BlockRotation::BackDown => Face::Back,
            BlockRotation::BackRight => Face::Back,
            BlockRotation::BackLeft => Face::Back,
            BlockRotation::UpFront => Face::Up,
            BlockRotation::UpBack => Face::Up,
            BlockRotation::UpRight => Face::Up,
            BlockRotation::UpLeft => Face::Up,
            BlockRotation::DownFront => Face::Down,
            BlockRotation::DownBack => Face::Down,
            BlockRotation::DownRight => Face::Down,
            BlockRotation::DownLeft => Face::Down,
            BlockRotation::RightFront => Face::Right,
            BlockRotation::RightBack => Face::Right,
            BlockRotation::RightUp => Face::Right,
            BlockRotation::RightDown => Face::Right,
            BlockRotation::LeftFront => Face::Left,
            BlockRotation::LeftBack => Face::Left,
            BlockRotation::LeftUp => Face::Left,
            BlockRotation::LeftDown => Face::Left,
        }
    }
    pub fn up_face(self) -> Face {
        match self {
            BlockRotation::FrontUp => Face::Up,
            BlockRotation::FrontDown => Face::Down,
            BlockRotation::FrontRight => Face::Right,
            BlockRotation::FrontLeft => Face::Left,
            BlockRotation::BackUp => Face::Up,
            BlockRotation::BackDown => Face::Down,
            BlockRotation::BackRight => Face::Right,
            BlockRotation::BackLeft => Face::Left,
            BlockRotation::UpFront => Face::Front,
            BlockRotation::UpBack => Face::Back,
            BlockRotation::UpRight => Face::Right,
            BlockRotation::UpLeft => Face::Left,
            BlockRotation::DownFront => Face::Front,
            BlockRotation::DownBack => Face::Back,
            BlockRotation::DownRight => Face::Right,
            BlockRotation::DownLeft => Face::Left,
            BlockRotation::RightFront => Face::Front,
            BlockRotation::RightBack => Face::Back,
            BlockRotation::RightUp => Face::Up,
            BlockRotation::RightDown => Face::Down,
            BlockRotation::LeftFront => Face::Front,
            BlockRotation::LeftBack => Face::Back,
            BlockRotation::LeftUp => Face::Up,
            BlockRotation::LeftDown => Face::Down,
        }
    }
}
impl Default for BlockRotation {
    fn default() -> Self {
        BlockRotation::FrontUp
    }
}
impl BlockRotation {
    pub fn new(front: Face, up: Face) -> Option<BlockRotation> {
        precompute_table().rotations[front as usize + up as usize * 6]
    }
    pub fn looking_to(face: Face) -> BlockRotation {
        BlockRotation::new(
            face,
            match face {
                Face::Up => Face::Back,
                Face::Down => Face::Front,
                _ => Face::Up,
            },
        )
        .unwrap()
    }
    pub fn looking_to_horizontal(face: HorizontalFace) -> BlockRotation {
        BlockRotation::new(face.face(), Face::Up).unwrap()
    }
    pub fn compose(self, other: BlockRotation) -> BlockRotation {
        precompute_table().compositions[self as usize + other as usize * 24]
    }
    pub fn right_face(self) -> Face {
        self.front_face().cross(self.up_face())
    }
    pub fn rotate_aabb(self, aabb: AABB<f32>) -> AABB<f32> {
        let aabb = aabb.offset(Pos::all(-0.5));
        let orientation = Orientation::from_block_rotation(self);
        AABB::new(
            orientation.rotate_pos(aabb.min),
            orientation.rotate_pos(aabb.max),
        )
        .offset(Pos::all(0.5))
    }
    pub fn rotate_block_aabb(self, aabb: AABB<i32>) -> AABB<i32> {
        let orientation = Orientation::from_block_rotation(self);
        AABB::new(
            orientation.rotate_block_pos(aabb.min),
            orientation.rotate_block_pos(aabb.max),
        )
    }
    pub fn rotate_face(self, face: Face) -> Face {
        precompute_table().face[self as usize + face as usize * 24]
    }
    pub fn inverse_rotate_face(self, face: Face) -> Face {
        precompute_table().face_inverse[self as usize + face as usize * 24]
    }
    pub fn rotate_pos(self, v: Pos) -> Pos {
        Orientation::from_block_rotation(self).rotate_pos(v)
    }
    pub fn rotate_block_pos(self, v: BlockPos) -> BlockPos {
        Orientation::from_block_rotation(self).rotate_block_pos(v)
    }
}
struct RotationPrecompute {
    orientations: [Orientation; 24],
    rotations: [Option<BlockRotation>; 36],
    compositions: [BlockRotation; 24 * 24],
    face: [Face; 24 * 6],
    face_inverse: [Face; 24 * 6],
}
static ROTATION_TABLE: OnceLock<RotationPrecompute> = OnceLock::new();
fn precompute_table() -> &'static RotationPrecompute {
    ROTATION_TABLE.get_or_init(|| {
        fn rot_to_or(block_rotation: BlockRotation) -> Orientation {
            Orientation::from_front_up(block_rotation.front_face(), block_rotation.up_face())
                .unwrap()
        }
        fn or_to_rot(orientation: Orientation) -> BlockRotation {
            for i in 0u8..24 {
                let rotation: BlockRotation = unsafe { std::mem::transmute(i) };
                if rot_to_or(rotation) == orientation {
                    return rotation;
                }
            }
            unreachable!()
        }
        let orientations = std::array::from_fn(|i| {
            let rotation: BlockRotation = unsafe { std::mem::transmute(i as u8) };
            Orientation::from_front_up(rotation.front_face(), rotation.up_face()).unwrap()
        });
        let rotations = std::array::from_fn(|i| {
            Orientation::from_front_up(Face::all()[i % 6], Face::all()[i / 6])
                .map(|orientation| or_to_rot(orientation))
        });
        let compositions = std::array::from_fn(|i| {
            let first = &orientations[i % 24];
            let second = &orientations[i / 24];
            or_to_rot(first.compose(*second))
        });
        let face = std::array::from_fn(|i| {
            let orientation = orientations[i % 24];
            let face = Face::all()[i / 24];
            orientation.apply(face)
        });
        let face_inverse = std::array::from_fn(|i| {
            let orientation = orientations[i % 24];
            let face = Face::all()[i / 24];
            orientation.inverse_apply(face)
        });
        RotationPrecompute {
            orientations,
            rotations,
            compositions,
            face,
            face_inverse,
        }
    })
}
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
struct Orientation {
    right: Face,
    up: Face,
    forward: Face,
}
impl Orientation {
    fn compose(self, other: Orientation) -> Orientation {
        Orientation {
            right: self.apply(other.right),
            up: self.apply(other.up),
            forward: self.apply(other.forward),
        }
    }
    fn apply(self, face: Face) -> Face {
        let (axis, dir) = face.axis_direction();
        let base = match axis {
            Axis::X => self.right,
            Axis::Y => self.up,
            Axis::Z => self.forward.opposite(),
        };

        if dir { base.opposite() } else { base }
    }
    fn inverse_apply(self, face: Face) -> Face {
        for test in Face::all() {
            if self.apply(test) == face {
                return test;
            }
        }
        unreachable!()
    }
    fn rotate_pos(self, v: Pos) -> Pos {
        let mut out = Pos::ZERO;
        out += self.right.get_offset() * v.x;
        out += self.up.get_offset() * v.y;
        out += -self.forward.get_offset() * v.z;
        out
    }
    fn rotate_block_pos(self, v: BlockPos) -> BlockPos {
        let mut out = BlockPos::ZERO;
        out += self.right.get_block_offset() * v.x;
        out += self.up.get_block_offset() * v.y;
        out += -self.forward.get_block_offset() * v.z;
        out
    }
    fn from_front_up(front: Face, up: Face) -> Option<Self> {
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
    fn from_block_rotation(block_rotation: BlockRotation) -> Orientation {
        precompute_table().orientations[block_rotation as usize]
    }
}
