use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{OnceLock, atomic::AtomicBool},
};

use block_byte_common::{
    coord::{AABB, CHUNK_SIZE, ChunkOffset, ChunkPos, Pos},
    net::NetworkMessageS2C,
    registry::{BiomeKey, BlockData, BlockKey, BlockPalette, EntityKey},
    world::{
        BlockDamage, BlockPlants, ChunkBlockComponents, ClientBlockComponentUpdate,
        ClientBlockDamage, ComponentClientFromServer,
    },
};
use noise::{BasicMulti, NoiseFn, Perlin};
use palettevec::{PaletteVec, index_buffer::AlignedIndexBuffer, palette::HybridPalette};
use parking_lot::{Mutex, RwLock};
use rand::{Rng, rngs::StdRng};
use rand_seeder::Seeder;
use serde::{Deserialize, Serialize};
use slotmap::new_key_type;
use splines::{Interpolation, Spline};
use uuid::Uuid;

use crate::{
    Server, UserIndex,
    inventory::Inventory,
    registry::{Key, RegistryConfigLoadable},
};
#[derive(Serialize, Deserialize)]
pub struct ChunkSaveData {
    pub blocks: BlockPalette,
    pub block_events: Vec<(ChunkOffset, BlockEvent)>,
    pub components: ChunkBlockComponents,
    pub entities: Vec<Entity>,
}
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
        let seed = 1;

        /*use noise::MultiFractal;
        let height_noise: BasicMulti<Perlin> = BasicMulti::new(seed)
            .set_octaves(4)
            .set_frequency(1.0)
            .set_lacunarity(2.0)
            .set_persistence(0.5);*/
        let height_noise = Perlin::new(seed);

        let mut blocks = BlockPalette::filled(
            air_block(),
            CHUNK_SIZE as usize * CHUNK_SIZE as usize * CHUNK_SIZE as usize,
        );
        let mut components = ChunkBlockComponents::default();
        let mut height_map = [[0; CHUNK_SIZE as usize]; CHUNK_SIZE as usize];
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block_x = x as i32 + position.x as i32 * CHUNK_SIZE as i32;
                let block_z = z as i32 + position.z as i32 * CHUNK_SIZE as i32;
                let mountain_spline = Spline::from_vec(vec![
                    splines::Key::new(-1., 60., Interpolation::Linear),
                    splines::Key::new(0., 80., Interpolation::Linear),
                    splines::Key::new(0.5, 100., Interpolation::Cosine),
                    splines::Key::new(1., 200., Interpolation::Linear),
                ]);
                let mountain_height = height_noise
                    .get([block_x as f64 / 500., block_z as f64 / 500.])
                    .clamp(-0.99, 0.99);
                let small_spline = Spline::from_vec(vec![
                    splines::Key::new(-1., -3., Interpolation::Linear),
                    splines::Key::new(1., 3., Interpolation::Linear),
                ]);
                let small_noise = height_noise
                    .get([block_x as f64 / 30., block_z as f64 / 30.])
                    .clamp(-0.99, 0.99);
                let height = (mountain_spline.sample(mountain_height).unwrap()
                    + small_spline.sample(small_noise).unwrap());
                height_map[x as usize][z as usize] = height as i32;
            }
        }
        let biome = BiomeKey::id("forest").unwrap().data();
        use rand::SeedableRng;
        let mut rng = StdRng::from_seed(Seeder::from((seed, position)).make_seed());
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let offset = ChunkOffset::new(x, y, z);
                    let y_pos = y as i32 + position.y as i32 * CHUNK_SIZE as i32;
                    let height = height_map[x as usize][z as usize];
                    if y_pos == height {
                        blocks.set(offset.index(), &biome.top_block);
                        let spawned_plants: Vec<_> = biome
                            .plants
                            .iter()
                            .filter_map(|spawner| {
                                if rng.random_bool(spawner.chance as f64) {
                                    Some((spawner.plant, 0.))
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if !spawned_plants.is_empty() {
                            components.plant.write().set(
                                offset,
                                BlockPlants {
                                    plants: spawned_plants,
                                },
                            );
                        }
                    } else if y_pos < height - 3 {
                        blocks.set(offset.index(), &biome.bottom_block);
                    } else if y_pos < height {
                        blocks.set(offset.index(), &biome.middle_block);
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
#[derive(Serialize, Deserialize)]
pub enum BlockEvent {
    Damage { damage: f32 },
}

static AIR_BLOCK: OnceLock<BlockKey> = OnceLock::new();
pub fn air_block() -> BlockKey {
    *AIR_BLOCK.get_or_init(|| Key::id("air").unwrap())
}

new_key_type! {pub struct EntityIndex;}
#[derive(Serialize, Deserialize)]
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
    pub fn get_hitbox(&self) -> AABB<f32> {
        let entity_data = self.key.data();
        AABB {
            min: Pos {
                x: -entity_data.hitbox_size,
                y: 0.,
                z: -entity_data.hitbox_size,
            },
            max: Pos {
                x: entity_data.hitbox_size,
                y: entity_data.hitbox_height,
                z: entity_data.hitbox_size,
            },
        }
        .offset(self.position)
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
