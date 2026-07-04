use std::collections::{HashMap, VecDeque};

use block_byte_common::{
    coord::{BlockPos, Face},
    registry::{BlockKey, PrefabData},
};
use noise::{Fbm, MultiFractal, NoiseFn, Perlin};

const PATH: &'static str = "worldgen_vis";
pub fn visualise() {
    std::fs::remove_dir_all(PATH);
    std::fs::create_dir(PATH).unwrap();
    visualise_biome_graph();
    visualise_map();
}

fn visualise_biome_graph() {
    /*let mut image = image::DynamicImage::new(100, 100, image::ColorType::Rgb8);
    for x in 0..100 {
        for y in 0..100 {
            let biome = BiomeKey::entries()
                .min_by(|a, b| {
                    let biome_fitness = |biome: BiomeKey| -> f32 {
                        let biome = biome.data();
                        let temperature_error = biome.temperature.get_error(x as f32 / 100.);
                        let moisture_error = biome.moisture.get_error(y as f32 / 100.);
                        (temperature_error.powi(2) + moisture_error.powi(2)).sqrt()
                    };
                    biome_fitness(*a).total_cmp(&biome_fitness(*b))
                })
                .unwrap();
            let biome_color = biome.data().debug_color;
            image.put_pixel(x, y, image::Rgba(biome_color.into()));
        }
    }
    image.save(Path::new(PATH).join("vis.png")).unwrap();*/
}
fn visualise_map() {
    /*let mut image = image::DynamicImage::new(2000, 2000, image::ColorType::Rgb8);
    let land_noise = Fbm::<Perlin>::new(3)
        .set_octaves(6)
        .set_frequency(2.0)
        .set_lacunarity(2.0)
        .set_persistence(0.5);
    let temperature_noise = Fbm::<Perlin>::new(1)
        .set_octaves(6)
        .set_frequency(2.0)
        .set_lacunarity(2.0)
        .set_persistence(0.5);
    let moisture_noise = Fbm::<Perlin>::new(1)
        .set_octaves(6)
        .set_frequency(2.0)
        .set_lacunarity(2.0)
        .set_persistence(0.5);
    for x in 0..2000 {
        for y in 0..2000 {
            let land_offset =
                ((((1000u32 - x) as f32).powi(2) + ((1000u32 - y) as f32).powi(2)) as f64).sqrt()
                    / 1500.
                    - 0.5;
            let is_land = /*land_noise.get([x as f64 / 5000., y as f64 / 5000.]) */- land_offset > 0.;
            let height_color = if is_land { 255 } else { 0 } as u8;
            image.put_pixel(
                x,
                y,
                image::Rgba([height_color, height_color, height_color, 255]),
            );
        }
    }
    image.save(Path::new(PATH).join("map.png")).unwrap();*/
}

pub fn generate_tree(
    _log_block: BlockKey,
    _slab_block: BlockKey,
    _branch_block: BlockKey,
    _leave_block: BlockKey,
) -> PrefabData {
    #[derive(Clone, Copy, Debug)]
    enum TreeBlockType {
        Log,
        Branch,
        Slab,
        Leaves,
    }
    impl TreeBlockType {
        fn connects_to(self, other: Self) -> bool {
            match (self, other) {
                (TreeBlockType::Log | TreeBlockType::Slab, TreeBlockType::Log) => true,
                (TreeBlockType::Log | TreeBlockType::Slab, _) => false,
                (TreeBlockType::Branch, TreeBlockType::Branch | TreeBlockType::Log) => true,
                (TreeBlockType::Branch, _) => false,
                (TreeBlockType::Leaves, _) => true,
            }
        }
    }
    const DENSITY_SIZE: usize = 65;
    let mut density_field = [[[0.; DENSITY_SIZE]; DENSITY_SIZE]; DENSITY_SIZE];

    for i in 0..10 {
        density_field[DENSITY_SIZE / 2][i][DENSITY_SIZE / 2] = 1.2 * (1. - i as f32 / 10.);
    }

    let mut blocks = HashMap::new();

    let noise = Fbm::<Perlin>::new(3)
        .set_octaves(6)
        .set_frequency(2.0)
        .set_lacunarity(2.0)
        .set_persistence(0.5);

    let mut density_field_blurred = [[[0.; DENSITY_SIZE]; DENSITY_SIZE]; DENSITY_SIZE];
    {
        let size = 2;
        let query_density = |x: isize, y: isize, z: isize| {
            if x < 0
                || x >= DENSITY_SIZE as isize
                || y < 0
                || y >= DENSITY_SIZE as isize
                || z < 0
                || z >= DENSITY_SIZE as isize
            {
                return 0.;
            }
            density_field[x as usize][y as usize][z as usize]
        };
        for x in 0..DENSITY_SIZE {
            for y in 0..DENSITY_SIZE {
                for z in 0..DENSITY_SIZE {
                    density_field_blurred[x][y][z] =
                        noise.get([x as f64 / 10., y as f64 / 10., z as f64 / 10.]) as f32 * 0.3;
                    for x2 in -size..=size {
                        for y2 in -size..=size {
                            for z2 in -size..=size {
                                let value = query_density(
                                    x as isize + x2,
                                    y as isize + y2,
                                    z as isize + z2,
                                );
                                //exp(-(i_f32*i_f32 + j_f32*j_f32) / k) / (PI * k);
                                density_field_blurred[x][y][z] += value
                                    * (0.5_f32).powf(
                                        ((x2.pow(2) + y2.pow(2) + z2.pow(2)) as f32).sqrt() * 2.,
                                    );
                            }
                        }
                    }
                    let density = density_field_blurred[x][y][z];
                    let block = if density >= 1. {
                        Some(TreeBlockType::Log)
                    } else if density >= 0.9 {
                        Some(TreeBlockType::Slab)
                    }
                    /*  else if density >= 0.6 {
                        None
                    }*/
                    else if density >= 0.6 {
                        Some(TreeBlockType::Branch)
                    } else if density >= 0.2 {
                        Some(TreeBlockType::Leaves)
                    } else {
                        None
                    };
                    if let Some(block) = block {
                        blocks.insert(
                            BlockPos {
                                x: x as i32 - (DENSITY_SIZE / 2) as i32,
                                y: y as i32,
                                z: z as i32 - (DENSITY_SIZE / 2) as i32,
                            },
                            block,
                        );
                    }
                }
            }
        }
    }

    blocks.insert(BlockPos::all(0), TreeBlockType::Log);

    //println!("{:?}", blocks);

    let mut directions = HashMap::new();
    {
        let mut directions_queue = VecDeque::new();
        directions_queue.push_front((BlockPos::all(0), Face::Down, TreeBlockType::Log));
        while let Some((block, direction, from)) = directions_queue.pop_back() {
            let local_block = *blocks.get(&block).unwrap();
            if !local_block.connects_to(from) {
                continue;
            }
            if !directions.contains_key(&block) {
                directions.insert(block, direction);
                for neighbor in [
                    Face::Front,
                    Face::Back,
                    Face::Left,
                    Face::Right,
                    Face::Up,
                    Face::Down,
                ] {
                    let neighbor_block = block + neighbor.get_block_offset();
                    if blocks.contains_key(&neighbor_block) {
                        directions_queue.push_front((
                            neighbor_block,
                            neighbor.opposite(),
                            local_block,
                        ));
                    }
                }
            }
        }
    }
    /*let part = PrefabEntry {

        blocks: blocks
            .into_iter()
            .filter_map(|(position, block_type)| {
                let block_key = match block_type {
                    TreeBlockType::Log => log_block,
                    TreeBlockType::Slab => slab_block,
                    TreeBlockType::Branch => branch_block,
                    TreeBlockType::Leaves => leave_block,
                };
                match directions.get(&position) {
                    Some(rotation) => {
                        let rotation = BlockRotation::looking_to(*rotation);
                        let rotation = block_key.data().rotation.get_nearest_valid(rotation);
                        Some((
                            position,
                            BlockEntry {
                                block: block_key,
                                rotation,
                                color: BlockColor::default(),
                            },
                        ))
                    }
                    None => None,
                }
            })
            .collect(),
    };
    PrefabData {
        parts: vec![part],
        bb: OnceLock::new(),
    }*/
    unreachable!()
}
