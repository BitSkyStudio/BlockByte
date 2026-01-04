use serde::{Deserialize, Serialize};

pub mod coord;
pub mod net;
pub mod registry;
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
