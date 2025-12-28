use serde::{Deserialize, Serialize};

use crate::coord::ChunkOffset;

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
}
impl<T, U> From<&BlockComponentStorage<T>> for BlockComponentStorage<U>
where
    U: for<'a> From<&'a T>,
{
    fn from(value: &BlockComponentStorage<T>) -> Self {
        BlockComponentStorage {
            components: value
                .components
                .iter()
                .map(|(offset, data)| (*offset, data.into()))
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
            #[serde(skip_serializing_if = "skip_serializing_component_storage_rwlock")]
            pub $($id: parking_lot::RwLock<BlockComponentStorage<$type>>,)*
        }
        #[derive(Serialize, Deserialize)]
        pub struct ClientChunkBlockComponents{
            //#[serde(skip_serializing_if = "skip_serializing_component_storage")]
            pub $($id: BlockComponentStorage<$ctype>,)*
        }
        #[cfg(feature="server")]
        impl ChunkBlockComponents{
            pub fn client(&self) -> ClientChunkBlockComponents{
                ClientChunkBlockComponents{
                    $($id: (&*self.$id.read()).into(),)*
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

create_chunk_block_components!(BlockDamage, ClientBlockDamage, damage);

#[derive(Serialize, Deserialize)]
#[cfg(feature = "server")]
pub struct BlockDamage {
    pub damage: f32,
}
#[derive(Serialize, Deserialize)]
pub struct ClientBlockDamage {
    pub damage: f32,
}
impl From<&BlockDamage> for ClientBlockDamage {
    fn from(value: &BlockDamage) -> Self {
        ClientBlockDamage {
            damage: value.damage,
        }
    }
}
