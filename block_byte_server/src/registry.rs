use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{collections::HashMap, hash::Hash, marker::PhantomData, num::NonZero};

use serde::de::{DeserializeSeed, Visitor};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

pub struct Key<T>(NonZero<usize>, PhantomData<T>);
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
        deserializer.deserialize_str(KeyVisitor::<T>(PhantomData))
    }
}
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
        let id = LOAD_REGISTRIES
            .get()
            .unwrap()
            .get_load_registry()
            .key(v)
            .unwrap();
        Ok(id)
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
        self.id_list[key.0.get() - 1].as_str()
    }
    pub fn load(&self, loader: impl Fn(&D) -> T) -> Registry<T> {
        Registry {
            id_map: self.id_map.clone(),
            data_list: self.data_list.iter().map(|data| loader(data)).collect(),
            id_list: self.id_list.clone(),
        }
    }
}
pub struct Registry<T> {
    id_map: HashMap<String, Key<T>>,
    data_list: Vec<T>,
    id_list: Vec<String>,
}
impl<T> Registry<T> {
    pub fn by_key(&self, key: Key<T>) -> &T {
        &self.data_list[key.0.get() - 1]
    }
}

static LOAD_REGISTRIES: OnceLock<LoadRegistryStorage> = OnceLock::new();

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
        pub trait RegistryConfigLoadable{
            fn registry_load_from_config(config: &Path) -> Self;
        }
        pub fn load_registries(asset_path: &Path) -> RegistryStorage {
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
            RegistryStorage {
                $($id: load_registry.$id.load(|data| {
                    <$type as RegistryConfigLoadable>::registry_load_from_config(
                        &data,
                    )
                }),)*
            }
        }
    }
}
create_registries!(crate::world::BlockData, block; crate::inventory::ItemData, item);

impl<T: for<'de> Deserialize<'de>> RegistryConfigLoadable for T {
    fn registry_load_from_config(config: &Path) -> Self {
        ron::from_str(std::fs::read_to_string(config).unwrap().as_str()).unwrap()
    }
}
