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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawConfig {
    agent_id: String,
    llm: LlmConfig,
    current_role: String,
    current_mode: RawMode,
    channel_bindings: Vec<ChannelBinding>,
    admin_users: Vec<AdminUser>,
    stations: Vec<StationConfig>,
    memory_struct_url: Option<String>,
    ws_reconnect_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawMode {
    #[serde(rename = "type")]
    mode_type: String,
    event_id: Option<String>,
}

/// 配置变更监听器：当配置被管理命令修改时，通知外部协调器
pub trait ConfigChangeListener: Send + Sync {
    fn on_config_changed(&self, config_manager: &ConfigManager);
}

pub struct ConfigManager {
    config_path: String,
    inner: RwLock<ConfigInner>,
    listeners: DashMap<String, Arc<dyn ConfigChangeListener>>,
}

struct ConfigInner {
    agent_id: String,
    llm: LlmConfig,
    current_role: String,
    current_mode: Mode,
    channel_bindings: Vec<ChannelBinding>,
    admin_users: Vec<AdminUser>,
    stations: Vec<StationConfig>,
    memory_struct_url: Option<String>,
    ws_reconnect_interval_secs: u64,
}

impl ConfigManager {
    /// 从文件加载配置
    pub async fn load(config_path: &str) -> Result<Self> {
        let content = tokio::fs::read_to_string(config_path).await
            .map_err(|e| Error::ConfigNotFound(format!("{}: {}", config_path, e)))?;
        let raw: RawConfig = serde_json::from_str(&content)
            .map_err(|e| Error::ConfigParseError(e.to_string()))?;

        let mode = match raw.current_mode.mode_type.as_str() {
            "role" => Mode::Role,
            "event" => Mode::Event(raw.current_mode.event_id.unwrap_or_default()),
            _ => Mode::Role,
        };

        let inner = ConfigInner {
            agent_id: raw.agent_id,
            llm: raw.llm,
            current_role: raw.current_role,
            current_mode: mode,
            channel_bindings: raw.channel_bindings,
            admin_users: raw.admin_users,
            stations: raw.stations,
            memory_struct_url: raw.memory_struct_url,
            ws_reconnect_interval_secs: raw.ws_reconnect_interval_secs,
        };

        Ok(Self {
            config_path: config_path.to_string(),
            inner: RwLock::new(inner),
            listeners: DashMap::new(),
        })
    }

    /// 持久化当前配置到文件
    pub async fn save(&self) -> Result<()> {
        let inner = self.inner.read().await;
        let mode = match &inner.current_mode {
            Mode::Role => RawMode { mode_type: "role".to_string(), event_id: None },
            Mode::Event(id) => RawMode { mode_type: "event".to_string(), event_id: Some(id.clone()) },
        };
        let raw = RawConfig {
            agent_id: inner.agent_id.clone(),
            llm: inner.llm.clone(),
            current_role: inner.current_role.clone(),
            current_mode: mode,
            channel_bindings: inner.channel_bindings.clone(),
            admin_users: inner.admin_users.clone(),
            stations: inner.stations.clone(),
            memory_struct_url: inner.memory_struct_url.clone(),
            ws_reconnect_interval_secs: inner.ws_reconnect_interval_secs,
        };
        let json = serde_json::to_string_pretty(&raw)?;
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

    // ========== Getter ==========

    pub async fn agent_id(&self) -> String {
        self.inner.read().await.agent_id.clone()
    }

    pub async fn llm_config(&self) -> LlmConfig {
        self.inner.read().await.llm.clone()
    }

    pub async fn current_role(&self) -> String {
        self.inner.read().await.current_role.clone()
    }

    pub async fn current_mode(&self) -> Mode {
        self.inner.read().await.current_mode.clone()
    }

    pub async fn channel_bindings(&self) -> Vec<ChannelBinding> {
        self.inner.read().await.channel_bindings.clone()
    }

    pub async fn admin_users(&self) -> Vec<AdminUser> {
        self.inner.read().await.admin_users.clone()
    }

    #[allow(dead_code)]
    pub async fn stations(&self) -> Vec<StationConfig> {
        self.inner.read().await.stations.clone()
    }

    #[allow(dead_code)]
    pub async fn ws_reconnect_interval_secs(&self) -> u64 {
        self.inner.read().await.ws_reconnect_interval_secs
    }

    pub async fn memory_struct_url(&self) -> String {
        self.inner.read().await.memory_struct_url.clone().unwrap_or_default()
    }

    // ========== Setter（管理命令调用，自动持久化） ==========

    pub async fn set_current_role(&self, role: Option<String>) -> Result<()> {
        self.inner.write().await.current_role = role.unwrap_or_default();
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn set_current_mode(&self, mode: Mode) -> Result<()> {
        self.inner.write().await.current_mode = mode;
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn add_binding(&self, binding: ChannelBinding) -> Result<()> {
        self.inner.write().await.channel_bindings.push(binding);
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn remove_binding(&self, messenger_id: &str) -> Result<()> {
        self.inner.write().await.channel_bindings.retain(|b| b.messenger_id != messenger_id);
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn add_admin(&self, admin: AdminUser) -> Result<()> {
        self.inner.write().await.admin_users.push(admin);
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }

    pub async fn remove_admin(&self, messenger_id: &str, user_id: &str) -> Result<()> {
        self.inner.write().await.admin_users
            .retain(|a| !(a.messenger_id == messenger_id && a.user_id == user_id));
        self.save().await?;
        self.notify_listeners().await;
        Ok(())
    }
}
