//mod render;

use std::path::Path;

use block_byte_common::registry::{self, Registry, TextureData, TextureKey, load_registries};
use image::RgbaImage;

fn main() {
    load_registries(&Path::new("assets"));
    use block_byte_common::registry::RegistryProvider;
    let (atlas, image) = TextureAtlas::pack(registry::REGISTRIES.get().unwrap().get_registry());
    for a in atlas.textures {
        println!("{} {}", a.u1, a.v1);
    }
}

pub struct TextureAtlas {
    textures: Vec<TexCoords>,
}
impl TextureAtlas {
    pub fn pack(texture_registry: &Registry<TextureData>) -> (Self, RgbaImage) {
        let mut packer =
            texture_packer::TexturePacker::new_skyline(texture_packer::TexturePackerConfig {
                max_width: 2048,
                max_height: 2048,
                allow_rotation: false,
                texture_outlines: false,
                border_padding: 0,
                texture_padding: 0,
                trim: false,
                texture_extrusion: 0,
            });
        for (i, texture) in texture_registry.data_entries().enumerate() {
            packer.pack_ref(i, &texture.texture).unwrap();
        }
        use texture_packer::exporter::ImageExporter;
        use texture_packer::texture::Texture;
        let exporter = ImageExporter::export(&packer).unwrap();
        if false {
            exporter.save(Path::new("textureatlasdump.png")).unwrap();
        }
        (
            TextureAtlas {
                textures: (0..packer.get_frames().len())
                    .map(|i| {
                        let frame = packer.get_frame(&i).unwrap();
                        TexCoords {
                            u1: frame.frame.x as f32 / packer.width() as f32,
                            v1: frame.frame.y as f32 / packer.height() as f32,
                            u2: (frame.frame.x + frame.frame.w) as f32 / packer.width() as f32,
                            v2: (frame.frame.y + frame.frame.h) as f32 / packer.height() as f32,
                        }
                    })
                    .collect(),
            },
            exporter.to_rgba8(),
        )
    }
}
impl std::ops::Index<TextureKey> for TextureAtlas {
    type Output = TexCoords;
    fn index(&self, texture: TextureKey) -> &Self::Output {
        &self.textures[texture.numeric_id()]
    }
}
impl TextureAtlas {}
#[derive(Clone, Copy)]
pub struct TexCoords {
    pub u1: f32,
    pub v1: f32,
    pub u2: f32,
    pub v2: f32,
}
