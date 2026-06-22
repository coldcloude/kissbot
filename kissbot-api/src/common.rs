use std::sync::Arc;

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
