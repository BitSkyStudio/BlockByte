use std::cell::OnceCell;
use std::collections::HashSet;
#[cfg(feature = "client")]
use std::default;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::{collections::HashMap, hash::Hash, marker::PhantomData, num::NonZero};

use anyhow::anyhow;
use image::DynamicImage;
use image_overlay::overlay_dyn_img;
use palettevec::PaletteVec;
use palettevec::index_buffer::AlignedIndexBuffer;
use palettevec::palette::HybridPalette;
use serde::de::{DeserializeSeed, Visitor};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::coord::{AABB, BlockPos, Face, FaceMap, Orientation, Pos};
use crate::model::Model;
use crate::ui::{UIScreen, UIScreenKey, UIStyleList};
use crate::{Color, DamageTable, DamageType, InventoryView, LookDirection};

use serde_default_utils::*;

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
pub struct LoadRegistry<T> {
    id_map: HashMap<String, Key<T>>,
    data_list: Vec<PathBuf>,
    id_list: Vec<String>,
    group_id_map: HashMap<String, KeyGroup<T>>,
    groups: Vec<(Vec<Key<T>>, HashSet<Key<T>>)>,
}
impl<T> Default for LoadRegistry<T> {
    fn default() -> Self {
        Self {
            id_map: Default::default(),
            data_list: Default::default(),
            id_list: Default::default(),
            group_id_map: Default::default(),
            groups: Default::default(),
        }
    }
}
impl<T> LoadRegistry<T> {
    fn register(&mut self, id: String, data: PathBuf) -> Key<T> {
        self.id_list.push(id.clone());
        self.data_list.push(data);
        let key = Key(
            unsafe { NonZero::new_unchecked(self.id_list.len()) },
            PhantomData,
        );
        self.id_map.insert(id, key);
        key
    }
    fn register_group(&mut self, id: String, group: HashSet<Key<T>>) -> KeyGroup<T> {
        self.groups.push((group.iter().cloned().collect(), group));
        let key_group = KeyGroup::Group(self.id_list.len());
        self.group_id_map.insert(id, key_group);
        key_group
    }
}
pub struct Registry<T> {
    pub data_list: Vec<T>,
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
        pub struct LoadRegistryStorage{
            $($id: LoadRegistry<$type>,)*
        }
        pub trait LoadRegistryProvider<T> {
            fn get_load_registry(&self) -> &LoadRegistry<T>;
        }
        $(
            impl RegistryProvider<$type> for RegistryStorage{
                fn get_registry(&self) -> &Registry<$type>{
                    &self.$id
                }
            }
            impl LoadRegistryProvider<$type> for LoadRegistryStorage{
                fn get_load_registry(&self) -> &LoadRegistry<$type>{
                    &self.$id
                }
            }
        )*
        pub trait RegistryConfigLoadable: Sized{
            fn registry_load_from_config(config: &Path, key: Key<Self>) -> anyhow::Result<Self>;
        }
        pub fn load_registries(asset_path: &Path) {
            let mut load_registry = LoadRegistryStorage {
                $($id: LoadRegistry::default(),)*
            };
            $({
                let base_asset_path = asset_path.join(stringify!($id));
                let mut groups = HashMap::new();
                for entry in WalkDir::new(&base_asset_path) {
                    match entry {
                        Ok(entry) => {
                            if entry.file_type().is_file() {
                                let stripped_path = entry.path().strip_prefix(&base_asset_path).unwrap();
                                let id = stripped_path
                                    .with_extension("")
                                    .as_os_str()
                                    .to_string_lossy()
                                    .replace("/", ".")
                                    .replace("\\", ".");
                                if id.starts_with("#"){
                                    groups.insert(id[1..].to_string(), entry.into_path());
                                } else {
                                    load_registry.$id.register(id, entry.into_path());
                                }
                            }
                        }
                        Err(_) => {}
                    }
                }
                let groups: HashMap<_, _> = groups.into_iter().map(|(id, group)|(id, std::fs::read_to_string(group).unwrap())).collect();
                for id in groups.keys() {
                    let mut entries = HashSet::new();
                    //todo: missing error handling
                    fn recursively_load(registry: &LoadRegistry<$type>, groups: &HashMap<String, String>, entries: &mut HashSet<Key<$type>>, id: &str){
                        for line in groups.get(id).unwrap().lines(){
                            if line.starts_with("#"){
                                recursively_load(registry, groups, entries, &line[1..]);
                            } else {
                                entries.insert(registry.id_map.get(line).cloned().unwrap());
                            }
                        }
                    }
                    recursively_load(&load_registry.$id, &groups, &mut entries, &id);
                    load_registry.$id.register_group(id.to_string(), entries);
                }
            })*
            LOAD_REGISTRIES.set(load_registry).ok().unwrap();
            let load_registry = LOAD_REGISTRIES.get().unwrap();
            let mut encountered_error = false;
            let registries = RegistryStorage {
                $($id: {
                    let load_registry = &load_registry.$id;
                    let mut data_list = Vec::with_capacity(load_registry.data_list.len());
                    for (i, data) in load_registry.data_list.iter().enumerate(){
                        match <$type as RegistryConfigLoadable>::registry_load_from_config(&data,Key(unsafe{NonZero::new_unchecked(i+1)}, PhantomData)){
                            Ok(data) => {data_list.push(data);},
                            Err(error) => {
                                eprintln!("error loading {} {} - {}", stringify!($id), load_registry.id_list[i], error);
                                encountered_error = true;
                            }
                        }
                    }
                    Registry {
                        data_list,
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
impl<T: 'static> Serialize for Key<T>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
    RegistryStorage: RegistryProvider<T>,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.text_id())
    }
}
impl<'de, T: 'static> Deserialize<'de> for Key<T>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
    RegistryStorage: RegistryProvider<T>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct KeyVisitor<T>(PhantomData<T>);
        impl<'de, T: 'static> Visitor<'de> for KeyVisitor<T>
        where
            LoadRegistryStorage: LoadRegistryProvider<T>,
            RegistryStorage: RegistryProvider<T>,
        {
            type Value = Key<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("valid string key")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Key::<T>::id(v)
                    .ok_or_else(|| serde::de::Error::custom(format!("id {} not found", v)))
            }
        }
        deserializer.deserialize_str(KeyVisitor::<T>(PhantomData))
    }
}
static LOAD_REGISTRIES: OnceLock<LoadRegistryStorage> = OnceLock::new();
pub static REGISTRIES: OnceLock<RegistryStorage> = OnceLock::new();

create_registries!(BlockData, block; ItemData, item; TextureData, texture; EntityData, entity; PlantData, plant; BiomeData, biome; LootTableData, loot_table; UIScreen, ui; UIStyleList, ui_style; ModelData, model; TranslationLanguageData, language; RecipeData, recipe; BlockStructureData, structure; ResearchData, research);

impl<T: 'static> Key<T>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
    RegistryStorage: RegistryProvider<T>,
{
    pub fn data(self) -> &'static T {
        &REGISTRIES.get().unwrap().get_registry().data_list[self.numeric_id()]
    }
    pub fn text_id(self) -> &'static str {
        LOAD_REGISTRIES.get().unwrap().get_load_registry().id_list[self.numeric_id()].as_str()
    }
    pub fn id(id: &str) -> Option<Self> {
        LOAD_REGISTRIES
            .get()
            .unwrap()
            .get_load_registry()
            .id_map
            .get(id)
            .cloned()
    }
    pub fn path(self) -> &'static Path {
        LOAD_REGISTRIES.get().unwrap().get_load_registry().data_list[self.numeric_id()].as_path()
    }
    pub fn entries() -> impl Iterator<Item = Self> {
        let load_registry = LOAD_REGISTRIES.get().unwrap().get_load_registry();
        (0..load_registry.data_list.len())
            .map(|i| Key(unsafe { NonZero::new_unchecked(i + 1) }, PhantomData))
    }
}

impl<T: for<'de> Deserialize<'de>> RegistryConfigLoadable for T {
    fn registry_load_from_config(config: &Path, key: Key<Self>) -> anyhow::Result<Self> {
        ron::from_str::<T>(std::fs::read_to_string(config).unwrap().as_str())
            .map_err(|error| anyhow::anyhow!("{}", error))
    }
}

#[derive(Deserialize)]
pub enum OwnOrKey<T: 'static>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
    RegistryStorage: RegistryProvider<T>,
{
    Own(T),
    Key(Key<T>),
}
impl<T: 'static> OwnOrKey<T>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
    RegistryStorage: RegistryProvider<T>,
{
    pub fn data(&self) -> &T {
        match self {
            OwnOrKey::Own(data) => &data,
            OwnOrKey::Key(key) => key.data(),
        }
    }
}
pub enum KeyGroup<T> {
    Single(Key<T>),
    Group(usize),
}
impl<T: 'static> KeyGroup<T>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
    RegistryStorage: RegistryProvider<T>,
{
    pub fn parse(id: &str) -> Option<Self> {
        if id.starts_with("#") {
            LOAD_REGISTRIES
                .get()
                .unwrap()
                .get_load_registry()
                .group_id_map
                .get(&id[1..])
                .cloned()
        } else {
            Some(KeyGroup::Single(Key::<T>::id(id)?))
        }
    }
    pub fn contains(self, key: Key<T>) -> bool {
        match self {
            KeyGroup::Single(v) => v == key,
            KeyGroup::Group(group) => {
                let (_, group) = &LOAD_REGISTRIES.get().unwrap().get_load_registry().groups[group];
                group.contains(&key)
            }
        }
    }
    pub fn list(&self) -> &[Key<T>] {
        match self {
            KeyGroup::Single(key) => std::slice::from_ref(key),
            KeyGroup::Group(group) => {
                let (group, _) = &LOAD_REGISTRIES.get().unwrap().get_load_registry().groups[*group];
                &group[..]
            }
        }
    }
}
impl<T> Copy for KeyGroup<T> {}
impl<T> Clone for KeyGroup<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Single(v) => Self::Single(*v),
            Self::Group(v) => Self::Group(*v),
        }
    }
}
impl<T> PartialEq for KeyGroup<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Single(l0), Self::Single(r0)) => l0 == r0,
            (Self::Group(l0), Self::Group(r0)) => l0 == r0,
            _ => false,
        }
    }
}
impl<T> Eq for KeyGroup<T> {}
impl<T> Hash for KeyGroup<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
        match self {
            KeyGroup::Single(key) => key.hash(state),
            KeyGroup::Group(i) => i.hash(state),
        }
    }
}
impl<'de, T: 'static> Deserialize<'de> for KeyGroup<T>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
    RegistryStorage: RegistryProvider<T>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct KeyVisitor<T>(PhantomData<T>);
        impl<'de, T: 'static> Visitor<'de> for KeyVisitor<T>
        where
            LoadRegistryStorage: LoadRegistryProvider<T>,
            RegistryStorage: RegistryProvider<T>,
        {
            type Value = KeyGroup<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("valid string key")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match KeyGroup::parse(v) {
                    Some(group) => Ok(group),
                    None => Err(serde::de::Error::custom(format!("group {} not found", v))),
                }
            }
        }
        deserializer.deserialize_str(KeyVisitor::<T>(PhantomData))
    }
}

static AIR_BLOCK: OnceLock<BlockKey> = OnceLock::new();
pub fn air_block() -> BlockKey {
    *AIR_BLOCK.get_or_init(|| BlockKey::id("air").unwrap())
}

#[derive(Deserialize)]
pub struct ItemData {
    pub model: ItemModel,
    pub stack_size: u16,
    #[serde(default)]
    pub tool: Option<ToolData>,
    #[serde(default)]
    pub action: ItemAction,
}
#[derive(Deserialize)]
pub enum ItemAction {
    Ignore,
    Place(Vec<ItemBlockPlacement>),
}
#[derive(Deserialize)]
pub struct ItemBlockPlacement {
    pub block: BlockKey,
    #[serde(default = "default_u16::<1>")]
    pub use_count: u16,
    #[serde(default)]
    pub research: Option<ResearchKey>,
}
impl Default for ItemAction {
    fn default() -> Self {
        Self::Ignore
    }
}
#[derive(Deserialize)]
pub enum ItemModel {
    Block(BlockKey),
    Model(ModelKey),
}
#[derive(Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolData {
    pub damage: f32,
    pub swing_time: f32,
    pub hit_time: f32,
    pub damage_type: DamageType,
}
impl ToolData {
    pub fn hand() -> ToolData {
        ToolData {
            damage: 1.,
            swing_time: 0.5,
            hit_time: 0.25,
            damage_type: DamageType::Blunt,
        }
    }
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
    pub selection: Vec<AABB<f32>>,
    #[serde(default)]
    pub plantable: bool,
    pub loot_table: OwnOrKey<LootTableData>,
    #[serde(default)]
    pub interact_action: BlockInteractAction,
    #[serde(default)]
    pub machine: Option<BlockMachineData>,
    #[serde(default)]
    pub rotation: BlockRotationMode,
}
#[derive(Copy, Clone, Deserialize)]
pub enum BlockRotationMode {
    None,
    Horizontal,
    Full,
    FullOriented,
}
impl BlockRotationMode {
    pub fn get_nearest_valid(self, value: BlockRotation) -> BlockRotation {
        match self {
            BlockRotationMode::None => BlockRotation::default(),
            BlockRotationMode::Horizontal => match Into::<Orientation>::into(value).forward {
                Face::Up | Face::Down => Orientation::from_front_up(Face::Front, Face::Up).unwrap(),
                front => Orientation::from_front_up(front, Face::Up).unwrap(),
            }
            .into(),
            BlockRotationMode::Full => match Into::<Orientation>::into(value).forward {
                Face::Up => Orientation::from_front_up(Face::Up, Face::Back).unwrap(),
                Face::Down => Orientation::from_front_up(Face::Down, Face::Front).unwrap(),
                front => Orientation::from_front_up(front, Face::Up).unwrap(),
            }
            .into(),
            BlockRotationMode::FullOriented => value,
        }
    }
}
impl Default for BlockRotationMode {
    fn default() -> Self {
        BlockRotationMode::None
    }
}
#[derive(Deserialize)]
pub struct BlockHealthData {
    pub health: f32,
    pub health_regen: f32,
    pub table: DamageTable,
    #[serde(default = "air_block")]
    pub transform_block: BlockKey,
}
#[derive(Deserialize)]
pub enum BlockInteractAction {
    Ignore,
    OpenInventory {
        screen: UIScreenKey,
        view: InventoryView,
    },
    Pickup,
}
impl BlockInteractAction {
    pub fn tooltip(&self) -> Option<&str> {
        match self {
            BlockInteractAction::Ignore => None,
            BlockInteractAction::OpenInventory { screen, view } => {
                Some("block_action.open_inventory")
            }
            BlockInteractAction::Pickup => Some("block_action.pickup"),
        }
    }
}
impl Default for BlockInteractAction {
    fn default() -> Self {
        Self::Ignore
    }
}
#[derive(Deserialize)]
pub struct BlockMachineData {
    pub inventory_size: usize,
    #[serde(default)]
    pub actions: Vec<BlockMachineAction>,
    pub faces: FaceMap<BlockMachineFace>,
}
#[derive(Default, Deserialize)]
pub struct BlockMachineFace {
    #[serde(default)]
    pub input: InventoryView,
}
#[derive(Deserialize)]
pub enum BlockMachineAction {
    Craft {
        base_speed: f32,
        recipes: KeyGroup<RecipeData>,
        input_view: InventoryView,
        output_view: InventoryView,
    },
    TransferItem {
        view: InventoryView,
        speed: f32,
        face: Face,
        offset: BlockPos,
        pull: bool,
    },
    MoveItem {
        from: InventoryView,
        to: InventoryView,
        speed: f32,
    },
}
#[derive(Deserialize)]
#[cfg(feature = "client")]
pub enum BlockRenderData {
    Air,
    Full {
        faces: FaceMap<KeyGroup<TextureData>>,
    },
    Model(ModelKey),
}

pub type BlockKey = Key<BlockData>;
pub type BlockPalette = PaletteVec<BlockEntry, HybridPalette<16, BlockEntry>, AlignedIndexBuffer>;
#[derive(PartialEq, Eq, Hash, Copy, Clone, Serialize, Deserialize)]
pub struct BlockEntry {
    pub block: BlockKey,
    #[serde(default, skip_serializing_if = "skip_if_default")]
    pub color: Color,
    #[serde(default, skip_serializing_if = "skip_if_default")]
    pub rotation: BlockRotation,
}
pub fn skip_if_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}
impl Default for BlockRotation {
    fn default() -> Self {
        BlockRotation::from(Orientation::IDENTITY)
    }
}
impl Into<Orientation> for BlockRotation {
    fn into(self) -> Orientation {
        Orientation::all()[self.0 as usize]
    }
}
impl From<Orientation> for BlockRotation {
    fn from(value: Orientation) -> Self {
        //todo: this shouldnt iterate
        BlockRotation(
            Orientation::all()
                .iter()
                .position(|orientation| *orientation == value)
                .unwrap() as u8,
        )
    }
}
impl From<LookDirection> for BlockRotation {
    fn from(value: LookDirection) -> Self {
        fn closest_face_to_offset(offset: Pos) -> Face {
            let face_fitness = |face: Face| -> f32 {
                let face_offset = face.get_offset();
                (offset.x * face_offset.x) + (offset.y * face_offset.y) + (offset.z * face_offset.z)
            };
            *Face::all()
                .iter()
                .max_by(|face1, face2| face_fitness(**face1).total_cmp(&face_fitness(**face2)))
                .unwrap()
        }
        let orientation = Orientation::from_front_right(
            closest_face_to_offset(value.make_front()),
            closest_face_to_offset(value.make_right()),
        )
        .unwrap_or(Orientation::IDENTITY);
        orientation.into()
    }
}
#[derive(PartialEq, Eq, Hash, Copy, Clone, Serialize, Deserialize)]
pub struct BlockRotation(u8);

//todo: config client
static IMAGE_CACHE: OnceLock<Mutex<HashMap<TextureKey, Arc<DynamicImage>>>> = OnceLock::new();
fn image_cache() -> MutexGuard<'static, HashMap<TextureKey, Arc<DynamicImage>>> {
    IMAGE_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
}
pub struct TextureData {
    pub texture: Arc<DynamicImage>,
}
pub type TextureKey = Key<TextureData>;
impl RegistryConfigLoadable for TextureData {
    fn registry_load_from_config(config: &Path, key: Key<Self>) -> anyhow::Result<TextureData> {
        let texture = match config.extension().unwrap().to_str().unwrap() {
            "png" => Arc::new(image::open(config)?),
            "ron" => {
                let texture =
                    ron::from_str::<ComposedTexture>(std::fs::read_to_string(config)?.as_str())?;
                texture.resolve(config.parent().unwrap())
            }
            _ => panic!(),
        };
        image_cache().insert(key, texture.clone());
        Ok(TextureData { texture })
    }
}
#[derive(Deserialize)]
enum ComposedTexture {
    Image(TextureKey),
    Overlay {
        base: Box<ComposedTexture>,
        overlay: Box<ComposedTexture>,
    },
}
impl ComposedTexture {
    pub fn resolve(&self, texture_path: &Path) -> Arc<DynamicImage> {
        match self {
            ComposedTexture::Image(key) => {
                if let Some(image) = image_cache().get(key) {
                    return Arc::clone(image);
                }
                TextureData::registry_load_from_config(key.path(), *key)
                    .unwrap()
                    .texture
            }
            ComposedTexture::Overlay { base, overlay } => {
                let mut base = Arc::unwrap_or_clone(base.resolve(texture_path));
                let overlay = overlay.resolve(texture_path);
                overlay_dyn_img(&mut base, &overlay, 0, 0, image_overlay::BlendMode::Normal);
                Arc::new(base)
            }
        }
    }
}
#[derive(Deserialize)]
pub struct EntityData {
    #[cfg(feature = "server")]
    pub inventory_size: usize,
    pub hitbox_size: f32,
    pub hitbox_height: f32,
    #[serde(default)]
    pub eye_height: f32,
    pub model: ModelKey,
    #[serde(default)]
    pub interact_action: EntityInteractAction,
    pub health: f32,
    pub damage_table: DamageTable,
}
#[derive(Deserialize)]
pub enum EntityInteractAction {
    Ignore,
    Pickup,
}
impl EntityInteractAction {
    pub fn tooltip(&self) -> Option<&str> {
        match self {
            EntityInteractAction::Ignore => None,
            EntityInteractAction::Pickup => Some("block_action.pickup"),
        }
    }
}
impl Default for EntityInteractAction {
    fn default() -> Self {
        Self::Ignore
    }
}
impl EntityData {
    pub fn hitbox(&self) -> AABB<f32> {
        AABB {
            min: Pos {
                x: -self.hitbox_size,
                y: 0.,
                z: -self.hitbox_size,
            },
            max: Pos {
                x: self.hitbox_size,
                y: self.hitbox_height,
                z: self.hitbox_size,
            },
        }
    }
}
pub type EntityKey = Key<EntityData>;

#[derive(Deserialize)]
pub struct PlantData {
    #[cfg(feature = "client")]
    pub stages: Vec<TextureKey>,
    #[cfg(feature = "client")]
    pub size: f32,
    #[cfg(feature = "client")]
    pub height: f32,
    #[cfg(feature = "client")]
    pub blades: u32,
    #[cfg(feature = "client")]
    pub translation: f32,
}
pub type PlantKey = Key<PlantData>;

#[derive(Deserialize)]
pub struct BiomeData {
    pub top_block: BlockKey,
    pub middle_block: BlockKey,
    pub bottom_block: BlockKey,
    pub plants: Vec<PlantSpawner>,
    pub decorators: Vec<BiomeDecorator>,
}
#[derive(Deserialize)]
pub struct PlantSpawner {
    pub chance: f32,
    pub plant: PlantKey,
}
#[derive(Deserialize)]
pub struct BiomeDecorator {
    pub structure: StructureKey,
    pub count: u32,
    pub chance: f32,
}
pub type BiomeKey = Key<BiomeData>;

#[derive(Deserialize)]
pub struct LootTableData {
    pub entries: Vec<LootTableEntry>,
}
pub type LootTableKey = Key<LootTableData>;
#[derive(Deserialize)]
pub struct LootTableEntry {
    pub item: ItemKey,
    pub chance: f32,
}
pub struct ModelData {
    pub model: Model,
}
pub type ModelKey = Key<ModelData>;
impl RegistryConfigLoadable for ModelData {
    fn registry_load_from_config(config: &Path, key: Key<Self>) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(config).map_err(|_| anyhow!("error loading"))?;
        Ok(ModelData {
            model: serde_json::from_str(&json).map_err(|err| anyhow!("error loading {:?}", err))?,
        })
    }
}

pub struct TranslationLanguageData {
    pub translations: HashMap<String, String>,
}
impl TranslationLanguageData {
    pub fn translate<'a>(&'a self, key: &'a str) -> &'a str {
        self.translations
            .get(key)
            .map(|s| s.as_str())
            .unwrap_or(key)
    }
}
impl RegistryConfigLoadable for TranslationLanguageData {
    fn registry_load_from_config(config: &Path, key: Key<Self>) -> anyhow::Result<Self> {
        let mut translation = TranslationLanguageData {
            translations: HashMap::new(),
        };
        for line in std::fs::read_to_string(config).unwrap().lines() {
            let (key, value) = line.split_once("=").unwrap();
            translation
                .translations
                .insert(key.trim().to_string(), value.trim().to_string());
        }
        Ok(translation)
    }
}
#[derive(Deserialize)]
pub struct RecipeData {
    pub inputs: HashMap<ItemKey, u16>,
    pub outputs: OwnOrKey<LootTableData>,
    pub craft_time: f32,
    #[serde(default)]
    pub research: Option<ResearchKey>,
    #[serde(default)]
    pub icon_override: Option<ItemModel>,
}
pub type RecipeKey = Key<RecipeData>;
#[derive(Serialize, Deserialize)]
pub struct BlockStructurePart {
    pub blocks: HashMap<BlockPos, BlockEntry>,
    pub chance: f32,
}
#[derive(Serialize, Deserialize)]
pub struct BlockStructureData {
    pub parts: Vec<BlockStructurePart>,
}
pub type StructureKey = Key<BlockStructureData>;
#[derive(Serialize, Deserialize)]
pub struct ResearchData {}
pub type ResearchKey = Key<ResearchData>;
