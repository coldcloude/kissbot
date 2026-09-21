//! 内置工具注册表：按 group 名给出内置工具（元数据 + 原生实现）

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use super::Group;
use crate::types::Result;

/// 内置工具的参数 JSON Schema（固定结构，不做手拼 JSON）
#[derive(Debug, Serialize)]
pub struct ObjectSchema {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub properties: BTreeMap<&'static str, PropertySchema>,
    pub required: &'static [&'static str],
}

impl ObjectSchema {
    pub fn value(&self) -> Value {
        serde_json::to_value(self).expect("内置工具参数 schema 应可序列化")
    }
}

/// 参数中单个属性的 JSON Schema
#[derive(Debug, Serialize)]
pub struct PropertySchema {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub description: &'static str,
}

/// 内置注册表：按 group 名给出该 group 的内置工具（元数据 + 原生实现）
/// 配置显式声明对应 group 名时才注册；未声明则不注册（内置工具也须显式配置 group 才可用）
pub fn builtin_group(name: &str) -> Result<Option<Group>> {
    match name {
        "filesystem" => Ok(Some(super::filesystem::group(name)?)),
        _ => Ok(None),
    }
}
