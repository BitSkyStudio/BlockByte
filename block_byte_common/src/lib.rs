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
#[derive(Clone)]
pub struct InventoryView {
    pub slots: Vec<usize>,
}
impl InventoryView {
    pub fn from_range(range: std::ops::Range<usize>) -> Self {
        InventoryView {
            slots: range.collect(),
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
            slots: Vec::<usize>::deserialize(deserializer)?,
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
        #[derive(Deserialize)]
        pub struct DamageTable{
            $(
                #[serde(default = "default_damage_vulnerability")]
                $id: f32,
            )*
        }
        impl std::ops::Index<DamageType> for DamageTable {
            type Output = f32;

            fn index(&self, damage_type: DamageType) -> &Self::Output {
                match damage_type {
                    $(DamageType::$id => &self.$id,)*
                }
            }
        }
    };
}
create_damage_types!(Blunt, Pierce, Slash, Cut);
