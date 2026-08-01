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
    /// 绑定用户（必填；auto-bind 功能以后再做）
    /// 旧字段名 default_bind_user 别名兼容旧 nexus.json
    #[serde(alias = "default_bind_user")]
    pub bind_user: ChannelUser,
    /// 绑定的 agent_id（仅空 = 脱离 agent，该 channel 只处理管理命令；"0" = 挂载保留 agent "0"）
    #[serde(default)]
    pub agent_id: Arc<String>,
    #[serde(default)]
    pub role_name: Arc<String>,
    /// 是否选为该会话的发送 channel
    #[serde(default)]
    pub is_send_channel: bool,
    /// 是否启用（连接由 enabled 控制）
    /// 旧字段名 enabled_by_default 别名兼容旧 nexus.json
    #[serde(alias = "enabled_by_default")]
    pub enabled: bool,
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
    pub default_system_prompt: Arc<String>,   // 保留 agent "0" 的默认系统提示词（不调 memory-ego 时用）
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
    pub fn default_system_prompt(&self) -> &str { &self.agent_config.default_system_prompt }

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

    /// 修改 channel 配置并落盘（绑定/agent/role/is_send_channel 等运行时回写统一入口）
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
        Some(EffectiveModelConfig {
            provider_type: provider.provider_type.clone(),
            base_url: provider.base_url.clone(),
            api_key: provider.api_key.clone(),
            model: pm.model.clone(),   // 用切换指令的模型名（未配置也有效）
            max_tokens: model_cfg.as_ref().and_then(|m| m.max_tokens).unwrap_or(provider.default_max_tokens),
            temperature: model_cfg.as_ref().and_then(|m| m.temperature).unwrap_or(provider.default_temperature),
            timeout_secs: model_cfg.as_ref().and_then(|m| m.timeout_secs).unwrap_or(provider.default_timeout_secs),
            retry_count: model_cfg.as_ref().and_then(|m| m.retry_count).unwrap_or(provider.default_retry_count),
            context_length: model_cfg.as_ref().and_then(|m| m.context_length).unwrap_or(provider.default_context_length),
        })
    }

    /// 按名取 provider 配置（Arc 快照），供 provider 构造（model_client.list_models 使用）
    /// 不存在返回 None
    pub async fn provider_config_by_name(&self, name: &str) -> Option<Arc<ProviderConfig>> {
        self.nexus_repo.read().await.providers.get(name).map(|s| s.load_full())
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
    pub async fn memory_structs(&self) -> Vec<MemoryStructConfig> {
        let repo = self.nexus_repo.read().await;
        repo.memory_structs.iter().map(|(_, v)| (*v.load_full()).clone()).collect()
    }

    // ---------- default 读写 ----------
    #[allow(dead_code)]
    pub async fn default_agent_id(&self) -> String { self.nexus_repo.read().await.default_agent_id.to_string() }
    #[allow(dead_code)]
    pub async fn default_role(&self) -> String { self.nexus_repo.read().await.default_role.to_string() }
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
            let target = ChannelUser { messenger_id: Arc::new(messenger_id.into()), user_id: Arc::new(user_id.into()) };
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
            default_system_prompt: Arc::new("你是 kissbot 智能助手".into()),
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
    async fn nexus_json_file_roundtrip() {
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
        let admin = ChannelUser { messenger_id: Arc::new("m1".into()), user_id: Arc::new("u1".into()) };
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
            bind_user: ChannelUser { messenger_id: Arc::new("web".into()), user_id: Arc::new("u1".into()) },
            agent_id: Arc::new("0".into()),
            role_name: Arc::new("0".into()),
            is_send_channel: true,
            enabled: true,
        }
    }

    #[test]
    fn channel_config_new_shape_serde_roundtrip() {
        let ch = sample_channel("web-main");
        let json = serde_json::to_string(&ch).unwrap();
        assert!(json.contains("\"bind_user\""), "应序列化 bind_user");
        assert!(json.contains("\"agent_id\""));
        assert!(json.contains("\"role_name\""));
        assert!(json.contains("\"is_send_channel\""));
        assert!(json.contains("\"enabled\""));
        let back: ChannelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(*back.channel_id, "web-main");
        assert_eq!(*back.bind_user.user_id, "u1");
    }

    #[test]
    fn channel_config_old_shape_alias_migration() {
        // 旧格式：default_bind_user / enabled_by_default，缺 agent_id/role_name/is_send_channel
        let old = r#"{
            "channel_id": "web-main",
            "ws_url": "ws://127.0.0.1:8201",
            "admins": [],
            "default_bind_user": { "messenger_id": "web", "user_id": "u1" },
            "enabled_by_default": true
        }"#;
        let ch: ChannelConfig = serde_json::from_str(old).unwrap();
        assert_eq!(*ch.bind_user.messenger_id, "web");
        assert!(ch.enabled, "旧字段 enabled_by_default 应映射到 enabled");
        assert!(ch.agent_id.is_empty(), "缺省 agent_id 应为空（脱离态）");
        assert!(ch.role_name.is_empty());
        assert!(!ch.is_send_channel);
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

        // 修改 agent_id/role_name/is_send_channel
        manager.update_channel("web-main", |c| {
            c.agent_id = Arc::new("a1".into());
            c.role_name = Arc::new("r1".into());
            c.is_send_channel = false;
        }).await.unwrap();

        // 内存可见
        let ch = manager.channels().await.into_iter()
            .find(|(id, _)| id == "web-main").map(|(_, c)| c).unwrap();
        assert_eq!(*ch.agent_id, "a1");
        assert!(!ch.is_send_channel);

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
        // provider 不存在 → None
        assert!(manager.resolve_effective_config(&ProviderModel { provider: "nope".into(), model: "m".into() }).await.is_none());
        // provider 存在但 model 未配置 → Some（用 provider 默认值合成）
        manager.add_provider(sample_provider("deepseek")).await.unwrap();
        let eff = manager.resolve_effective_config(&ProviderModel { provider: "deepseek".into(), model: "nope".into() }).await
            .expect("model 未配置也应合成");
        assert_eq!(eff.model, "nope", "model 用切换指令的模型名");
        assert_eq!(eff.max_tokens, 4096, "未配置参数取 provider 默认值");
        assert_eq!(eff.temperature, 0.7);
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
        assert_eq!(eff.temperature, 0.7);
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
