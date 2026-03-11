use std::num::NonZero;

use serde::{Deserialize, Serialize};

use crate::{coord::ChunkOffset, registry::PlantKey};

pub struct BlockComponentStorage<T> {
    pub components: Vec<(ChunkOffset, T)>,
    pub tree: BlockComponentTree,
}
impl<T: Serialize> Serialize for BlockComponentStorage<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.components.serialize(serializer)
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for BlockComponentStorage<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let components = Vec::<(ChunkOffset, T)>::deserialize(deserializer)?;
        Ok(BlockComponentStorage {
            tree: {
                let mut tree = BlockComponentTree::default();
                for (index, (offset, _)) in components.iter().enumerate() {
                    tree.set(*offset, index as u16);
                }
                tree
            },
            components,
        })
    }
}
impl<T> BlockComponentStorage<T> {
    pub fn get(&self, block: ChunkOffset) -> Option<&T> {
        let index = self.tree.get(block)?;
        Some(&self.components.get(index as usize).unwrap().1)
    }
    pub fn get_mut(&mut self, block: ChunkOffset) -> Option<&mut T> {
        let index = self.tree.get(block)?;
        Some(&mut self.components.get_mut(index as usize).unwrap().1)
    }
    pub fn set(&mut self, block: ChunkOffset, data: T) {
        if let Some(existing) = self.get_mut(block) {
            *existing = data;
        } else {
            self.tree.set(block, self.components.len() as u16);
            self.components.push((block, data));
        }
    }
    pub fn get_or_init(&mut self, block: ChunkOffset, init: impl FnOnce() -> T) -> &mut T {
        let index = self.tree.get(block);
        match index {
            Some(index) => &mut self.components.get_mut(index as usize).unwrap().1,
            None => {
                self.set(block, init());
                self.get_mut(block).unwrap()
            }
        }
    }
    pub fn remove(&mut self, block: ChunkOffset) -> Option<T> {
        if let Some(index) = self.tree.remove(block) {
            let (_, removed) = self.components.swap_remove(index as usize);
            if let Some((offset, _)) = self.components.get(index as usize) {
                self.tree.set(*offset, index);
            }
            Some(removed)
        } else {
            None
        }
    }
}
type TreeIndex = Option<NonZero<u16>>;
#[derive(Clone, Default)]
struct BlockComponentTree {
    roots: [TreeIndex; 8],
    blocks: Vec<BlockComponentTreeBlock>,
}
#[derive(Clone)]
struct BlockComponentTreeBlock {
    children: [TreeIndex; 64],
    use_count: u16, //this is probably bad for alignment, maybe split into two vecs?
}
impl BlockComponentTree {
    fn get(&self, block: ChunkOffset) -> Option<u16> {
        let (first, second, third) = Self::chunk_offset_indices(block);
        let first_block = self.roots[first]?.get() as usize - 1;
        let second_block = self.blocks[first_block].children[second]?.get() as usize - 1;
        Some(self.blocks[second_block].children[third]?.get() as u16 - 1)
    }
    fn set(&mut self, block: ChunkOffset, value: u16) {
        let (first, second, third) = Self::chunk_offset_indices(block);
        let first_block = match self.roots[first] {
            Some(first_block) => first_block.get() as usize - 1,
            None => {
                let new_block = self.allocate_block() as usize;
                self.roots[first] = NonZero::new(new_block as u16 + 1);
                new_block
            }
        };
        let second_block = match self.blocks[first_block].children[second] {
            Some(second_block) => second_block.get() as usize - 1,
            None => {
                self.blocks[first_block].use_count += 1;
                let new_block = self.allocate_block() as usize;
                self.blocks[first_block].children[second] = NonZero::new(new_block as u16 + 1);
                new_block
            }
        };
        self.blocks[second_block].use_count += 1;
        self.blocks[second_block].children[third] = NonZero::new(value + 1);
    }
    fn remove(&mut self, block: ChunkOffset) -> Option<u16> {
        let (first, second, third) = Self::chunk_offset_indices(block);
        let first_block = self.roots[first]?.get() as usize - 1;
        let second_block = self.blocks[first_block].children[second]?.get() as usize - 1;
        let previous = self.blocks[second_block].children[third].take()?;
        self.blocks[second_block].use_count -= 1;
        if self.blocks[second_block].use_count == 0 {
            self.blocks[first_block].children[second] = None;
            self.blocks[first_block].use_count -= 1;
            if self.blocks[first_block].use_count == 0 {
                self.roots[first] = None;
            }
        }
        Some(previous.get() - 1)
    }
    fn allocate_block(&mut self) -> u8 {
        if let Some(existing_block) = self.blocks.iter().position(|block| block.use_count == 0) {
            return existing_block as u8;
        }
        self.blocks.push(BlockComponentTreeBlock {
            children: [None; 64],
            use_count: 0,
        });
        self.blocks.len() as u8 - 1
    }
    fn chunk_offset_indices(block: ChunkOffset) -> (usize, usize, usize) {
        fn shift_bits(bits: usize, from: u8, to: u8, mask: usize) -> usize {
            ((bits >> from) & mask) << to
        }
        fn second_level(bits: usize) -> usize {
            shift_bits(bits, 10, 4, 3) | shift_bits(bits, 5, 2, 3) | shift_bits(bits, 0, 0, 3)
        }
        let first = shift_bits(block.0 as usize, 14, 2, 1)
            | shift_bits(block.0 as usize, 9, 1, 1)
            | shift_bits(block.0 as usize, 4, 0, 1);
        let second = second_level(block.0 as usize >> 2);
        let third = second_level(block.0 as usize);
        (first, second, third)
    }
}
impl<T, U> Into<BlockComponentStorage<U>> for &BlockComponentStorage<T>
where
    for<'a> &'a T: Into<U>,
{
    fn into(self) -> BlockComponentStorage<U> {
        BlockComponentStorage {
            tree: self.tree.clone(),
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
            tree: BlockComponentTree::default(),
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
    pub plants: Vec<(PlantKey, u8)>,
}
