use std::{collections::HashSet, sync::Arc};

use kissbot_api::{ChannelUser, MergeBy, MergeEffectiveConfig, MergeSelf};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompressConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_prompt: Option<Arc<String>>,
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
pub const DEFAULT_COMPRESS_PROMPT: &str = "请用简洁的语言总结以上对话的关键信息，保留重要细节、结论与未完成事项，供后续对话参考。";

/// 压缩默认阈值
pub const DEFAULT_COMPRESS_THRESHOLD: f64 = 0.8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveCompressConfig {
    pub compress_prompt: Arc<String>,
    pub compress_threshold: f64,
}

impl EffectiveCompressConfig {
    pub fn new() -> Self {
        Self {
            compress_prompt: Arc::new(DEFAULT_COMPRESS_PROMPT.to_string()),
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

/// out_channel 配置（(agent, role) 级回复通道，持久化到 nexus.json；channel_id 为发送目标）
/// 由 /bind-outgoing 在来源 channel 构造（channel_id = 来源 channel），Agentic Loop 回复经此发送
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutChannel {
    pub channel_id: Arc<String>,
    pub user: Arc<ChannelUser>,
    pub group_id: Arc<String>,
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

// ============= group ====================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupSetConfig {
    /// 会话启用的 group 名集合
    pub group_set: Arc<HashSet<String>>,
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

// ============ pipeline =================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PipelineConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_processor: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompter: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_sender: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_caller: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_processor: Option<String>,
}

impl MergeBy<PipelineConfig> for PipelineConfig {
    fn merge(&mut self, other: &Self) {
        if let Some(preset) = other.preset.as_ref() {
            self.preset = Some(preset.clone());
        }
        if let Some(input_processor) = other.input_processor.as_ref() {
            self.input_processor = Some(input_processor.clone());
        }
        if let Some(system_prompter) = other.system_prompter.as_ref() {
            self.system_prompter = Some(system_prompter.clone());
        }
        if let Some(message_sender) = other.message_sender.as_ref() {
            self.message_sender = Some(message_sender.clone());
        }
        if let Some(tool_caller) = other.tool_caller.as_ref() {
            self.tool_caller = Some(tool_caller.clone());
        }
        if let Some(output_processor) = other.output_processor.as_ref() {
            self.output_processor = Some(output_processor.clone());
        }
    }
}

impl MergeSelf for PipelineConfig {}

impl MergeEffectiveConfig<PipelineConfig> for PipelineConfig {
    fn get_effective_config(&self) -> PipelineConfig {
        self.clone()
    }
}
