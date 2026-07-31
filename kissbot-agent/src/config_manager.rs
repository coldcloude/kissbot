use std::sync::Arc;
use std::collections::HashSet;

use arc_swap::ArcSwap;
use dashmap::DashMap;
use kissbot_api::ArcSwapHashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::types::{Result, Error};

// ========== 配置数据结构 ==========

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
    pub default_temperature: f32,
    pub default_timeout_secs: u64,
    pub default_retry_count: u32,
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
    pub temperature: f32,
    pub timeout_secs: u64,
    pub retry_count: u32,
    #[allow(dead_code)]   // 本期只落位：默认上下文长度（token），截断逻辑后续接入
    pub context_length: u32,
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
}

/// nexus 可改配置，持久化到 <data_dir>/nexus.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusRepo {
    pub channels: Arc<ArcSwapHashMap<String, ChannelConfig>>,
    pub providers: Arc<ArcSwapHashMap<String, ProviderConfig>>, // key = provider 名
    pub memory_structs: Arc<ArcSwapHashMap<String, MemoryStructConfig>>,
    // nexus 可对接的 station 列表
    pub stations: Arc<ArcSwapHashMap<String, StationConfig>>,
    pub default_agent_id: Arc<String>,
    pub default_role: Arc<String>,
    pub default_model: Arc<ProviderModel>,   // (provider, model) 打包
}

impl Default for NexusRepo {
    fn default() -> Self {
        Self {
            channels: Arc::new(ArcSwapHashMap::new()),
            providers: Arc::new(ArcSwapHashMap::new()),
            memory_structs: Arc::new(ArcSwapHashMap::new()),
            stations: Arc::new(ArcSwapHashMap::new()),
            default_agent_id: Arc::new(String::new()),
            default_role: Arc::new(String::new()),
            default_model: Arc::new(ProviderModel { provider: String::new(), model: String::new() }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub channel_id: Arc<String>,         // agent 内部唯一标识，与消息方 messenger 无关
    pub ws_url: Arc<String>,
    pub admins: Arc<HashSet<ChannelUser>>,
    pub default_bind_user: Option<ChannelUser>,
    pub enabled_by_default: bool,
}

/// 机器人绑定身份 / 管理员身份统一结构
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct ChannelUser {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
}

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
}

/// 静态配置：来自 KISSBOT_CONFIG 的 agent 段，启动后不变
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub data_dir: Arc<String>,
    pub mgmt_host: Arc<String>,
    pub mgmt_port: u16,
    pub ws_reconnect_interval_secs: u64,
    pub init_agent_id: Arc<String>,
    pub init_role: Arc<String>,
    pub init_model: Arc<ProviderModel>,   // 种子 NexusRepo.default_model（(provider, model) 打包）
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

        let nexus_repo = Self::load_or_create_nexus(&nexus_path, &agent_config).await?;
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

    async fn load_or_create_nexus(path: &str, cfg: &AgentConfig) -> Result<NexusRepo> {
        if std::path::Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await
                .map_err(|e| Error::ConfigNotFound(format!("{}: {}", path, e)))?;
            let repo: NexusRepo = serde_json::from_str(&content)
                .map_err(|e| Error::ConfigParseError(e.to_string()))?;
            Ok(repo)
        } else {
            // 首次创建：用 init_* 种子 3 个 default，集合为空
            let repo = NexusRepo {
                default_agent_id: cfg.init_agent_id.clone(),
                default_role: cfg.init_role.clone(),
                default_model: cfg.init_model.clone(),
                ..NexusRepo::default()
            };
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

    pub async fn save_nexus(&self) -> Result<()> {
        let repo = self.nexus_repo.read().await;
        let json = serde_json::to_string_pretty(&*repo)?;
        tokio::fs::write(&self.nexus_path, json).await.map_err(|e| Error::IoError(e.to_string()))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn save_station(&self) -> Result<()> {
        let repo = self.station_repo.read().await;
        let json = serde_json::to_string_pretty(&*repo)?;
        tokio::fs::write(&self.station_path, json).await.map_err(|e| Error::IoError(e.to_string()))?;
        Ok(())
    }

    // ========== 静态配置 Getter（直接读，无锁） ==========

    pub fn ws_reconnect_interval_secs(&self) -> u64 { self.agent_config.ws_reconnect_interval_secs }
    pub fn mgmt_host(&self) -> &str { &self.agent_config.mgmt_host }
    pub fn mgmt_port(&self) -> u16 { self.agent_config.mgmt_port }
    #[allow(dead_code)]
    pub fn data_dir(&self) -> &str { &self.agent_config.data_dir }

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
    #[allow(dead_code)]
    pub async fn channels(&self) -> Vec<(String, Arc<ChannelConfig>)> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter().map(|(k, v)| (k.clone(), v.load().clone())).collect()
    }
    #[allow(dead_code)]
    pub async fn channel_ws_url(&self, channel_id: &str) -> Option<String> {
        let repo = self.nexus_repo.read().await;
        repo.channels.get(channel_id).map(|s| s.load().ws_url.to_string())
    }
    #[allow(dead_code)]
    pub async fn add_channel(&self, ch: ChannelConfig) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.channels);
            map.insert(ch.channel_id.to_string(), ArcSwap::new(Arc::new(ch)));
        }
        self.save_nexus().await
    }
    #[allow(dead_code)]
    pub async fn remove_channel(&self, channel_id: &str) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.channels);
            map.remove(channel_id);
        }
        self.save_nexus().await
    }

    // ---------- providers ----------
    /// 合成 provider 默认 + model 覆盖的有效参数（每次调用现场合成，配置永远最新）
    pub async fn resolve_effective_config(&self, pm: &ProviderModel) -> Option<EffectiveModelConfig> {
        let repo = self.nexus_repo.read().await;
        let provider = repo.providers.get(&pm.provider)?.load_full();
        let model_cfg = provider.models.get(&pm.model)?.load_full();
        Some(EffectiveModelConfig {
            provider_type: provider.provider_type.clone(),
            base_url: provider.base_url.clone(),
            api_key: provider.api_key.clone(),
            model: model_cfg.model.clone(),
            max_tokens: model_cfg.max_tokens.unwrap_or(provider.default_max_tokens),
            temperature: model_cfg.temperature.unwrap_or(provider.default_temperature),
            timeout_secs: model_cfg.timeout_secs.unwrap_or(provider.default_timeout_secs),
            retry_count: model_cfg.retry_count.unwrap_or(provider.default_retry_count),
            context_length: model_cfg.context_length.unwrap_or(provider.default_context_length),
        })
    }

    // ---------- providers CRUD（管理 API 使用，落盘） ----------
    /// 添加 provider（重名报 ConfigNotFound），落盘
    #[allow(dead_code)] // 管理 API（http_server）消费后移除
    pub async fn add_provider(&self, cfg: ProviderConfig) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.providers);
            if map.contains_key(&cfg.name.to_string()) {
                return Err(Error::ConfigNotFound(format!("provider 已存在: {}", cfg.name)));
            }
            map.insert(cfg.name.to_string(), ArcSwap::new(Arc::new(cfg)));
        }
        self.save_nexus().await
    }
    /// 删除 provider（不存在报 ConfigNotFound），落盘
    #[allow(dead_code)] // 管理 API（http_server）消费后移除
    pub async fn remove_provider(&self, name: &str) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.providers);
            if !map.contains_key(name) {
                return Err(Error::ConfigNotFound(format!("provider 不存在: {}", name)));
            }
            map.remove(name);
        }
        self.save_nexus().await
    }
    /// 返回 NexusRepo 快照（管理 API GET /config 使用）
    #[allow(dead_code)] // 管理 API（http_server）消费后移除
    pub async fn nexus_snapshot(&self) -> NexusRepo {
        self.nexus_repo.read().await.clone()
    }

    // ---------- memory_structs ----------
    pub async fn memory_structs(&self) -> Vec<MemoryStructConfig> {
        let repo = self.nexus_repo.read().await;
        repo.memory_structs.iter().map(|(_, v)| (*v.load_full()).clone()).collect()
    }

    // ---------- default 读写 ----------
    #[allow(dead_code)]
    pub async fn default_agent_id(&self) -> String { self.nexus_repo.read().await.default_agent_id.to_string() }
    pub async fn default_role(&self) -> String { self.nexus_repo.read().await.default_role.to_string() }
    pub async fn default_model(&self) -> ProviderModel { (*self.nexus_repo.read().await.default_model).clone() }
    /// 设置默认模型（(provider, model) 打包），落盘
    #[allow(dead_code)] // 管理 API（http_server）消费后移除
    pub async fn set_default_model(&self, pm: ProviderModel) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            *Arc::make_mut(&mut repo.default_model) = pm;
        }
        self.save_nexus().await
    }

    // ===== admins（永久操作：聚合 + NexusRepo 回写，check_admin 使用）=====
    /// 聚合所有 channel 的 admins
    pub async fn admin_users(&self) -> Vec<ChannelUser> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter()
            .flat_map(|(_, v)| {
                let c = v.load();
                c.admins.iter().cloned().collect::<Vec<ChannelUser>>()
            })
            .collect()
    }
    /// 添加管理权限（回写 NexusRepo；channel 不存在则报错）
    pub async fn add_admin(&self, channel_id: &str, admin: &ChannelUser) -> Result<()> {
        {
            let repo = self.nexus_repo.write().await;
            let swap = repo.channels.get(channel_id)
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let mut ch = swap.load().clone();
            let ch_mut = Arc::make_mut(&mut ch);
            Arc::make_mut(&mut ch_mut.admins).insert(admin.clone());
            swap.store(ch);
        }
        self.save_nexus().await
    }
    /// 移除管理权限（回写 NexusRepo；channel 不存在则报错）
    /// channel_id 定位 channel，messenger_id 为消息层身份（admin 条目的匹配键）
    pub async fn remove_admin(&self, channel_id: &str, messenger_id: &str, user_id: &str) -> Result<()> {
        {
            let repo = self.nexus_repo.write().await;
            let swap = repo.channels.get(channel_id)
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let mut ch = swap.load().clone();
            let ch_mut = Arc::make_mut(&mut ch);
            let target = ChannelUser { messenger_id: Arc::new(messenger_id.into()), user_id: Arc::new(user_id.into()) };
            Arc::make_mut(&mut ch_mut.admins).remove(&target);
            swap.store(ch);
        }
        self.save_nexus().await
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
            init_agent_id: Arc::new("agent-1".into()),
            init_role: Arc::new("dev".into()),
            init_model: Arc::new(ProviderModel { provider: "deepseek".into(), model: "gpt-4o".into() }),
        }
    }

    #[tokio::test]
    async fn bootstrap_creates_nexus_with_seeds() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let cfg = agent_config(dir.path().to_str().unwrap());
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap(), &cfg).await.unwrap();
        assert_eq!(*repo.default_agent_id, "agent-1");
        assert_eq!(*repo.default_role, "dev");
        assert_eq!(*repo.default_model, ProviderModel { provider: "deepseek".into(), model: "gpt-4o".into() });
        assert!(repo.channels.is_empty());
        assert!(path.exists(), "首次创建应写文件");
    }

    #[tokio::test]
    async fn bootstrap_loads_existing_nexus() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let cfg = agent_config(dir.path().to_str().unwrap());
        // 第一次创建
        let _ = ConfigManager::load_or_create_nexus(path.to_str().unwrap(), &cfg).await.unwrap();
        // 改 init 不影响第二次（文件已存在为权威）
        let cfg2 = AgentConfig { init_agent_id: Arc::new("changed".into()), ..cfg };
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap(), &cfg2).await.unwrap();
        assert_eq!(*repo.default_agent_id, "agent-1", "文件存在时 init_* 应被忽略");
    }

    #[tokio::test]
    async fn save_nexus_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let cfg = agent_config(dir.path().to_str().unwrap());
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap(), &cfg).await.unwrap();
        // 模拟写回再读
        let json = serde_json::to_string_pretty(&repo).unwrap();
        std::fs::write(&path, json).unwrap();
        let back: NexusRepo = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(*back.default_agent_id, "agent-1");
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
        let admin = ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) };
        let err = manager.add_admin("nope", &admin).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        let err = manager.remove_admin("nope", "m1", "u1").await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
    }

    #[test]
    fn channel_user_hash_eq_by_value() {
        let a = ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) };
        let b = ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) };
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
            default_agent_id: Arc::new("agent-1".into()),
            default_role: Arc::new("dev".into()),
            default_model: Arc::new(ProviderModel { provider: "deepseek".into(), model: "gpt-4o".into() }),
        };
        let json = serde_json::to_string(&repo).unwrap();
        let back: NexusRepo = serde_json::from_str(&json).unwrap();
        assert_eq!(*back.default_agent_id, "agent-1");
        assert_eq!(*back.default_role, "dev");
        assert_eq!(*back.default_model, ProviderModel { provider: "deepseek".into(), model: "gpt-4o".into() });
    }

    #[test]
    fn nexus_repo_default_empty() {
        let repo = NexusRepo::default();
        assert!(repo.channels.is_empty());
        assert!(repo.providers.is_empty());
        assert!(repo.memory_structs.is_empty());
        assert!(repo.stations.is_empty());
        assert!(repo.default_agent_id.is_empty());
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
            default_temperature: 0.7,
            default_timeout_secs: 60,
            default_retry_count: 3,
            models: Arc::new(ArcSwapHashMap::new()),
        }
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
        assert_eq!(eff.temperature, 0.3, "model 的 temperature 应生效");
        assert_eq!(eff.timeout_secs, 30);
        assert_eq!(eff.retry_count, 2);
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
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("deepseek-4-flash".to_string(), ArcSwap::new(Arc::new(ModelConfig {
                model: "deepseek-4-flash".into(),
                max_tokens: None,
                temperature: None,
                timeout_secs: None,
                retry_count: None,
                context_length: Some(131072),  // 覆盖 context_length
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
        assert_eq!(eff.temperature, 0.7);
        assert_eq!(eff.timeout_secs, 60);
        assert_eq!(eff.retry_count, 3);
        assert_eq!(eff.context_length, 131072, "model 覆盖 context_length 应生效");
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
        assert!(manager.resolve_effective_config(&ProviderModel { provider: "nope".into(), model: "m".into() }).await.is_none());
        assert!(manager.resolve_effective_config(&ProviderModel { provider: "deepseek".into(), model: "nope".into() }).await.is_none());
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
        let eff = manager.resolve_effective_config(&ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() }).await;
        assert!(eff.is_none(), "provider 无 models 时 model 不存在返回 None");
        // 带 models 的 provider
        let mut provider = sample_provider("openai");
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("gpt-4o".into(), ArcSwap::new(Arc::new(ModelConfig {
                model: "gpt-4o".into(), max_tokens: None, temperature: None,
                timeout_secs: None, retry_count: None, context_length: None,
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
}
