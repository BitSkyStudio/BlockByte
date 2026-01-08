use serde::{Deserialize, Serialize};

use crate::registry::ItemKey;

pub mod coord;
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
