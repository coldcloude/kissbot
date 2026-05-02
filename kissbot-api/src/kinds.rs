use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;
use dashmap::{DashMap, DashSet};
use serde::{Deserialize, Serialize};

// StringKind trait for string type abstraction
pub trait StringKind: Clone {
    type Type: Clone;
}

// SyncString uses Arc<String>
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SyncString;

impl StringKind for SyncString {
    type Type = Arc<String>;
}

// LocalString uses String
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LocalString;

impl StringKind for LocalString {
    type Type = String;
}

// MapKind trait for map type abstraction
pub trait MapKind: Clone {
    type Map<K, V>: Clone
    where
        K: Eq + Hash + Clone,
        V: Clone;
}

// SyncMap uses DashMap
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SyncMap;

impl MapKind for SyncMap {
    type Map<K, V> = Arc<DashMap<K, V>>
    where
        K: Eq + Hash + Clone,
        V: Clone;
}

// LocalMap uses HashMap
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LocalMap;

impl MapKind for LocalMap {
    type Map<K, V> = HashMap<K, V>
    where
        K: Eq + Hash + Clone,
        V: Clone;
}

// SetKind trait for set type abstraction
pub trait SetKind: Clone {
    type Set<T>: Clone
    where
        T: Eq + Hash + Clone;
}

// SyncSet uses DashSet
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SyncSet;

impl SetKind for SyncSet {
    type Set<T> = Arc<DashSet<T>>
    where
        T: Eq + Hash + Clone;
}

// LocalSet uses HashSet
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LocalSet;

impl SetKind for LocalSet {
    type Set<T> = HashSet<T>
    where
        T: Eq + Hash + Clone;
}
