use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{OnceLock, atomic::AtomicBool},
};

use block_byte_common::{
    coord::{CHUNK_SIZE, ChunkOffset, ChunkPos, Pos},
    net::NetworkMessageS2C,
    registry::{BlockData, BlockKey, BlockPalette, EntityKey},
    world::{
        BlockDamage, BlockPlants, ChunkBlockComponents, ClientBlockComponentUpdate,
        ClientBlockDamage, ComponentClientFromServer,
    },
};
use palettevec::{PaletteVec, index_buffer::AlignedIndexBuffer, palette::HybridPalette};
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use slotmap::new_key_type;
use uuid::Uuid;

use crate::{
    Server, UserIndex,
    inventory::Inventory,
    registry::{Key, RegistryConfigLoadable},
};
pub struct Chunk {
    pub position: ChunkPos,
    pub blocks: RwLock<BlockPalette>,
    pub viewers: HashSet<UserIndex>,
    pub block_events: Mutex<Vec<(ChunkOffset, BlockEvent)>>,
    pub components: ChunkBlockComponents,
    pub entities: Vec<EntityIndex>,
}
impl Chunk {
    pub fn generate(position: ChunkPos) -> Chunk {
        let air = air_block();
        let grass = Key::id("nature.grass").unwrap();

        let mut blocks = BlockPalette::filled(
            air,
            CHUNK_SIZE as usize * CHUNK_SIZE as usize * CHUNK_SIZE as usize,
        );
        let mut components = ChunkBlockComponents::default();
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let offset = ChunkOffset::new(x, y, z);
                    let block_pos = position.to_block_pos() + offset.xyz();
                    let height = 45 + ((block_pos.x + block_pos.z).abs() as i32 % 10 - 5).abs();
                    if block_pos.y == height {
                        blocks.set(offset.index(), &grass);
                        components.plant.write().set(
                            offset,
                            BlockPlants {
                                plants: vec![(Key::id("grass").unwrap(), 0.)],
                            },
                        );
                    } else if block_pos.y < height {
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
            components,
            entities: Vec::new(),
        }
    }
    pub fn tick(&self, server: &Server) {
        let mut processing_events = Vec::new();
        std::mem::swap(&mut processing_events, &mut *self.block_events.lock());
        for (block, event) in processing_events {
            match event {
                BlockEvent::Damage { damage } => {
                    let block_data = &self.blocks.read().get(block.index()).unwrap().data();
                    if let Some(health) = &block_data.health {
                        let mut damage_component = self.components.damage.write();
                        let mut destroy = false;

                        if damage >= health.health {
                            destroy = true;
                        } else if let Some(block_damage) = damage_component.get_mut(block) {
                            block_damage.damage += damage;
                            if block_damage.damage >= health.health {
                                destroy = true;
                            }
                        } else {
                            damage_component.set(block, BlockDamage { damage });
                        }
                        if destroy {
                            damage_component.remove(block);
                            self.blocks.write().set(block.index(), &air_block());
                            if block_data.plantable {
                                self.components.plant.write().remove(block);
                            }
                            server.send_message_multiple(
                                self.viewers.iter(),
                                NetworkMessageS2C::SetBlock {
                                    position: self.position.to_block_pos() + block.xyz(),
                                    block: air_block(),
                                },
                            );
                        }
                        server.send_message_multiple(
                            self.viewers.iter(),
                            NetworkMessageS2C::UpdateBlockComponents {
                                chunk: self.position,
                                offset: block,
                                data: ClientBlockComponentUpdate::BlockDamage(
                                    damage_component
                                        .get(block)
                                        .map(ClientBlockDamage::from_server),
                                ),
                            },
                        );
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

static AIR_BLOCK: OnceLock<BlockKey> = OnceLock::new();
pub fn air_block() -> BlockKey {
    *AIR_BLOCK.get_or_init(|| Key::id("air").unwrap())
}

new_key_type! {pub struct EntityIndex;}
pub struct Entity {
    pub key: EntityKey,
    pub uuid: Uuid,
    pub position: Pos,
    pub teleport: Mutex<Option<Pos>>,
    pub removed: AtomicBool,
    pub inventory: RwLock<Inventory>,
}
impl Entity {
    pub fn new(key: EntityKey, position: Pos) -> Entity {
        let entity_data = key.data();
        Entity {
            key,
            uuid: Uuid::new_v4(),
            position,
            teleport: Mutex::new(None),
            removed: AtomicBool::new(false),
            inventory: RwLock::new(Inventory::new(entity_data.inventory_size)),
        }
    }
}
impl Entity {
    pub fn create_add_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::AddEntity {
            uuid: self.uuid,
            key: self.key,
            position: self.position,
        }
    }
    pub fn create_move_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::MoveEntity {
            uuid: self.uuid,
            position: self.position,
        }
    }
    pub fn create_remove_message(&self) -> NetworkMessageS2C {
        NetworkMessageS2C::RemoveEntity { uuid: self.uuid }
    }
}
