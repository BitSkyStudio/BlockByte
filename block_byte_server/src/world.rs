use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::OnceLock,
};

use block_byte_common::{
    coord::{CHUNK_SIZE, ChunkOffset, ChunkPos},
    net::NetworkMessageS2C,
    registry::{BlockData, BlockKey, BlockPalette},
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
    pub components: ChunkBlockComponents,
}
impl Chunk {
    pub fn generate(position: ChunkPos) -> Chunk {
        let air = air_block();
        let grass = Key::id("nature.grass").unwrap();

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
            components: ChunkBlockComponents::default(),
        }
    }
    pub fn tick(&self, server: &Server) {
        let mut processing_events = Vec::new();
        std::mem::swap(&mut processing_events, &mut *self.block_events.lock());
        for (block, event) in processing_events {
            match event {
                BlockEvent::Damage { damage } => {
                    let health = self.blocks.read().get(block.index()).unwrap().data().health;
                    if let Some(health) = health {
                        let mut damage_component = self.components.damage.write();
                        let mut destroy = false;

                        if damage >= health {
                            destroy = true;
                        } else if let Some(block_damage) = damage_component.get_mut(block) {
                            block_damage.damage += damage;
                            if block_damage.damage >= health {
                                destroy = true;
                            }
                        } else {
                            damage_component.set(block, BlockDamage { damage });
                        }
                        if destroy {
                            damage_component.remove(block);
                            self.blocks.write().set(block.index(), &air_block());
                            server.send_message_multiple(
                                self.viewers.iter().cloned(),
                                NetworkMessageS2C::SetBlock {
                                    position: self.position.to_block_pos() + block.xyz(),
                                    block: air_block(),
                                },
                            );
                        }
                    }
                }
            }
        }
        {
            self.components
                .damage
                .write()
                .components
                .retain_mut(|(_, damage)| {
                    damage.damage -= 1. / server.tps as f32;
                    damage.damage > 0.
                });
        }
    }
}
pub enum BlockEvent {
    Damage { damage: f32 },
}

pub struct BlockDamage {
    pub damage: f32,
}

static AIR_BLOCK: OnceLock<BlockKey> = OnceLock::new();
pub fn air_block() -> BlockKey {
    *AIR_BLOCK.get_or_init(|| Key::id("air").unwrap())
}
pub struct BlockComponentStorage<T> {
    pub components: Vec<(ChunkOffset, T)>,
}
impl<T> BlockComponentStorage<T> {
    pub fn get(&self, block: ChunkOffset) -> Option<&T> {
        self.components
            .iter()
            .find(|(offset, _)| *offset == block)
            .map(|(_, data)| data)
    }
    pub fn get_mut(&mut self, block: ChunkOffset) -> Option<&mut T> {
        self.components
            .iter_mut()
            .find(|(offset, _)| *offset == block)
            .map(|(_, data)| data)
    }
    pub fn set(&mut self, block: ChunkOffset, data: T) {
        if let Some(existing) = self.get_mut(block) {
            *existing = data;
        } else {
            self.components.push((block, data));
        }
    }
    pub fn remove(&mut self, block: ChunkOffset) -> bool {
        if let Some(index) = self
            .components
            .iter()
            .position(|(offset, _)| *offset == block)
        {
            self.components.swap_remove(index);
            true
        } else {
            false
        }
    }
}
impl<T> Default for BlockComponentStorage<T> {
    fn default() -> Self {
        BlockComponentStorage {
            components: Vec::new(),
        }
    }
}
macro_rules! create_chunk_block_components{
    ($($type:ty,$id:ident);*) => {
        trait GetComponentStorage<T>{
            fn get_component_storage(&self) -> &RwLock<BlockComponentStorage<T>>;
        }
        #[derive(Default)]
        pub struct ChunkBlockComponents{
            $($id: RwLock<BlockComponentStorage<$type>>,)*
        }
        $(
        impl GetComponentStorage<$type> for ChunkBlockComponents{
            fn get_component_storage(&self) -> &RwLock<BlockComponentStorage<$type>>{
                &self.$id
            }
        }
        )*
    }
}

create_chunk_block_components!(BlockDamage, damage);
