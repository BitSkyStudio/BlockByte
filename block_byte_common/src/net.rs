use palettevec::PaletteVec;
use serde::{Deserialize, Serialize};

use crate::{
    coord::{ChunkPos, Pos},
    registry::BlockPalette,
};

#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {
    PlayerPosition { position: Pos },
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
}
