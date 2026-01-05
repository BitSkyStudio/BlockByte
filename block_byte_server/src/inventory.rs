use std::path::Path;

use block_byte_common::registry::{ItemKey, LootTableKey};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::registry::{Key, RegistryConfigLoadable};

pub type ItemCount = u16;

#[derive(Clone, Serialize, Deserialize)]
pub struct ItemStack {
    pub item: ItemKey,
    pub count: ItemCount,
    pub components: ItemComponentStorage,
}
impl ItemStack {
    pub fn copy(&self, new_count: ItemCount) -> ItemStack {
        ItemStack {
            item: self.item,
            count: new_count,
            components: self.components.clone(),
        }
    }
    pub fn merge(&self, other: &ItemStack) -> Option<(ItemStack, Option<ItemStack>)> {
        if self.item != other.item {
            return None;
        }
        let merged_components = merge_components(&self.components, &other.components)?;
        let count = self.count + other.count;
        let stack_size = self.item.data().stack_size;
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
    }
}

create_component_enum!(ItemDurability, ItemMana);

pub trait ItemComponentManipulation: Sized {
    fn merge(&self, other: &Self) -> Option<Self>;
    fn split(&self, first_count: ItemCount, second_count: ItemCount) -> (Self, Self);
}

#[derive(Clone, Serialize, Deserialize)]
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
}

#[derive(Clone, Serialize, Deserialize)]
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
}

pub struct InventoryView {
    slots: Vec<usize>,
}
impl InventoryView {
    pub fn size(&self) -> usize {
        self.slots.len()
    }
    pub fn get_slot<'a>(&self, inventory: &'a Inventory, slot: usize) -> Option<&'a ItemStack> {
        inventory.items[*self.slots.get(slot)?].as_ref()
    }
    pub fn set_slot(
        &self,
        inventory: &mut Inventory,
        slot: usize,
        item: Option<ItemStack>,
    ) -> Result<(), ()> {
        match self.slots.get(slot) {
            Some(slot) => {
                inventory.items[*slot] = item;
                Ok(())
            }
            None => Err(()),
        }
    }
}
pub fn generate_loot_table(loot_table: LootTableKey) -> Vec<ItemStack> {
    let loot_table = loot_table.data();
    let mut items = Vec::new();
    for entry in &loot_table.entries {
        if rand::random_bool(entry.chance as f64) {
            items.push(ItemStack {
                item: entry.item,
                count: 1,
                components: ItemComponentStorage::new(),
            });
        }
    }
    items
}
