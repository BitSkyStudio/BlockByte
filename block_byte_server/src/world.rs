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
        let air = key_of_id("air").unwrap();
        let grass = key_of_id("nature.grass").unwrap();

        let mut blocks = BlockPalette::filled(
            air,
            CHUNK_SIZE as usize * CHUNK_SIZE as usize * CHUNK_SIZE as usize,
        );
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let offset = ChunkOffset::new(x, y, z);
                    let block_pos = position.to_block_pos() + offset.xyz();
                    if block_pos.y < 45 + ((block_pos.x + block_pos.z).abs() as i32 % 10 - 5).abs()
                    {
                        blocks.set(offset.0 as usize, &grass);
                    }
                }
            }
        }
        //blocks.set(ChunkOffset::new(16, 16, 16).0 as usize, &grass);
        Chunk {
            blocks: RwLock::new(blocks),
            viewers: HashSet::new(),
        }
    }
}
