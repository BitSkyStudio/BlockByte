use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use block_byte_common::{
    coord::ChunkPos,
    registry::{BlockKey, BlockPalette},
};
use palettevec::{PaletteVec, index_buffer::AlignedIndexBuffer, palette::HybridPalette};
use parking_lot::RwLock;
use serde::Deserialize;

use crate::{
    UserIndex,
    registry::{Key, RegistryConfigLoadable},
};
pub struct Chunk {
    pub blocks: RwLock<BlockPalette>,
    pub viewers: HashSet<UserIndex>,
}
