// ========== context 配置段（agent→role 三层继承） ==========

use std::collections::HashSet;
use std::sync::Arc;

use kissbot_api::ArcSwapHashMap;
use serde::{Deserialize, Serialize};

// ========== 全局默认值 ==========

/// channel 合批最小间隔（秒）
pub const DEFAULT_CHANNEL_BATCH_INTERVAL_SECS: u64 = 3;
/// 记忆提取时间窗（秒）
pub const DEFAULT_MEMORY_TIME_SECS: u64 = 3600;
/// 记忆提取条数
pub const DEFAULT_MEMORY_COUNT: usize = 50;
/// event 压缩指令默认模板
pub const DEFAULT_COMPRESS_PROMPT: &str = "请用简洁的语言总结以上对话的关键信息，保留重要细节、结论与未完成事项，供后续对话参考。";

// ========== 配置结构 ==========

/// agent 级 context 配置（key = agent_name，覆盖全局默认；类似 ProviderConfig 的 default_*）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContextConfig {
    pub default_channel_batch_interval_secs: u64,
    pub default_memory_time_secs: u64,
    pub default_memory_count: usize,
    pub default_compress_prompt: Arc<String>,
    /// 启用的 station_id 集合（Set 形式）
    pub default_stations: Arc<HashSet<String>>,
    /// key = role_name
    pub roles: Arc<ArcSwapHashMap<String, RoleContextConfig>>,
}

/// role 级 context 配置（可选覆盖 agent 默认；类似 ModelConfig 的 Option 继承）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleContextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_batch_interval_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_time_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compress_prompt: Option<Arc<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stations: Option<Arc<HashSet<String>>>,
}

/// 合并后的有效配置（现场合成，不持久化）
#[derive(Debug, Clone)]
pub struct EffectiveContextConfig {
    pub channel_batch_interval_secs: u64,
    pub memory_time_secs: u64,
    pub memory_count: usize,
    pub compress_prompt: String,
    pub stations: HashSet<String>,
}

/// 三层合并：全局默认 ← agent ← role（role Some 覆盖 agent；无 agent 用全局默认）
/// role 只可能来自 agent.roles（role 无 agent 时不可达），故 agent 为 None 时直接返回全局默认
pub fn merge_context_config(
    agent: Option<&AgentContextConfig>,
    role: Option<&RoleContextConfig>,
) -> EffectiveContextConfig {
    let Some(a) = agent else {
        return EffectiveContextConfig {
            channel_batch_interval_secs: DEFAULT_CHANNEL_BATCH_INTERVAL_SECS,
            memory_time_secs: DEFAULT_MEMORY_TIME_SECS,
            memory_count: DEFAULT_MEMORY_COUNT,
            compress_prompt: DEFAULT_COMPRESS_PROMPT.to_string(),
            stations: HashSet::new(),
        };
    };
    EffectiveContextConfig {
        channel_batch_interval_secs: role.and_then(|x| x.channel_batch_interval_secs)
            .unwrap_or(a.default_channel_batch_interval_secs),
        memory_time_secs: role.and_then(|x| x.memory_time_secs)
            .unwrap_or(a.default_memory_time_secs),
        memory_count: role.and_then(|x| x.memory_count)
            .unwrap_or(a.default_memory_count),
        compress_prompt: role.and_then(|x| x.compress_prompt.as_ref().map(|s| s.to_string()))
            .unwrap_or_else(|| a.default_compress_prompt.to_string()),
        stations: role.and_then(|x| x.stations.clone())
            .map(|s| (*s).clone())
            .unwrap_or_else(|| (*a.default_stations).clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_none_uses_globals() {
        let eff = merge_context_config(None, None);
        assert_eq!(eff.channel_batch_interval_secs, DEFAULT_CHANNEL_BATCH_INTERVAL_SECS);
        assert_eq!(eff.memory_time_secs, DEFAULT_MEMORY_TIME_SECS);
        assert_eq!(eff.memory_count, DEFAULT_MEMORY_COUNT);
        assert!(eff.stations.is_empty());
    }

    #[test]
    fn merge_agent_then_role_override() {
        let agent = AgentContextConfig {
            default_channel_batch_interval_secs: 5,
            default_memory_time_secs: 7200,
            default_memory_count: 100,
            default_compress_prompt: Arc::new("agent模板".into()),
            default_stations: Arc::new(["s1".into()].into_iter().collect()),
            roles: Arc::new(ArcSwapHashMap::new()),
        };
        let role = RoleContextConfig {
            channel_batch_interval_secs: Some(7),
            memory_time_secs: None,
            memory_count: None,
            compress_prompt: None,
            stations: None,
        };
        let eff = merge_context_config(Some(&agent), Some(&role));
        assert_eq!(eff.channel_batch_interval_secs, 7, "role 覆盖 agent");
        assert_eq!(eff.memory_time_secs, 7200, "role 未配继承 agent");
        assert_eq!(eff.memory_count, 100);
        assert_eq!(eff.compress_prompt, "agent模板");
        assert!(eff.stations.contains("s1"));
    }

    #[test]
    fn role_stations_override_agent() {
        let agent = AgentContextConfig {
            default_channel_batch_interval_secs: 3,
            default_memory_time_secs: 3600,
            default_memory_count: 50,
            default_compress_prompt: Arc::new("t".into()),
            default_stations: Arc::new(["s1".into()].into_iter().collect()),
            roles: Arc::new(ArcSwapHashMap::new()),
        };
        let role = RoleContextConfig {
            channel_batch_interval_secs: None,
            memory_time_secs: None,
            memory_count: None,
            compress_prompt: None,
            stations: Some(Arc::new(["s2".into()].into_iter().collect())),
        };
        let eff = merge_context_config(Some(&agent), Some(&role));
        assert!(eff.stations.contains("s2") && !eff.stations.contains("s1"), "role stations 整体覆盖");
    }

    #[test]
    fn agent_role_config_serde_roundtrip() {
        let agent = AgentContextConfig {
            default_channel_batch_interval_secs: 3,
            default_memory_time_secs: 3600,
            default_memory_count: 50,
            default_compress_prompt: Arc::new("t".into()),
            default_stations: Arc::new(HashSet::new()),
            roles: Arc::new(ArcSwapHashMap::new()),
        };
        let json = serde_json::to_string(&agent).unwrap();
        let back: AgentContextConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.default_memory_count, 50);
    }
}
