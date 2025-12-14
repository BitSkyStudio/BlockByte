use palettevec::PaletteVec;
use serde::{Deserialize, Serialize};

use crate::{coord::ChunkPos, registry::BlockPalette};

#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {}
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
