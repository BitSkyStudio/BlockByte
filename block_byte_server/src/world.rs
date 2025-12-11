use std::{collections::HashMap, path::Path};

use block_byte_common::coord::ChunkPos;
use palettevec::{PaletteVec, index_buffer::AlignedIndexBuffer, palette::HybridPalette};
use parking_lot::RwLock;
use serde::Deserialize;

use crate::registry::{Key, RegistryConfigLoadable};

#[derive(Deserialize)]
pub struct BlockData {}
pub type BlockKey = Key<BlockData>;

pub struct World {
    pub chunks: HashMap<ChunkPos, Chunk>,
}
pub struct Chunk {
    pub blocks: RwLock<PaletteVec<BlockKey, HybridPalette<16, BlockKey>, AlignedIndexBuffer>>,
}
