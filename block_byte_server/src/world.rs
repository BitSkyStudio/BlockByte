use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use block_byte_common::{
    coord::{CHUNK_SIZE, ChunkOffset, ChunkPos},
    registry::{BlockKey, BlockPalette, key_of_id},
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
impl Chunk {
    pub fn generate(position: ChunkPos) -> Chunk {
        let mut blocks = BlockPalette::new();
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let block_pos = position.to_block_pos() + ChunkOffset::new(x, y, z).xyz();
                    blocks.push(
                        key_of_id(if block_pos.y < 15 {
                            "nature.grass"
                        } else {
                            "air"
                        })
                        .unwrap(),
                    );
                }
            }
        }
        Chunk {
            blocks: RwLock::new(blocks),
            viewers: HashSet::new(),
        }
    }
}
