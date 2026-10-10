use std::sync::Arc;

use arcstr::ArcStr;
use serde::{Deserialize, Serialize};
use serde_json::Value;


/// 内容块注解
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Annotations {
    /// 目标受众（user / assistant，可多个）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<Audience>>,
    /// 重要度（0~1）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(rename = "lastModified", default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<ArcStr>,
}

/// 注解的受众角色
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Audience {
    User,
    Assistant,
}

/// 图标（ResourceLink 可选携带）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Icon {
    pub src: ArcStr,
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<ArcStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<ArcStr>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<IconTheme>,
}

/// 图标主题
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconTheme {
    Light,
    Dark,
}

/// 嵌入资源的内容（按有无 `text` / `blob` 区分两种形状，无 type 标签）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceContents {
    Text {
        uri: ArcStr,
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<ArcStr>,
        text: ArcStr,
        #[serde(rename = "_meta", default, skip_serializing_if = "Value::is_null")]
        meta: Value,
    },
    Blob {
        uri: ArcStr,
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<ArcStr>,
        blob: ArcStr,
        #[serde(rename = "_meta", default, skip_serializing_if = "Value::is_null")]
        meta: Value,
    },
}

/// 工具结果内容块：`type` 即枚举变体（内部标签序列化，`type` 与其他字段平级）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// 文本块
    Text {
        text: ArcStr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Value::is_null")]
        meta: Value,
    },
    /// 图片块（`data` 为 base64）
    Image {
        data: ArcStr,
        #[serde(rename = "mimeType")]
        mime_type: ArcStr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Value::is_null")]
        meta: Value,
    },
    /// 音频块（`data` 为 base64）
    Audio {
        data: ArcStr,
        #[serde(rename = "mimeType")]
        mime_type: ArcStr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Value::is_null")]
        meta: Value,
    },
    /// 资源链接
    ResourceLink {
        uri: ArcStr,
        name: ArcStr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<ArcStr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<ArcStr>,
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<ArcStr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icons: Option<Vec<Arc<Icon>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Value::is_null")]
        meta: Value,
    },
    /// 嵌入资源
    Resource {
        resource: Arc<ResourceContents>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Arc<Annotations>>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Value::is_null")]
        meta: Value,
    },
}

impl ContentBlock {
    /// 文本块快捷构造
    pub fn text(text: ArcStr) -> Self {
        ContentBlock::Text {
            text,
            annotations: None,
            meta: Value::Null,
        }
    }
}
