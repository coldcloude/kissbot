use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::types::{Mode, Result, Error};

// ========== 配置数据结构 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub timeout_secs: u64,
    pub retry_count: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            endpoint: String::new(),
            api_key: String::new(),
            model: "gpt-4o".to_string(),
            max_tokens: 4096,
            temperature: 0.7,
            timeout_secs: 60,
            retry_count: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelBinding {
    pub messenger_id: String,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUser {
    pub messenger_id: String,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationConfig {
    pub station_id: String,
    pub base_url: String,
    pub timeout_secs: u64,
}

/// JSON 文件完整配置结构：仅用于序列化/反序列化，加载后拆分为只读+可变两部分
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentConfigFile {
    agent_id: String,
    llm: LlmConfig,
    current_role: String,
    current_mode: AgentModeFile,
    channel_bindings: Vec<ChannelBinding>,
    admin_users: Vec<AdminUser>,
    stations: Vec<StationConfig>,
    channel_ws_url: Option<String>,
    memory_struct_url: Option<String>,
    ws_reconnect_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentModeFile {
    #[serde(rename = "type")]
    mode_type: String,
    event_id: Option<String>,
}

/// 只读配置（从 JSON 加载后不变）
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub agent_id: String,
    pub llm: LlmConfig,
    pub stations: Vec<StationConfig>,
    channel_ws_url: Option<String>,
    memory_struct_url: Option<String>,
    ws_reconnect_interval_secs: u64,
}

/// 可变运行时配置（管理命令修改，持久化到同一 JSON 文件）
#[derive(Debug, Clone)]
pub struct AgentRuntimeConfig {
    pub current_role: String,
    pub current_mode: Mode,
    pub channel_bindings: Vec<ChannelBinding>,
    pub admin_users: Vec<AdminUser>,
}

/// 配置变更监听器：当配置被管理命令修改时，通知外部协调器
pub trait ConfigChangeListener: Send + Sync {
    fn on_config_changed(&self, config_manager: &ConfigManager);
}

pub struct ConfigManager {
    config_path: String,
    /// 只读配置，加载后不变
    agent_config: AgentConfig,
    /// 可变运行时配置
    runtime_config: RwLock<AgentRuntimeConfig>,
    listeners: DashMap<String, Arc<dyn ConfigChangeListener>>,
}

impl ConfigManager {
    /// 从文件加载配置
    pub async fn load(config_path: &str) -> Result<Self> {
        let content = tokio::fs::read_to_string(config_path).await
            .map_err(|e| Error::ConfigNotFound(format!("{}: {}", config_path, e)))?;
        let file: AgentConfigFile = serde_json::from_str(&content)
            .map_err(|e| Error::ConfigParseError(e.to_string()))?;

        let mode = match file.current_mode.mode_type.as_str() {
            "role" => Mode::Role,
            "event" => Mode::Event(file.current_mode.event_id.unwrap_or_default()),
            _ => Mode::Role,
        };

        let agent_config = AgentConfig {
            agent_id: file.agent_id,
            llm: file.llm,
            stations: file.stations,
            channel_ws_url: file.channel_ws_url,
            memory_struct_url: file.memory_struct_url,
            ws_reconnect_interval_secs: file.ws_reconnect_interval_secs,
        };

        let runtime_config = AgentRuntimeConfig {
            current_role: file.current_role,
            current_mode: mode,
            channel_bindings: file.channel_bindings,
            admin_users: file.admin_users,
        };

        Ok(Self {
            config_path: config_path.to_string(),
            agent_config,
            runtime_config: RwLock::new(runtime_config),
            listeners: DashMap::new(),
        })
    }

    /// 持久化当前配置到文件
    pub async fn save(&self) -> Result<()> {
        let runtime = self.runtime_config.read().await;
        let mode = match &runtime.current_mode {
            Mode::Role => AgentModeFile { mode_type: "role".to_string(), event_id: None },
            Mode::Event(id) => AgentModeFile { mode_type: "event".to_string(), event_id: Some(id.clone()) },
        };
        let file = AgentConfigFile {
            agent_id: self.agent_config.agent_id.clone(),
            llm: self.agent_config.llm.clone(),
            current_role: runtime.current_role.clone(),
            current_mode: mode,
            channel_bindings: runtime.channel_bindings.clone(),
            admin_users: runtime.admin_users.clone(),
            stations: self.agent_config.stations.clone(),
            channel_ws_url: self.agent_config.channel_ws_url.clone(),
            memory_struct_url: self.agent_config.memory_struct_url.clone(),
            ws_reconnect_interval_secs: self.agent_config.ws_reconnect_interval_secs,
        };
        let json = serde_json::to_string_pretty(&file)?;
        tokio::fs::write(&self.config_path, json).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        Ok(())
    }

    /// 注册配置变更监听器
    #[allow(dead_code)]
    pub fn add_listener(&self, key: &str, listener: Arc<dyn ConfigChangeListener>) {
        self.listeners.insert(key.to_string(), listener);
    }

    /// 通知所有监听器
    async fn notify_listeners(&self) {
        let listeners: Vec<Arc<dyn ConfigChangeListener>> = self.listeners
            .iter().map(|e| e.value().clone()).collect();
        for listener in &listeners {
            listener.on_config_changed(self);
        }
    }

    // ========== 只读配置 Getter（直接读，无锁） ==========

    pub fn agent_id(&self) -> &str {
        &self.agent_config.agent_id
    }

    pub fn llm_config(&self) -> &LlmConfig {
        &self.agent_config.llm
    }

    #[allow(dead_code)]
    pub fn stations(&self) -> &[StationConfig] {
        &self.agent_config.stations
    }

    pub fn ws_reconnect_interval_secs(&self) -> u64 {
        self.agent_config.ws_reconnect_interval_secs
    }

    pub fn channel_ws_url(&self) -> &str {
        self.agent_config.channel_ws_url.as_deref().unwrap_or("ws://localhost:8080/ws")
    }

    pub fn memory_struct_url(&self) -> &str {
        self.agent_config.memory_struct_url.as_deref().unwrap_or("")
    }

    // ========== 可变配置 Getter（读锁） ==========

    pub async fn current_role(&self) -> String {
        self.runtime_config.read().await.current_role.clone()
    }

    pub async fn current_mode(&self) -> Mode {
        self.runtime_config.read().await.current_mode.clone()
    }

    pub async fn channel_bindings(&self) -> Vec<ChannelBinding> {
        self.runtime_config.read().await.channel_bindings.clone()
    }

    pub async fn admin_users(&self) -> Vec<AdminUser> {
        self.runtime_config.read().await.admin_users.clone()
    }

    // ========== Setter（管理命令调用，自动持久化） ==========

    pub async fn set_current_role(&self, role: Option<String>) -> Result<()> {
        self.runtime_config.write().await.current_role = role.unwrap_or_default();
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn set_current_mode(&self, mode: Mode) -> Result<()> {
        self.runtime_config.write().await.current_mode = mode;
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn add_binding(&self, binding: ChannelBinding) -> Result<()> {
        self.runtime_config.write().await.channel_bindings.push(binding);
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn remove_binding(&self, messenger_id: &str) -> Result<()> {
        self.runtime_config.write().await.channel_bindings.retain(|b| b.messenger_id != messenger_id);
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn add_admin(&self, admin: AdminUser) -> Result<()> {
        self.runtime_config.write().await.admin_users.push(admin);
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn remove_admin(&self, messenger_id: &str, user_id: &str) -> Result<()> {
        self.runtime_config.write().await.admin_users
            .retain(|a| !(a.messenger_id == messenger_id && a.user_id == user_id));
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }
}
