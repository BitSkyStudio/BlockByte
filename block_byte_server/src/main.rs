use crate::{
    inventory::{ItemData, ItemDurability, ItemKey, ItemStack},
    registry::{Key, Registry, RegistryProvider},
};

mod inventory;
mod registry;
mod world;

fn main() {
    println!("Hello, world!");
}

pub struct Server {
    item_registry: Registry<ItemData>,
}

impl RegistryProvider<ItemData> for Server {
    fn get_registry(&self) -> &Registry<ItemData> {
        &self.item_registry
    }
}
