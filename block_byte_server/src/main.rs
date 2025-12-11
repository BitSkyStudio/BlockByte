use std::{path::Path, sync::OnceLock};

use crate::{
    inventory::{ItemData, ItemDurability, ItemKey, ItemStack},
    registry::{Key, Registry, RegistryProvider, RegistryStorage},
    world::BlockData,
};

mod inventory;
mod registry;
mod world;

static SERVER: OnceLock<Server> = OnceLock::new();
pub fn server() -> &'static Server {
    SERVER.get().unwrap()
}

fn main() {
    let server = Server {
        registries: registry::load_registries(&Path::new("assets")),
    };
    SERVER.set(server).ok().unwrap();
}

pub struct Server {
    registries: RegistryStorage,
}

impl Server {
    fn data<T>(&self, key: Key<T>) -> &T
    where
        RegistryStorage: RegistryProvider<T>,
    {
        self.registries.get_registry().by_key(key)
    }
}
