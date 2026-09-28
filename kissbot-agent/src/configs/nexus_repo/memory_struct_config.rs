use arcstr::ArcStr;
use serde::{Deserialize, Serialize};


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStructConfig {
    pub name: ArcStr,
    pub url: ArcStr,
}
