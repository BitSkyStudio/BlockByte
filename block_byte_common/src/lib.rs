use serde::{Deserialize, Serialize};

use crate::registry::ItemKey;

pub mod coord;
pub mod model;
pub mod net;
pub mod registry;
pub mod ui;
pub mod world;

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
}
#[derive(Clone, Serialize, Deserialize)]
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
    pub fn map(self, coords: (f32, f32)) -> (f32, f32) {
        let self_w = self.u2 - self.u1;
        let self_h = self.v2 - self.v1;
        (self.u1 + (coords.0 * self_w), self.v1 + (coords.1 * self_h))
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

#[derive(Copy, Clone)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
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
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct LookDirection {
    pub pitch: f32,
    pub yaw: f32,
}
