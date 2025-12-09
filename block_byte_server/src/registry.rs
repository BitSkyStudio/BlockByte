use std::{collections::HashMap, hash::Hash, marker::PhantomData, num::NonZero};

pub struct Key<T>(NonZero<usize>, PhantomData<T>);
impl<T> Clone for Key<T> {
    fn clone(&self) -> Self {
        Self(self.0, PhantomData)
    }
}
impl<T> Copy for Key<T> {}
impl<T> PartialEq for Key<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<T> Eq for Key<T> {}
impl<T> Hash for Key<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

pub struct LoadRegistry<T, D> {
    id_map: HashMap<String, Key<T>>,
    data_list: Vec<D>,
    id_list: Vec<String>,
}
impl<T, D> LoadRegistry<T, D> {
    pub fn new() -> Self {
        LoadRegistry {
            id_map: Default::default(),
            data_list: Default::default(),
            id_list: Default::default(),
        }
    }
    pub fn register(&mut self, id: String, data: D) -> Key<T> {
        self.id_list.push(id.clone());
        self.data_list.push(data);
        let key = Key(
            unsafe { NonZero::new_unchecked(self.id_list.len()) },
            PhantomData,
        );
        self.id_map.insert(id, key);
        key
    }
    pub fn key(&self, id: &str) -> Option<Key<T>> {
        self.id_map.get(id).cloned()
    }
    pub fn load(&self, loader: impl Fn(&D) -> T) -> Registry<T> {
        Registry {
            id_map: self.id_map.clone(),
            data_list: self.data_list.iter().map(|data| loader(data)).collect(),
            id_list: self.id_list.clone(),
        }
    }
}
pub struct Registry<T> {
    id_map: HashMap<String, Key<T>>,
    data_list: Vec<T>,
    id_list: Vec<String>,
}
impl<T> Registry<T> {
    pub fn by_key(&self, key: Key<T>) -> &T {
        &self.data_list[key.0.get() - 1]
    }
}
pub trait RegistryProvider<T> {
    fn get_registry(&self) -> &Registry<T>;
    fn data(&self, key: Key<T>) -> &T {
        self.get_registry().by_key(key)
    }
}
