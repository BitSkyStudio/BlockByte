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
use ron::extensions::Extensions;
use serde::de::{DeserializeSeed, Visitor};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::coord::{AABB, BlockPos, Face, FaceMap, Orientation, Pos, Vec3};
use crate::model::Model;
use crate::scripts::{
    CompiledScript, ExternalScriptByteCode, RegisterId, RegisterOrImmediate, ScriptLabel,
    ScriptParseContext, ScriptParseError, expect_argument_count,
};
use crate::ui::{UIScreen, UIScreenKey, UIStyleList};
use crate::{Color, DamageTable, DamageType, InventoryView, LookDirection, ViewSlot};

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
        if self.id_map.contains_key(&id) {
            panic!("double registration of {}", id);
        }
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
        pub trait RegistryConfigLoadable: Sized{
            fn registry_load_from_config(config: &Path, key: Key<Self>) -> anyhow::Result<Self>;
        }
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
                                        "py" => continue,
                                        _ => {}
                                    }
                                    let id = stripped_path
                                        .with_extension("")
                                        .as_os_str()
                                        .to_string_lossy()
                                        .replace("/", ".")
                                        .replace("\\", ".");
                                    if id.starts_with("#"){
                                        *groups.entry(id[1..].to_string()).or_insert_with(||String::new()) += format!("\n{}", std::fs::read_to_string(entry.into_path()).unwrap()).as_str();
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
        let data = std::fs::read_to_string(config).unwrap();
        ron::Options::default()
            .with_default_extension(Extensions::IMPLICIT_SOME)
            .from_str::<T>(&data)
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
    SpawnEntity(EntityKey),
    Plant(PlantKey),
}
impl ItemAction {
    pub fn variation_count(&self) -> usize {
        match self {
            ItemAction::Ignore | ItemAction::SpawnEntity(_) | ItemAction::Plant(_) => 1,
            ItemAction::Place(item_block_placements) => item_block_placements.len(),
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
#[derive(Deserialize)]
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
            reach: 7.,
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
    #[serde(default)]
    pub health: Option<BlockHealthData>,
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
}
impl BlockRotationMode {
    pub fn from_look_direction(self, direction: LookDirection) -> BlockRotation {
        fn closest_face_to_offset(offset: Pos, enable_vertical: bool) -> Face {
            let face_fitness = |face: Face| -> f32 {
                let face_offset = face.get_offset();
                (offset.x * face_offset.x) + (offset.y * face_offset.y) + (offset.z * face_offset.z)
            };
            *Face::all()
                .iter()
                .filter(|face| enable_vertical || face.get_block_offset().y == 0)
                .max_by(|face1, face2| face_fitness(**face1).total_cmp(&face_fitness(**face2)))
                .unwrap()
        }
        let orientation = match self {
            BlockRotationMode::None => Orientation::IDENTITY,
            BlockRotationMode::Horizontal => {
                let face = closest_face_to_offset(direction.make_front(), false);
                Orientation::from_front_right(face, face.cross(Face::Up))
                    .unwrap_or(Orientation::IDENTITY)
            }
            BlockRotationMode::Full => {
                let face = closest_face_to_offset(direction.make_front(), true);
                Orientation::from_front_right(
                    face,
                    face.cross(if face.get_block_offset().y == 0 {
                        Face::Up
                    } else {
                        Face::Front
                    }),
                )
                .unwrap_or(Orientation::IDENTITY)
            }
            BlockRotationMode::FullOriented => Orientation::from_front_right(
                closest_face_to_offset(direction.make_front(), true),
                closest_face_to_offset(direction.make_right(), true),
            )
            .unwrap_or(Orientation::IDENTITY),
        };
        orientation.into()
    }
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
    pub faces: FaceMap<BlockMachineFace>,
    pub script: CompiledScript<MachineInstrution>,
    #[serde(default)]
    pub script_views: Vec<InventoryView>,
}
pub enum MachineInstrution {
    Next,
    Sleep {
        time: f32,
    },
    Suspend,
    TranferItem {
        self_view: usize,
        other: BlockPos,
        other_face: Face,
        pull: bool,
    },
    ReadSignal {
        face: Face,
        register: RegisterId,
        success: ScriptLabel,
    },
    ReadSignalBlock {
        face: Face,
        register: RegisterId,
    },
    ReadLogic {
        face: Face,
        register: RegisterId,
    },
    WriteSignal {
        face: Face,
        value: RegisterOrImmediate,
    },
    WriteValue {
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
    },
    Craft {
        recipes: KeyGroup<RecipeData>,
        input_view: usize,
        output_view: usize,
        speed: f32,
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
            "next" => MachineInstrution::Next,
            "sleep" => {
                expect_argument_count(parse_context.current_line_num, arguments, 2)?;
                let sleep_time = arguments[0].parse().unwrap();
                MachineInstrution::Sleep { time: sleep_time }
            }
            "suspend" => MachineInstrution::Suspend,
            "transfer_pull" | "transfer_push" => {
                expect_argument_count(parse_context.current_line_num, arguments, 5)?;
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
                }
            }
            "get_slot_item_count" => {
                expect_argument_count(parse_context.current_line_num, arguments, 2)?;
                MachineInstrution::GetSlotItemCount {
                    slot: parse_context.parse_value(arguments[1]),
                    register: parse_context.parse_register(arguments[0]),
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
    },
    Model(ModelInstance),
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
    #[serde(default, skip_serializing_if = "skip_if_default")]
    pub state: u16,
}
impl BlockEntry {
    pub fn simple(block: BlockKey) -> BlockEntry {
        BlockEntry {
            block,
            color: Default::default(),
            rotation: Default::default(),
            state: Default::default(),
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
        let orientation = Into::<Orientation>::into(self.rotation);
        let face = orientation.inverse_apply(world_face);
        *block_data.supporting.by_face(face)
    }
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
#[derive(PartialEq, Eq, Hash, Copy, Clone, Serialize, Deserialize)]
pub struct BlockRotation(u8);
impl BlockRotation {
    pub fn rotate_aabb(self, aabb: AABB<f32>) -> AABB<f32> {
        let aabb = aabb.offset(Vec3::all(-0.5));
        let orientation: Orientation = self.into();
        AABB::bound(
            [
                orientation.rotate_pos(aabb.min),
                orientation.rotate_pos(Vec3 {
                    x: aabb.max.x,
                    y: aabb.min.y,
                    z: aabb.min.z,
                }),
                orientation.rotate_pos(Vec3 {
                    x: aabb.min.x,
                    y: aabb.max.y,
                    z: aabb.min.z,
                }),
                orientation.rotate_pos(Vec3 {
                    x: aabb.min.x,
                    y: aabb.min.y,
                    z: aabb.max.z,
                }),
            ]
            .into_iter(),
        )
        .unwrap()
        .offset(Vec3::all(0.5))
    }
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Serialize, Deserialize)]
pub struct BlockColor(u16);
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
        Self(2 ^ 15 - 1)
    }
}

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
    Color {
        base: Box<ComposedTexture>,
        color: Color,
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
            ComposedTexture::Color { base, color } => {
                let mut base = Arc::unwrap_or_clone(base.resolve(texture_path));
                let mut base = base.to_rgba8();
                for pixel in base.pixels_mut() {
                    for (i, v) in Into::<[u8; 4]>::into(*color).into_iter().enumerate() {
                        let p = pixel.0[i] as u16 * v as u16;
                        pixel.0[i] = ((p + 1 + (p >> 8)) >> 8) as u8;
                    }
                }
                Arc::new(base.into())
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
    pub model: ModelInstance,
    #[serde(default)]
    pub interact_action: EntityInteractAction,
    pub health: f32,
    #[serde(default)]
    pub damage_table: DamageTable,
    #[serde(default)]
    pub ai_tasks: Vec<MobAiTask>,
}
#[derive(Deserialize)]
pub enum MobAiTask {
    Attack {
        targets: KeyGroup<EntityData>,
        damage: f32,
        damage_type: DamageType,
    },
    Wander,
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
    pub stages: Vec<TextureKey>,
    #[cfg(feature = "client")]
    pub size: f32,
    #[cfg(feature = "client")]
    pub height: f32,
    #[cfg(feature = "client")]
    pub blades: u32,
    #[cfg(feature = "client")]
    pub translation: f32,
    pub growth_length: f32,
    #[serde(default)]
    pub harvest_reset: f32,
    pub harvest_loot: OwnOrKey<LootTableData>,
    pub break_loot: OwnOrKey<LootTableData>,
    pub allowed_soil: KeyGroup<BlockData>,
}
pub type PlantKey = Key<PlantData>;

#[derive(Deserialize)]
pub struct BiomeData {
    pub top_block: BlockKey,
    pub middle_block: BlockKey,
    pub bottom_block: BlockKey,
    pub plants: Vec<PlantSpawner>,
    pub decorators: Vec<BiomeDecorator>,
    #[serde(default)]
    pub debug_color: Color,
    pub temperature: BiomeNoiseConfig,
    pub moisture: BiomeNoiseConfig,
    pub elevation: BiomeNoiseConfig,
}
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
pub type ResearchKey = Key<ResearchData>;
