use std::ops::DerefMut;
use std::{collections::HashMap, ops::Deref};
use std::hash::Hash;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
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
