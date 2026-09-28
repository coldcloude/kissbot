use std::collections::hash_map::Entry;
use std::hash::Hash;
use std::ops::DerefMut;
use std::sync::Arc;
use std::{collections::HashMap, ops::Deref};

use arc_swap::ArcSwap;
use arcstr::ArcStr;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::RwLock;

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<ArcStr>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(message: impl Into<ArcStr>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArcSwapHashMap<K, T>
where
    K: Eq + Hash + Clone + 'static,
{
    base: HashMap<K, ArcSwap<T>>
}

impl<K, T> ArcSwapHashMap<K, T>
where
    K: Eq + Hash + Clone + 'static,
{
    pub fn new() -> Self {
        Self {
            base: HashMap::new(),
        }
    }
}

impl<K, T> Default for ArcSwapHashMap<K, T>
where
    K: Eq + Hash + Clone + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, T> From<HashMap<K, ArcSwap<T>>> for ArcSwapHashMap<K, T>
where
    K: Eq + Hash + Clone + 'static,
{
    fn from(base: HashMap<K, ArcSwap<T>>) -> Self {
        ArcSwapHashMap { base }
    }
}

// 实现 Clone
impl<K, T> Clone for ArcSwapHashMap<K, T>
where
    K: Eq + Hash + Clone + 'static,
{
    fn clone(&self) -> Self {
        let mut new_map = HashMap::new();
        for (k, arc_swap) in self.base.iter() {
            let inner = arc_swap.load_full();
            new_map.insert(k.clone(), ArcSwap::new(inner));
        }
        ArcSwapHashMap::from(new_map)
    }
}

impl<K, T> Deref for ArcSwapHashMap<K, T>
where
    K: Eq + Hash + Clone + 'static,
{
    type Target = HashMap<K, ArcSwap<T>>;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl<K, T> DerefMut for ArcSwapHashMap<K, T>
where
    K: Eq + Hash + Clone + 'static,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

impl<K, T> ArcSwapHashMap<K, T>
where
    K: Eq + Hash + Clone + 'static,
{
    pub fn replace_exist(&self, key: &K, value: Arc<T>) -> Result<(),()> {
        if let Some(arc_swap) = self.base.get(key) {
            arc_swap.store(value);
            Ok(())
        } else {
            Err(())
        }
    }

    pub fn replace(&mut self, key: &K, value: Arc<T>) {
        match self.base.entry(key.clone()) {
            Entry::Occupied(entry) => {
                entry.get().store(value);
            }
            Entry::Vacant(entry) => {
                entry.insert(ArcSwap::new(value));
            }
        }
    }
}

pub struct AsyncCacheFactory<V> {
    map: RwLock<HashMap<ArcStr,Arc<V>>>,
}

impl<V> AsyncCacheFactory<V> {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
        }
    }

    pub async fn get_or_create<F>(&self, name: impl Into<ArcStr>, creator: F) -> Arc<V>
    where
        F: FnOnce() -> V
    {
        let name = name.into();
        // 先尝试读
        {
            let map = self.map.read().await;
            if let Some(provider) = map.get(name.as_str()) {
                return provider.clone();
            }
        }

        let mut map = self.map.write().await;
        map.entry(name).or_insert_with(|| {
            Arc::new(creator())
        }).clone()
    }
}

// ========== 配置继承与合并约定 ==========

/// 自合并配置项：可被同类型逐字段覆盖（配置继承链的每一级都用它承载）
pub trait MergeSelf: Default + MergeBy<Self> {}

/// 从「可继承配置」合成「有效配置」（叶子配置 → 运行期有效配置）
pub trait MergeEffectiveConfig<E> {
    fn get_effective_config(&self) -> E;
}

/// 逐字段合并配置（Option 覆盖语义：None 表示沿用已有值）
pub trait MergeBy<C>: Serialize + DeserializeOwned {
    fn merge(&mut self, other: &C);
}

pub trait ArcField<T> {
    fn as_ref(&self) -> &T;
    fn as_mut(&mut self) -> &mut T;
}

pub trait OptionArcField<T> {
    fn as_deref(&self) -> Option<&T>;
    fn as_deref_mut(&mut self) -> Option<&mut T>;
    fn insert(&mut self, value: Arc<T>);
    fn replace(&mut self, value: Arc<T>) -> Option<Arc<T>>;
    fn remove(&mut self);
    fn take(&mut self) -> Option<Arc<T>>;
    fn get(&self) -> Option<Arc<T>>;
}

/// 为「Arc 字段型配置表」实现 ArcField（字段读写转发到内部字段）
#[macro_export]
macro_rules! impl_arc_field {
    ($ty:ty => $field:ident, $owner:ty) => {
        impl ArcField<$ty> for $owner {
            fn as_ref(&self) -> &$ty {
                self.$field.as_ref()
            }
            fn as_mut(&mut self) -> &mut $ty {
                Arc::make_mut(&mut self.$field)
            }
        }
    };
}

/// 为「Option<Arc> 字段型配置表」实现 OptionArcField（写时复制）
#[macro_export]
macro_rules! impl_option_arc_field {
    ($ty:ty => $field:ident, $owner:ty) => {
        impl OptionArcField<$ty> for $owner {
            fn as_deref(&self) -> Option<&$ty> {
                self.$field.as_deref()
            }
            fn as_deref_mut(&mut self) -> Option<&mut $ty> {
                self.$field.as_mut().map(|arc| Arc::make_mut(arc))
            }
            fn insert(&mut self, value: Arc<$ty>) {
                self.$field = Some(value);
            }
            fn replace(&mut self, value: Arc<$ty>) -> Option<Arc<$ty>> {
                self.$field.replace(value)
            }
            fn remove(&mut self) {
                self.$field = None;
            }
            fn take(&mut self) -> Option<Arc<$ty>> {
                self.$field.take()
            }
            fn get(&self) -> Option<Arc<$ty>> {
                self.$field.clone()
            }
        }
    };
}

/// 为「Arc 字段型配置表」生成泛型读写方法（get / get_mut）
#[macro_export]
macro_rules! impl_arc_field_map {
    ($map:ty) => {
        impl $map {
            fn get<T>(&self) -> &T
            where
                Self: ArcField<T>,
            {
                <Self as ArcField<T>>::as_ref(self)
            }

            fn get_mut<T>(&mut self) -> Option<&mut T>
            where
                Self: ArcField<T>,
            {
                <Self as ArcField<T>>::as_mut(self)
            }
        }
    };
}

/// 为「Option<Arc> 字段型配置表」生成泛型读写方法（get_deref / insert / take / ...）
#[macro_export]
macro_rules! impl_option_arc_field_map {
    ($map:ty) => {
        impl $map {
            pub fn get_deref<T>(&self) -> Option<&T>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::as_deref(self)
            }

            pub fn get_deref_mut<T>(&mut self) -> Option<&mut T>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::as_deref_mut(self)
            }

            pub fn insert<T>(&mut self, value: Arc<T>)
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::insert(self, value)
            }

            pub fn replace<T>(&mut self, value: Arc<T>) -> Option<Arc<T>>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::replace(self, value)
            }

            pub fn remove<T>(&mut self)
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::remove(self)
            }

            pub fn take<T>(&mut self) -> Option<Arc<T>>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::take(self)
            }

            pub fn get<T>(&self) -> Option<Arc<T>>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::get(self)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_response_success() {
        let resp = ApiResponse::success("hello");
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["data"], "hello");
        assert_eq!(json["error"], serde_json::Value::Null);

        let deserialized: ApiResponse<String> = serde_json::from_value(json).unwrap();
        assert!(deserialized.success);
        assert_eq!(deserialized.data.unwrap(), "hello");
    }

    #[test]
    fn test_api_response_error() {
        let resp: ApiResponse<()> = ApiResponse::error("something went wrong".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], false);
        assert_eq!(json["data"], serde_json::Value::Null);
        assert_eq!(json["error"], "something went wrong");

        let deserialized: ApiResponse<()> = serde_json::from_value(json).unwrap();
        assert!(!deserialized.success);
        assert_eq!(deserialized.error.unwrap(), "something went wrong");
    }
}
