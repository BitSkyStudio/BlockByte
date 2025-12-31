use serde::{Deserialize, Serialize};

use crate::{coord::ChunkOffset, registry::PlantKey};

#[derive(Serialize, Deserialize)]
pub struct BlockComponentStorage<T> {
    pub components: Vec<(ChunkOffset, T)>,
}
impl<T> BlockComponentStorage<T> {
    pub fn get(&self, block: ChunkOffset) -> Option<&T> {
        self.components
            .iter()
            .find(|(offset, _)| *offset == block)
            .map(|(_, data)| data)
    }
    pub fn get_mut(&mut self, block: ChunkOffset) -> Option<&mut T> {
        self.components
            .iter_mut()
            .find(|(offset, _)| *offset == block)
            .map(|(_, data)| data)
    }
    pub fn set(&mut self, block: ChunkOffset, data: T) {
        if let Some(existing) = self.get_mut(block) {
            *existing = data;
        } else {
            self.components.push((block, data));
        }
    }
    pub fn remove(&mut self, block: ChunkOffset) -> bool {
        if let Some(index) = self
            .components
            .iter()
            .position(|(offset, _)| *offset == block)
        {
            self.components.swap_remove(index);
            true
        } else {
            false
        }
    }
    #[cfg(feature = "server")]
    pub fn client<U>(&self) -> BlockComponentStorage<U>
    where
        U: ComponentClientFromServer<T>,
    {
        BlockComponentStorage {
            components: self
                .components
                .iter()
                .map(|(offset, data)| (*offset, U::from_server(data)))
                .collect(),
        }
    }
}
impl<T> Default for BlockComponentStorage<T> {
    fn default() -> Self {
        BlockComponentStorage {
            components: Vec::new(),
        }
    }
}
pub fn skip_serializing_component_storage<T>(components: &BlockComponentStorage<T>) -> bool {
    components.components.is_empty()
}
#[cfg(feature = "server")]
pub fn skip_serializing_component_storage_rwlock<T>(
    components: &parking_lot::RwLock<BlockComponentStorage<T>>,
) -> bool {
    components.read().components.is_empty()
}
macro_rules! create_chunk_block_components{
    ($($type:tt, $ctype:ty, $id:ident);*) => {
        #[derive(Default, Serialize, Deserialize)]
        #[cfg(feature="server")]
        pub struct ChunkBlockComponents{
            $(
                #[serde(skip_serializing_if = "skip_serializing_component_storage_rwlock", default)]
                pub $id: parking_lot::RwLock<BlockComponentStorage<$type>>,
            )*
        }
        #[derive(Serialize, Deserialize)]
        pub struct ClientChunkBlockComponents{
            //#[serde(skip_serializing_if = "skip_serializing_component_storage")]
            $(pub $id: BlockComponentStorage<$ctype>,)*
        }
        impl ClientChunkBlockComponents{
            pub fn remove_block(&mut self, offset: ChunkOffset){
                $(self.$id.remove(offset);)*
            }
        }
        #[cfg(feature="server")]
        impl ChunkBlockComponents{
            pub fn client(&self) -> ClientChunkBlockComponents{
                ClientChunkBlockComponents{
                    $($id: (&*self.$id.read()).client(),)*
                }
            }
        }
        #[derive(Serialize, Deserialize)]
        pub enum ClientBlockComponentUpdate{
            $($type(Option<$ctype>),)*
        }
        impl ClientBlockComponentUpdate{
            pub fn update(self, offset: ChunkOffset, components: &mut ClientChunkBlockComponents){
                match self{
                    $(ClientBlockComponentUpdate::$type(data) => {
                        match data{
                            Some(data) => {
                                components.$id.set(offset, data);
                            }
                            None => {
                                components.$id.remove(offset);
                            }
                        }
                    })*
                }
            }
        }
    }
}

create_chunk_block_components!(BlockDamage, ClientBlockDamage, damage; BlockPlants, ClientBlockPlants, plant);

pub trait ComponentClientFromServer<T> {
    fn from_server(server: &T) -> Self;
}

#[derive(Serialize, Deserialize)]
#[cfg(feature = "server")]
pub struct BlockDamage {
    pub damage: f32,
}
#[derive(Serialize, Deserialize)]
pub struct ClientBlockDamage {
    pub damage: f32,
}
impl ComponentClientFromServer<BlockDamage> for ClientBlockDamage {
    fn from_server(value: &BlockDamage) -> Self {
        ClientBlockDamage {
            damage: value.damage,
        }
    }
}
impl<T> ComponentClientFromServer<T> for () {
    fn from_server(server: &T) -> Self {
        ()
    }
}
#[derive(Serialize, Deserialize)]
pub struct BlockPlants {
    pub plants: Vec<(PlantKey, f32)>,
}
#[derive(Serialize, Deserialize)]
pub struct ClientBlockPlants {
    pub plants: Vec<PlantKey>,
}
impl ComponentClientFromServer<BlockPlants> for ClientBlockPlants {
    fn from_server(server: &BlockPlants) -> Self {
        ClientBlockPlants {
            plants: server.plants.iter().map(|(plant, _)| *plant).collect(),
        }
    }
}
