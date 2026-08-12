use std::sync::Arc;
use std::collections::HashSet;

use arc_swap::ArcSwap;
use dashmap::DashMap;
use kissbot_api::{ArcSwapHashMap, ChannelUser};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::types::{Result, Error, RESERVED_AGENT_ID};

// ========== 配置数据结构 ==========

// ========== Context 配置（agent→role 三层继承，原 context_config.rs 迁入） ==========

// ---- 全局默认值 ----

/// channel 合批最小间隔（秒）
pub const DEFAULT_CHANNEL_BATCH_INTERVAL_SECS: u64 = 3;
/// 记忆提取时间窗（秒）
pub const DEFAULT_MEMORY_TIME_SECS: u64 = 3600;
/// 记忆提取条数
pub const DEFAULT_MEMORY_COUNT: usize = 50;
/// event 压缩指令默认模板
pub const DEFAULT_COMPRESS_PROMPT: &str = "请用简洁的语言总结以上对话的关键信息，保留重要细节、结论与未完成事项，供后续对话参考。";

// ---- 配置结构 ----

/// agent 级 context 配置（key = agent_id，覆盖全局默认；类似 ProviderConfig 的 default_*）
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

// ========== Provider 配置 ==========

// (provider, model) 固定一起出现：函数调用、current 运行状态、default 配置共用
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderModel {
    pub provider: String,
    pub model: String,
}

// ProviderConfig 定义在本文件，供 provider / model_client 与本文件的 NexusRepo.providers 共用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: Arc<String>,               // provider 名（providers map 的 key）
    pub provider_type: String,           // "openai" | "anthropic"，决定 Provider 实现
    pub base_url: String,                // URL 前缀，如 https://api.deepseek.com（原 endpoint）
    pub api_key: String,                 // provider 级密钥（从 model 级移上）
    pub default_context_length: u32,     // 默认上下文长度（token），本期只落位
    pub default_max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_temperature: Option<f32>,
    pub default_timeout_secs: u64,
    pub default_retry_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_thinking: Option<String>,          // 默认思考模式开关值（原样进 {"thinking":{"type":...}}）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,  // 默认思考力度
    pub default_max_context_messages: usize,     // 上下文消息条数上限（溢出触发重置/压缩）
    pub models: Arc<ArcSwapHashMap<String, ModelConfig>>,  // key = model 标识
}

// 合并后的有效配置（provider 默认 + model 覆盖），运行时合成、不持久化
#[derive(Debug, Clone)]
pub struct EffectiveModelConfig {
    pub provider_type: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub timeout_secs: u64,
    pub retry_count: u32,
    #[allow(dead_code)]   // 本期只落位：默认上下文长度（token），截断逻辑后续接入
    pub context_length: u32,
    /// 上下文消息条数上限（溢出触发重置/压缩；来自 provider 默认 + model 覆盖）
    pub max_context_messages: usize,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}

// ModelConfig 定义在本文件，供 model_client 与本文件的 NexusRepo.providers[].models 共用
// 字段均为可继承参数（Option），不配时使用所属 ProviderConfig 的 default_* 值
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model: String,                   // 与所属 provider 的 models map key 相同
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// 合成 provider 默认 + model 覆盖的有效参数（与 merge_context_config 同模式：
/// provider 默认 ← model 覆盖，model 未配字段继承 provider；model 为 None 时用 provider 默认值合成）
pub fn merge_model_config(
    provider: &ProviderConfig,
    model: Option<&ModelConfig>,
    model_name: &str,
) -> EffectiveModelConfig {
    EffectiveModelConfig {
        provider_type: provider.provider_type.clone(),
        base_url: provider.base_url.clone(),
        api_key: provider.api_key.clone(),
        model: model_name.to_string(),   // 用切换指令的模型名（未配置也有效）
        max_tokens: model.and_then(|m| m.max_tokens).unwrap_or(provider.default_max_tokens),
        temperature: model.and_then(|m| m.temperature).or(provider.default_temperature),
        timeout_secs: model.and_then(|m| m.timeout_secs).unwrap_or(provider.default_timeout_secs),
        retry_count: model.and_then(|m| m.retry_count).unwrap_or(provider.default_retry_count),
        context_length: model.and_then(|m| m.context_length).unwrap_or(provider.default_context_length),
        max_context_messages: model.and_then(|m| m.max_context_messages).unwrap_or(provider.default_max_context_messages),
        thinking: model.and_then(|m| m.thinking.clone()).or(provider.default_thinking.clone()),
        reasoning_effort: model.and_then(|m| m.reasoning_effort.clone()).or(provider.default_reasoning_effort.clone()),
    }
}

/// nexus 可改配置，持久化到 <data_dir>/nexus.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusRepo {
    pub channels: Arc<ArcSwapHashMap<String, ChannelConfig>>,
    pub providers: Arc<ArcSwapHashMap<String, ProviderConfig>>, // key = provider 名
    pub memory_structs: Arc<ArcSwapHashMap<String, MemoryStructConfig>>,
    // nexus 可对接的 station 列表
    pub stations: Arc<ArcSwapHashMap<String, StationConfig>>,
    /// agent_id → AgentContextConfig（上下文配置，三层继承见 merge_context_config）
    /// serde(default)：旧 nexus.json 无 context 段时反序列化为空 map（= 全局默认，兼容旧配置）
    #[serde(default)]
    pub context: Arc<ArcSwapHashMap<String, AgentContextConfig>>,
    pub default_model: Arc<ProviderModel>,   // (provider, model) 打包
    /// 保留 agent 的默认系统提示词（不调 memory-ego 时用），nexus.json 可持久化修改
    pub default_system_prompt: Arc<String>,
}

impl Default for NexusRepo {
    fn default() -> Self {
        Self {
            channels: Arc::new(ArcSwapHashMap::new()),
            providers: Arc::new(ArcSwapHashMap::new()),
            memory_structs: Arc::new(ArcSwapHashMap::new()),
            stations: Arc::new(ArcSwapHashMap::new()),
            context: Arc::new(ArcSwapHashMap::new()),
            default_model: Arc::new(ProviderModel { provider: String::new(), model: String::new() }),
            default_system_prompt: Arc::new(String::new()),
        }
    }
}

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
    /// 多绑定身份（bind 追加去重，unbind 带 ChannelUser 移除）
    pub bind_users: Vec<ChannelUser>,
    /// out_channel 配置（Option，至多 1 个；存于被绑定的 channel 下）
    pub outgoing: Option<OutChannelConfig>,
    /// 绑定的 agent_id（UUID；缺省/空 = 保留 agent = "0"，建会话用默认系统提示词，不调 memory-ego）
    #[serde(default = "default_agent_id", deserialize_with = "deserialize_agent_id")]
    pub agent_id: Arc<String>,
    #[serde(default)]
    pub role_name: Arc<String>,
    /// 是否启用（连接由 enabled 控制）
    pub enabled: bool,
}

/// out_channel 配置（持久化到 nexus.json；与 /bind-outgoing 三参数对应）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutChannelConfig {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
}

/// out_channel 运行态（coordinator 实时从配置构造）
#[derive(Debug, Clone)]
pub struct OutChannel {
    pub channel_id: Arc<String>,
    pub user: ChannelUser,
    pub group_id: Arc<String>,
}

/// 机器人绑定身份 / 管理员身份统一结构（定义于 kissbot-api::channel）

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStructConfig {
    pub name: Arc<String>,
    pub url: Arc<String>,
}

/// station 可改配置，持久化到 <data_dir>/station.json（本轮占位，暂无字段）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StationRepo {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationConfig {
    pub station_id: Arc<String>,
    pub base_url: Arc<String>,
    pub timeout_secs: u64,
    /// 工具列表（key = 工具名）
    /// serde(default)：旧 station 条目无 tools 时反序列化为空 map（= 不宣传任何工具，兼容旧配置）
    #[serde(default)]
    pub tools: Arc<ArcSwapHashMap<String, ToolConfig>>,
}

/// 工具配置（StationConfig.tools 的 value；name 与 map key 一致）
/// 字段按编码规范用 Arc<String>/Arc<Value>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub name: Arc<String>,
    pub description: Arc<String>,
    /// JSON Schema（OpenAI tools[].function.parameters）
    pub parameters: Arc<serde_json::Value>,
}

/// 静态配置：来自 KISSBOT_CONFIG 的 agent 段，启动后不变
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub data_dir: Arc<String>,
    pub mgmt_host: Arc<String>,
    pub mgmt_port: u16,
    pub ws_reconnect_interval_secs: u64,
    // 注：default_system_prompt 已移入 NexusRepo（nexus.json），config.json 不再承载
}

impl AgentConfig {
    /// 从 kissbot-config 全局单例的 agent 段加载
    pub fn from_public_config() -> Self {
        kissbot_config::Config::get().get_section("agent")
    }
}

/// 配置变更监听器：当配置被管理命令修改时，通知外部协调器
pub trait ConfigChangeListener: Send + Sync {
    #[allow(dead_code)]
    fn on_config_changed(&self, config_manager: &ConfigManager);
}

pub struct ConfigManager {
    agent_config: AgentConfig,
    nexus_repo: Arc<RwLock<NexusRepo>>,
    station_repo: Arc<RwLock<StationRepo>>,
    nexus_path: String,
    station_path: String,
    listeners: DashMap<String, Arc<dyn ConfigChangeListener>>,
}

impl ConfigManager {
    /// 从公共配置加载 AgentConfig，按 data_dir 加载/引导 NexusRepo/StationRepo
    pub async fn new() -> Result<Self> {
        let agent_config = AgentConfig::from_public_config();
        let data_dir = agent_config.data_dir.to_string();
        tokio::fs::create_dir_all(&data_dir).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        // 派生子目录（仅创建，功能本轮不实现）
        for sub in ["sessions", "attachments", "station"] {
            let _ = tokio::fs::create_dir_all(format!("{}/{}", data_dir, sub)).await;
        }
        let nexus_path = format!("{}/nexus.json", data_dir);
        let station_path = format!("{}/station.json", data_dir);

        let nexus_repo = Self::load_or_create_nexus(&nexus_path).await?;
        let station_repo = Self::load_or_create_station(&station_path).await?;

        Ok(Self {
            agent_config,
            nexus_repo: Arc::new(RwLock::new(nexus_repo)),
            station_repo: Arc::new(RwLock::new(station_repo)),
            nexus_path,
            station_path,
            listeners: DashMap::new(),
        })
    }

    async fn load_or_create_nexus(path: &str) -> Result<NexusRepo> {
        if std::path::Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await
                .map_err(|e| Error::ConfigNotFound(format!("{}: {}", path, e)))?;
            let repo: NexusRepo = serde_json::from_str(&content)
                .map_err(|e| Error::ConfigParseError(e.to_string()))?;
            Ok(repo)
        } else {
            // 首次创建：默认空配置（default_model/default_system_prompt 由 nexus.json 模板或人工填写）
            let repo = NexusRepo::default();
            let json = serde_json::to_string_pretty(&repo)?;
            tokio::fs::write(path, json).await.map_err(|e| Error::IoError(e.to_string()))?;
            Ok(repo)
        }
    }

    async fn load_or_create_station(path: &str) -> Result<StationRepo> {
        if std::path::Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await
                .map_err(|e| Error::ConfigNotFound(format!("{}: {}", path, e)))?;
            let repo: StationRepo = serde_json::from_str(&content)
                .map_err(|e| Error::ConfigParseError(e.to_string()))?;
            Ok(repo)
        } else {
            let repo = StationRepo::default();
            let json = serde_json::to_string_pretty(&repo)?;
            tokio::fs::write(path, json).await.map_err(|e| Error::IoError(e.to_string()))?;
            Ok(repo)
        }
    }

    /// 写 Nexus 配置（参考 WebMessengerRepo.write_config 模式）：
    /// 获取写锁 → op 在 &mut NexusRepo 内直接修改 → 序列化 → 写文件，锁全程持有
    async fn write_nexus_config<F, R>(&self, op: F) -> Result<R>
    where
        F: FnOnce(&mut NexusRepo) -> Result<R>,
    {
        let mut guard = self.nexus_repo.write().await;
        let rst = op(&mut *guard)?;
        let json = serde_json::to_string_pretty(&*guard)?;
        tokio::fs::write(&self.nexus_path, json.as_bytes()).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        Ok(rst)
    }

    /// 写 Station 配置（同 write_nexus_config 模式；station 功能未实现，暂无调用方）
    #[allow(dead_code)]
    async fn write_station_config<F, R>(&self, op: F) -> Result<R>
    where
        F: FnOnce(&mut StationRepo) -> Result<R>,
    {
        let mut guard = self.station_repo.write().await;
        let rst = op(&mut *guard)?;
        let json = serde_json::to_string_pretty(&*guard)?;
        tokio::fs::write(&self.station_path, json.as_bytes()).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        Ok(rst)
    }

    // ========== 静态配置 Getter（直接读，无锁） ==========

    pub fn ws_reconnect_interval_secs(&self) -> u64 { self.agent_config.ws_reconnect_interval_secs }
    pub fn mgmt_host(&self) -> &str { &self.agent_config.mgmt_host }
    pub fn mgmt_port(&self) -> u16 { self.agent_config.mgmt_port }
    #[allow(dead_code)]
    pub fn data_dir(&self) -> &str { &self.agent_config.data_dir }

    #[allow(dead_code)]
    /// 保留 agent 默认系统提示词（读 NexusRepo，nexus.json 可持久化修改）
    pub async fn default_system_prompt(&self) -> String {
        self.nexus_repo.read().await.default_system_prompt.to_string()
    }

    /// 注册配置变更监听器
    #[allow(dead_code)]
    pub fn add_listener(&self, key: &str, listener: Arc<dyn ConfigChangeListener>) {
        self.listeners.insert(key.to_string(), listener);
    }

    /// 通知所有监听器
    #[allow(dead_code)]
    async fn notify_listeners(&self) {
        let listeners: Vec<Arc<dyn ConfigChangeListener>> = self.listeners
            .iter().map(|e| e.value().clone()).collect();
        for listener in &listeners {
            listener.on_config_changed(self);
        }
    }

    // ========== NexusRepo CRUD ==========

    // ---------- channels ----------
    /// 返回所有 channel 配置快照（channel_id -> Arc<ChannelConfig>）
    pub async fn channels(&self) -> Vec<(String, Arc<ChannelConfig>)> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter().map(|(k, v)| (k.clone(), v.load().clone())).collect()
    }
    /// 返回所有 station 配置快照（station_id -> Arc<StationConfig>）
    pub async fn stations(&self) -> Vec<(String, Arc<StationConfig>)> {
        let repo = self.nexus_repo.read().await;
        repo.stations.iter().map(|(k, v)| (k.clone(), v.load().clone())).collect()
    }
    /// 按 channel_id 直接查找单个 channel 配置（map O(1) get，不克隆整个 map 再遍历）
    pub async fn channel(&self, channel_id: &str) -> Option<Arc<ChannelConfig>> {
        let repo = self.nexus_repo.read().await;
        repo.channels.get(channel_id).map(|s| s.load().clone())
    }
    #[allow(dead_code)]
    pub async fn channel_ws_url(&self, channel_id: &str) -> Option<String> {
        let repo = self.nexus_repo.read().await;
        repo.channels.get(channel_id).map(|s| s.load().ws_url.to_string())
    }
    #[allow(dead_code)]
    pub async fn add_channel(&self, ch: ChannelConfig) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.channels);
            map.insert(ch.channel_id.to_string(), ArcSwap::new(Arc::new(ch)));
            Ok(())
        }).await
    }
    #[allow(dead_code)]
    pub async fn remove_channel(&self, channel_id: &str) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.channels);
            map.remove(channel_id);
            Ok(())
        }).await
    }

    /// 修改 channel 配置并落盘（绑定/agent/role/outgoing 等运行时回写统一入口）
    /// channel 不存在返回 ConfigNotFound
    pub async fn update_channel<F>(&self, channel_id: &str, f: F) -> Result<()>
    where
        F: FnOnce(&mut ChannelConfig) + Send,
    {
        self.write_nexus_config(|repo| {
            let swap = repo.channels.get(channel_id)
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let mut ch = swap.load().clone();
            let ch_mut = Arc::make_mut(&mut ch);
            f(ch_mut);
            swap.store(ch);
            Ok(())
        }).await
    }

    // ---------- providers ----------
    /// 合成 provider 默认 + model 覆盖的有效参数（每次调用现场合成，配置永远最新）
    /// model 未在 provider.models 配置时用 provider 默认值合成（极端 models={} 也可用）
    pub async fn resolve_effective_config(&self, pm: &ProviderModel) -> Option<EffectiveModelConfig> {
        let repo = self.nexus_repo.read().await;
        let provider = repo.providers.get(&pm.provider)?.load_full();
        let model_cfg = provider.models.get(&pm.model).map(|s| s.load_full());
        Some(merge_model_config(&provider, model_cfg.as_deref(), &pm.model))
    }

    /// 按名取 provider 配置（Arc 快照），供 provider 构造（model_client.list_models 使用）
    /// 不存在返回 None
    pub async fn provider_config_by_name(&self, name: &str) -> Option<Arc<ProviderConfig>> {
        self.nexus_repo.read().await.providers.get(name).map(|s| s.load_full())
    }

    /// 按 (agent_id, role_name) 合并 context 配置（三层继承：全局默认 ← agent ← role）
    pub async fn context_config(&self, agent_id: &str, role_name: &str) -> EffectiveContextConfig {
        let repo = self.nexus_repo.read().await;
        let agent = repo.context.get(agent_id).map(|s| s.load_full());
        let role = agent.as_ref().and_then(|a| a.roles.get(role_name).map(|s| s.load_full()));
        merge_context_config(agent.as_deref(), role.as_deref())
    }

    // ---------- providers CRUD（管理 API 使用，落盘） ----------
    /// 添加 provider（重名报 ConfigNotFound），落盘
    pub async fn add_provider(&self, cfg: ProviderConfig) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.providers);
            if map.contains_key(&cfg.name.to_string()) {
                return Err(Error::ConfigNotFound(format!("provider 已存在: {}", cfg.name)));
            }
            map.insert(cfg.name.to_string(), ArcSwap::new(Arc::new(cfg)));
            Ok(())
        }).await
    }
    /// 删除 provider（不存在报 ConfigNotFound），落盘
    pub async fn remove_provider(&self, name: &str) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.providers);
            if !map.contains_key(name) {
                return Err(Error::ConfigNotFound(format!("provider 不存在: {}", name)));
            }
            map.remove(name);
            Ok(())
        }).await
    }
    /// 返回 NexusRepo 快照（管理 API GET /config 使用）
    pub async fn nexus_snapshot(&self) -> NexusRepo {
        self.nexus_repo.read().await.clone()
    }

    // ---------- memory_structs ----------
    #[allow(dead_code)] // memory-struct 功能未实现，待后续接入时使用
    pub async fn memory_structs(&self) -> Vec<MemoryStructConfig> {
        let repo = self.nexus_repo.read().await;
        repo.memory_structs.iter().map(|(_, v)| (*v.load_full()).clone()).collect()
    }

    // ---------- default 读写 ----------
    pub async fn default_model(&self) -> ProviderModel { (*self.nexus_repo.read().await.default_model).clone() }
    /// 设置默认模型（(provider, model) 打包），落盘
    pub async fn set_default_model(&self, pm: ProviderModel) -> Result<()> {
        self.write_nexus_config(|repo| {
            *Arc::make_mut(&mut repo.default_model) = pm;
            Ok(())
        }).await
    }

    // ===== admins（永久操作：聚合 + NexusRepo 回写，check_admin 使用）=====
    /// 聚合所有 channel 的 admins（per-channel 检查改为 channel_admins 后暂无消费，保留）
    #[allow(dead_code)]
    pub async fn admin_users(&self) -> Vec<ChannelUser> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter()
            .flat_map(|(_, v)| {
                let c = v.load();
                c.admins.iter().cloned().collect::<Vec<ChannelUser>>()
            })
            .collect()
    }
    /// 读取指定 channel 的管理员列表（per-channel admin 检查用）
    pub async fn channel_admins(&self, channel_id: &str) -> Vec<ChannelUser> {
        let repo = self.nexus_repo.read().await;
        repo.channels.get(channel_id)
            .map(|s| s.load().admins.iter().cloned().collect())
            .unwrap_or_default()
    }
    /// 添加管理权限（回写 NexusRepo；channel 不存在则报错）
    pub async fn add_admin(&self, channel_id: &str, admin: &ChannelUser) -> Result<()> {
        self.write_nexus_config(|repo| {
            let swap = repo.channels.get(channel_id)
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let mut ch = swap.load().clone();
            let ch_mut = Arc::make_mut(&mut ch);
            Arc::make_mut(&mut ch_mut.admins).insert(admin.clone());
            swap.store(ch);
            Ok(())
        }).await
    }
    /// 移除管理权限（回写 NexusRepo；channel 不存在则报错）
    /// channel_id 定位 channel，messenger_id 为消息层身份（admin 条目的匹配键）
    pub async fn remove_admin(&self, channel_id: &str, messenger_id: &str, user_id: &str) -> Result<()> {
        self.write_nexus_config(|repo| {
            let swap = repo.channels.get(channel_id)
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let mut ch = swap.load().clone();
            let ch_mut = Arc::make_mut(&mut ch);
            let target = ChannelUser { messenger_id: messenger_id.into(), user_id: user_id.into() };
            Arc::make_mut(&mut ch_mut.admins).remove(&target);
            swap.store(ch);
            Ok(())
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn agent_config(data_dir: &str) -> AgentConfig {
        AgentConfig {
            data_dir: Arc::new(data_dir.into()),
            mgmt_host: Arc::new("127.0.0.1".into()),
            mgmt_port: 9090,
            ws_reconnect_interval_secs: 5,
        }
    }

    #[tokio::test]
    async fn bootstrap_creates_nexus_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap()).await.unwrap();
        // 首次创建为空默认（default_model/default_system_prompt 由模板或人工填写）
        assert!(repo.default_model.provider.is_empty());
        assert!(repo.default_system_prompt.is_empty());
        assert!(repo.channels.is_empty());
        assert!(path.exists(), "首次创建应写文件");
    }

    #[tokio::test]
    async fn bootstrap_loads_existing_nexus() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        // 第一次创建（空默认）
        let _ = ConfigManager::load_or_create_nexus(path.to_str().unwrap()).await.unwrap();
        // 第二次加载：文件已存在为权威（内容不变）
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap()).await.unwrap();
        assert!(repo.default_model.provider.is_empty(), "文件存在时不应重新种子");
    }

    #[tokio::test]
    async fn nexus_json_file_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap()).await.unwrap();
        // 模拟写回再读
        let json = serde_json::to_string_pretty(&repo).unwrap();
        std::fs::write(&path, json).unwrap();
        let back: NexusRepo = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(back.default_model.provider.is_empty());
    }

    #[tokio::test]
    async fn write_config_op_error_skips_persist() {
        // write_nexus_config 模式语义：op 返回 Err 时不应序列化写盘（文件保持不变）
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        manager.add_channel(sample_channel("web-main")).await.unwrap();
        let before = std::fs::read_to_string(dir.path().join("nexus.json")).unwrap();

        // update_channel 的 op 前校验失败（channel 不存在）→ 返回 Err 且不落盘
        let err = manager.update_channel("nope", |_| {}).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        let after = std::fs::read_to_string(dir.path().join("nexus.json")).unwrap();
        assert_eq!(before, after, "op 失败不应写入文件");

        // 成功路径：落盘可见
        manager.update_channel("web-main", |c| c.agent_id = Arc::new("a1".into())).await.unwrap();
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("nexus.json")).unwrap()).unwrap();
        assert_eq!(saved["channels"]["web-main"]["agent_id"], "a1");
    }

    #[tokio::test]
    async fn add_remove_admin_missing_channel_errors() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        // channel 不存在：add_admin / remove_admin 都应返回 ConfigNotFound 而非静默成功
        let admin = ChannelUser { messenger_id: "m1".into(), user_id: "u1".into() };
        let err = manager.add_admin("nope", &admin).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        let err = manager.remove_admin("nope", "m1", "u1").await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
    }

    fn sample_channel(id: &str) -> ChannelConfig {
        ChannelConfig {
            channel_id: Arc::new(id.into()),
            ws_url: Arc::new("ws://127.0.0.1:8201".into()),
            admins: Arc::new(HashSet::new()),
            bind_users: vec![ChannelUser { messenger_id: "web".into(), user_id: "u1".into() }],
            outgoing: None,
            agent_id: Arc::new("0".into()),
            role_name: Arc::new("".into()),
            enabled: true,
        }
    }

    #[test]
    fn channel_config_bind_users_and_outgoing_roundtrip() {
        let ch = ChannelConfig {
            channel_id: Arc::new("c1".into()),
            ws_url: Arc::new("ws://127.0.0.1:8201".into()),
            admins: Arc::new(HashSet::new()),
            bind_users: vec![
                ChannelUser { messenger_id: "web".into(), user_id: "u1".into() },
                ChannelUser { messenger_id: "web".into(), user_id: "u2".into() },
            ],
            outgoing: Some(OutChannelConfig {
                messenger_id: Arc::new("web".into()),
                user_id: Arc::new("u1".into()),
                group_id: Arc::new("g1".into()),
            }),
            agent_id: Arc::new("a1".into()),
            role_name: Arc::new("r1".into()),
            enabled: true,
        };
        let json = serde_json::to_value(&ch).unwrap();
        assert_eq!(json["bind_users"][0]["user_id"], "u1", "bind_users 数组序列化");
        assert_eq!(json["outgoing"]["group_id"], "g1");
        assert!(json.get("is_send_channel").is_none(), "is_send_channel 已删除");
        let back: ChannelConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.bind_users.len(), 2);
        assert_eq!(back.outgoing.as_ref().unwrap().group_id.as_str(), "g1");
    }

    #[test]
    fn channel_config_new_shape_serde_roundtrip() {
        let ch = sample_channel("web-main");
        let json = serde_json::to_string(&ch).unwrap();
        assert!(json.contains("\"bind_users\""), "应序列化 bind_users");
        assert!(json.contains("\"agent_id\""));
        assert!(json.contains("\"role_name\""));
        assert!(!json.contains("\"is_send_channel\""), "is_send_channel 已删除");
        assert!(json.contains("\"enabled\""));
        let back: ChannelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(*back.channel_id, "web-main");
        assert_eq!(back.bind_users[0].user_id, "u1");
    }

    #[test]
    fn channel_config_old_shape_no_longer_parses() {
        // 旧格式（default_bind_user / enabled_by_default / is_send_channel）不再兼容：
        // bind_users 为必填数组，旧单值字段无 serde alias（不兼容旧配置，配置文件直接改）
        let old = r#"{
            "channel_id": "web-main",
            "ws_url": "ws://127.0.0.1:8201",
            "admins": [],
            "default_bind_user": { "messenger_id": "web", "user_id": "u1" },
            "enabled_by_default": true
        }"#;
        assert!(serde_json::from_str::<ChannelConfig>(old).is_err(), "旧格式应解析失败（不兼容）");
    }

    #[test]
    fn channel_config_agent_id_empty_normalizes_to_reserved() {
        // 显式空串 → 归一化为 "0"
        let json = r#"{"channel_id":"c1","ws_url":"ws://127.0.0.1:8201","admins":[],"bind_users":[],"agent_id":"","role_name":"","enabled":true}"#;
        let ch: ChannelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(ch.agent_id.as_str(), "0", "空串应归一化为保留 id");
    }

    #[test]
    fn channel_config_agent_id_missing_defaults_to_reserved() {
        // 字段缺省 → "0"
        let json = r#"{"channel_id":"c1","ws_url":"ws://127.0.0.1:8201","admins":[],"bind_users":[],"role_name":"","enabled":true}"#;
        let ch: ChannelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(ch.agent_id.as_str(), "0", "缺省应回退保留 id");
    }

    #[tokio::test]
    async fn update_channel_mutates_and_persists() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        manager.add_channel(sample_channel("web-main")).await.unwrap();

        // 修改 agent_id/role_name/bind_users（is_send_channel 已删除，绑定改为数组追加）
        manager.update_channel("web-main", |c| {
            c.agent_id = Arc::new("a1".into());
            c.role_name = Arc::new("r1".into());
            c.bind_users.push(ChannelUser { messenger_id: "web".into(), user_id: "u2".into() });
        }).await.unwrap();

        // 内存可见
        let ch = manager.channels().await.into_iter()
            .find(|(id, _)| id == "web-main").map(|(_, c)| c).unwrap();
        assert_eq!(*ch.agent_id, "a1");
        assert_eq!(ch.bind_users.len(), 2, "bind_users 追加应可见");

        // 落盘可见（重新读文件）
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("nexus.json")).unwrap()).unwrap();
        assert_eq!(saved["channels"]["web-main"]["agent_id"], "a1");

        // channel 不存在报错
        let err = manager.update_channel("nope", |_| {}).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
    }

    #[test]
    fn channel_user_hash_eq_by_value() {
        let a = ChannelUser { messenger_id: "m1".into(), user_id: "u1".into() };
        let b = ChannelUser { messenger_id: "m1".into(), user_id: "u1".into() };
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 ChannelUser 应命中 HashSet");
    }

    #[test]
    fn nexus_repo_serde_roundtrip() {
        let repo = NexusRepo {
            channels: Arc::new(ArcSwapHashMap::new()),
            providers: Arc::new(ArcSwapHashMap::new()),
            memory_structs: Arc::new(ArcSwapHashMap::new()),
            stations: Arc::new(ArcSwapHashMap::new()),
            context: Arc::new(ArcSwapHashMap::new()),
            default_model: Arc::new(ProviderModel { provider: "deepseek".into(), model: "gpt-4o".into() }),
            default_system_prompt: Arc::new("你是 kissbot 智能助手".into()),
        };
        let json = serde_json::to_string(&repo).unwrap();
        let back: NexusRepo = serde_json::from_str(&json).unwrap();
        assert_eq!(*back.default_model, ProviderModel { provider: "deepseek".into(), model: "gpt-4o".into() });
        assert_eq!(*back.default_system_prompt, "你是 kissbot 智能助手");
    }

    #[test]
    fn nexus_repo_default_empty() {
        let repo = NexusRepo::default();
        assert!(repo.channels.is_empty());
        assert!(repo.providers.is_empty());
        assert!(repo.memory_structs.is_empty());
        assert!(repo.stations.is_empty());
    }

    // ---------- Provider 配置 ----------

    fn sample_provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: Arc::new(name.into()),
            provider_type: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key: "sk-test".into(),
            default_context_length: 65536,
            default_max_tokens: 4096,
            default_temperature: Some(0.7),
            default_timeout_secs: 60,
            default_retry_count: 3,
            default_thinking: None,
            default_reasoning_effort: None,
            default_max_context_messages: 100,
            models: Arc::new(ArcSwapHashMap::new()),
        }
    }

    #[test]
    fn provider_config_old_shape_migration() {
        // 旧格式：default_temperature 为数值，无 default_thinking/default_reasoning_effort
        let old = r#"{
            "name": "deepseek",
            "provider_type": "openai",
            "base_url": "https://api.deepseek.com",
            "api_key": "sk-test",
            "default_context_length": 65536,
            "default_max_tokens": 4096,
            "default_temperature": 0.7,
            "default_timeout_secs": 60,
            "default_retry_count": 3,
            "default_max_context_messages": 100,
            "models": {}
        }"#;
        let pc: ProviderConfig = serde_json::from_str(old).unwrap();
        assert_eq!(pc.default_temperature, Some(0.7), "旧数值应映射为 Some");
        assert_eq!(pc.default_thinking, None);
        assert_eq!(pc.default_reasoning_effort, None);
    }

    #[tokio::test]
    async fn resolve_effective_config_merges_provider_and_model() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        // 构造 provider + model（ModelConfig 为 Option 可继承参数）
        let mut provider = sample_provider("deepseek");
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("deepseek-4-flash".to_string(), ArcSwap::new(Arc::new(ModelConfig {
                model: "deepseek-4-flash".into(),
                max_tokens: Some(2048),
                temperature: Some(0.3),
                timeout_secs: Some(30),
                retry_count: Some(2),
                context_length: None,  // 未配 → 继承 provider 默认
                max_context_messages: Some(80),
                thinking: Some("enabled".into()),
                reasoning_effort: Some("high".into()),
            })));
        }
        {
            let mut repo = manager.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.providers);
            map.insert("deepseek".to_string(), ArcSwap::new(Arc::new(provider)));
        }
        let pm = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let eff = manager.resolve_effective_config(&pm).await.expect("应能合成");
        assert_eq!(eff.provider_type, "openai");
        assert_eq!(eff.base_url, "https://api.deepseek.com");
        assert_eq!(eff.api_key, "sk-test");
        assert_eq!(eff.model, "deepseek-4-flash");
        assert_eq!(eff.max_tokens, 2048, "model 的 max_tokens 应生效");
        assert_eq!(eff.temperature, Some(0.3), "model 的 temperature 应生效");
        assert_eq!(eff.thinking.as_deref(), Some("enabled"), "model 的 thinking 应生效");
        assert_eq!(eff.reasoning_effort.as_deref(), Some("high"), "model 的 reasoning_effort 应生效");
        assert_eq!(eff.timeout_secs, 30);
        assert_eq!(eff.retry_count, 2);
        assert_eq!(eff.max_context_messages, 80, "model 的 max_context_messages 应生效");
        assert_eq!(eff.context_length, 65536, "context_length 未配应继承 provider 默认");
    }

    #[tokio::test]
    async fn resolve_effective_config_inherits_provider_defaults() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        let mut provider = sample_provider("deepseek");
        // 设置思考相关默认值（验证未配置时继承 provider 默认）
        provider.default_thinking = Some("disabled".into());
        provider.default_reasoning_effort = Some("low".into());
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("deepseek-4-flash".to_string(), ArcSwap::new(Arc::new(ModelConfig {
                model: "deepseek-4-flash".into(),
                max_tokens: None,
                temperature: None,
                timeout_secs: None,
                retry_count: None,
                context_length: Some(131072),  // 覆盖 context_length
                max_context_messages: None,  // 未配 → 继承 provider 默认
                thinking: None,
                reasoning_effort: None,
            })));
        }
        {
            let mut repo = manager.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.providers);
            map.insert("deepseek".to_string(), ArcSwap::new(Arc::new(provider)));
        }
        let pm = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let eff = manager.resolve_effective_config(&pm).await.expect("应能合成");
        assert_eq!(eff.max_tokens, 4096, "缺省继承 provider 默认");
        assert_eq!(eff.temperature, Some(0.7));
        assert_eq!(eff.timeout_secs, 60);
        assert_eq!(eff.retry_count, 3);
        assert_eq!(eff.context_length, 131072, "model 覆盖 context_length 应生效");
        assert_eq!(eff.max_context_messages, 100, "max_context_messages 未配应继承 provider 默认");
        assert_eq!(eff.thinking.as_deref(), Some("disabled"), "thinking 未配应继承 provider 默认");
        assert_eq!(eff.reasoning_effort.as_deref(), Some("low"), "reasoning_effort 未配应继承 provider 默认");
    }

    #[tokio::test]
    async fn resolve_effective_config_missing_returns_none() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        // provider 不存在 → None
        assert!(manager.resolve_effective_config(&ProviderModel { provider: "nope".into(), model: "m".into() }).await.is_none());
        // provider 存在但 model 未配置 → Some（用 provider 默认值合成）
        manager.add_provider(sample_provider("deepseek")).await.unwrap();
        let eff = manager.resolve_effective_config(&ProviderModel { provider: "deepseek".into(), model: "nope".into() }).await
            .expect("model 未配置也应合成");
        assert_eq!(eff.model, "nope", "model 用切换指令的模型名");
        assert_eq!(eff.max_tokens, 4096, "未配置参数取 provider 默认值");
        assert_eq!(eff.temperature, Some(0.7));
        assert_eq!(eff.timeout_secs, 60);
    }

    #[tokio::test]
    async fn resolve_effective_config_synthesizes_unconfigured_model() {
        // manager 构造沿用本文件其它测试的内联模式；provider.deepseek 的 models 为空（models={} 极端情况）
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        manager.add_provider(sample_provider("deepseek")).await.unwrap();
        let eff = manager.resolve_effective_config(&ProviderModel { provider: "deepseek".into(), model: "deepseek-v4-flash".into() }).await;
        let eff = eff.expect("model 未配置也应合成");
        assert_eq!(eff.model, "deepseek-v4-flash");
        assert_eq!(eff.max_tokens, 4096);          // provider 默认值
        assert_eq!(eff.temperature, Some(0.7));
        assert_eq!(eff.timeout_secs, 60);
    }

    #[tokio::test]
    async fn provider_config_by_name_getter() {
        // 构造 manager：provider_config_by_name("deepseek") 返回 Some、("nope") 返回 None
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        // 未添加前 None
        assert!(manager.provider_config_by_name("deepseek").await.is_none());
        manager.add_provider(sample_provider("deepseek")).await.unwrap();
        let pc = manager.provider_config_by_name("deepseek").await.expect("应能查到");
        assert_eq!(*pc.name, "deepseek");
        assert_eq!(pc.base_url, "https://api.deepseek.com");
        assert_eq!(pc.api_key, "sk-test");
        assert!(manager.provider_config_by_name("nope").await.is_none());
    }

    #[tokio::test]
    async fn provider_crud_and_default_set() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        // add_provider → resolve 可见
        manager.add_provider(sample_provider("deepseek")).await.unwrap();
        let eff = manager.resolve_effective_config(&ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() }).await
            .expect("provider 无 models 时 model 未配置也应合成（取 provider 默认值）");
        assert_eq!(eff.max_tokens, 4096, "未配置参数取 provider 默认值");
        // 带 models 的 provider
        let mut provider = sample_provider("openai");
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("gpt-4o".into(), ArcSwap::new(Arc::new(ModelConfig {
                model: "gpt-4o".into(), max_tokens: None, temperature: None,
                timeout_secs: None, retry_count: None, context_length: None,
                max_context_messages: None, thinking: None, reasoning_effort: None,
            })));
        }
        manager.add_provider(provider).await.unwrap();
        // 重名报错
        let err = manager.add_provider(sample_provider("openai")).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        // set_default_model → getter 可见 + 落盘
        let pm = ProviderModel { provider: "openai".into(), model: "gpt-4o".into() };
        manager.set_default_model(pm.clone()).await.unwrap();
        assert_eq!(manager.default_model().await, pm);
        // remove_provider → resolve 返回 None；不存在报错
        manager.remove_provider("openai").await.unwrap();
        assert!(manager.resolve_effective_config(&pm).await.is_none());
        let err = manager.remove_provider("nope").await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        // nexus_snapshot 反映变更
        let snap = manager.nexus_snapshot().await;
        assert_eq!(*snap.default_model, pm);
        assert!(!snap.providers.contains_key("openai"));
    }

    #[test]
    fn station_config_tools_roundtrip() {
        let sc = StationConfig {
            station_id: Arc::new("local".into()),
            base_url: Arc::new(String::new()),
            timeout_secs: 5,
            tools: Arc::new(ArcSwapHashMap::new()),
        };
        let json = serde_json::to_string(&sc).unwrap();
        let back: StationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(*back.station_id, "local");
        assert!(back.tools.is_empty());

        // ToolConfig 序列化
        let tc = ToolConfig {
            name: Arc::new("read".into()),
            description: Arc::new("读取文本文件".into()),
            parameters: Arc::new(serde_json::json!({ "type": "object", "properties": { "path": { "type": "string" } } })),
        };
        let tj = serde_json::to_value(&tc).unwrap();
        assert_eq!(tj["name"], "read");
        assert_eq!(tj["parameters"]["properties"]["path"]["type"], "string");
    }

    // ---- context 配置合并（原 context_config.rs 测试迁入） ----

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
