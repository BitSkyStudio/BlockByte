use std::{
    cell::{RefCell, UnsafeCell},
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    num::{NonZero, NonZeroU32},
    sync::Arc,
};

use block_byte_common::{
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, HorizontalFace},
    registry::{BiomeKey, BlockEntry, BlockPalette, KeyGroup, PrefabKey, air_block},
    rotation::BlockRotation,
};
use moka::sync::Cache;
use noise::{NoiseFn, Perlin};
use rand::{Rng, RngCore, SeedableRng};
use rand_seeder::Seeder;
use rand_xoshiro::Xoshiro256PlusPlus;
use serde::Deserialize;
use smallvec::SmallVec;
use splines::{Interpolation, Spline};

use crate::world::{BlockPlants, Chunk, ChunkBlockComponents, WorldAccessCell};

pub struct RegionGeneration {
    pub x: i16,
    pub z: i16,
    pub structures: Vec<RegionStructure>,
    pub structure_grid: [[Option<NonZeroU32>; Self::GRID_SIZE]; Self::GRID_SIZE], //could probably be u16
    pub structure_grid_prefabs: Vec<StructureGridPrefab>,
    pub roads: [[u8; Self::REGION_CHUNK_SIZE]; Self::REGION_CHUNK_SIZE],
}
impl RegionGeneration {
    pub fn generate_structure_list(
        x: i16,
        z: i16,
        world_generator: &WorldGenerator,
    ) -> Vec<RegionStructure> {
        let mut structures: Vec<RegionStructure> = Vec::new();
        let mut rng = Xoshiro256PlusPlus::from_seed(
            Seeder::from((world_generator.seed as u32, x, z)).make_seed(),
        );
        for _ in 0..world_generator.config.region_structure_spawn_attempts {
            let index = rng.next_u32() as usize % world_generator.config.structures.len();
            let x = x as i32 * Self::REGION_CHUNK_SIZE as i32
                + (rng.next_u32() % Self::REGION_CHUNK_SIZE as u32) as i32;
            let z = z as i32 * Self::REGION_CHUNK_SIZE as i32
                + (rng.next_u32() % Self::REGION_CHUNK_SIZE as u32) as i32;
            let seed = rng.next_u64();
            let structure_data = &world_generator.config.structures[index];
            if structures.iter().any(|structure| {
                let distance = (structure.x - x).pow(2) + (structure.z - z).pow(2);
                distance < (structure.exclusion_zone + structure_data.exclusion_zone).pow(2) as i32
            }) {
                continue;
            }
            structures.push(RegionStructure {
                x,
                z,
                exclusion_zone: structure_data.exclusion_zone,
                index,
                seed,
            });
        }
        structures
    }
    pub fn add_structure_prefab(
        &mut self,
        position: BlockPos,
        rotation: HorizontalFace,
        prefab: PrefabKey,
        seed: u32,
    ) {
        let grid_corner_x =
            self.x * Self::REGION_CHUNK_SIZE as i16 - Self::BORDER_CHUNK_SIZE as i16;
        let grid_corner_z =
            self.z * Self::REGION_CHUNK_SIZE as i16 - Self::BORDER_CHUNK_SIZE as i16;

        let aabb = prefab.data().bounding_box();
        let aabb = BlockRotation::looking_to_horizontal(rotation).rotate_block_aabb(aabb);
        let aabb = aabb.offset(position);
        for chunk in aabb.to_chunk().vertically_flatten() {
            let offset_x = chunk.x - grid_corner_x;
            let offset_z = chunk.z - grid_corner_z;
            if offset_x >= 0
                && offset_z >= 0
                && offset_x < Self::GRID_SIZE as i16
                && offset_z < Self::GRID_SIZE as i16
            {
                self.structure_grid_prefabs.push(StructureGridPrefab {
                    position,
                    rotation,
                    prefab,
                    seed: seed, //todo: do something
                    next: self.structure_grid[offset_x as usize][offset_z as usize],
                });
                self.structure_grid[offset_x as usize][offset_z as usize] =
                    NonZero::new(self.structure_grid_prefabs.len() as u32);
            } else {
                println!("structure generation outside bounds");
            }
        }
    }
}
struct StructureGridPrefab {
    pub position: BlockPos,
    pub rotation: HorizontalFace,
    pub prefab: PrefabKey,
    pub seed: u32,
    pub next: Option<NonZeroU32>,
}
impl RegionGeneration {
    pub const REGION_CHUNK_SIZE: usize = 64;
    pub const BORDER_CHUNK_SIZE: usize = 8;
    const GRID_SIZE: usize = Self::REGION_CHUNK_SIZE + Self::BORDER_CHUNK_SIZE * 2;
}
#[derive(Clone, Copy)]
pub struct RegionStructure {
    pub x: i32,
    pub z: i32,
    pub exclusion_zone: u16,
    pub index: usize,
    pub seed: u64,
}
pub struct ChunkColumnGeneration {
    pub x: i16,
    pub z: i16,
    pub biomes: [[BiomeKey; CHUNK_SIZE as usize]; CHUNK_SIZE as usize],
    pub height: [[u16; CHUNK_SIZE as usize]; CHUNK_SIZE as usize],
    pub unique_biomes: Vec<BiomeKey>,
    pub decorations: Vec<ChunkColumnDecoration>,
}
impl ChunkColumnGeneration {
    pub fn is_blocked(&self, x: i32, z: i32, exclusion_radius: u8) -> bool {
        for decoration in &self.decorations {
            let decoration_x = (decoration.x as i32 + self.x as i32 * CHUNK_SIZE as i32);
            let decoration_z = (decoration.z as i32 + self.z as i32 * CHUNK_SIZE as i32);
            let distance = (decoration_x - x).pow(2) + (decoration_z - z).pow(2);
            let decoration_data = decoration.key.data();
            if distance <= (decoration.exclusion_zone as i32 + exclusion_radius as i32).pow(2) {
                return true;
            }
        }
        false
    }
    const NEIGHBOR_CHUNK_BLOCKERS: [(i8, i8); 4] = [(0, -1), (-1, -1), (-1, 0), (-1, 1)];
    pub fn get_legal_decorations<'a>(
        &'a self,
        world_generator: &WorldGenerator,
    ) -> impl Iterator<Item = &'a ChunkColumnDecoration> {
        let blocking_neighbors = Self::NEIGHBOR_CHUNK_BLOCKERS.map(|(x, z)| {
            world_generator.get_column_generation(self.x + x as i16, self.z + z as i16)
        });
        self.decorations.iter().filter(move |decoration| {
            !blocking_neighbors.iter().any(|neighbor| {
                neighbor.is_blocked(
                    decoration.x as i32 + self.x as i32 * CHUNK_SIZE as i32,
                    decoration.z as i32 + self.z as i32 * CHUNK_SIZE as i32,
                    decoration.exclusion_zone,
                )
            })
        })
    }
}
struct ChunkColumnDecoration {
    key: PrefabKey,
    x: u8,
    z: u8,
    exclusion_zone: u8,
    rotation: HorizontalFace,
    seed: u64,
}
#[derive(Deserialize)]
pub struct WorldGeneratorConfigStructure {
    pub exclusion_zone: u16,
    pub root_room: String,
    pub rooms: HashMap<String, StructureConfigRoom>,
}
#[derive(Deserialize)]
pub struct StructureConfigConnection {
    pub position: BlockPos,
    pub facing: HorizontalFace,
    pub rooms: Vec<StructureConfigRoomSelection>,
}
#[derive(Deserialize)]
pub struct StructureConfigRoomSelection {
    pub room: String,
    pub weight: f32,
    #[serde(default)]
    pub weight_depth_bias: f32,
}
#[derive(Deserialize)]
pub struct StructureConfigRoom {
    pub prefab: PrefabKey,
    pub connections: Vec<StructureConfigConnection>,
    #[serde(default)]
    pub road: Option<BlockPos>,
}
#[derive(Deserialize)]
pub struct WorldGeneratorConfig {
    pub structures: Vec<WorldGeneratorConfigStructure>,
    pub region_structure_spawn_attempts: u32,
}
pub struct WorldGenerator {
    pub seed: u64,
    pub config: WorldGeneratorConfig,
    pub chunk_column_cache: Cache<(i16, i16), Arc<ChunkColumnGeneration>>,
    pub region_cache: Cache<(i16, i16), Arc<RegionGeneration>>,
}
impl WorldGenerator {
    pub fn new(config: WorldGeneratorConfig, seed: u64) -> WorldGenerator {
        WorldGenerator {
            seed,
            config,
            chunk_column_cache: Cache::new(1024),
            region_cache: Cache::new(64),
        }
    }
    pub fn get_region_generation(&self, region_x: i16, region_z: i16) -> Arc<RegionGeneration> {
        self.region_cache.get_with((region_x, region_z), || {
            let structure_blockers =
                ChunkColumnGeneration::NEIGHBOR_CHUNK_BLOCKERS.map(|(offset_x, offset_z)| {
                    RegionGeneration::generate_structure_list(
                        region_x + offset_x as i16,
                        region_z + offset_z as i16,
                        self,
                    )
                });
            let structures: Vec<_> =
                RegionGeneration::generate_structure_list(region_x, region_z, self)
                    .into_iter()
                    .filter(|structure| {
                        !structure_blockers.iter().any(|blocker| {
                            blocker.iter().any(|blocker| {
                                let distance = (structure.x - blocker.x).pow(2)
                                    + (structure.z - blocker.z).pow(2);
                                distance
                                    < (structure.exclusion_zone + blocker.exclusion_zone).pow(2)
                                        as i32
                            })
                        })
                    })
                    .collect();
            let mut region = RegionGeneration {
                x: region_x,
                z: region_z,
                structures: structures.clone(), //todo: probably not even required
                structure_grid: [[None; RegionGeneration::GRID_SIZE]; RegionGeneration::GRID_SIZE],
                structure_grid_prefabs: Vec::new(),
                roads: [[0; RegionGeneration::REGION_CHUNK_SIZE];
                    RegionGeneration::REGION_CHUNK_SIZE],
            };
            for structure in structures {
                let structure_data = &self.config.structures[structure.index];
                let mut rng = Xoshiro256PlusPlus::seed_from_u64(structure.seed);
                let start_rotation = HorizontalFace::all()[rng.random_range(0..4)];
                let mut queue = VecDeque::new();
                let mut bounding_boxes = Vec::new();
                struct QueueEntry<'a> {
                    depth: u32,
                    position: BlockPos,
                    rotation: HorizontalFace,
                    room: &'a StructureConfigRoom,
                }
                queue.push_front(QueueEntry {
                    depth: 0,
                    position: BlockPos {
                        x: structure.x,
                        y: 80,
                        z: structure.z,
                    },
                    rotation: start_rotation,
                    room: structure_data.rooms.get(&structure_data.root_room).unwrap(),
                });
                while let Some(entry) = queue.pop_back() {
                    region.add_structure_prefab(
                        entry.position,
                        entry.rotation,
                        entry.room.prefab,
                        rng.random::<u32>(),
                    );
                    let rotation = BlockRotation::looking_to_horizontal(entry.rotation);
                    bounding_boxes.push(
                        rotation
                            .rotate_block_aabb(entry.room.prefab.data().bounding_box())
                            .offset(entry.position),
                    );
                    if entry.depth > 32 {
                        continue;
                    }
                    for connection in &entry.room.connections {
                        let connection_position =
                            entry.position + rotation.rotate_block_pos(connection.position);
                        let connection_facing = rotation
                            .rotate_face(connection.facing.face())
                            .horizontal()
                            .unwrap();
                        let connection_rotation =
                            BlockRotation::looking_to_horizontal(connection_facing);
                        let mut list: Vec<_> = connection
                            .rooms
                            .iter()
                            .map(|room| {
                                (
                                    room,
                                    rng.random::<f32>().powf(
                                        1. / (room.weight
                                            + room.weight_depth_bias * entry.depth as f32),
                                    ),
                                )
                            })
                            .collect();
                        list.sort_by(|(_, a), (_, b)| b.total_cmp(a));
                        for (room, _) in list {
                            let room = structure_data.rooms.get(&room.room).unwrap();
                            let room_bb = connection_rotation
                                .rotate_block_aabb(room.prefab.data().bounding_box())
                                .offset(connection_position);
                            if bounding_boxes
                                .iter()
                                .any(|existing_bb| existing_bb.intersects(room_bb))
                            {
                                continue;
                            }
                            queue.push_front(QueueEntry {
                                depth: entry.depth + 1,
                                position: connection_position,
                                rotation: connection_facing,
                                room,
                            });
                            break;
                        }
                    }
                }
            }

            Arc::new(region)
        })
    }
    pub fn get_column_generation(&self, chunk_x: i16, chunk_z: i16) -> Arc<ChunkColumnGeneration> {
        self.chunk_column_cache.get_with((chunk_x, chunk_z), || {
            let height_noise = Perlin::new(self.seed as u32);
            let density_noise = Perlin::new(self.seed as u32 ^ 583279234);
            let mut height_map = [[0; CHUNK_SIZE as usize]; CHUNK_SIZE as usize];
            let forest = BiomeKey::id("forest").unwrap();
            let mut biome_map = [[forest; CHUNK_SIZE as usize]; CHUNK_SIZE as usize];
            let mountain_spline = Spline::from_vec(vec![
                splines::Key::new(-1., 60., Interpolation::Linear),
                splines::Key::new(0., 80., Interpolation::Linear),
                splines::Key::new(0.5, 100., Interpolation::Cosine),
                splines::Key::new(1., 200., Interpolation::Linear),
            ]);
            let small_spline = Spline::from_vec(vec![
                splines::Key::new(-1., -3., Interpolation::Linear),
                splines::Key::new(1., 3., Interpolation::Linear),
            ]);
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let block_x = x as i32 + chunk_x as i32 * CHUNK_SIZE as i32;
                    let block_z = z as i32 + chunk_z as i32 * CHUNK_SIZE as i32;

                    let mountain_height = height_noise
                        .get([block_x as f64 / 500., block_z as f64 / 500.])
                        .clamp(-0.99, 0.99);

                    let small_noise = height_noise
                        .get([block_x as f64 / 30., block_z as f64 / 30.])
                        .clamp(-0.99, 0.99);
                    let height = (mountain_spline.sample(mountain_height).unwrap()
                        + small_spline.sample(small_noise).unwrap());
                    height_map[x as usize][z as usize] = height as u16;
                    //biome_map[x as usize][z as usize] = biome;
                }
            }
            let hole_spline = Spline::from_vec(vec![
                splines::Key::new(100., 0., Interpolation::Linear),
                splines::Key::new(120., 0.8, Interpolation::Linear),
                splines::Key::new(200., 0., Interpolation::Linear),
            ]);
            //todo: this is probably broken between runs
            let unique_biomes = vec![forest];
            let mut rng = Xoshiro256PlusPlus::from_seed(
                Seeder::from((self.seed as u32, chunk_x, chunk_z)).make_seed(),
            );
            let mut chunk_column_generation = ChunkColumnGeneration {
                x: chunk_x,
                z: chunk_z,
                biomes: biome_map,
                height: height_map,
                unique_biomes,
                decorations: Vec::new(),
            };
            for biome in &chunk_column_generation.unique_biomes {
                for decorator in &biome.data().decorators {
                    for i in 0..decorator.count {
                        if !rng.random_bool(decorator.chance as f64) {
                            continue;
                        }
                        for _ in 0..10 {
                            let rotation = rng.random_range(0..4) as u8;
                            let seed = rng.next_u64();
                            let offset_x = rng.random_range(0..CHUNK_SIZE) as u8;
                            let offset_z = rng.random_range(0..CHUNK_SIZE) as u8;
                            if biome_map[offset_x as usize][offset_z as usize] != *biome {
                                continue;
                            }
                            if !chunk_column_generation.is_blocked(
                                offset_x as i32 + chunk_x as i32 * CHUNK_SIZE as i32,
                                offset_z as i32 + chunk_z as i32 * CHUNK_SIZE as i32,
                                decorator.exclusion_zone,
                            ) {
                                chunk_column_generation
                                    .decorations
                                    .push(ChunkColumnDecoration {
                                        key: decorator.prefab,
                                        x: offset_x,
                                        z: offset_z,
                                        exclusion_zone: decorator.exclusion_zone,
                                        rotation: HorizontalFace::all()[rotation as usize],
                                        seed,
                                    });
                                break;
                            }
                        }
                    }
                }
            }
            Arc::new(chunk_column_generation)
        })
    }
}
pub fn generate_chunk(position: ChunkPos, generator: &WorldGenerator) -> Chunk {
    /*use noise::MultiFractal;
    let height_noise: BasicMulti<Perlin> = BasicMulti::new(seed)
        .set_octaves(4)
        .set_frequency(1.0)
        .set_lacunarity(2.0)
        .set_persistence(0.5);*/

    let column_data = generator.get_column_generation(position.x, position.z);

    let mut blocks = BlockPalette::filled(
        BlockEntry::simple(air_block()),
        CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE,
    );
    let mut components = ChunkBlockComponents::default();

    let mut rng =
        Xoshiro256PlusPlus::from_seed(Seeder::from((generator.seed as u32, position)).make_seed());
    for z in 0..CHUNK_SIZE as u8 {
        for y in 0..CHUNK_SIZE as u8 {
            for x in 0..CHUNK_SIZE as u8 {
                let offset = ChunkOffset::new(x, y, z);
                let y_pos = y as i32 + position.y as i32 * CHUNK_SIZE as i32;
                let biome = column_data.biomes[x as usize][y as usize].data();
                let height = column_data.height[x as usize][z as usize] as i32;
                /*let holes = hole_spline.clamped_sample(height as f32).unwrap();
                let density = density_noise.get([
                    (position.x as f64 * CHUNK_SIZE as f64 + x as f64) / 10.,
                    (position.y as f64 * CHUNK_SIZE as f64 + y as f64) / 10.,
                    (position.z as f64 * CHUNK_SIZE as f64 + z as f64) / 10.,
                ]) as f32
                    * holes
                    + (height - y_pos) as f32 / 20.;
                if density > 0. {
                    blocks.set(
                        offset.index(),
                        &BlockEntry {
                            block: biome.bottom_block,
                            color: Color::WHITE,
                            rotation: BlockRotation::default(),
                        },
                    );
                }*/
                if y_pos == height {
                    blocks.set(offset.index(), &BlockEntry::simple(biome.top_block));
                    let spawned_plants: SmallVec<_> = biome
                        .plants
                        .iter()
                        .filter_map(|spawner| {
                            if rng.random_bool(spawner.chance as f64) {
                                let plant_data = spawner.plant.data();
                                Some((
                                    spawner.plant,
                                    rng.random::<f32>() * plant_data.growth_length,
                                ))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !spawned_plants.is_empty() {
                        components.plant.set(
                            offset,
                            WorldAccessCell::new(BlockPlants {
                                plants: spawned_plants,
                            }),
                        );
                    }
                } else if y_pos < height - 3 {
                    blocks.set(offset.index(), &BlockEntry::simple(biome.bottom_block));
                } else if y_pos < height {
                    blocks.set(offset.index(), &BlockEntry::simple(biome.middle_block));
                }
            }
        }
    }
    let chunk_aabb = (AABB {
        min: BlockPos::all(0),
        max: BlockPos::all(CHUNK_SIZE as i32),
    })
    .offset(position.to_block_pos());
    for neighbor_chunk in (AABB {
        min: ChunkPos { x: -1, y: 0, z: -1 },
        max: ChunkPos { x: 1, y: 0, z: 1 },
    })
    .offset(position)
    {
        let block_offset = neighbor_chunk.to_block_pos();
        let column = generator.get_column_generation(neighbor_chunk.x, neighbor_chunk.z);
        for placed_decoration in column.get_legal_decorations(generator) {
            let height = column.height[placed_decoration.x as usize][placed_decoration.z as usize]
                as i32
                + 1;
            let block_position = BlockPos {
                x: block_offset.x + placed_decoration.x as i32,
                y: height,
                z: block_offset.z + placed_decoration.z as i32,
            };
            let prefab = placed_decoration.key.data();
            if BlockRotation::looking_to_horizontal(placed_decoration.rotation)
                .rotate_block_aabb(prefab.bounding_box())
                .offset(block_position)
                .intersects(chunk_aabb)
            {
                prefab.build(
                    block_position,
                    placed_decoration.rotation,
                    placed_decoration.seed,
                    |place_position, block, replace_list| {
                        let (place_chunk, place_chunk_offset) =
                            place_position.to_chunk_pos_offset();
                        if place_chunk == position {
                            if replace_list
                                .contains(blocks.get(place_chunk_offset.index()).unwrap().block)
                            {
                                blocks.set(place_chunk_offset.index(), &block);
                            }
                        }
                    },
                );
            }
        }
    }
    let region = generator.get_region_generation(
        position.x / RegionGeneration::GRID_SIZE as i16,
        position.z / RegionGeneration::GRID_SIZE as i16,
    );
    {
        let mut next_placement = region.structure_grid
            [(position.x.rem_euclid(RegionGeneration::GRID_SIZE as i16)) as usize]
            [(position.z.rem_euclid(RegionGeneration::GRID_SIZE as i16)) as usize];
        while let Some(placement) = next_placement {
            let placement = &region.structure_grid_prefabs[(placement.get() - 1) as usize];
            placement.prefab.data().build(
                placement.position,
                placement.rotation,
                placement.seed as u64,
                |place_position, block, replace_list| {
                    let (place_chunk, place_chunk_offset) = place_position.to_chunk_pos_offset();
                    if place_chunk == position {
                        if replace_list
                            .contains(blocks.get(place_chunk_offset.index()).unwrap().block)
                        {
                            blocks.set(place_chunk_offset.index(), &block);
                        }
                    }
                },
            );
            next_placement = placement.next;
        }
    }
    //blocks.set(ChunkOffset::new(16, 16, 16).index(), &grass);
    Chunk {
        position,
        blocks: RefCell::new(blocks),
        viewers: HashSet::new(),
        events: RefCell::new(VecDeque::new()),
        components,
        entities: BTreeMap::new(),
    }
}
