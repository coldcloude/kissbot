use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

pub trait ArcUnwrapOrClone<T> {
    fn unwrap_or_clone(self) -> T
    where
        T: Clone;
}

impl<T> ArcUnwrapOrClone<T> for Arc<T> {
    fn unwrap_or_clone(self) -> T
    where
        T: Clone,
    {
        Arc::try_unwrap(self).unwrap_or_else(|arc| (*arc).clone())
    }
}

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

/// 手动深复制 HashMap<K, ArcSwap<V>>，逐个 load().clone() 后 ArcSwap::from 重建。
/// 因 ArcSwap 不实现 Clone，无法直接 .clone() HashMap。
pub fn clone_arcswap_map<K, V>(map: &HashMap<K, ArcSwap<V>>) -> HashMap<K, ArcSwap<V>>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    map.iter().map(|(k, v)| {
        (k.clone(), ArcSwap::from(v.load().clone()))
    }).collect()
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
