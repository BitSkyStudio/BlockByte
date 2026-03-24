use std::path::Path;

use block_byte_common::registry::BiomeKey;
use image::GenericImage;
use noise::{Fbm, MultiFractal, NoiseFn, Perlin};

const PATH: &'static str = "worldgen_vis";
pub fn visualise() {
    std::fs::remove_dir_all(PATH);
    std::fs::create_dir(PATH).unwrap();
    visualise_biome_graph();
    visualise_map();
}

fn visualise_biome_graph() {
    let mut image = image::DynamicImage::new(100, 100, image::ColorType::Rgb8);
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
    image.save(Path::new(PATH).join("vis.png")).unwrap();
}
fn visualise_map() {
    let mut image = image::DynamicImage::new(2000, 2000, image::ColorType::Rgb8);
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
    image.save(Path::new(PATH).join("map.png")).unwrap();
}
