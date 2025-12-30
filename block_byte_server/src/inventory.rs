use std::path::Path;

use block_byte_common::registry::ItemKey;
use serde::Deserialize;
use smallvec::SmallVec;

use crate::registry::{Key, RegistryConfigLoadable};

pub type ItemCount = u16;

#[derive(Clone)]
pub struct ItemStack {
    pub item: ItemKey,
    pub count: ItemCount,
    pub components: SmallVec<[ItemComponent; 4]>,
}
impl ItemStack {
    pub fn get_component<T>(&self) -> Option<&T>
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        for component in &self.components {
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
        for component in &mut self.components {
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
            let current = self.components[index].get_component_mut().unwrap();
            *current = component;
            return current;
        }
        self.components
            .push(<ItemComponent as ItemComponentQuery<T>>::create_component(
                component,
            ));
        self.components
            .last_mut()
            .unwrap()
            .get_component_mut()
            .unwrap()
    }
    pub fn get_or_init_component<T>(&mut self, initializer: impl FnOnce() -> T) -> &mut T
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        if let Some(index) = self.component_index() {
            return self.components[index].get_component_mut().unwrap();
        }

        self.components
            .push(<ItemComponent as ItemComponentQuery<T>>::create_component(
                initializer(),
            ));
        self.components
            .last_mut()
            .unwrap()
            .get_component_mut()
            .unwrap()
    }
    pub fn remove_component<T>(&mut self) -> bool
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        if let Some(index) = self.component_index() {
            self.components.swap_remove(index);
            true
        } else {
            false
        }
    }
    fn component_index<T>(&self) -> Option<usize>
    where
        ItemComponent: ItemComponentQuery<T>,
    {
        self.components
            .iter()
            .enumerate()
            .find(|(_, c)| c.is_component())
            .map(|(i, _)| i)
    }
    pub fn copy(&self, new_count: ItemCount) -> ItemStack {
        ItemStack {
            item: self.item,
            count: new_count,
            components: self.components.clone(),
        }
    }
    pub fn merge(&self, other: &ItemStack) -> Option<(ItemStack, ItemCount)> {
        unimplemented!()
    }
}

macro_rules! create_component_enum{
    ($($type:tt),*) => {
        #[derive(Clone)]
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
    }
}

create_component_enum!(ItemDurability, ItemMana);

/*pub trait ItemComponentMerge {
    pub fn merge(
        &self,
        self_count: ItemCount,
        other: Option<&Self>,
        other_count: ItemCount,
    ) -> Option<Self>;
}*/

#[derive(Clone)]
pub struct ItemDurability(pub f32);

#[derive(Clone)]
pub struct ItemMana(pub f32);

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

pub struct InventoryView<'a> {
    slots: Vec<usize>,
    inventory: &'a mut Inventory,
}
impl InventoryView<'_> {
    pub fn size(&self) -> usize {
        self.slots.len()
    }
    pub fn get_slot(&self, slot: usize) -> Option<&ItemStack> {
        self.inventory.items[*self.slots.get(slot)?].as_ref()
    }
    pub fn set_slot(&mut self, slot: usize, item: Option<ItemStack>) -> Result<(), ()> {
        match self.slots.get(slot) {
            Some(slot) => {
                self.inventory.items[*slot] = item;
                Ok(())
            }
            None => Err(()),
        }
    }
}
