use std::sync::Arc;

use serde::{Deserialize, Serialize};


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStructConfig {
    pub name: Arc<String>,
    pub url: Arc<String>,
}
