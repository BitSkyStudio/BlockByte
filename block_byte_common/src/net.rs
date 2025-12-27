use palettevec::PaletteVec;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    coord::{BlockPos, ChunkPos, Face, Pos},
    registry::{BlockKey, BlockPalette, EntityKey},
};

#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {
    PlayerPosition { position: Pos },
    AttackBlock { position: BlockPos },
    InteractBlock { position: BlockPos, face: Face },
}
#[derive(Serialize, Deserialize)]
pub enum NetworkMessageS2C {
    GameTick {
        ticks_passed: u64,
        dt: f32,
    },
    LoadChunk {
        position: ChunkPos,
        blocks: BlockPalette,
    },
    UnloadChunk {
        position: ChunkPos,
    },
    SetBlock {
        position: BlockPos,
        block: BlockKey,
    },
    AddEntity {
        uuid: Uuid,
        key: EntityKey,
        position: Pos,
    },
    MoveEntity {
        uuid: Uuid,
        position: Pos,
    },
    RemoveEntity {
        uuid: Uuid,
    },
}
