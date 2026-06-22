use std::cell::OnceCell;
use std::collections::HashSet;
#[cfg(feature = "client")]
use std::default;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::{collections::HashMap, hash::Hash, marker::PhantomData, num::NonZero};

use anyhow::anyhow;
use image::{DynamicImage, GenericImageView};
use image_overlay::overlay_dyn_img;
use palettevec::PaletteVec;
use palettevec::index_buffer::AlignedIndexBuffer;
use palettevec::palette::HybridPalette;
use rand::{SeedableRng, random_bool};
use rand_xoshiro::Xoshiro256PlusPlus;
use ron::extensions::Extensions;
use serde::de::{DeserializeSeed, Visitor};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::coord::{AABB, Axis, BlockPos, Face, FaceMap, HorizontalFace, Pos, Vec3};
use crate::model::Model;
use crate::net::PropertyModifyMode;
use crate::rotation::BlockRotation;
use crate::scripts::{
    CompiledScript, ExternalScriptByteCode, RegisterId, RegisterOrImmediate, ScriptByteCode,
    ScriptLabel, ScriptParseContext, ScriptParseError, expect_argument_count,
};
use crate::ui::{UIScreen, UIScreenKey, UIStyleList};
use crate::{
    Color, DamageTable, DamageType, EntityPose, EntityStats, GRAVITY_ACCELERATION, InventoryView,
    LookDirection, ViewSlot, WeightedEntry,
};

use serde_default_utils::*;

pub struct Key<T>(NonZero<u32>, PhantomData<T>);
impl<T> Key<T> {
    pub fn numeric_id(self) -> usize {
        self.0.get() as usize - 1
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
        if self.id_map.contains_key(&id) {
            panic!("double registration of {}", id);
        }
        self.id_list.push(id.clone());
        self.data_list.push(data);
        let key = Key(
            unsafe { NonZero::new_unchecked(self.id_list.len() as u32) },
            PhantomData,
        );
        self.id_map.insert(id, key);
        key
    }
    fn register_group(&mut self, id: String, group: HashSet<Key<T>>) -> KeyGroup<T> {
        self.groups.push((group.iter().cloned().collect(), group));
        let key_group = KeyGroup::Group(self.groups.len() - 1);
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
        pub fn load_registries(asset_paths: &[&Path]) {
            let mut load_registry = LoadRegistryStorage {
                $($id: LoadRegistry::default(),)*
            };
            let mut encountered_error = false;
            $({
                let mut groups = HashMap::new();
                for asset_path in asset_paths{
                    let base_asset_path = asset_path.join(stringify!($id));
                    for entry in WalkDir::new(&base_asset_path) {
                        match entry {
                            Ok(entry) => {
                                if entry.file_type().is_file() {
                                    let stripped_path = entry.path().strip_prefix(&base_asset_path).unwrap();
                                    let extension = stripped_path.extension();
                                    match stripped_path.extension().and_then(|ext| ext.to_str()).unwrap_or(""){
                                        "py" | "png~" => continue,
                                        _ => {}
                                    }
                                    if <$type>::should_skip(entry.path()){
                                        continue;
                                    }
                                    let id = stripped_path
                                        .with_extension("")
                                        .as_os_str()
                                        .to_string_lossy()
                                        .replace("/", ".")
                                        .replace("\\", ".");
                                    if id.split(".").last().unwrap().starts_with("#"){
                                        *groups.entry(id.replace("#", "")).or_insert_with(||String::new()) += format!("\n{}", std::fs::read_to_string(entry.into_path()).unwrap()).as_str();
                                    } else {
                                        load_registry.$id.register(id, entry.into_path());
                                    }
                                }
                            }
                            Err(_) => {}
                        }
                    }
                }
                for id in groups.keys() {
                    let mut entries = HashSet::new();
                    //todo: missing error handling
                    fn recursively_load(registry: &LoadRegistry<$type>, groups: &HashMap<String, String>, entries: &mut HashSet<Key<$type>>, id: &str, encountered_error: &mut bool){
                        for line in groups.get(id).unwrap().lines(){
                            if line.is_empty(){
                                continue;
                            }
                            if line.starts_with("#"){
                                recursively_load(registry, groups, entries, &line[1..], encountered_error);
                            } else {
                                match registry.id_map.get(line).cloned(){
                                    Some(id) => {
                                        entries.insert(id);
                                    }
                                    None => {
                                        eprintln!("error loading {} tag {} - {} not found", stringify!($id), id, line);
                                        *encountered_error = true;
                                    }
                                }
                            }
                        }
                    }
                    recursively_load(&load_registry.$id, &groups, &mut entries, &id, &mut encountered_error);
                    load_registry.$id.register_group(id.to_string(), entries);
                }
            })*
            LOAD_REGISTRIES.set(load_registry).ok().unwrap();
            let load_registry = LOAD_REGISTRIES.get().unwrap();
            let registries = RegistryStorage {
                $($id: {
                    let load_registry = &load_registry.$id;
                    let mut data_list = Vec::with_capacity(load_registry.data_list.len());
                    for (i, data) in load_registry.data_list.iter().enumerate(){
                        match <$type as RegistryConfigLoadable>::registry_load_from_config(&data,Key(unsafe{NonZero::new_unchecked(i as u32+1)}, PhantomData)){
                            Ok(mut data) => {data_list.push(data);},
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

create_registries!(BlockData, block; ItemData, item; TextureData, texture; EntityData, entity; PlantData, plant; BiomeData, biome; LootTableData, loot_table; UIScreen, ui; UIStyleList, ui_style; ModelData, model; TranslationLanguageData, language; RecipeData, recipe; PrefabData, prefab; ResearchData, research; WorldGenStructureData, structure);

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
            .map(|i| Key(unsafe { NonZero::new_unchecked(i as u32 + 1) }, PhantomData))
    }
}

pub trait RegistryConfigLoadable: Sized {
    fn registry_load_from_config(config: &Path, key: Key<Self>) -> anyhow::Result<Self>;
    fn should_skip(path: &Path) -> bool {
        false
    }
}

pub trait RegistryRonConfigLoadable: for<'a> Deserialize<'a> {
    fn preload_hook(&mut self) {}
}

impl<T: RegistryRonConfigLoadable> RegistryConfigLoadable for T {
    fn registry_load_from_config(config: &Path, key: Key<Self>) -> anyhow::Result<Self> {
        let data = std::fs::read_to_string(config).unwrap();
        let mut data = ron::Options::default()
            .with_default_extension(Extensions::IMPLICIT_SOME)
            .from_str::<T>(&data)
            .map_err(|error| anyhow::anyhow!("{}", error))?;
        data.preload_hook();
        Ok(data)
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
    Empty,
}
impl<T> Default for KeyGroup<T> {
    fn default() -> Self {
        KeyGroup::Empty
    }
}
impl<T: 'static> KeyGroup<T>
where
    LoadRegistryStorage: LoadRegistryProvider<T>,
    RegistryStorage: RegistryProvider<T>,
{
    pub fn parse(id: &str) -> Option<Self> {
        if id.is_empty() {
            return Some(KeyGroup::Empty);
        }
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
            KeyGroup::Empty => false,
        }
    }
    pub fn list(&self) -> &[Key<T>] {
        match self {
            KeyGroup::Single(key) => std::slice::from_ref(key),
            KeyGroup::Group(group) => {
                let (group, _) = &LOAD_REGISTRIES.get().unwrap().get_load_registry().groups[*group];
                &group[..]
            }
            KeyGroup::Empty => &[],
        }
    }
}
impl<T> Copy for KeyGroup<T> {}
impl<T> Clone for KeyGroup<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Single(v) => Self::Single(*v),
            Self::Group(v) => Self::Group(*v),
            Self::Empty => Self::Empty,
        }
    }
}
impl<T> PartialEq for KeyGroup<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Single(l0), Self::Single(r0)) => l0 == r0,
            (Self::Group(l0), Self::Group(r0)) => l0 == r0,
            (Self::Empty, Self::Empty) => true,
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
            KeyGroup::Empty => {}
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
    #[serde(default)]
    pub equip_stats: EntityStats,
}
impl RegistryRonConfigLoadable for ItemData {}
#[derive(Deserialize)]
pub enum ItemAction {
    Ignore,
    Place(Vec<ItemBlockPlacement>),
    SpawnEntity(EntityKey),
    Plant(PlantKey),
    RotateBlock,
    Consume {
        effects: EntityStats,
        effect_duration: f32,
    },
}
impl ItemAction {
    pub fn variation_count(&self) -> usize {
        match self {
            ItemAction::Place(item_block_placements) => item_block_placements.len(),
            _ => 1,
        }
    }
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
#[derive(Clone, Deserialize)]
pub enum ItemModel {
    Block(BlockKey),
    Model(ModelInstance),
}
#[derive(Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolData {
    pub damage: f32,
    pub swing_time: f32,
    pub damage_type: DamageType,
    pub reach: f32,
    pub knockback: f32,
    pub stamina: f32,
}
impl ToolData {
    pub fn hand() -> ToolData {
        ToolData {
            damage: 1.,
            swing_time: 0.5,
            damage_type: DamageType::Blunt,
            reach: 5.,
            knockback: 2.,
            stamina: 10.,
        }
    }
}
pub type ItemKey = Key<ItemData>;

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
    pub health: BlockHealthData,
    #[cfg(feature = "client")]
    pub render_data: BlockRenderData,
    #[serde(default = "full_aabb")]
    #[cfg(feature = "client")]
    pub selection: Vec<AABB<f32>>,
    #[serde(default = "full_aabb")]
    pub collision: Vec<AABB<f32>>,
    pub loot_table: OwnOrKey<LootTableData>,
    #[serde(default)]
    pub interact_action: BlockInteractAction,
    #[serde(default)]
    pub machine: Option<BlockMachineData>,
    #[serde(default)]
    pub rotation: BlockRotationMode,
    #[serde(default = "default_item_scale")]
    pub item_scale: f32,
    #[serde(default)]
    pub hanging: Option<Face>,
    #[serde(default = "default_supporting_map")]
    pub supporting: FaceMap<bool>,
}
#[derive(Deserialize)]
pub struct BlockRenderConnection {
    pub model: ModelInstance,
    #[serde(default = "default_connection_rotations_all")]
    pub rotations: Vec<BlockRotation>,
    pub contain: HashSet<String>,
    #[serde(default)]
    pub deny: HashSet<String>,
    #[serde(default = "default_front_face")]
    pub offset: Face,
    #[serde(default = "default_bool::<true>")]
    pub lod_hidden: bool,
}
fn default_connection_rotations_all() -> Vec<BlockRotation> {
    Face::all()
        .iter()
        .map(|face| BlockRotation::looking_to(*face))
        .collect()
}
fn default_front_face() -> Face {
    Face::Front
}
impl RegistryRonConfigLoadable for BlockData {
    fn preload_hook(&mut self) {
        match &mut self.render_data {
            BlockRenderData::Model {
                render_connectors, ..
            } => {
                if let Some(machine) = self.machine.as_mut() {
                    for face in Face::all() {
                        let connector = match machine.faces.by_face(face) {
                            BlockMachineFace::InventoryAccess { input, output } => {
                                Some("inventory")
                            }
                            BlockMachineFace::LogicInput => Some("logic_input"),
                            BlockMachineFace::LogicOutput => Some("logic_output"),
                            BlockMachineFace::SignalInput => Some("signal_input"),
                            BlockMachineFace::SignalOutput => Some("signal_output"),
                            BlockMachineFace::Empty => None,
                        };
                        if let Some(connector) = connector {
                            render_connectors
                                .by_face_mut(face)
                                .insert(connector.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn default_supporting_map() -> FaceMap<bool> {
    FaceMap::init(|_| true)
}
fn default_item_scale() -> f32 {
    0.35
}
#[derive(Copy, Clone, Deserialize)]
pub enum BlockRotationMode {
    None,
    Horizontal,
    Full,
    FullOriented,
    Axis,
}
impl BlockRotationMode {
    pub fn from_look_direction(self, direction: LookDirection, up_face: Face) -> BlockRotation {
        fn closest_face_to_offset(offset: Pos, filter: impl Fn(Face) -> bool) -> Face {
            let face_fitness = |face: Face| -> f32 {
                let face_offset = face.get_offset();
                (offset.x * face_offset.x) + (offset.y * face_offset.y) + (offset.z * face_offset.z)
            };
            *Face::all()
                .iter()
                .filter(|face| filter(**face))
                .max_by(|face1, face2| face_fitness(**face1).total_cmp(&face_fitness(**face2)))
                .unwrap()
        }
        match self {
            BlockRotationMode::None => BlockRotation::default(),
            BlockRotationMode::Horizontal => {
                let face = closest_face_to_offset(direction.make_front(), |face| {
                    face.axis_direction().0 != Axis::Y
                });
                BlockRotation::looking_to(face)
            }
            BlockRotationMode::Full => {
                let face = closest_face_to_offset(direction.make_front(), |_| true);
                BlockRotation::looking_to(face)
            }
            BlockRotationMode::FullOriented => BlockRotation::new(
                closest_face_to_offset(direction.make_front(), |face| {
                    face.axis_direction().0 != up_face.axis_direction().0
                }),
                up_face,
            )
            .unwrap(),
            BlockRotationMode::Axis => {
                let face = closest_face_to_offset(direction.make_front(), |_| true);
                let face = match face {
                    Face::Back | Face::Right | Face::Up => face,
                    Face::Front | Face::Left | Face::Down => face.opposite(),
                };
                BlockRotation::new(
                    match face {
                        Face::Front | Face::Back => Face::Up,
                        _ => Face::Front,
                    },
                    face,
                )
                .unwrap()
            }
        }
    }
    pub fn get_nearest_valid(self, value: BlockRotation) -> BlockRotation {
        match self {
            BlockRotationMode::None => BlockRotation::default(),
            BlockRotationMode::Horizontal => match value.front_face() {
                Face::Up | Face::Down => BlockRotation::new(Face::Front, Face::Up).unwrap(),
                front => BlockRotation::new(front, Face::Up).unwrap(),
            },
            BlockRotationMode::Full => match value.front_face() {
                Face::Up => BlockRotation::new(Face::Up, Face::Back).unwrap(),
                Face::Down => BlockRotation::new(Face::Down, Face::Front).unwrap(),
                front => BlockRotation::new(front, Face::Up).unwrap(),
            },
            BlockRotationMode::FullOriented => value,
            BlockRotationMode::Axis => {
                let face = value.up_face();
                let face = match face {
                    Face::Back | Face::Right | Face::Up => face,
                    Face::Front | Face::Left | Face::Down => face.opposite(),
                };
                BlockRotation::new(
                    match face {
                        Face::Front | Face::Back => Face::Up,
                        _ => Face::Front,
                    },
                    face,
                )
                .unwrap()
            }
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
    ModifyProperty {
        property: String,
        value: u16,
        mode: PropertyModifyMode,
    },
}
impl BlockInteractAction {
    pub fn tooltip(&self) -> Option<&str> {
        match self {
            BlockInteractAction::Ignore => None,
            BlockInteractAction::OpenInventory { screen, view } => {
                Some("block_action.open_inventory")
            }
            BlockInteractAction::Pickup => Some("block_action.pickup"),
            BlockInteractAction::ModifyProperty { .. } => Some("block_action.modify_property"),
        }
    }
}
impl Default for BlockInteractAction {
    fn default() -> Self {
        Self::Ignore
    }
}
fn machine_default_script() -> CompiledScript<MachineInstrution> {
    CompiledScript {
        instructions: vec![ScriptByteCode::External(MachineInstrution::Block)],
        named_registers: Vec::new(),
    }
}

#[derive(Deserialize)]
pub struct BlockMachineData {
    pub inventory_size: usize,
    #[serde(default)]
    pub faces: FaceMap<BlockMachineFace>,
    #[serde(default = "machine_default_script")]
    pub script: CompiledScript<MachineInstrution>,
    #[serde(default)]
    pub script_views: Vec<InventoryView>,
    #[serde(default)]
    pub model: Option<ModelInstance>,
    #[serde(default)]
    pub model_animations: Vec<String>,
}
pub enum MachineInstrution {
    Yield,
    Sleep {
        time: f32,
    },
    Block,
    TranferItem {
        self_view: usize,
        other: BlockPos,
        other_face: Face,
        pull: bool,
        success: ScriptLabel,
    },
    AddWakeupObserver {
        other: BlockPos,
    },
    ReadSignal {
        face: Face,
        register: RegisterId,
        success: ScriptLabel,
    },
    ReadLogic {
        face: Face,
        register: RegisterId,
        success: ScriptLabel,
    },
    WriteSignal {
        face: Face,
        value: RegisterOrImmediate,
    },
    WriteLogic {
        face: Face,
        value: RegisterOrImmediate,
    },
    GetSlotItemCount {
        slot: RegisterOrImmediate,
        register: RegisterId,
    },
    MoveItem {
        from_view: usize,
        to_view: usize,
        success: ScriptLabel,
    },
    Craft {
        recipes: KeyGroup<RecipeData>,
        input_view: usize,
        output_view: usize,
        speed: f32,
        success: ScriptLabel,
    },
    PlayAnimation {
        animation: String,
    },
}
impl ExternalScriptByteCode for MachineInstrution {
    fn parse<'a>(
        opcode: &'a str,
        arguments: &[&'a str],
        parse_context: &mut ScriptParseContext,
    ) -> Result<Self, ScriptParseError<'a>> {
        let parse_face = |input: &str| -> Result<Face, ScriptParseError<'static>> {
            Ok(match input {
                "front" => Face::Front,
                "back" => Face::Back,
                "left" => Face::Left,
                "right" => Face::Right,
                "up" => Face::Up,
                "down" => Face::Down,
                other => {
                    return Err(ScriptParseError::ExternalError {
                        line: parse_context.current_line_num,
                        error: format!("expected face, got {}", other),
                    });
                }
            })
        };
        Ok(match opcode {
            "yield" => MachineInstrution::Yield,
            "sleep" => {
                expect_argument_count(parse_context, arguments, 2)?;
                MachineInstrution::Sleep {
                    time: arguments[0].parse().unwrap(),
                }
            }
            "block" => MachineInstrution::Block,
            "read_logic" => {
                expect_argument_count(parse_context, arguments, 3)?;
                MachineInstrution::ReadLogic {
                    face: parse_face(arguments[0])?,
                    register: arguments[1].parse().unwrap(),
                    success: parse_context.parse_label(arguments[2])?,
                }
            }
            "read_signal" => {
                expect_argument_count(parse_context, arguments, 3)?;
                MachineInstrution::ReadSignal {
                    face: parse_face(arguments[0])?,
                    register: arguments[1].parse().unwrap(),
                    success: parse_context.parse_label(arguments[2])?,
                }
            }
            "write_logic" => {
                expect_argument_count(parse_context, arguments, 2)?;
                MachineInstrution::WriteLogic {
                    face: parse_face(arguments[0])?,
                    value: parse_context.parse_value(arguments[1]),
                }
            }
            "write_signal" => {
                expect_argument_count(parse_context, arguments, 2)?;
                MachineInstrution::WriteSignal {
                    face: parse_face(arguments[0])?,
                    value: parse_context.parse_value(arguments[1]),
                }
            }
            "transfer_pull" | "transfer_push" => {
                expect_argument_count(parse_context, arguments, 5)?;
                let x = arguments[1].parse().unwrap();
                let y = arguments[2].parse().unwrap();
                let z = arguments[3].parse().unwrap();
                MachineInstrution::TranferItem {
                    self_view: arguments[0].parse().unwrap(),
                    other: BlockPos { x, y, z },
                    other_face: parse_face(arguments[4])?,
                    pull: match opcode {
                        "transfer_pull" => true,
                        "transfer_push" => false,
                        _ => unreachable!(),
                    },
                    success: parse_context.parse_label(arguments[5]).unwrap(),
                }
            }
            "get_slot_item_count" => {
                expect_argument_count(parse_context, arguments, 2)?;
                MachineInstrution::GetSlotItemCount {
                    slot: parse_context.parse_value(arguments[1]),
                    register: parse_context.parse_register(arguments[0]),
                }
            }
            "move_item" => {
                expect_argument_count(parse_context, arguments, 3)?;
                MachineInstrution::MoveItem {
                    from_view: arguments[0].parse().unwrap(),
                    to_view: arguments[1].parse().unwrap(),
                    success: parse_context.parse_label(arguments[2]).unwrap(),
                }
            }
            "craft" => {
                expect_argument_count(parse_context, arguments, 5)?;
                MachineInstrution::Craft {
                    recipes: KeyGroup::parse(arguments[0]).unwrap(),
                    input_view: arguments[1].parse().unwrap(),
                    output_view: arguments[2].parse().unwrap(),
                    speed: arguments[3].parse().unwrap(),
                    success: parse_context.parse_label(arguments[4]).unwrap(),
                }
            }
            "play_animation" => {
                expect_argument_count(parse_context, arguments, 1)?;
                MachineInstrution::PlayAnimation {
                    animation: arguments[0].to_string(),
                }
            }
            _ => {
                return Err(ScriptParseError::UnknownOpCode {
                    line: parse_context.current_line_num,
                    opcode,
                });
            }
        })
    }
}

#[derive(Deserialize)]
pub enum BlockMachineFace {
    InventoryAccess {
        #[serde(default)]
        input: InventoryView,
        #[serde(default)]
        output: InventoryView,
    },
    LogicInput,
    LogicOutput,
    SignalInput,
    SignalOutput,
    Empty,
}
impl Default for BlockMachineFace {
    fn default() -> Self {
        BlockMachineFace::Empty
    }
}
#[derive(Deserialize)]
#[cfg(feature = "client")]
pub enum BlockRenderData {
    Air,
    Full {
        faces: FaceMap<KeyGroup<TextureData>>,
        #[serde(default)]
        render_connectors: FaceMap<HashSet<String>>,
    },
    Model {
        model: ModelInstance,
        #[serde(default)]
        render_flags: u8,
        #[serde(default)]
        lod_hidden: bool,
        #[serde(default)]
        render_connectors: FaceMap<HashSet<String>>,
        #[serde(default)]
        render_connections: Vec<BlockRenderConnection>,
    },
}

pub type BlockKey = Key<BlockData>;
pub type BlockPalette = PaletteVec<BlockEntry, HybridPalette<16, BlockEntry>, AlignedIndexBuffer>;
#[derive(PartialEq, Eq, Hash, Copy, Clone, Serialize, Deserialize)]
pub struct BlockEntry {
    pub block: BlockKey,
    #[serde(default, skip_serializing_if = "skip_if_default")]
    pub color: BlockColor,
    #[serde(default, skip_serializing_if = "skip_if_default")]
    pub rotation: BlockRotation,
}
impl BlockEntry {
    pub fn simple(block: BlockKey) -> BlockEntry {
        BlockEntry {
            block,
            color: Default::default(),
            rotation: Default::default(),
        }
    }
    pub fn colliders(&self, position: BlockPos) -> impl Iterator<Item = AABB<f32>> {
        self.block.data().collision.iter().map(move |collider| {
            self.rotation
                .rotate_aabb(*collider)
                .offset(position.to_pos())
        })
    }
    pub fn supports(&self, world_face: Face) -> bool {
        let block_data = self.block.data();
        let face = self.rotation.inverse_rotate_face(world_face);
        *block_data.supporting.by_face(face)
    }
}
pub fn skip_if_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Serialize, Deserialize)]
pub struct BlockColor(pub u16);
impl Into<Color> for BlockColor {
    fn into(self) -> Color {
        Color {
            r: ((self.0 & 31) << 3) as u8,
            g: (((self.0 >> 5) & 31) << 3) as u8,
            b: (((self.0 >> 10) & 31) << 3) as u8,
            a: 255,
        }
    }
}
impl Default for BlockColor {
    fn default() -> Self {
        Self(32767)
    }
}
static IMAGE_CACHE: OnceLock<Mutex<HashMap<TextureKey, TextureData>>> = OnceLock::new();
fn image_cache() -> MutexGuard<'static, HashMap<TextureKey, TextureData>> {
    IMAGE_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
}
#[derive(Clone)]
pub struct TextureData {
    pub texture: Arc<DynamicImage>,
    pub color_mask: Option<Arc<DynamicImage>>,
    pub emissive: Option<Arc<DynamicImage>>,
}
pub type TextureKey = Key<TextureData>;
impl RegistryConfigLoadable for TextureData {
    fn registry_load_from_config(config: &Path, key: Key<Self>) -> anyhow::Result<TextureData> {
        if let Some(cached) = image_cache().get(&key) {
            return Ok(cached.clone());
        }
        fn load_image(
            path: &Path,
            load_texture_type: LoadTextureType,
        ) -> anyhow::Result<Arc<DynamicImage>> {
            match path.extension().unwrap().to_str().unwrap() {
                "png" => Ok(Arc::new(
                    image::open(path).map_err(|error| anyhow!("{:?}", error))?,
                )),
                "ron" => {
                    let texture = ron::from_str::<ComposedTexture>(
                        std::fs::read_to_string(path).unwrap().as_str(),
                    )
                    .map_err(|error| anyhow!("{:?}", error))?;
                    texture.resolve(load_texture_type)
                }
                _ => panic!(),
            }
        }
        let texture = load_image(config, LoadTextureType::Texture)?;
        let mut material = HashMap::new();
        for texture_type in [LoadTextureType::ColorMask, LoadTextureType::Emissive] {
            let mut path_search = config.to_path_buf();
            path_search.set_extension(match texture_type {
                LoadTextureType::Texture => unreachable!(),
                LoadTextureType::ColorMask => "color_mask",
                LoadTextureType::Emissive => "emissive",
            });
            path_search.add_extension("png");
            let loaded_texture = if path_search.exists() {
                load_image(&path_search, texture_type)?
            } else {
                path_search.set_extension("ron");
                if path_search.exists() {
                    load_image(&path_search, texture_type)?
                } else {
                    continue;
                }
            };
            if loaded_texture.width() != texture.width()
                || loaded_texture.height() != texture.height()
            {
                return Err(anyhow!(
                    "{:?}'s size {}x{} doesn't match base's size {}x{}",
                    texture_type,
                    loaded_texture.width(),
                    loaded_texture.height(),
                    texture.width(),
                    texture.height()
                ));
            }
            material.insert(texture_type, loaded_texture);
        }
        let texture_data = TextureData {
            texture,
            color_mask: material.remove(&LoadTextureType::ColorMask),
            emissive: material.remove(&LoadTextureType::Emissive),
        };
        image_cache().insert(key, texture_data.clone());
        Ok(texture_data)
    }
    fn should_skip(path: &Path) -> bool {
        let file_name = path.file_stem().unwrap().to_str().unwrap();
        file_name.ends_with(".emissive") || file_name.ends_with(".color_mask")
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum LoadTextureType {
    Texture,
    ColorMask,
    Emissive,
}
#[derive(Deserialize)]
enum ComposedTexture {
    Image(TextureKey),
    Overlay {
        base: Box<ComposedTexture>,
        overlay: Box<ComposedTexture>,
    },
    Color {
        base: Box<ComposedTexture>,
        color: Color,
    },
    Fill {
        width: u32,
        height: u32,
        color: Color,
    },
    Flip {
        base: Box<ComposedTexture>,
        #[serde(default)]
        horizontal: bool,
        #[serde(default)]
        vertical: bool,
        #[serde(default)]
        diagonal: bool,
    },
}
impl ComposedTexture {
    pub fn resolve(&self, load_texture_type: LoadTextureType) -> anyhow::Result<Arc<DynamicImage>> {
        Ok(match self {
            ComposedTexture::Image(key) => {
                let texture = TextureData::registry_load_from_config(key.path(), *key)
                    .map_err(|_| anyhow!("error loading texture {}", key.text_id()))?;
                match load_texture_type {
                    LoadTextureType::Texture => texture.texture,
                    LoadTextureType::ColorMask => texture.color_mask.unwrap(),
                    LoadTextureType::Emissive => texture.emissive.unwrap(),
                }
            }
            ComposedTexture::Overlay { base, overlay } => {
                let mut base = Arc::unwrap_or_clone(base.resolve(load_texture_type)?);
                let overlay = overlay.resolve(load_texture_type)?;

                if base.width() != overlay.width() || base.height() != overlay.height() {
                    return Err(anyhow!(
                        "base's size {}x{} doesn't match overlay's size {}x{}",
                        base.width(),
                        base.height(),
                        overlay.width(),
                        overlay.height()
                    ));
                }

                overlay_dyn_img(&mut base, &overlay, 0, 0, image_overlay::BlendMode::Normal);
                Arc::new(base)
            }
            ComposedTexture::Color { base, color } => {
                let mut base = Arc::unwrap_or_clone(base.resolve(load_texture_type)?);
                let mut base = base.to_rgba8();
                for pixel in base.pixels_mut() {
                    for (i, v) in Into::<[u8; 4]>::into(*color).into_iter().enumerate() {
                        let p = pixel.0[i] as u16 * v as u16;
                        pixel.0[i] = ((p + 1 + (p >> 8)) >> 8) as u8;
                    }
                }
                Arc::new(base.into())
            }
            ComposedTexture::Fill {
                width,
                height,
                color,
            } => {
                let mut image = DynamicImage::new_rgba8(*width, *height);
                let rgba = image::Rgba((*color).into());
                for mut pixel in image.as_mut_rgba8().unwrap().pixels_mut() {
                    *pixel = rgba;
                }
                Arc::new(image)
            }
            ComposedTexture::Flip {
                base,
                horizontal,
                vertical,
                diagonal,
            } => {
                let mut base = Arc::unwrap_or_clone(base.resolve(load_texture_type)?);
                if *horizontal {
                    base.apply_orientation(image::metadata::Orientation::FlipHorizontal);
                }
                if *vertical {
                    base.apply_orientation(image::metadata::Orientation::FlipVertical);
                }
                if *diagonal {
                    base.apply_orientation(image::metadata::Orientation::Rotate90FlipH);
                }
                Arc::new(base)
            }
        })
    }
}
#[derive(Deserialize)]
pub struct EntityData {
    #[cfg(feature = "server")]
    pub inventory_size: usize,
    #[serde(default)]
    pub equipment_slots: std::ops::Range<usize>,
    pub hitbox_size: f32,
    pub hitbox_height: f32,
    #[serde(default)]
    pub eye_height: f32,
    #[serde(default)]
    pub crouch_height_difference: f32,
    pub model: ModelInstance,
    #[serde(default)]
    pub viewmodel: Option<ModelInstance>,
    #[serde(default)]
    pub interact_action: EntityInteractAction,
    #[serde(default)]
    pub damage_table: DamageTable,
    #[serde(default)]
    pub ai: Option<MobAI>,
    #[serde(default)]
    pub base_stats: EntityStats,
}
impl RegistryRonConfigLoadable for EntityData {}
#[derive(Deserialize)]
pub struct MobAI {
    #[serde(default)]
    pub attacks: KeyGroup<EntityData>,
    #[serde(default)]
    pub self_defends: KeyGroup<EntityData>,
    #[serde(default)]
    pub fears: KeyGroup<EntityData>,
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
    pub fn hitbox(&self, pose: EntityPose) -> AABB<f32> {
        AABB {
            min: Pos {
                x: -self.hitbox_size,
                y: 0.,
                z: -self.hitbox_size,
            },
            max: Pos {
                x: self.hitbox_size,
                y: pose.height(self),
                z: self.hitbox_size,
            },
        }
    }
}
pub type EntityKey = Key<EntityData>;

#[derive(Deserialize)]
pub struct PlantData {
    pub stages: Vec<TextureKey>,
    #[cfg(feature = "client")]
    pub size: f32,
    #[cfg(feature = "client")]
    pub height: f32,
    #[cfg(feature = "client")]
    pub blades: u32,
    #[cfg(feature = "client")]
    pub center_offset: f32,
    pub growth_length: f32,
    #[serde(default)]
    pub harvest_reset: f32,
    pub harvest_loot: OwnOrKey<LootTableData>,
    pub break_loot: OwnOrKey<LootTableData>,
    pub allowed_soil: KeyGroup<BlockData>,
}
impl RegistryRonConfigLoadable for PlantData {}
pub type PlantKey = Key<PlantData>;

#[derive(Deserialize)]
pub struct RoadPlacementEntry {
    pub weight: i32,
    pub weight_center_distance_bias: i32,
    pub block: Option<BlockKey>,
}
impl WeightedEntry for RoadPlacementEntry {
    type WeightModifier = u32;
    fn get_weight(&self, modifier: Self::WeightModifier) -> f32 {
        (self.weight + self.weight_center_distance_bias * modifier as i32).max(0) as f32
    }
}
#[derive(Deserialize)]
pub struct RoadPlacementInfo(pub Vec<RoadPlacementEntry>);

#[derive(Deserialize)]
pub struct BiomeStructureEntry(pub WorldGenStructureKey, pub u32);
impl WeightedEntry for BiomeStructureEntry {
    type WeightModifier = ();
    fn get_weight(&self, modifier: Self::WeightModifier) -> f32 {
        self.1 as f32
    }
}

#[derive(Deserialize)]
pub struct BiomeData {
    pub top_block: BlockKey,
    pub middle_block: BlockKey,
    pub bottom_block: BlockKey,
    pub plants: Vec<PlantSpawner>,
    pub decorators: Vec<BiomeDecorator>,
    pub structures: Vec<BiomeStructureEntry>,
    #[serde(default)]
    pub debug_color: Color,
    pub temperature: BiomeNoiseConfig,
    pub moisture: BiomeNoiseConfig,
    pub elevation: BiomeNoiseConfig,
    pub road: RoadPlacementInfo,
}
impl RegistryRonConfigLoadable for BiomeData {}
#[derive(Deserialize)]
pub struct BiomeNoiseConfig {
    pub target: f32,
    pub weight: f32,
}
impl BiomeNoiseConfig {
    pub fn get_error(&self, value: f32) -> f32 {
        (self.target - value).abs() * self.weight
    }
}
#[derive(Deserialize)]
pub struct PlantSpawner {
    pub chance: f32,
    pub plant: PlantKey,
}
#[derive(Deserialize)]
pub struct BiomeDecorator {
    pub prefab: PrefabKey,
    pub count: u32,
    pub chance: f32,
    #[serde(default)]
    pub exclusion_zone: u8,
}
pub type BiomeKey = Key<BiomeData>;

#[derive(Deserialize)]
pub struct LootTableData {
    pub entries: Vec<LootTableEntry>,
}
impl RegistryRonConfigLoadable for LootTableData {}
pub type LootTableKey = Key<LootTableData>;
#[derive(Deserialize)]
pub struct LootTableEntry {
    pub item: ItemKey,
    #[serde(default)]
    pub modifiers: Vec<LootItemModifier>,
}
#[derive(Deserialize)]
pub enum LootItemModifier {
    SetCount(LootModifierInteger),
    ApplyQuality,
}
#[derive(Deserialize)]
pub enum LootModifierInteger {
    Constant(u32),
    Random(u32, u32),
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
#[derive(Clone)]
pub struct ModelInstance {
    pub model: ModelKey,
    pub textures: Vec<TextureKey>,
}
impl<'de> Deserialize<'de> for ModelInstance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ModelInstanceVisitor;
        impl<'de> Visitor<'de> for ModelInstanceVisitor {
            type Value = ModelInstance;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("valid model")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let mut split = v.split(";");
                let model = split.next().unwrap();
                let model = ModelKey::id(model).ok_or_else(|| {
                    serde::de::Error::custom(format!("model {} not found", model))
                })?;
                Ok(ModelInstance {
                    model,
                    textures: split
                        .map(|texture| {
                            TextureKey::id(texture).ok_or_else(|| {
                                serde::de::Error::custom(format!("texture {} not found", texture))
                            })
                        })
                        .collect::<Result<Vec<TextureKey>, E>>()?,
                })
            }
        }
        deserializer.deserialize_str(ModelInstanceVisitor)
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
    #[serde(default)]
    pub craft_time: f32,
    #[serde(default)]
    pub research: Option<ResearchKey>,
    #[serde(default)]
    pub icon_override: Option<ItemModel>,
}
impl RegistryRonConfigLoadable for RecipeData {}
pub type RecipeKey = Key<RecipeData>;
fn default_prefab_entry_true_chance() -> f32 {
    1.
}
fn default_prefab_replace() -> KeyGroup<BlockData> {
    KeyGroup::parse("#prefab_replacable").unwrap()
}
#[derive(Serialize, Deserialize)]
pub struct PrefabEntry {
    //we unfortunately cannot flatten here
    pub x: i32,
    pub y: i32,
    pub z: i32,
    #[serde(default = "default_prefab_entry_true_chance", skip_serializing)]
    pub chance: f32,
    #[serde(default = "default_prefab_replace", skip_serializing)]
    pub replace: KeyGroup<BlockData>,
    #[serde(default, skip_serializing)]
    pub replace_inverted: bool,
    pub block: BlockKey,
    #[serde(default, skip_serializing_if = "skip_if_default")]
    pub rotation: BlockRotation,
    #[serde(default, skip_serializing_if = "skip_if_default")]
    pub color: BlockColor,
    #[serde(default, skip_serializing_if = "skip_if_default")]
    pub loot_table: Option<LootTableKey>,
}
#[derive(Serialize, Deserialize)]
pub struct PrefabData {
    pub blocks: Vec<PrefabEntry>,
    #[serde(skip_deserializing, skip_serializing, default)]
    pub bb: OnceLock<AABB<i32>>,
}
impl RegistryRonConfigLoadable for PrefabData {}
impl PrefabData {
    pub fn bounding_box(&self) -> AABB<i32> {
        *self.bb.get_or_init(|| {
            AABB::bound(self.blocks.iter().map(|part| BlockPos {
                x: part.x,
                y: part.y,
                z: part.z,
            }))
            .unwrap()
        })
    }
    pub fn build(
        &self,
        position: BlockPos,
        rotation: HorizontalFace,
        seed: u64,
        mut callback: impl FnMut(BlockPos, BlockEntry, &PrefabEntry),
    ) {
        let rotation = BlockRotation::looking_to_horizontal(rotation);
        use rand::Rng;
        use rand::SeedableRng;
        let mut random = Xoshiro256PlusPlus::seed_from_u64(seed);
        for entry in &self.blocks {
            if random.random_bool(entry.chance as f64) {
                callback(
                    position
                        + rotation.rotate_block_pos(BlockPos {
                            x: entry.x,
                            y: entry.y,
                            z: entry.z,
                        }),
                    BlockEntry {
                        block: entry.block,
                        color: entry.color,
                        rotation: entry
                            .block
                            .data()
                            .rotation
                            .get_nearest_valid(rotation.compose(entry.rotation)),
                    },
                    entry,
                );
            }
        }
    }
}
pub type PrefabKey = Key<PrefabData>;
#[derive(Deserialize)]
pub struct ResearchData {
    pub icon: ItemModel,
    #[serde(default)]
    pub requirements: HashMap<ItemKey, u16>,
    #[serde(default)]
    pub dependencies: Vec<ResearchKey>,
    pub x: f32,
    pub y: f32,
}
impl RegistryRonConfigLoadable for ResearchData {}
pub type ResearchKey = Key<ResearchData>;

#[derive(Deserialize)]
pub struct WorldGenStructureData {
    pub exclusion_zone: u16,
    pub root_room: String,
    pub rooms: HashMap<String, WorldGenStructureRoom>,
}
impl RegistryRonConfigLoadable for WorldGenStructureData {}
pub type WorldGenStructureKey = Key<WorldGenStructureData>;
#[derive(Deserialize)]
pub struct WorldGenStructureConnection {
    pub position: BlockPos,
    pub facing: HorizontalFace,
    pub rooms: Vec<WorldGenStructureRoomSelection>,
}
#[derive(Deserialize)]
pub struct WorldGenStructureRoomSelection {
    pub room: String,
    pub weight: f32,
    #[serde(default)]
    pub weight_depth_bias: f32,
}
impl WeightedEntry for WorldGenStructureRoomSelection {
    type WeightModifier = u32;
    fn get_weight(&self, modifier: Self::WeightModifier) -> f32 {
        (self.weight + self.weight_depth_bias * modifier as f32).max(0.)
    }
}
#[derive(Deserialize)]
pub struct WorldGenStructureRoom {
    pub prefab: PrefabKey,
    pub connections: Vec<WorldGenStructureConnection>,
    #[serde(default)]
    pub road: Option<(BlockPos, u8)>,
}
