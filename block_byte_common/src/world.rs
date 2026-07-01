use std::{cell::RefCell, num::NonZero, sync::Mutex};

use num_integer::Integer;
use priority_queue::PriorityQueue;
use serde::{Deserialize, Serialize, ser::SerializeTuple};
use smallvec::SmallVec;

use crate::{coord::ChunkOffset, registry::PlantKey};

pub struct BlockComponentStorage<T> {
    pub components: Vec<(ChunkOffset, T)>,
    pub tree: BlockComponentTree,
    pub tick_list: Mutex<BlockTickList>, //todo: change to refcell
}
impl<T: Serialize> Serialize for BlockComponentStorage<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut tup = serializer.serialize_tuple(2)?;
        tup.serialize_element(&self.components)?;
        tup.serialize_element(&*self.tick_list.lock().unwrap())?;
        tup.end()
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for BlockComponentStorage<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (components, tick_list) =
            <(Vec<(ChunkOffset, T)>, BlockTickList)>::deserialize(deserializer)?;
        Ok(BlockComponentStorage {
            tree: {
                let mut tree = BlockComponentTree::default();
                for (index, (offset, _)) in components.iter().enumerate() {
                    tree.set(*offset, index as u16);
                }
                tree
            },
            components,
            tick_list: Mutex::new(tick_list),
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
            self.tick_list
                .lock()
                .unwrap()
                .remove(block, index as usize, self.components.len());
            let (_, removed) = self.components.swap_remove(index as usize);
            if let Some((offset, _)) = self.components.get(index as usize) {
                self.tree.set(*offset, index);
            }
            Some(removed)
        } else {
            None
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (ChunkOffset, &T)> {
        self.components.iter().map(|(offset, data)| (*offset, data))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (ChunkOffset, &mut T)> {
        self.components
            .iter_mut()
            .map(|(offset, data)| (*offset, data))
    }
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}
type TreeIndex = Option<NonZero<u16>>;
#[derive(Clone, Default)]
pub struct BlockComponentTree {
    roots: [TreeIndex; 8],
    blocks: Vec<BlockComponentTreeBlock>,
}
#[derive(Clone)]
struct BlockComponentTreeBlock {
    children: [TreeIndex; 64],
    use_count: u16, //this is probably bad for alignment, maybe split into two vecs?
}
impl BlockComponentTree {
    pub fn get(&self, block: ChunkOffset) -> Option<u16> {
        let (first, second, third) = Self::chunk_offset_indices(block);
        let first_block = self.roots[first]?.get() as usize - 1;
        let second_block = self.blocks[first_block].children[second]?.get() as usize - 1;
        Some(self.blocks[second_block].children[third]?.get() as u16 - 1)
    }
    pub fn set(&mut self, block: ChunkOffset, value: u16) {
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
    pub fn remove(&mut self, block: ChunkOffset) -> Option<u16> {
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
impl<T> BlockComponentStorage<T> {
    pub fn map<U>(&self, mut mapper: impl FnMut(&T) -> U) -> BlockComponentStorage<U> {
        BlockComponentStorage {
            tree: self.tree.clone(),
            components: self
                .components
                .iter()
                .map(|(offset, data)| (*offset, mapper(data)))
                .collect(),
            tick_list: Mutex::new(self.tick_list.lock().unwrap().clone()),
        }
    }
}
impl<T> Default for BlockComponentStorage<T> {
    fn default() -> Self {
        BlockComponentStorage {
            components: Vec::new(),
            tree: BlockComponentTree::default(),
            tick_list: Mutex::new(BlockTickList::default()),
        }
    }
}
macro_rules! create_client_chunk_block_components{
    ($($type:tt, $id:ident);*) => {
        #[derive(Serialize, Deserialize)]
        pub struct ClientChunkBlockComponents{
            $(pub $id: BlockComponentStorage<$type>,)*
        }
        $(
            impl ComponentTypeAccess<$type> for ClientChunkBlockComponents{
                type Item = BlockComponentStorage<$type>;
                fn get_component_type(&self) -> &Self::Item{
                    &self.$id
                }
                fn get_component_type_mut(&mut self) -> &mut Self::Item{
                    &mut self.$id
                }
            }
        )*
        #[derive(Serialize, Deserialize)]
        pub enum ClientBlockComponentUpdate{
            $($type(Option<$type>),)*
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

create_client_chunk_block_components!(ClientBlockDamage, damage; ClientBlockPlants, plant; ClientBlockMachine, machine);

pub trait ComponentTypeAccess<T> {
    type Item;
    fn get_component_type(&self) -> &Self::Item;
    fn get_component_type_mut(&mut self) -> &mut Self::Item;
}

#[derive(Serialize, Deserialize)]
pub struct ClientBlockDamage {
    pub damage: f32,
}
#[derive(Serialize, Deserialize)]
pub struct ClientBlockPlants {
    pub plants: SmallVec<[(PlantKey, u8); 1]>,
}
#[derive(Serialize, Deserialize)]
pub struct ClientBlockMachine {
    pub animation: u16,
    pub animation_start_time: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct BlockTickList {
    pub tick_mask: Vec<u64>,
    pub ticking_count: usize,
    pub wakeup_timer: PriorityQueue<ChunkOffset, u64>,
}
impl BlockTickList {
    pub fn set_ticking(&mut self, id: usize, ticking: bool) {
        let (index, shift) = id.div_mod_floor(&(u64::BITS as usize));
        if index >= self.tick_mask.len() {
            self.tick_mask.resize(index + 1, 0);
        }
        let previous = ((self.tick_mask[index] >> shift) & 1) != 0;
        match (previous, ticking) {
            (false, true) => {
                self.ticking_count += 1;
            }
            (true, false) => {
                self.ticking_count -= 1;
            }
            _ => {}
        }
        if ticking {
            self.tick_mask[index] |= 1 << shift;
        } else {
            self.tick_mask[index] &= !(1 << shift);
        }
    }
    pub fn get_ticking(&mut self, id: usize) -> bool {
        let (index, shift) = id.div_mod_floor(&(u64::BITS as usize));
        if index >= self.tick_mask.len() {
            return false;
        }
        ((self.tick_mask[index] >> shift) & 1) != 0
    }
    pub fn remove(&mut self, block: ChunkOffset, remove_index: usize, components_length: usize) {
        self.wakeup_timer.remove(&block);
        let last_index = components_length - 1;
        if remove_index != last_index {
            let moved_ticking = self.get_ticking(last_index);
            self.set_ticking(remove_index, moved_ticking);
        }
        self.set_ticking(last_index, false);
    }
    pub fn process_timer(&mut self, current_ticks_passed: u64, tree: &BlockComponentTree) {
        loop {
            let Some(head_timer) = self.wakeup_timer.peek().map(|(_, t)| *t) else {
                return;
            };
            if head_timer <= current_ticks_passed {
                let (block, _) = self.wakeup_timer.pop().unwrap();
                self.set_ticking(tree.get(block).unwrap() as usize, true);
            } else {
                return;
            }
        }
    }
    pub fn schedule_wakeup(&mut self, block: ChunkOffset, at: u64) {
        self.wakeup_timer.push(block, at);
    }
    pub fn get_scheduled_wakeup_in(&self, block: ChunkOffset) -> Option<u64> {
        self.wakeup_timer.get_priority(&block).cloned()
    }
    pub fn start_index(&self) -> BlockTickIndex {
        BlockTickIndex {
            big_index: 0,
            internal_index: 0,
            current_bits: if self.tick_mask.is_empty() {
                0
            } else {
                self.tick_mask[0]
            },
        }
    }
    pub fn next_index(&self, index: &mut BlockTickIndex) -> Option<usize> {
        loop {
            let next = index.current_bits.trailing_zeros() as usize + 1;
            index.internal_index += next;
            if index.internal_index > 64 {
                index.big_index += 1;
                index.internal_index = 0;
                index.current_bits = 0; //not needed, just in case we call this after iteration is done(to be consistent with the iterator api)
                if index.big_index >= self.tick_mask.len() {
                    return None;
                }
                index.current_bits = self.tick_mask[index.big_index];
            } else {
                index.current_bits >>= next;
                return Some(index.big_index * 64 + index.internal_index - 1);
            }
        }
    }
}
pub struct BlockTickIndex {
    big_index: usize,
    internal_index: usize,
    current_bits: u64,
}
