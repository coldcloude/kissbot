use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

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

/// 对 map 中已存在键做「写时复制修改」：remove 取回该键的键值对（独占 `Arc<T>`），
/// 用 `op(&key, &mut value)` 原地修改其指向的内容，再把原键（含被修改的键）放回。
/// 键存在、完成修改返回 true；键不存在返回 false（不插入）。
///
/// remove_entry 保留原键对象（op 可直接读取、甚至修改 key），避免键的克隆；
/// remove 得到的 `Arc<T>` 已独占所有权，只要没有其它持有者，
/// `Arc::make_mut` 就原地返回 `&mut T`，不会发生复制。
/// 仅替换 map 的值 Arc（不修改其指向内容）时不要用本函数，直接 remove/insert 或 insert。
pub fn write_key_value<K, Q, T, F>(map: &mut HashMap<K, Arc<T>>, key: &Q, op: F) -> Option<Arc<T>>
where
    K: Borrow<Q> + Eq + Hash,
    Q: Eq + Hash + ?Sized,
    T: Clone,
    F: FnOnce(&K, &mut T),
{
    match map.remove_entry(key) {
        Some((key, mut value)) => {
            op(&key, Arc::make_mut(&mut value));
            map.insert(key, value.clone());
            Some(value)
        }
        None => None,
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

    /// Map key / Set 元素用 `Arc<K>` 时，std 已经提供了内容语义的比较与 `Borrow<K>`：
    /// - `PartialEq` / `Eq` / `Hash` / `Ord` 都转发到 `K`，因此两把内容相同的 `Arc<K>`
    ///   是同一个 key（不是按指针区分）；
    /// - `Borrow<K> for Arc<K>` 使 `get` / `contains` / `remove` 可以直接传 `&K`，
    ///   只有 `insert` / `entry` 需要传 `Arc<K>`。
    /// 因此无需为 `Arc<K>` 自定义这些 trait。`ArcStr` 同理（`Borrow<str>` + 内容比较）。
    #[test]
    fn arc_key_is_content_keyed_and_borrow_queryable() {
        use std::collections::{BTreeMap, HashMap, HashSet};

        use crate::{ChannelKey, ChannelUser, RoleKey};

        let make_key = || Arc::new(RoleKey {
            agent_id: ArcStr::from("a1"),
            role_name: ArcStr::from("dev"),
        });
        let a = make_key();
        let b = make_key();
        assert!(!Arc::ptr_eq(&a, &b), "两次构造是不同分配");
        assert_eq!(a, b, "Arc<K> 按内容比较而非指针");

        // 内容相同的两把 Arc<K> 命中同一个 key
        let mut map: HashMap<Arc<RoleKey>, u32> = HashMap::new();
        map.insert(a, 1);
        map.insert(b, 2);
        assert_eq!(map.len(), 1, "内容相同的 key 覆盖同一项");

        // 查询只需 &RoleKey（Borrow<K> for Arc<K>）
        let probe = RoleKey { agent_id: ArcStr::from("a1"), role_name: ArcStr::from("dev") };
        assert_eq!(map.get(&probe), Some(&2));
        assert!(map.contains_key(&probe));
        map.remove(&probe);
        assert!(map.is_empty());

        // BTreeMap 的 Ord 同样按内容
        let ck = |g: &str| Arc::new(ChannelKey::new("web", "self1", g));
        let mut tree: BTreeMap<Arc<ChannelKey>, u32> = BTreeMap::new();
        tree.insert(ck("g1"), 7);
        tree.insert(ck("g0"), 8);
        assert_eq!(tree.get(&ChannelKey::new("web", "self1", "g1")), Some(&7));
        assert_eq!(tree.first_key_value().unwrap().0.as_ref(), &ChannelKey::new("web", "self1", "g0"));

        // Set 元素：contains 传 &K
        let user = ChannelUser { messenger_id: ArcStr::from("web"), user_id: ArcStr::from("u1") };
        let mut set: HashSet<Arc<ChannelUser>> = HashSet::new();
        set.insert(Arc::new(user.clone()));
        assert!(set.contains(&user));
        assert!(set.remove(&user));

        // ArcStr 作为 key：get 直接传 &str
        let mut smap: HashMap<ArcStr, u32> = HashMap::new();
        smap.insert(ArcStr::from("k"), 1);
        assert_eq!(smap.get("k"), Some(&1));
    }
}
