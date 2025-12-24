use palettevec::PaletteVec;
use serde::{Deserialize, Serialize};

use crate::{
    coord::{BlockPos, ChunkPos, Face, Pos},
    registry::{BlockKey, BlockPalette},
};

#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {
    PlayerPosition { position: Pos },
    AttackBlock { position: BlockPos },
    InteractBlock { position: BlockPos, face: Face },
}
#[derive(Serialize, Deserialize)]
pub enum NetworkMessageS2C {
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
}
