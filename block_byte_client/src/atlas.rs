use core::f32;
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{BinaryHeap, HashMap, HashSet},
    fmt::format,
    hash::{Hash, Hasher},
    net::{SocketAddr, UdpSocket},
    ops::ControlFlow,
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU32, AtomicU64},
    },
    time::{Duration, Instant, SystemTime},
    u32,
};

use ahash::{AHashMap, AHashSet};
use base64::{Engine, prelude::BASE64_STANDARD};
use block_byte_common::{
    ACCELERATION_COEFFICIENT, CharacterController, ClientItem, Color, EntityAction, EntityPose,
    EntityStats, HitTimer, ItemMoveMode, LookDirection, MoveMode, NORMAL_SPEED, SERVER_DT,
    TexCoords,
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, FaceMap, Pos, Ray, Vec3},
    model::{DrawAnimation, LoopMode, ModelGeometry, ModelTexture},
    net::{ItemInteractTarget, NetworkMessageC2S, NetworkMessageS2C, make_connection_config},
    number_approach_smooth,
    registry::{
        self, BlockColor, BlockEntry, BlockInteractAction, BlockPalette, BlockRenderData,
        EntityData, EntityInteractAction, EntityKey, ItemAction, ItemKey, ItemModel, Key, KeyGroup,
        ModelData, ModelInstance, ModelKey, Registry, ResearchKey, TextureData, TextureKey,
        ToolData, TranslationLanguageData, air_block, load_registries,
    },
    rotation::BlockRotation,
    ui::{PropertyMap, SlotId},
    world::{self, ClientBlockComponentUpdate, ClientChunkBlockComponents},
};
use bytemuck::Pod;
use cgmath::{Matrix4, Rad, SquareMatrix, Transform, Vector3, Vector4};
use image::{DynamicImage, GenericImage, RgbaImage};
use parking_lot::{Mutex, RwLock};
use rand::{Rng, rngs::StdRng};
use rand_seeder::Seeder;
use rayon::{
    ThreadPoolBuilder,
    iter::{IntoParallelIterator, ParallelIterator},
};
use renet::{ConnectionConfig, DefaultChannel, RenetClient};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};
use smallvec::SmallVec;
use uuid::Uuid;
use wgpu::{
    Buffer, CommandEncoder, Device, Queue,
    util::{DeviceExt, StagingBelt},
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalPosition,
    event::{DeviceEvent, ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Fullscreen, Window, WindowAttributes, WindowId},
};

use crate::{
    game::{ClientConnection, ClientConnectionState, ClientGame, profiler},
    render::{
        BaseMesh, CameraUniform, ChunkMesh, ChunkVertex, DamageMesh, DamageVertex, GPUMesh,
        GUIMesh, GUIVertex, Mesh, MeshVertex, MeshVertexConsumer, RenderState, SurfaceError,
        Vertex, draw_model, get_block_matrix, get_block_rotation_face_vertices,
    },
    ui::{ScreenData, TextRenderer, UIPos, UIRect, render_screen, text_renderer},
};

pub struct TextureAtlas {
    textures: Vec<Option<TexCoords>>,
    pub models: Vec<Vec<TexCoords>>,
    pub text_renderer: TextRenderer,
    pub texture_mips: Vec<RgbaImage>,
    pub texture_material: RgbaImage,
}

impl TextureAtlas {
    pub fn pack() -> Self {
        #[derive(Hash, PartialEq, Eq, Clone, Copy)]
        enum TextureAtlasKey {
            Texture(usize),
            Model(usize, usize),
            Glyph(usize),
        }
        let texture_dimensions = 2048;
        let mut packer =
            texture_packer::TexturePacker::new_skyline(texture_packer::TexturePackerConfig {
                max_width: texture_dimensions,
                max_height: texture_dimensions,
                allow_rotation: false,
                texture_outlines: false,
                border_padding: 0,
                texture_padding: 0,
                trim: false,
                texture_extrusion: 0,
                force_max_dimensions: true,
            });
        fn get_nearest_texture_multiple(size: u32) -> u32 {
            let mut current = 16;
            loop {
                if size <= current {
                    return current;
                }
                current <<= 1;
                if current == 0 {
                    panic!()
                }
            }
        }
        struct TextureAtlasEntry {
            width_multiple: f32,
            height_multiple: f32,
        }
        let mut textures = HashMap::new();
        let mut add_texture = |key: TextureAtlasKey, image: &DynamicImage| {
            let new_width = get_nearest_texture_multiple(image.width());
            let new_height = get_nearest_texture_multiple(image.height());
            let mut new_image = DynamicImage::new_rgba8(new_width, new_height);
            new_image.copy_from(image, 0, 0).unwrap();
            textures.insert(
                key,
                TextureAtlasEntry {
                    width_multiple: image.width() as f32 / new_width as f32,
                    height_multiple: image.height() as f32 / new_height as f32,
                },
            );
            packer.pack_own(key, new_image);
        };
        for (i, texture) in TextureKey::entries().enumerate() {
            if texture.text_id().ends_with("!") {
                continue;
            }
            let texture = texture.data();
            add_texture(TextureAtlasKey::Texture(i), &*texture.texture);
        }
        for (i, model) in ModelKey::entries().enumerate() {
            let model = model.data();
            for (j, (texture, _, _)) in model.model.textures.iter().enumerate() {
                match texture {
                    ModelTexture::Embed(texture, _) => {
                        let image = image::load_from_memory_with_format(
                            BASE64_STANDARD
                                .decode(&texture["data:image/png;base64,".len()..])
                                .unwrap()
                                .as_slice(),
                            image::ImageFormat::Png,
                        )
                        .unwrap();

                        add_texture(TextureAtlasKey::Model(i, j), &image);
                    }
                    _ => {}
                }
            }
        }
        let font = rusttype::Font::try_from_vec(std::fs::read("assets/font.ttf").unwrap()).unwrap();
        {
            let glyphs: Vec<_> = (0..font.glyph_count())
                .map(|i| {
                    font.glyph(rusttype::GlyphId(i as u16))
                        .scaled(rusttype::Scale::uniform(60.))
                        .positioned(rusttype::Point { x: 0., y: 0. })
                })
                .collect();
            for (i, g) in glyphs.iter().enumerate() {
                if let Some(bb) = g.pixel_bounding_box() {
                    let mut font_texture =
                        DynamicImage::new_rgba8(bb.width() as u32, bb.height() as u32);
                    let font_buffer = match &mut font_texture {
                        DynamicImage::ImageRgba8(buffer) => buffer,
                        _ => panic!(),
                    };
                    g.draw(|x, y, v| {
                        font_buffer.put_pixel(x, y, image::Rgba([255, 255, 255, (255. * v) as u8]));
                    });
                    add_texture(TextureAtlasKey::Glyph(i), &font_texture);
                }
            }
        }
        let get_texture = |key: TextureAtlasKey| {
            let entry = textures.get(&key)?;
            let frame = packer.get_frame(&key)?;
            Some(TexCoords {
                u1: frame.frame.x as f32 / packer.width() as f32,
                v1: frame.frame.y as f32 / packer.height() as f32,
                u2: (frame.frame.x as f32 + frame.frame.w as f32 * entry.width_multiple)
                    / packer.width() as f32,
                v2: (frame.frame.y as f32 + frame.frame.h as f32 * entry.height_multiple)
                    / packer.height() as f32,
            })
        };
        use texture_packer::exporter::ImageExporter;
        use texture_packer::texture::Texture;
        let exporter: DynamicImage = ImageExporter::export(&packer, None).unwrap();
        if false {
            exporter.save(Path::new("textureatlasdump.png")).unwrap();
        }
        let mut texture_atlas_mips = vec![exporter.to_rgba8()];
        for _ in 0..4 {
            let mut last_level = texture_atlas_mips.last().unwrap();
            let mut new_image =
                DynamicImage::new_rgba8(last_level.width() / 2, last_level.height() / 2);
            let mut image_buffer = new_image.as_mut_rgba8().unwrap();
            for x in 0..image_buffer.width() {
                for y in 0..image_buffer.height() {
                    let mut color_sums = [0.; 3];
                    let mut alpha_sum = 0.;
                    for ux in 0..2 {
                        for uy in 0..2 {
                            let pixel = last_level.get_pixel(x * 2 + ux, y * 2 + uy);
                            let alpha = pixel.0[3] as f32 / 255.;
                            for i in 0..3 {
                                color_sums[i] += pixel.0[i] as f32 / 255. * alpha;
                            }
                            alpha_sum += alpha;
                        }
                    }
                    let mut color_sums = color_sums.map(|c| (c / alpha_sum * 255.) as u8);
                    image_buffer.put_pixel(
                        x,
                        y,
                        image::Rgba::<u8>([
                            color_sums[0],
                            color_sums[1],
                            color_sums[2],
                            (alpha_sum * 255.) as u8,
                        ]),
                    );
                }
            }
            texture_atlas_mips.push(new_image.to_rgba8());
        }
        let mut material_texture = RgbaImage::new(texture_dimensions, texture_dimensions);
        for texture in TextureKey::entries() {
            let texture_data = texture.data();
            if let Some(tex_coords) = get_texture(TextureAtlasKey::Texture(texture.numeric_id())) {
                let [color_mask, emissive] = [
                    texture_data.color_mask.as_ref(),
                    texture_data.emissive.as_ref(),
                ]
                .map(|t| t.map(|t| t.grayscale().into_luma8()));
                let start_x = (tex_coords.u1 * texture_dimensions as f32) as u32;
                let start_y = (tex_coords.v1 * texture_dimensions as f32) as u32;
                for x in 0..texture_data.texture.width() {
                    for y in 0..texture_data.texture.height() {
                        let [color_mask, emissive] = [color_mask.as_ref(), emissive.as_ref()]
                            .map(|t| t.map(|t| t.get_pixel(x, y).0[0]).unwrap_or(0));
                        material_texture.get_pixel_mut(start_x + x, start_y + y).0 =
                            [color_mask, emissive, 0, 0];
                    }
                }
            }
        }

        TextureAtlas {
            textures: TextureKey::entries()
                .map(|texture| get_texture(TextureAtlasKey::Texture(texture.numeric_id())))
                .collect(),
            models: ModelKey::entries()
                .enumerate()
                .map(|(i, model)| {
                    let model = model.data();
                    model
                        .model
                        .textures
                        .iter()
                        .enumerate()
                        .filter_map(|(j, _)| get_texture(TextureAtlasKey::Model(i, j)))
                        .collect()
                })
                .collect(),
            text_renderer: TextRenderer {
                glyphs: (0..font.glyph_count())
                    .map(|i| {
                        get_texture(TextureAtlasKey::Glyph(i)).unwrap_or(TexCoords {
                            u1: 0.,
                            v1: 0.,
                            u2: 0.,
                            v2: 0.,
                        })
                    })
                    .collect(),
                font,
            },
            texture_material: material_texture,
            texture_mips: texture_atlas_mips,
        }
    }
}
impl std::ops::Index<TextureKey> for TextureAtlas {
    type Output = TexCoords;
    fn index(&self, texture: TextureKey) -> &Self::Output {
        self.textures[texture.numeric_id()]
            .as_ref()
            .expect("this texture is not included in atlas")
    }
}

pub static TEXTURE_ATLAS: OnceLock<TextureAtlas> = OnceLock::new();
pub trait TexCoordsExt {
    fn tex_coords(self) -> TexCoords;
}
impl TexCoordsExt for TextureKey {
    fn tex_coords(self) -> TexCoords {
        TEXTURE_ATLAS.get().unwrap()[self]
    }
}
pub trait TexCoordsIndexExt {
    fn tex_coords(self, index: usize) -> TexCoords;
    fn variant_count(self) -> usize;
}
impl TexCoordsIndexExt for KeyGroup<TextureData> {
    fn tex_coords(self, index: usize) -> TexCoords {
        self.list()[index % self.list().len()].tex_coords()
    }
    fn variant_count(self) -> usize {
        self.list().len()
    }
}
pub fn init_texture_atlas() {
    let texture_atlas = TextureAtlas::pack();
    TEXTURE_ATLAS.set(texture_atlas);
}
