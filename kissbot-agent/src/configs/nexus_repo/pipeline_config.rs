use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use kissbot_api::ChannelUser;
use serde::{Deserialize, Serialize};

use crate::{config_manager::ConfigManager, configs::{MergeConfig, MergeEffectiveConfig, ProviderModel}};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LLMConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<Arc<ProviderModel>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub has_max_tokens: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub has_temperature: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Arc<String>>,
    pub has_thinking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<Arc<String>>,
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

impl MergeConfig for LLMConfig {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveLLMConfig {
    pub provider_model: Arc<ProviderModel>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub thinking: Option<Arc<String>>,
    pub reasoning_effort: Option<Arc<String>>,
}

#[async_trait]
impl MergeEffectiveConfig<EffectiveLLMConfig> for LLMConfig {
    async fn get_effective_config(&self) -> EffectiveLLMConfig {
        let provider_model = if let Some(provider_model) = self.provider_model.as_ref() {
            provider_model.clone()
        } else {
            ConfigManager::get().default_model().await
        };
        EffectiveLLMConfig {
            provider_model,
            max_tokens: if self.has_max_tokens { self.max_tokens } else { None },
            temperature: if self.has_temperature { self.temperature } else { None },
            thinking: if self.has_thinking { self.thinking.clone() } else { None },
            reasoning_effort: if self.has_reasoning_effort { self.reasoning_effort.clone() } else { None },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompressConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_prompt: Option<Arc<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_threshold: Option<f64>,
}

impl MergeConfig for CompressConfig {
    fn merge(&mut self, other: &Self) {
        if let Some(compress_prompt) = other.compress_prompt.as_ref() {
            self.compress_prompt = Some(compress_prompt.clone());
        }
        if let Some(compress_threshold) = other.compress_threshold.as_ref() {
            self.compress_threshold = Some(compress_threshold.clone());
        }
    }
}

/// 压缩默认模板
pub const DEFAULT_COMPRESS_PROMPT: &str = "请用简洁的语言总结以上对话的关键信息，保留重要细节、结论与未完成事项，供后续对话参考。";

/// 压缩默认阈值
pub const DEFAULT_COMPRESS_THRESHOLD: f64 = 0.8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveCompressConfig {
    pub compress_prompt: Arc<String>,
    pub compress_threshold: f64,
}

#[async_trait]
impl MergeEffectiveConfig<EffectiveCompressConfig> for CompressConfig {
    async fn get_effective_config(&self) -> EffectiveCompressConfig {
        let compress_prompt = if let Some(compress_prompt) = self.compress_prompt.as_ref() {
            compress_prompt.clone()
        } else {
            Arc::new(DEFAULT_COMPRESS_PROMPT.to_string())
        };
        EffectiveCompressConfig {
            compress_prompt,
            compress_threshold: self.compress_threshold.unwrap_or(DEFAULT_COMPRESS_THRESHOLD),
        }
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

impl MergeConfig for MemoryRecoverConfig {
    fn merge(&mut self, other: &Self) {
        if let Some(memory_time_secs) = other.memory_time_secs {
            self.memory_time_secs = Some(memory_time_secs);
        }
        if let Some(memory_count) = other.memory_count {
            self.memory_count = Some(memory_count);
        }
    }
}

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

#[async_trait]
impl MergeEffectiveConfig<EffectiveMemoryRecoverConfig> for MemoryRecoverConfig {
    async fn get_effective_config(&self) -> EffectiveMemoryRecoverConfig {
        EffectiveMemoryRecoverConfig {
            memory_time_secs: self.memory_time_secs.unwrap_or(DEFAULT_MEMORY_TIME_SECS),
            memory_count: self.memory_count.unwrap_or(DEFAULT_MEMORY_COUNT),
        }
    }
}

// ========== Context 配置（agent→role 三层继承） ==========

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelBatchConfig {
    pub channel_batch_interval_secs: u64,
}

impl MergeConfig for ChannelBatchConfig {
    fn merge(&mut self, other: &Self) {
        self.channel_batch_interval_secs = other.channel_batch_interval_secs;
    }
}

/// channel 合批最小间隔默认值（秒）
pub const DEFAULT_CHANNEL_BATCH_INTERVAL_SECS: u64 = 3;

#[async_trait]
impl MergeEffectiveConfig<ChannelBatchConfig> for ChannelBatchConfig {
    async fn get_effective_config(&self) -> ChannelBatchConfig {
        let mut channel_batch_interval_secs = self.channel_batch_interval_secs;
        if channel_batch_interval_secs == 0 {
            channel_batch_interval_secs = DEFAULT_CHANNEL_BATCH_INTERVAL_SECS;
        }
        ChannelBatchConfig {
            channel_batch_interval_secs,
        }
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

impl MergeConfig for OutChannelConfig {
    fn merge(&mut self, other: &Self) {
        self.out_channel = other.out_channel.clone();
    }
}

#[async_trait]
impl MergeEffectiveConfig<OutChannelConfig> for OutChannelConfig {
    async fn get_effective_config(&self) -> OutChannelConfig {
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

impl MergeConfig for ToolkitSetConfig {
    fn merge(&mut self, other: &Self) {
        let toolkit_set = Arc::make_mut(&mut self.toolkit_set);
        for toolkit in other.toolkit_set.iter() {
            toolkit_set.insert(toolkit.clone());
        }
    }
}

#[async_trait]
impl MergeEffectiveConfig<ToolkitSetConfig> for ToolkitSetConfig {
    async fn get_effective_config(&self) -> ToolkitSetConfig {
        ToolkitSetConfig {
            toolkit_set: self.toolkit_set.clone(),
        }
    }
}
