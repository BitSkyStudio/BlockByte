use std::{collections::HashMap, sync::RwLock};

use block_byte_common::coord::ChunkPos;
use palettevec::{PaletteVec, index_buffer::AlignedIndexBuffer, palette::HybridPalette};

use crate::registry::Key;

pub struct BlockData {}
pub type BlockKey = Key<BlockData>;

pub struct World {
    pub chunks: HashMap<ChunkPos, Chunk>,
}
pub struct Chunk {
    pub blocks: RwLock<PaletteVec<BlockKey, HybridPalette<16, BlockKey>, AlignedIndexBuffer>>,
}
