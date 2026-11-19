use std::{collections::HashSet, sync::Arc};

use arcstr::ArcStr;
use kissbot_api::{ChannelUser, MergeBy, MergeEffectiveConfig, MergeSelf};
use serde::{Deserialize, Serialize};

// ============ pipeline =================

/// pipeline 实现配置（顶层 pipelines 段的值，key 为 RoleKey）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineConfig {
    pub pipeline_name: ArcStr,
}

// ============ 压缩设置 ===================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompressConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_prompt: Option<ArcStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_threshold: Option<f64>,
}

impl MergeBy<CompressConfig> for CompressConfig {
    fn merge(&mut self, other: &CompressConfig) {
        if let Some(compress_prompt) = other.compress_prompt.as_ref() {
            self.compress_prompt = Some(compress_prompt.clone());
        }
        if let Some(compress_threshold) = other.compress_threshold {
            self.compress_threshold = Some(compress_threshold);
        }
    }
}

impl MergeSelf for CompressConfig {}

/// 压缩默认模板
pub const DEFAULT_COMPRESS_PROMPT: ArcStr = arcstr::literal!("请用简洁的语言总结以上对话的关键信息，保留重要细节、结论与未完成事项，供后续对话参考。");

/// 压缩默认阈值
pub const DEFAULT_COMPRESS_THRESHOLD: f64 = 0.8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveCompressConfig {
    pub compress_prompt: ArcStr,
    pub compress_threshold: f64,
}

impl EffectiveCompressConfig {
    pub fn new() -> Self {
        Self {
            compress_prompt: DEFAULT_COMPRESS_PROMPT.clone(),
            compress_threshold: DEFAULT_COMPRESS_THRESHOLD,
        }
    }
}

impl MergeBy<CompressConfig> for EffectiveCompressConfig {
    fn merge(&mut self, other: &CompressConfig) {
        if let Some(compress_prompt) = other.compress_prompt.as_ref() {
            self.compress_prompt = compress_prompt.clone();
        }
        if let Some(compress_threshold) = other.compress_threshold {
            self.compress_threshold = compress_threshold;
        }
    }
}

impl MergeEffectiveConfig<EffectiveCompressConfig> for CompressConfig {
    fn get_effective_config(&self) -> EffectiveCompressConfig {
        let mut result = EffectiveCompressConfig::new();
        result.merge(self);
        result
    }
}

// ========== memory 回溯配置 ===============

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryRecoverConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_time_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_count: Option<usize>,
}

impl MergeBy<MemoryRecoverConfig> for MemoryRecoverConfig {
    fn merge(&mut self, other: &Self) {
        if let Some(memory_time_secs) = other.memory_time_secs {
            self.memory_time_secs = Some(memory_time_secs);
        }
        if let Some(memory_count) = other.memory_count {
            self.memory_count = Some(memory_count);
        }
    }
}

impl MergeSelf for MemoryRecoverConfig{}

// ---- 全局默认值 ----

/// 记忆提取时间窗（秒）
pub const DEFAULT_MEMORY_TIME_SECS: u64 = 3600;
/// 记忆提取条数
pub const DEFAULT_MEMORY_COUNT: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveMemoryRecoverConfig {
    pub memory_time_secs: u64,
    pub memory_count: usize,
}

impl EffectiveMemoryRecoverConfig {
    pub fn new() -> Self {
        Self {
            memory_time_secs: DEFAULT_MEMORY_TIME_SECS,
            memory_count: DEFAULT_MEMORY_COUNT,
        }
    }
}

impl MergeBy<MemoryRecoverConfig> for EffectiveMemoryRecoverConfig {
    fn merge(&mut self, other: &MemoryRecoverConfig) {
        if let Some(memory_time_secs) = other.memory_time_secs {
            self.memory_time_secs = memory_time_secs;
        }
        if let Some(memory_count) = other.memory_count {
            self.memory_count = memory_count;
        }
    }
}

impl MergeEffectiveConfig<EffectiveMemoryRecoverConfig> for MemoryRecoverConfig {
    fn get_effective_config(&self) -> EffectiveMemoryRecoverConfig {
        let mut result = EffectiveMemoryRecoverConfig::new();
        result.merge(self);
        result
    }
}

// ========== Context 配置（agent→role 三层继承） ==========

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelBatchConfig {
    pub channel_batch_interval_secs: u64,
}

impl MergeBy<ChannelBatchConfig> for ChannelBatchConfig {
    fn merge(&mut self, other: &Self) {
        if other.channel_batch_interval_secs != 0 {
            self.channel_batch_interval_secs = other.channel_batch_interval_secs;
        }
    }
}

impl MergeSelf for ChannelBatchConfig {}

/// channel 合批最小间隔默认值（秒）
pub const DEFAULT_CHANNEL_BATCH_INTERVAL_SECS: u64 = 3;

impl ChannelBatchConfig {
    pub fn new() -> Self {
        Self {
            channel_batch_interval_secs: DEFAULT_CHANNEL_BATCH_INTERVAL_SECS,
        }
    }
}

impl MergeEffectiveConfig<ChannelBatchConfig> for ChannelBatchConfig {
    fn get_effective_config(&self) -> ChannelBatchConfig {
        let mut result = ChannelBatchConfig::new();
        result.merge(self);
        result
    }
}

/// out_channel 配置（回复通道；可作 (agent, role) 级初始值，运行期由 pipeline 命令写入 session 配置；
/// channel_id 为发送目标）。由 /bind-outgoing 在来源 channel 构造（channel_id = 来源 channel），
/// Agentic Loop 回复经此发送
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutChannel {
    pub channel_id: ArcStr,
    pub user: Arc<ChannelUser>,
    pub group_id: ArcStr,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutChannelConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_channel: Option<Arc<OutChannel>>,
}

impl MergeBy<OutChannelConfig> for OutChannelConfig {
    fn merge(&mut self, other: &Self) {
        self.out_channel = other.out_channel.clone();
    }
}

impl MergeSelf for OutChannelConfig {}

impl MergeEffectiveConfig<OutChannelConfig> for OutChannelConfig {
    fn get_effective_config(&self) -> OutChannelConfig {
        self.clone()
    }
}

// ============ input channel =================

/// pipeline 的输入身份：某 channel 上以 messenger_id + user_id 接收消息，路由到引用它的 pipeline
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InputChannel {
    pub channel_id: ArcStr,
    pub messenger_id: ArcStr,
    pub user_id: ArcStr,
}

/// input channel 段（与 out_channel 同级）；一个输入身份只能绑定一条 pipeline，重复绑定由运行时记 error 跳过
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputChannelsConfig {
    #[serde(default)]
    pub input_channels: Arc<Vec<Arc<InputChannel>>>,
}

impl MergeBy<InputChannelsConfig> for InputChannelsConfig {
    fn merge(&mut self, other: &Self) {
        // 整体替换而非叠加：始终取 other 的列表；other 未配置时为空集合
        self.input_channels = other.input_channels.clone();
    }
}

impl MergeSelf for InputChannelsConfig {}

// ============= group ====================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupSetConfig {
    /// 会话启用的 group 名集合
    pub group_set: Arc<HashSet<ArcStr>>,
}

impl MergeBy<GroupSetConfig> for GroupSetConfig {
    fn merge(&mut self, other: &Self) {
        let group_set = Arc::make_mut(&mut self.group_set);
        for group in other.group_set.iter() {
            group_set.insert(group.clone());
        }
    }
}

impl MergeSelf for GroupSetConfig {}

impl MergeEffectiveConfig<GroupSetConfig> for GroupSetConfig {
    fn get_effective_config(&self) -> GroupSetConfig {
        self.clone()
    }
}

// ========== 记忆 pipeline 配置 ==========

/// 默认记忆 pipeline（default_memory）的记忆读取开关：
/// 只存 agent/role 可继承值与会话级覆盖，建立 session 时导入；事件模式忽略该开关。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryPipelineConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_memory: Option<bool>,
}

impl MergeBy<MemoryPipelineConfig> for MemoryPipelineConfig {
    fn merge(&mut self, other: &Self) {
        if let Some(load_memory) = other.load_memory {
            self.load_memory = Some(load_memory);
        }
    }
}

impl MergeSelf for MemoryPipelineConfig {}

/// 默认读取记忆
pub const DEFAULT_LOAD_MEMORY: bool = true;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveMemoryPipelineConfig {
    pub load_memory: bool,
}

impl EffectiveMemoryPipelineConfig {
    pub fn new() -> Self {
        Self { load_memory: DEFAULT_LOAD_MEMORY }
    }
}

impl Default for EffectiveMemoryPipelineConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl MergeBy<MemoryPipelineConfig> for EffectiveMemoryPipelineConfig {
    fn merge(&mut self, other: &MemoryPipelineConfig) {
        if let Some(load_memory) = other.load_memory {
            self.load_memory = load_memory;
        }
    }
}

impl MergeEffectiveConfig<EffectiveMemoryPipelineConfig> for MemoryPipelineConfig {
    fn get_effective_config(&self) -> EffectiveMemoryPipelineConfig {
        let mut result = EffectiveMemoryPipelineConfig::new();
        result.merge(self);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_api::{MergeBy, MergeEffectiveConfig};

    #[test]
    fn memory_pipeline_config_defaults_to_load_memory() {
        let effective = MemoryPipelineConfig::default().get_effective_config();
        assert!(effective.load_memory, "未配置时默认读取记忆");
    }

    #[test]
    fn memory_pipeline_config_merges_only_configured_value() {
        let mut config = MemoryPipelineConfig::default();
        config.merge(&MemoryPipelineConfig { load_memory: Some(false) });
        assert_eq!(config.load_memory, Some(false));
        // 未配置（None）不覆盖已有值
        config.merge(&MemoryPipelineConfig { load_memory: None });
        assert_eq!(config.load_memory, Some(false));
        assert!(!config.get_effective_config().load_memory);
    }

    #[test]
    fn memory_pipeline_config_serde_skips_absent_field() {
        assert_eq!(serde_json::to_string(&MemoryPipelineConfig::default()).unwrap(), "{}");
        let config: MemoryPipelineConfig = serde_json::from_str(r#"{"load_memory":false}"#).unwrap();
        assert_eq!(config.load_memory, Some(false));
    }

    #[test]
    fn input_channels_merge_replaces_with_other_or_empty() {
        let u1 = Arc::new(InputChannel {
            channel_id: ArcStr::from("web1"),
            messenger_id: ArcStr::from("web"),
            user_id: ArcStr::from("u1"),
        });
        let mut config = InputChannelsConfig { input_channels: Arc::new(vec![u1]) };
        // 合并默认配置 → 置空（整体替换语义）
        config.merge(&InputChannelsConfig::default());
        assert!(config.input_channels.is_empty(), "other 为默认空配置时结果为空");

        // other 有配置 → 整体替换
        let u2 = Arc::new(InputChannel {
            channel_id: ArcStr::from("web1"),
            messenger_id: ArcStr::from("web"),
            user_id: ArcStr::from("u2"),
        });
        config.merge(&InputChannelsConfig { input_channels: Arc::new(vec![u2]) });
        assert_eq!(config.input_channels.len(), 1);
        assert_eq!(config.input_channels[0].user_id.as_str(), "u2");
    }

    #[test]
    fn pipeline_config_requires_pipeline_name() {
        assert!(serde_json::from_str::<PipelineConfig>("{}").is_err(), "pipeline_name 必填");
        let config: PipelineConfig = serde_json::from_str(r#"{"pipeline_name":"default"}"#).unwrap();
        assert_eq!(config.pipeline_name.as_str(), "default");
    }
}
