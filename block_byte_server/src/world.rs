use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use block_byte_common::{
    coord::{CHUNK_SIZE, ChunkOffset, ChunkPos},
    net::NetworkMessageS2C,
    registry::{BlockKey, BlockPalette, key_of_id},
};
use palettevec::{PaletteVec, index_buffer::AlignedIndexBuffer, palette::HybridPalette};
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;

use crate::{
    Server, UserIndex,
    registry::{Key, RegistryConfigLoadable},
};
pub struct Chunk {
    pub position: ChunkPos,
    pub blocks: RwLock<BlockPalette>,
    pub viewers: HashSet<UserIndex>,
    pub block_events: Mutex<Vec<(ChunkOffset, BlockEvent)>>,
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
                        blocks.set(offset.index(), &grass);
                    }
                }
            }
        }
        //blocks.set(ChunkOffset::new(16, 16, 16).index(), &grass);
        Chunk {
            position,
            blocks: RwLock::new(blocks),
            viewers: HashSet::new(),
            block_events: Mutex::new(Vec::new()),
        }
    }
    pub fn tick(&self, server: &Server) {
        let mut processing_events = Vec::new();
        std::mem::swap(&mut processing_events, &mut *self.block_events.lock());
        for (block, event) in processing_events {
            match event {
                BlockEvent::Damage { damage } => {
                    self.blocks
                        .write()
                        .set(block.index(), &key_of_id("air").unwrap());
                    server.send_message_multiple(
                        self.viewers.iter().cloned(),
                        NetworkMessageS2C::SetBlock {
                            position: self.position.to_block_pos() + block.xyz(),
                            block: key_of_id("air").unwrap(),
                        },
                    );
                }
            }
        }
    }
}
pub enum BlockEvent {
    Damage { damage: u32 },
}
