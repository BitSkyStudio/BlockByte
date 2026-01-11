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
}
impl<T, U> Into<BlockComponentStorage<U>> for &BlockComponentStorage<T>
where
    for<'a> &'a T: Into<U>,
{
    fn into(self) -> BlockComponentStorage<U> {
        BlockComponentStorage {
            components: self
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
macro_rules! create_client_chunk_block_components{
    ($($type:tt, $id:ident);*) => {
        #[derive(Serialize, Deserialize)]
        pub struct ClientChunkBlockComponents{
            $(pub $id: BlockComponentStorage<$type>,)*
        }
        #[derive(Serialize, Deserialize)]
        pub enum ClientBlockComponentUpdate{
            $($type(Option<$type>),)*
        }
        $(
            impl From<Option<$type>> for ClientBlockComponentUpdate{
                fn from(value: Option<$type>) -> ClientBlockComponentUpdate{
                    ClientBlockComponentUpdate::$type(value)
                }
            }
        )*
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

create_client_chunk_block_components!(ClientBlockDamage, damage; ClientBlockPlants, plant);

#[derive(Serialize, Deserialize)]
pub struct ClientBlockDamage {
    pub damage: f32,
}
#[derive(Serialize, Deserialize)]
pub struct ClientBlockPlants {
    pub plants: Vec<PlantKey>,
}
