use std::path::Path;

use block_byte_common::{
    ClientItem, EntityStats, InventoryView, ViewSlot,
    registry::{
        ItemData, ItemKey, KeyGroup, LootItemModifier, LootModifierInteger, LootTableData,
        LootTableKey,
    },
};
use parking_lot::{RwLock, RwLockWriteGuard};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::registry::{Key, RegistryConfigLoadable};

pub type ItemCount = u16;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemStack {
    pub item: ItemKey,
    pub count: ItemCount,
    pub components: ItemComponentStorage,
}
impl ItemStack {
    pub fn new(item: ItemKey, count: ItemCount) -> ItemStack {
        ItemStack {
            item,
            count,
            components: ItemComponentStorage::new(),
        }
    }
    pub fn copy(&self, new_count: ItemCount) -> ItemStack {
        ItemStack {
            item: self.item,
            count: new_count,
            components: self.components.clone(),
        }
    }
    pub fn merge(
        &self,
        other: &ItemStack,
        view_slot: &ViewSlot,
    ) -> Option<(ItemStack, Option<ItemStack>)> {
        if self.item != other.item {
            return None;
        }
        let merged_components = merge_components(&self.components, &other.components)?;
        let count = self.count + other.count;
        let stack_size = match view_slot.stack_size_override {
            Some(stack_size) => stack_size,
            None => self.item.data().stack_size,
        };
        let item = ItemStack {
            item: self.item,
            count,
            components: merged_components,
        };
        if count <= stack_size {
            Some((item, None))
        } else {
            let (first, second) = item.split(stack_size);
            Some((first, Some(second)))
        }
    }
    pub fn split(&self, split_count: ItemCount) -> (ItemStack, ItemStack) {
        assert!(
            split_count > 0 && split_count < self.count,
            "Invalid split count"
        );
        let second_count = self.count - split_count;
        let (first_components, second_components) =
            split_components(&self.components, split_count, second_count);
        (
            ItemStack {
                item: self.item,
                count: split_count,
                components: first_components,
            },
            ItemStack {
                item: self.item,
                count: second_count,
                components: second_components,
            },
        )
    }
    pub fn client(&self) -> ClientItem {
        ClientItem {
            item: self.item,
            count: self.count,
            description: self.components.description(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ItemComponentStorage(SmallVec<[ItemComponent; 4]>);
impl ItemComponentStorage {
    pub fn new() -> Self {
        ItemComponentStorage(SmallVec::new())
    }
    pub fn get_component<T>(&self) -> Option<&T>
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        for component in &self.0 {
            if let Some(value) = component.get_component() {
                return Some(value);
            }
        }
        return None;
    }
    pub fn get_component_mut<T>(&mut self) -> Option<&mut T>
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        for component in &mut self.0 {
            if let Some(value) = component.get_component_mut() {
                return Some(value);
            }
        }
        return None;
    }
    pub fn has_component<T>(&self) -> bool
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        self.component_index().is_some()
    }
    pub fn set_component<T>(&mut self, component: T) -> &mut T
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        if let Some(index) = self.component_index() {
            let current = self.0[index].get_component_mut().unwrap();
            *current = component;
            return current;
        }
        self.0
            .push(<ItemComponent as ItemComponentQuery<T>>::create_component(
                component,
            ));
        self.0.last_mut().unwrap().get_component_mut().unwrap()
    }
    pub fn get_or_init_component<T>(&mut self, initializer: impl FnOnce() -> T) -> &mut T
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        if let Some(index) = self.component_index() {
            return self.0[index].get_component_mut().unwrap();
        }

        self.0
            .push(<ItemComponent as ItemComponentQuery<T>>::create_component(
                initializer(),
            ));
        self.0.last_mut().unwrap().get_component_mut().unwrap()
    }
    pub fn remove_component<T>(&mut self) -> bool
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        if let Some(index) = self.component_index() {
            self.0.swap_remove(index);
            true
        } else {
            false
        }
    }
    fn component_index<T>(&self) -> Option<usize>
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        self.0
            .iter()
            .enumerate()
            .find(|(_, c)| c.is_component())
            .map(|(i, _)| i)
    }
    fn description(&self) -> String {
        self.0
            .iter()
            .map(|component| component.description())
            .filter(|description| !description.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

macro_rules! create_component_enum{
    ($($type:tt),*) => {
        #[derive(Clone, Serialize, Deserialize)]
        pub enum ItemComponent{
            $($type($type),)*
        }
        pub trait ItemComponentQuery<T>{
            fn is_component(&self) -> bool;
            fn get_component(&self) -> Option<&T>;
            fn get_component_mut(&mut self) -> Option<&mut T>;
            fn create_component(component: T) -> ItemComponent;
        }
        $(impl ItemComponentQuery<$type> for ItemComponent{
            fn is_component(&self) -> bool{
                match self{
                    ItemComponent::$type(_) => true,
                    _ => false
                }
            }
            fn get_component(&self) -> Option<&$type>{
                match self{
                    ItemComponent::$type(val) => Some(val),
                    _ => None
                }
            }
            fn get_component_mut(&mut self) -> Option<&mut $type>{
                match self{
                    ItemComponent::$type(val) => Some(val),
                    _ => None
                }
            }
            fn create_component(component: $type) -> ItemComponent{
                ItemComponent::$type(component)
            }
        })*
        impl ItemComponent{
            pub fn description(&self) -> String{
                match self{
                    $(
                        ItemComponent::$type(component) => <$type as ItemComponentManipulation>::description(component),
                    )*
                }
            }
        }
        fn merge_components(first: &ItemComponentStorage, second: &ItemComponentStorage) -> Option<ItemComponentStorage>{
            let mut storage = ItemComponentStorage::new();
            $(
                match (first.get_component::<$type>(), second.get_component::<$type>()){
                    (Some(first), Some(second)) => {
                        storage.set_component(first.merge(second)?);
                    }
                    (Some(component), None) | (None, Some(component)) => {
                        storage.set_component(component.clone());
                    }
                    (None, None) => {}
                }
            )*
            Some(storage)
        }
        fn split_components(components: &ItemComponentStorage, first_count: ItemCount, second_count: ItemCount) -> (ItemComponentStorage, ItemComponentStorage){
            let mut first = ItemComponentStorage::new();
            let mut second = ItemComponentStorage::new();
            $(
                if let Some(component) = components.get_component::<$type>(){
                    let (first_component, second_component) = component.split(first_count, second_count);
                    first.set_component(first_component);
                    second.set_component(second_component);
                }
            )*
            (first, second)
        }
        impl PartialEq for ItemComponentStorage {
            fn eq(&self, other: &Self) -> bool {
                $(
                    match (self.get_component::<$type>(), other.get_component::<$type>()){
                        (Some(first), Some(second)) => {
                            if first != second{
                                return false;
                            }
                        }
                        (None, None) => {}
                        _ => return false,
                    }
                )*
                true
            }
        }
        impl Eq for ItemComponentStorage{}
    }
}

create_component_enum!(ItemDurability, ItemMana, ItemQuality, ItemCraftStats);

pub trait ItemComponentManipulation: Sized {
    fn merge(&self, other: &Self) -> Option<Self>;
    fn split(&self, first_count: ItemCount, second_count: ItemCount) -> (Self, Self);
    fn description(&self) -> String;
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemDurability(pub u32);
impl ItemComponentManipulation for ItemDurability {
    fn merge(&self, other: &Self) -> Option<Self> {
        Some(ItemDurability(self.0 + other.0))
    }
    fn split(&self, first_count: ItemCount, second_count: ItemCount) -> (Self, Self) {
        let take_count = (self.0 as f32 * first_count as f32 / (first_count + second_count) as f32)
            .ceil() as u32;
        (
            ItemDurability(take_count),
            ItemDurability(self.0 - take_count),
        )
    }
    fn description(&self) -> String {
        format!("Durability: {}", self.0)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemMana(pub u32);
impl ItemComponentManipulation for ItemMana {
    fn merge(&self, other: &Self) -> Option<Self> {
        Some(ItemMana(self.0 + other.0))
    }
    fn split(&self, first_count: ItemCount, second_count: ItemCount) -> (Self, Self) {
        let take_count = (self.0 as f32 * first_count as f32 / (first_count + second_count) as f32)
            .ceil() as u32;
        (ItemMana(take_count), ItemMana(self.0 - take_count))
    }
    fn description(&self) -> String {
        format!("Mana: {}", self.0)
    }
}

#[derive(Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub enum ItemQuality {
    Horrible,
    Bad,
    Normal,
    Good,
    VeryGood,
    Excellent,
    Masterpiece,
    Legendary,
}
impl ItemQuality {
    pub fn factor(self) -> f32 {
        match self {
            ItemQuality::Horrible => 0.5,
            ItemQuality::Bad => 0.8,
            ItemQuality::Normal => 1.,
            ItemQuality::Good => 1.2,
            ItemQuality::VeryGood => 1.4,
            ItemQuality::Excellent => 1.8,
            ItemQuality::Masterpiece => 2.5,
            ItemQuality::Legendary => 4.,
        }
    }
}
impl ItemComponentManipulation for ItemQuality {
    fn merge(&self, other: &Self) -> Option<Self> {
        if *self == *other { Some(*self) } else { None }
    }
    fn split(&self, first_count: ItemCount, second_count: ItemCount) -> (Self, Self) {
        (*self, *self)
    }
    fn description(&self) -> String {
        format!("Quality: {:?}", self)
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemCraftStats(pub Box<EntityStats>);
impl ItemComponentManipulation for ItemCraftStats {
    fn merge(&self, other: &Self) -> Option<Self> {
        None
    }
    fn split(&self, first_count: ItemCount, second_count: ItemCount) -> (Self, Self) {
        (self.clone(), self.clone())
    }
    fn description(&self) -> String {
        "todo".to_string()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub items: Box<[Option<ItemStack>]>,
}
impl Inventory {
    pub fn new(size: usize) -> Inventory {
        Inventory {
            items: (0..size)
                .map(|_| None)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        }
    }
    pub fn get_raw<'a>(&'a self, slot: usize) -> Option<&'a ItemStack> {
        if slot >= self.items.len() {
            return None;
        }
        self.items[slot].as_ref()
    }
    pub fn full_view(&self) -> InventoryView {
        InventoryView::from_range(0..self.items.len())
    }
    pub fn get_slot<'a>(&'a self, view: &InventoryView, slot: usize) -> Option<&'a ItemStack> {
        self.items[view.slots.get(slot).unwrap().slot].as_ref()
    }
    pub fn get_slot_mut<'a>(
        &'a mut self,
        view: &InventoryView,
        slot: usize,
    ) -> &'a mut Option<ItemStack> {
        let index = view.slots.get(slot).unwrap();
        &mut self.items[index.slot]
    }
    pub fn get_slot_mut_raw<'a>(&'a mut self, slot: usize) -> &'a mut Option<ItemStack> {
        &mut self.items[slot]
    }
    pub fn set_slot(
        &mut self,
        view: &InventoryView,
        slot: usize,
        item: Option<ItemStack>,
    ) -> Result<(), ()> {
        //todo: how should we handle filters and slot size overrides?
        match view.slots.get(slot) {
            Some(slot) => {
                self.items[slot.slot] = item;
                Ok(())
            }
            None => Err(()),
        }
    }
    pub fn add_item(&mut self, view: &InventoryView, mut item: ItemStack) -> Option<ItemStack> {
        let stack_size = item.item.data().stack_size;
        for slot_index in &view.slots {
            let mut slot = &mut self.items[slot_index.slot];
            if let Some(slot) = slot {
                if let Some((stack, rest)) = item.merge(&slot, slot_index) {
                    *slot = stack;
                    match rest {
                        Some(rest) => {
                            item = rest;
                        }
                        None => return None,
                    }
                }
            }
        }
        for slot_index in &view.slots {
            if let Some(filter) = &slot_index.filter {
                if !filter.contains(item.item) {
                    continue;
                }
            }
            let mut slot = &mut self.items[slot_index.slot];
            if slot.is_none() {
                if item.count > stack_size {
                    let (first, second) = item.split(stack_size);
                    *slot = Some(first);
                    item = second;
                } else {
                    *slot = Some(item);
                    return None;
                }
            }
        }
        Some(item)
    }
    pub fn count_item(&self, view: &InventoryView, item: impl ItemMatcher) -> ItemCount {
        let mut count = 0;
        for slot in &view.slots {
            match &self.items[slot.slot] {
                Some(stack) => {
                    if item.matches(stack) {
                        count += stack.count
                    }
                }
                None => {}
            }
        }
        count
    }
    pub fn remove_item(
        &mut self,
        view: &InventoryView,
        item: impl ItemMatcher,
        mut count: ItemCount,
    ) -> ItemCount {
        for slot_index in &view.slots {
            let mut slot = &mut self.items[slot_index.slot];
            if slot.is_some() {
                let mut item_slot = slot.as_mut().unwrap();
                if !item.matches(item_slot) {
                    continue;
                }
                let take = count.min(item_slot.count);
                item_slot.count -= take;
                if item_slot.count == 0 {
                    *slot = None;
                }
                count -= take;
                if count == 0 {
                    return 0;
                }
            }
        }
        count
    }
}
trait ItemMatcher {
    fn matches(&self, item: &ItemStack) -> bool;
}
impl ItemMatcher for ItemKey {
    fn matches(&self, item: &ItemStack) -> bool {
        item.item == *self
    }
}
impl ItemMatcher for KeyGroup<ItemData> {
    fn matches(&self, item: &ItemStack) -> bool {
        self.contains(item.item)
    }
}
pub struct LootGenerationContext {}
impl LootGenerationContext {
    pub fn generate_integer(&self, integer: &LootModifierInteger) -> u32 {
        match integer {
            LootModifierInteger::Constant(value) => *value,
            LootModifierInteger::Random(min, max) => rand::random_range(*min..*max),
        }
    }
}
pub fn generate_loot_table(loot_table: &LootTableData) -> Vec<ItemStack> {
    let mut items = Vec::new();
    for entry in &loot_table.entries {
        if rand::random_bool(entry.chance as f64) {
            let mut item = ItemStack {
                item: entry.item,
                count: 1,
                components: ItemComponentStorage::new(),
            };
            let context = LootGenerationContext {};
            for modifier in &entry.modifiers {
                match modifier {
                    LootItemModifier::SetCount(value) => {
                        item.count = context.generate_integer(value) as u16;
                    }
                    LootItemModifier::ApplyQuality => {
                        //todo: randomness
                        item.components.set_component(ItemQuality::Good);
                    }
                }
            }
            if item.count > 0 {
                items.push(item);
            }
        }
    }
    items
}
pub fn lock_inventories<'a>(
    a: &'a RwLock<Inventory>,
    b: &'a RwLock<Inventory>,
) -> (
    RwLockWriteGuard<'a, Inventory>,
    RwLockWriteGuard<'a, Inventory>,
) {
    if (a as *const _) < (b as *const _) {
        let a = a.write();
        let b = b.write();
        (a, b)
    } else {
        let b = b.write();
        let a = a.write();
        (a, b)
    }
}
