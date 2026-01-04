use std::cell::OnceCell;
#[cfg(feature = "client")]
use std::default;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{collections::HashMap, hash::Hash, marker::PhantomData, num::NonZero};

use anyhow::anyhow;
use palettevec::PaletteVec;
use palettevec::index_buffer::AlignedIndexBuffer;
use palettevec::palette::HybridPalette;
use serde::de::{DeserializeSeed, Visitor};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::coord::{FaceMap, Pos};

pub struct Key<T>(NonZero<usize>, PhantomData<T>);
impl<T> Key<T> {
    pub fn numeric_id(self) -> usize {
        self.0.get() - 1
    }
}
impl<T> Clone for Key<T> {
    fn clone(&self) -> Self {
        Self(self.0, PhantomData)
    }
}
impl<T> Copy for Key<T> {}
impl<T> PartialEq for Key<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<T> Eq for Key<T> {}
impl<T> Hash for Key<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

pub struct LoadRegistry<T, D> {
    id_map: HashMap<String, Key<T>>,
    data_list: Vec<D>,
    id_list: Vec<String>,
}
impl<T, D: Clone> Clone for LoadRegistry<T, D> {
    fn clone(&self) -> Self {
        LoadRegistry {
            id_map: self.id_map.clone(),
            data_list: self.data_list.clone(),
            id_list: self.id_list.clone(),
        }
    }
}
impl<T, D> LoadRegistry<T, D> {
    pub fn new() -> Self {
        LoadRegistry {
            id_map: Default::default(),
            data_list: Default::default(),
            id_list: Default::default(),
        }
    }
    pub fn register(&mut self, id: String, data: D) -> Key<T> {
        self.id_list.push(id.clone());
        self.data_list.push(data);
        let key = Key(
            unsafe { NonZero::new_unchecked(self.id_list.len()) },
            PhantomData,
        );
        self.id_map.insert(id, key);
        key
    }
    pub fn key(&self, id: &str) -> Option<Key<T>> {
        self.id_map.get(id).cloned()
    }
    pub fn id_of(&self, key: Key<T>) -> &str {
        self.id_list[key.numeric_id()].as_str()
    }
}
pub struct Registry<T> {
    id_map: HashMap<String, Key<T>>,
    data_list: Vec<T>,
    id_list: Vec<String>,
}
impl<T> Registry<T> {
    pub fn key(&self, id: &str) -> Option<Key<T>> {
        self.id_map.get(id).cloned()
    }
    pub fn by_key(&self, key: Key<T>) -> &T {
        &self.data_list[key.numeric_id()]
    }
    pub fn data_entries(&self) -> impl Iterator<Item = &T> {
        self.data_list.iter()
    }
}
macro_rules! create_registries{
    ($($type:ty,$id:ident);*) => {
        #[allow(non_snake_case)]
        pub struct RegistryStorage{
            $($id: Registry<$type>,)*
        }
        pub trait RegistryProvider<T> {
            fn get_registry(&self) -> &Registry<T>;
        }
        #[allow(non_snake_case)]
        #[derive(Clone)]
        pub struct LoadRegistryStorage{
            $($id: LoadRegistry<$type, PathBuf>,)*
        }
        pub trait LoadRegistryProvider<T> {
            fn get_load_registry(&self) -> &LoadRegistry<T, PathBuf>;
        }
        $(
            impl RegistryProvider<$type> for RegistryStorage{
                fn get_registry(&self) -> &Registry<$type>{
                    &self.$id
                }
            }
            impl LoadRegistryProvider<$type> for LoadRegistryStorage{
                fn get_load_registry(&self) -> &LoadRegistry<$type, PathBuf>{
                    &self.$id
                }
            }
        )*
        pub trait RegistryConfigLoadable: Sized{
            fn registry_load_from_config(config: &Path) -> anyhow::Result<Self>;
        }
        pub fn load_registries(asset_path: &Path) {
            let mut load_registry = LoadRegistryStorage {
                $($id: LoadRegistry::new(),)*
            };
            $({
                let base_asset_path = asset_path.join(stringify!($id));
                for entry in WalkDir::new(&base_asset_path) {
                    match entry {
                        Ok(entry) => {
                            if entry.file_type().is_file() {
                                let stripped_path = entry.path().strip_prefix(&base_asset_path).unwrap();
                                let id = stripped_path
                                    .with_extension("")
                                    .as_os_str()
                                    .to_string_lossy()
                                    .replace("/", ".");
                                load_registry.$id.register(id, entry.into_path());
                            }
                        }
                        Err(_) => {}
                    }
                }
            })*
            LOAD_REGISTRIES.set(load_registry.clone()).ok().unwrap();
            let mut encountered_error = false;
            let registries = RegistryStorage {
                $($id: {
                    let load_registry = &load_registry.$id;
                    let mut data_list = Vec::with_capacity(load_registry.data_list.len());
                    for (i, data) in load_registry.data_list.iter().enumerate(){
                        match <$type as RegistryConfigLoadable>::registry_load_from_config(&data,){
                            Ok(data) => {data_list.push(data);},
                            Err(error) => {
                                eprintln!("error loading {} {} - {}", stringify!($id), load_registry.id_list[i], error);
                                encountered_error = true;
                            }
                        }
                    }
                    Registry {
                        id_map: load_registry.id_map.clone(),
                        data_list,
                        id_list: load_registry.id_list.clone(),
                    }

                },)*
            };
            if encountered_error{
                eprintln!("Error encountered while loading registries, exiting");
                std::process::exit(0);
            }
            REGISTRIES.set(registries).ok().unwrap();
        }
    }
}
impl<T> Serialize for Key<T>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let id = LOAD_REGISTRIES
            .get()
            .unwrap()
            .get_load_registry()
            .id_of(*self);
        serializer.serialize_str(id)
    }
}
impl<'de, T> Deserialize<'de> for Key<T>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct KeyVisitor<T>(PhantomData<T>);
        impl<'de, T> Visitor<'de> for KeyVisitor<T>
        where
            LoadRegistryStorage: LoadRegistryProvider<T>,
        {
            type Value = Key<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("valid string key")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                LOAD_REGISTRIES
                    .get()
                    .unwrap()
                    .get_load_registry()
                    .key(v)
                    .ok_or_else(|| serde::de::Error::custom(format!("id {} not found", v)))
            }
        }
        deserializer.deserialize_str(KeyVisitor::<T>(PhantomData))
    }
}
static LOAD_REGISTRIES: OnceLock<LoadRegistryStorage> = OnceLock::new();
pub static REGISTRIES: OnceLock<RegistryStorage> = OnceLock::new();

create_registries!(BlockData, block; ItemData, item; TextureData, texture; EntityData, entity; PlantData, plant; BiomeData, biome);

impl<T> Key<T>
where
    RegistryStorage: RegistryProvider<T>,
{
    pub fn data(self) -> &'static T {
        REGISTRIES.get().unwrap().get_registry().by_key(self)
    }
    pub fn id(id: &str) -> Option<Self> {
        REGISTRIES.get().unwrap().get_registry().key(id)
    }
}

impl<T: for<'de> Deserialize<'de>> RegistryConfigLoadable for T {
    fn registry_load_from_config(config: &Path) -> anyhow::Result<Self> {
        ron::from_str::<T>(std::fs::read_to_string(config).unwrap().as_str())
            .map_err(|error| anyhow::anyhow!("{}", error))
    }
}

#[derive(Deserialize)]
pub struct ItemData {
    place: Option<BlockKey>,
    stack_size: u16,
}
pub type ItemKey = Key<ItemData>;

#[cfg(feature = "client")]
fn full_aabb() -> Vec<crate::coord::AABB<f32>> {
    vec![crate::coord::AABB {
        min: Pos {
            x: 0.,
            y: 0.,
            z: 0.,
        },
        max: Pos {
            x: 1.,
            y: 1.,
            z: 1.,
        },
    }]
}
#[derive(Deserialize)]
pub struct BlockData {
    pub health: Option<BlockHealthData>,
    #[cfg(feature = "client")]
    pub render_data: BlockRenderData,
    #[serde(default = "full_aabb")]
    #[cfg(feature = "client")]
    pub selection: Vec<crate::coord::AABB<f32>>,
    #[serde(default)]
    pub plantable: bool,
}
#[derive(Deserialize)]
pub struct BlockHealthData {
    pub health: f32,
}
#[derive(Deserialize)]
#[cfg(feature = "client")]
pub enum BlockRenderData {
    Air,
    Full { faces: FaceMap<TextureKey> },
}

pub type BlockKey = Key<BlockData>;
pub type BlockPalette = PaletteVec<BlockKey, HybridPalette<16, BlockKey>, AlignedIndexBuffer>;

pub struct TextureData {
    #[cfg(feature = "client")]
    pub texture: image::DynamicImage,
}
pub type TextureKey = Key<TextureData>;
impl RegistryConfigLoadable for TextureData {
    fn registry_load_from_config(config: &Path) -> anyhow::Result<TextureData> {
        Ok(Self {
            #[cfg(feature = "client")]
            texture: image::open(config)?,
        })
    }
}

#[derive(Deserialize)]
pub struct EntityData {
    #[cfg(feature = "server")]
    pub inventory_size: usize,
}
pub type EntityKey = Key<EntityData>;

#[derive(Deserialize)]
pub struct PlantData {
    #[cfg(feature = "client")]
    pub texture: TextureKey,
}
pub type PlantKey = Key<PlantData>;

#[derive(Deserialize)]
pub struct BiomeData {
    pub top_block: BlockKey,
    pub middle_block: BlockKey,
    pub bottom_block: BlockKey,
    pub plants: Vec<Spawner<PlantKey>>,
}
#[derive(Deserialize)]
pub struct Spawner<T> {
    pub chance: f32,
    pub entry: T,
}
pub type BiomeKey = Key<BiomeData>;
