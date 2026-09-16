use std::{collections::HashSet, sync::Arc};

use kissbot_api::ChannelUser;
use serde::{Deserialize, Serialize};

use crate::configs::{MergeBy, MergeSelf, MergeEffectiveConfig, ProviderModel};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LLMConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<Arc<ProviderModel>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_max_tokens: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_temperature: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Arc<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_thinking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<Arc<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_reasoning_effort: bool,
}

impl LLMConfig {
    pub fn set_max_tokens(&mut self, max_tokens: Option<u32>) {
        self.max_tokens = max_tokens;
        self.has_max_tokens = true;
    }
    pub fn unset_max_tokens(&mut self) {
        self.max_tokens = None;
        self.has_max_tokens = false;
    }
    pub fn set_temperature(&mut self, temperature: Option<f32>) {
        self.temperature = temperature;
        self.has_temperature = true;
    }
    pub fn unset_temperature(&mut self) {
        self.temperature = None;
        self.has_temperature = false;
    }
    pub fn set_thinking(&mut self, thinking: Option<Arc<String>>) {
        self.thinking = thinking;
        self.has_thinking = true;
    }
    pub fn unset_thinking(&mut self) {
        self.thinking = None;
        self.has_thinking = false;
    }
    pub fn set_reasoning_effort(&mut self, reasoning_effort: Option<Arc<String>>) {
        self.reasoning_effort = reasoning_effort;
        self.has_reasoning_effort = true;
    }
    pub fn unset_reasoning_effort(&mut self) {
        self.reasoning_effort = None;
        self.has_reasoning_effort = false;
    }
}

impl MergeBy<LLMConfig> for LLMConfig {
    fn merge(&mut self, other: &LLMConfig) {
        if let Some(model) = other.provider_model.as_ref() {
            self.provider_model = Some(model.clone());
        }
        if other.has_max_tokens {
            self.set_max_tokens(other.max_tokens);
        }
        if other.has_temperature {
            self.set_temperature(other.temperature);
        }
        if other.has_thinking {
            self.set_thinking(other.thinking.clone());
        }
        if other.has_reasoning_effort {
            self.set_reasoning_effort(other.reasoning_effort.clone());
        }
    }
}

impl MergeSelf for LLMConfig {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveLLMConfig {
    pub provider_model: Arc<ProviderModel>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub thinking: Option<Arc<String>>,
    pub reasoning_effort: Option<Arc<String>>,
}

impl EffectiveLLMConfig {
    pub fn new() -> Self {
        Self {
            provider_model: Arc::new(ProviderModel {
                provider: Arc::new(String::new()),
                model: Arc::new(String::new()),
            }),
            max_tokens: None,
            temperature: None,
            thinking: None,
            reasoning_effort: None,
        }
    }
}

impl MergeBy<LLMConfig> for EffectiveLLMConfig {
    fn merge(&mut self, other: &LLMConfig) {
        if let Some(model) = other.provider_model.as_ref() {
            self.provider_model = model.clone();
        }
        if other.has_max_tokens {
            self.max_tokens = other.max_tokens;
        }
        if other.has_temperature {
            self.temperature = other.temperature;
        }
        if other.has_thinking {
            self.thinking = other.thinking.clone();
        }
        if other.has_reasoning_effort {
            self.reasoning_effort = other.reasoning_effort.clone();
        }
     }
}

impl MergeEffectiveConfig<EffectiveLLMConfig> for LLMConfig {
    fn get_effective_config(&self) -> EffectiveLLMConfig {
        let mut result = EffectiveLLMConfig::new();
        result.merge(self);
        result
    }
}

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
        OutChannelConfig {
            out_channel: self.out_channel.clone(),
        }
    }
}

// ============= toolkit ====================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolkitSetConfig {
    /// 启用的 toolkit 名集合
    pub toolkit_set: Arc<HashSet<String>>,
}

impl MergeBy<ToolkitSetConfig> for ToolkitSetConfig {
    fn merge(&mut self, other: &Self) {
        let toolkit_set = Arc::make_mut(&mut self.toolkit_set);
        for toolkit in other.toolkit_set.iter() {
            toolkit_set.insert(toolkit.clone());
        }
    }
}

impl MergeSelf for ToolkitSetConfig {}

impl MergeEffectiveConfig<ToolkitSetConfig> for ToolkitSetConfig {
    fn get_effective_config(&self) -> ToolkitSetConfig {
        ToolkitSetConfig {
            toolkit_set: self.toolkit_set.clone(),
        }
    }
}
