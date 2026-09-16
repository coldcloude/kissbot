use std::{collections::HashSet, sync::Arc};

use kissbot_api::{ChannelUser, RESERVED_AGENT_ID};
use serde::{Deserialize, Serialize};

/// ChannelConfig.agent_id 缺省值：保留 agent（"0"）
fn default_agent_id() -> Arc<String> {
    Arc::new(RESERVED_AGENT_ID.to_string())
}

/// ChannelConfig.agent_id 反序列化：空串自动归一化为保留 agent（"0"），非空原样
fn deserialize_agent_id<'de, D>(d: D) -> std::result::Result<Arc<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    Ok(Arc::new(if s.is_empty() { RESERVED_AGENT_ID.to_string() } else { s }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub channel_id: Arc<String>,         // agent 内部唯一标识，与消息方 messenger 无关
    pub ws_url: Arc<String>,
    pub admins: Arc<HashSet<ChannelUser>>,
    /// 多绑定身份（bind 追加去重，unbind 带 ChannelUser 移除；HashSet 天然去重 + O(1) contains）
    pub bind_users: Arc<HashSet<ChannelUser>>,
    /// 绑定的 agent_id（UUID；缺省/空 = 保留 agent = "0"，建会话用默认系统提示词，不调 memory-ego）
    #[serde(default = "default_agent_id", deserialize_with = "deserialize_agent_id")]
    pub agent_id: Arc<String>,
    #[serde(default)]
    pub role_name: Arc<String>,
    /// 是否启用（连接由 enabled 控制）
    pub enabled: bool,
}
