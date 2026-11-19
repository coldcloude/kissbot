use std::sync::{Arc, OnceLock};

use arcstr::ArcStr;
use kissbot_api::{MergeBy, OptionArcField};
use kissbot_api::{ChannelUser, RoleKey, write_key_value};
use tokio::sync::RwLock;

use kissbot_api::{MergeEffectiveConfig, MergeSelf};
use kissbot_llm::{EffectiveModelConfig, ProviderConfig, ProviderModelConfig};

use crate::configs::*;
use crate::types::*;

/// ConfigManager 全局单例（进程内唯一；new() 完成时注册，此后 get() 可用）。
/// 与 Nexus 同模式：任何模块读配置直接 ConfigManager::get()，不传参、不持引用。
static INSTANCE: OnceLock<ConfigManager> = OnceLock::new();

// 注：过渡期曾 derive(Clone) 支撑“仍返回实例 + 注册单例”双所有权（Task 6 前）；
// 现 new() 不返回实例，无 Clone 消费方，移除
pub struct ConfigManager {
    agent_config: AgentConfig,
    nexus_repo: Arc<RwLock<NexusRepo>>,
    station_repo: Arc<RwLock<StationRepo>>,
    nexus_path: ArcStr,
    station_path: ArcStr,
    context_dir: ArcStr,
}

impl ConfigManager {
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn get() -> &'static ConfigManager {
        INSTANCE.get().expect("ConfigManager 未初始化")
    }

    /// 从公共配置加载 AgentConfig，按 data_dir 加载/引导 NexusRepo/StationRepo；
    /// 完成时注册全局单例（此后 get() 可用，不返回实例）
    pub async fn new() -> Result<()> {
        let agent_config = AgentConfig::from_public_config();
        let data_dir = agent_config.data_dir.to_string();
        tokio::fs::create_dir_all(&data_dir).await?;
        // 派生子目录（仅创建，功能本轮不实现）
        for sub in ["sessions", "attachments", "station"] {
            let _ = tokio::fs::create_dir_all(format!("{}/{}", data_dir, sub)).await;
        }
        let nexus_path = ArcStr::from(format!("{}/nexus.json", data_dir));
        let station_path = ArcStr::from(format!("{}/station.json", data_dir));
        let context_dir = ArcStr::from(format!("{}/context", data_dir));

        let nexus_repo = Self::load_or_create_nexus(&nexus_path).await?;
        let station_repo = Self::load_or_create_station(&station_path).await?;

        let manager = Self {
            agent_config,
            nexus_repo: Arc::new(RwLock::new(nexus_repo)),
            station_repo: Arc::new(RwLock::new(station_repo)),
            nexus_path,
            station_path,
            context_dir,
        };
        // 注册全局单例（此后 get() 可用；重复调用幂等，第二次 set 被忽略，与 Nexus 一致）
        let _ = INSTANCE.set(manager);
        Ok(())
    }

    async fn load_or_create_nexus(path: &str) -> Result<NexusRepo> {
        if std::path::Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await?;
            let repo: NexusRepo = serde_json::from_str(&content)?;
            Ok(repo)
        } else {
            // 首次创建：默认空配置（channels/providers/(agent, role) 配置由 nexus.json 模板或管理 API 填写）
            let repo = NexusRepo::default();
            let json = serde_json::to_string_pretty(&repo)?;
            tokio::fs::write(path, json).await?;
            Ok(repo)
        }
    }

    async fn load_or_create_station(path: &str) -> Result<StationRepo> {
        if std::path::Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await?;
            let repo: StationRepo = serde_json::from_str(&content)?;
            Ok(repo)
        } else {
            let repo = StationRepo::default();
            let json = serde_json::to_string_pretty(&repo)?;
            tokio::fs::write(path, json).await?;
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
        tokio::fs::write(self.nexus_path.as_str(), json.as_bytes()).await?;
        Ok(rst)
    }

    /// 写 Station 配置（同 write_nexus_config 模式；station 功能未实现，暂无调用方）
    async fn write_station_config<F, R>(&self, op: F) -> Result<R>
    where
        F: FnOnce(&mut StationRepo) -> Result<R>,
    {
        let mut guard = self.station_repo.write().await;
        let rst = op(&mut *guard)?;
        let json = serde_json::to_string_pretty(&*guard)?;
        tokio::fs::write(self.station_path.as_str(), json.as_bytes()).await?;
        Ok(rst)
    }

    // ========== 静态配置 Getter（直接读，无锁） ==========

    pub fn ws_reconnect_interval_secs(&self) -> u64 {
        self.agent_config.ws_reconnect_interval_secs
    }

    pub fn mgmt_host(&self) -> &str {
        &self.agent_config.mgmt_host
    }

    pub fn mgmt_port(&self) -> u16 {
        self.agent_config.mgmt_port
    }

    pub fn station_host(&self) -> &str {
        self.agent_config.station_host.as_str()
    }

    pub fn station_port(&self) -> u16 {
        self.agent_config.station_port
    }

    pub fn data_dir(&self) -> &str {
        self.agent_config.data_dir.as_str()
    }

    /// 会话上下文目录（<data_dir>/context）：SessionContext 的 data_dir 直接用它，
    /// 当前缓存放其下、版本放其 versions/ 子目录（保持原有缓存路径不变）
    pub fn context_dir(&self) -> &str {
        self.context_dir.as_str()
    }

    // ========== NexusRepo CRUD ==========

    // ---------- channels ----------
    /// 返回 StationRepo 快照（Station 单例构建使用）
    pub async fn station_repo_snapshot(&self) -> StationRepo {
        self.station_repo.read().await.clone()
    }
    /// 按 channel_id 直接查找单个 channel 配置（map O(1) get，不克隆整个 map 再遍历）
    pub async fn channel(&self, channel_id: &str) -> Option<Arc<ChannelConfig>> {
        let repo = self.nexus_repo.read().await;
        repo.channels.get(channel_id).map(|s| s.clone())
    }
    pub async fn add_channel(&self, ch: ChannelConfig) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.channels);
            map.insert(ch.channel_id.clone(), Arc::new(ch));
            Ok(())
        }).await
    }
    pub async fn remove_channel(&self, channel_id: &str) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.channels);
            map.remove(channel_id);
            Ok(())
        }).await
    }

    // ---------- providers ----------
    /// 合成 provider 默认 + model 覆盖的有效参数（每次调用现场合成，配置永远最新）
    /// model 未在 provider.model_configs 配置时用 provider 默认值合成（极端 model_configs={} 也可用）
    pub async fn provider_model_config(&self, provider: &str, model: &str) -> Option<EffectiveModelConfig> {
        let repo = self.nexus_repo.read().await;
        let provider = repo.providers.get(provider)?;
        Some(provider.get_effective_config(model))
    }

    /// 按名取 provider 配置（Arc 快照），供 provider 构造（kissbot-llm 的 create_*_provider / list_models 使用）
    /// 不存在返回 None
    pub async fn provider_config(&self, name: &str) -> Option<Arc<ProviderConfig>> {
        let repo = self.nexus_repo.read().await;
        let provider_model_config = repo.providers.get(name);
        if let Some(config) = provider_model_config {
            let provider_config = config.provider_config.clone();
            Some(provider_config)
        } else {
            None
        }
    }

    /// 设置 role 配置，role_name 为空时设置 agent配置
    pub async fn set_agent_role_config<C>(&self, agent_id: ArcStr, role_name: ArcStr, config: Arc<C>) -> Result<()>
    where
        C: MergeSelf + Clone,
        SessionConfigMergeMap: OptionArcField<C>,
    {
        self.write_nexus_config(|repo| {
            let agent_roles = Arc::make_mut(&mut repo.agent_roles);
            // agent 条目不存在则懒建
            let agent_role_config = if let Some(mut agent_role_config) = agent_roles.remove(&agent_id) {
                let agent_role_config_mut = Arc::make_mut(&mut agent_role_config);
                agent_role_config_mut.set::<C>(role_name, config);
                agent_role_config
            } else {
                let mut agent_role_config = AgentRoleConfig::default();
                agent_role_config.set::<C>(role_name, config);
                Arc::new(agent_role_config)
            };
            agent_roles.insert(agent_id, agent_role_config);
            Ok(())
        }).await
    }

    pub async fn agent_role_config<C>(&self, agent_id: &str, role_name: &str) -> Option<C>
    where
        C: MergeSelf + Clone,
        SessionConfigMergeMap: OptionArcField<C>,
    {
        let repo = self.nexus_repo.read().await;
        let agent_role_config = repo.agent_roles.get(agent_id);
        if let Some(agent_role_config) = agent_role_config {
            return Some(agent_role_config.build::<C>(role_name))
        }
        None
    }

    // ---------- pipelines ----------

    /// 全部已配置的 pipeline（身份 + 配置）；按 (agent_id, role_name) 排序保证启动装配顺序稳定
    pub async fn pipelines(&self) -> Vec<(Arc<RoleKey>, Arc<PipelineConfig>)> {
        let repo = self.nexus_repo.read().await;
        let mut result: Vec<(Arc<RoleKey>, Arc<PipelineConfig>)> = repo.pipelines.iter()
            .flat_map(|(agent_id, roles)| {
                roles.iter().map(move |(role_name, config)| {
                    let key = Arc::new(RoleKey {
                        agent_id: agent_id.clone(),
                        role_name: role_name.clone(),
                    });
                    (key, config.clone())
                })
            })
            .collect();
        result.sort_by(|a, b| {
            (a.0.agent_id.as_str(), a.0.role_name.as_str())
                .cmp(&(b.0.agent_id.as_str(), b.0.role_name.as_str()))
        });
        result
    }

    /// 单测用：写入/覆盖一条 pipeline 定义（生产环境由配置文件提供）
    #[cfg(test)]
    pub async fn set_pipeline_config(&self, agent_id: ArcStr, role_name: ArcStr, config: Arc<PipelineConfig>) -> Result<()> {
        self.write_nexus_config(|repo| {
            let pipelines = Arc::make_mut(&mut repo.pipelines);
            let roles = Arc::make_mut(pipelines.entry(agent_id).or_default());
            roles.insert(role_name, config);
            Ok(())
        }).await
    }

    /// 单测用：删除一条 pipeline 定义
    #[cfg(test)]
    pub async fn remove_pipeline_config(&self, agent_id: &str, role_name: &str) -> Result<()> {
        self.write_nexus_config(|repo| {
            let pipelines = Arc::make_mut(&mut repo.pipelines);
            if let Some(roles) = pipelines.get_mut(agent_id) {
                Arc::make_mut(roles).remove(role_name);
            }
            Ok(())
        }).await
    }

    /// 按 session_id 取 session 配置项；缺失时用 `C::default()` 的有效配置建项并缓存（不落盘）
    pub async fn session_config<C, E>(&self, session_id: ArcStr) -> Arc<E>
    where
        C: MergeSelf + MergeEffectiveConfig<E>,
        E: MergeBy<C>,
        SessionConfigMap: OptionArcField<E>,
    {
        // 先尝试获取 session 配置，只读repo
        {
            let repo = self.nexus_repo.read().await;
            if let Some(session_config_map) = repo.sessions.get(&session_id) {
                if let Some(session_config) = session_config_map.as_ref().get::<E>() {
                    return session_config;
                }
            }
        }

        // 需要新建 session 配置，需要写权限
        let mut repo = self.nexus_repo.write().await;
        // 新建 session 配置
        let mut config = Arc::new(C::default().get_effective_config());
        // 保存 session config
        let sessions = Arc::make_mut(&mut repo.sessions);
        let config_map = if let Some(mut config_map) = sessions.remove(&session_id) {
            let config_map_mut = Arc::make_mut(&mut config_map);
            config = config_map_mut.get_or_insert(config);
            config_map
        } else {
            let mut config_map = SessionConfigMap::default();
            config_map.insert::<E>(config.clone());
            Arc::new(config_map)
        };
        sessions.insert(session_id, config_map);
        config
    }

    /// 把 C 覆盖合并进 session 配置项并落盘；缺失时以 `C::default()` 为基底
    pub async fn set_session_config<C, E>(&self, session_id: ArcStr, config: &C) -> Result<()>
    where
        C: MergeSelf + MergeEffectiveConfig<E>,
        E: MergeBy<C> + Clone,
        SessionConfigMap: OptionArcField<E>,
    {
        self.write_nexus_config(|repo| {
            let sessions = Arc::make_mut(&mut repo.sessions);
            let config_map = if let Some(mut config_map) = sessions.remove(&session_id) {
                let config_map_mut = Arc::make_mut(&mut config_map);
                config_map_mut.merge::<C, E>(config);
                config_map
            } else {
                // 新建 session map
                let mut config_map = SessionConfigMap::default();
                config_map.merge::<C, E>(config);
                Arc::new(config_map)
            };
            sessions.insert(session_id, config_map);
            Ok(())
        }).await
    }

    /// session 配置项不存在时写入并落盘；已存在则保留
    pub async fn set_session_config_if_not_exists<E>(&self, session_id: ArcStr, config: Arc<E>) -> Result<()>
    where
        E: Clone,
        SessionConfigMap: OptionArcField<E>,
    {
        self.write_nexus_config(|repo| {
            let sessions = Arc::make_mut(&mut repo.sessions);
            let config_map = if let Some(mut config_map) = sessions.remove(&session_id) {
                let config_map_mut = Arc::make_mut(&mut config_map);
                let new_config = if let Some(exist_config) = config_map_mut.take::<E>() {
                    exist_config
                } else {
                    config
                };
                config_map_mut.insert::<E>(new_config);
                config_map
            } else {
                // 新建 session map
                let mut config_map = SessionConfigMap::default();
                config_map.insert::<E>(config);
                Arc::new(config_map)
            };
            sessions.insert(session_id, config_map);
            Ok(())
        }).await
    }

    /// 把 from 的整份 session 配置复制到 to 并落盘（to 已有配置时整体覆盖）。
    /// from 不存在时写入空配置，后续按需用 `C::default()` 建项
    pub async fn copy_session_config(&self, from: ArcStr, to: ArcStr) -> Result<()> {
        self.write_nexus_config(|repo| {
            let sessions = Arc::make_mut(&mut repo.sessions);
            let config_map = sessions.get(&from).cloned().unwrap_or_default();
            sessions.insert(to, config_map);
            Ok(())
        }).await
    }

    // ---------- providers CRUD（管理 API 使用，落盘） ----------
    /// 添加 provider（重名报 ConfigNotFound），落盘
    pub async fn add_provider(&self, name: &str, cfg: ProviderModelConfig) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.providers);
            if map.contains_key(name) {
                return Err(Error::ConfigNotFound(format!("provider 已存在: {}", name.to_string()).into()));
            }
            map.insert(ArcStr::from(name), Arc::new(cfg));
            Ok(())
        }).await
    }
    /// 删除 provider（不存在报 ConfigNotFound），落盘
    pub async fn remove_provider(&self, name: &str) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.providers);
            if !map.contains_key(name) {
                return Err(Error::ConfigNotFound(format!("provider 不存在: {}", name).into()));
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
    // memory-struct 功能未实现，待后续接入时使用
    pub async fn memory_structs(&self) -> Vec<MemoryStructConfig> {
        let repo = self.nexus_repo.read().await;
        repo.memory_structs.iter().map(|(_, v)| v.as_ref().clone()).collect()
    }

    // ===== admins =====
    /// 添加管理权限（回写 NexusRepo；channel 不存在则报错）
    pub async fn add_admin(&self, channel_id: &str, admin: Arc<ChannelUser>) -> Result<()> {
        self.write_nexus_config(|repo| {
            let channels = Arc::make_mut(&mut repo.channels);
            write_key_value(channels, channel_id, |_, ch: &mut ChannelConfig| {
                Arc::make_mut(&mut ch.admins).insert(admin);
            }).ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id).into()))?;
            Ok(())
        }).await
    }
    /// 移除管理权限（回写 NexusRepo；channel 不存在则报错）
    /// channel_id 定位 channel，messenger_id 为消息层身份（admin 条目的匹配键）
    pub async fn remove_admin(&self, channel_id: &str, target: &ChannelUser) -> Result<()> {
        self.write_nexus_config(|repo| {
            let channels = Arc::make_mut(&mut repo.channels);
            write_key_value(channels, channel_id, |_, ch: &mut ChannelConfig| {
                Arc::make_mut(&mut ch.admins).remove(target);
            }).ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id).into()))?;
            Ok(())
        }).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use kissbot_api::MergeEffectiveConfig;
    use kissbot_llm::{DEFAULT_RETRY_COUNT, DEFAULT_TIMEOUT_SECS, EffectiveLLMConfig, LLMConfig, ModelConfig};
    use tempfile::tempdir;

    fn agent_config(data_dir: &str) -> AgentConfig {
        AgentConfig {
            data_dir: ArcStr::from(data_dir),
            mgmt_host: ArcStr::from("127.0.0.1"),
            mgmt_port: 9090,
            station_host: ArcStr::from("127.0.0.1"),
            station_port: 9100,
            ws_reconnect_interval_secs: 5,
        }
    }

    /// 测试用 ConfigManager（字段私有：仅本模块可直接构造；不注册全局单例，各测试相互独立）。
    /// nexus/station 路径指向 tempdir，落盘断言直接读这两个文件
    fn test_manager(dir: &tempfile::TempDir) -> ConfigManager {
        ConfigManager {
            agent_config: agent_config(dir.path().to_str().unwrap()),
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: ArcStr::from(dir.path().join("nexus.json").to_str().unwrap()),
            station_path: ArcStr::from(dir.path().join("station.json").to_str().unwrap()),
            context_dir: ArcStr::from(dir.path().join("context").to_str().unwrap()),
        }
    }

    /// 读回落的 nexus.json（落盘断言助手）
    fn read_nexus_json(dir: &tempfile::TempDir) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("nexus.json")).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn bootstrap_creates_nexus_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap()).await.unwrap();
        // 首次创建为空默认（provider / (agent, role) 配置 / session 配置由模板或管理 API 填写）
        assert!(repo.channels.is_empty());
        assert!(repo.providers.is_empty());
        assert!(repo.memory_structs.is_empty());
        assert!(repo.agent_roles.is_empty());
        assert!(repo.pipelines.is_empty());
        assert!(repo.sessions.is_empty());
        assert!(path.exists(), "首次创建应写文件");
    }

    #[tokio::test]
    async fn bootstrap_loads_existing_nexus() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        // 第一次创建（空默认）
        let mut repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap()).await.unwrap();
        // 手工写入一个 provider 后落盘（模拟人工编辑/管理 API 写入）
        {
            let providers = Arc::make_mut(&mut repo.providers);
            providers.insert(ArcStr::from("deepseek"), Arc::new(sample_provider()));
        }
        std::fs::write(&path, serde_json::to_string_pretty(&repo).unwrap()).unwrap();
        // 第二次加载：文件已存在为权威（内容不变，不重新种子）
        let loaded = ConfigManager::load_or_create_nexus(path.to_str().unwrap()).await.unwrap();
        assert!(loaded.providers.contains_key("deepseek"), "文件存在时不应丢弃已有配置");
    }

    #[tokio::test]
    async fn nexus_json_file_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nexus.json");
        let mut repo = ConfigManager::load_or_create_nexus(path.to_str().unwrap()).await.unwrap();
        {
            let map = Arc::make_mut(&mut repo.providers);
            map.insert(ArcStr::from("deepseek"), Arc::new(sample_provider()));
        }
        // 模拟写回再读
        let json = serde_json::to_string_pretty(&repo).unwrap();
        std::fs::write(&path, json).unwrap();
        let back: NexusRepo = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let provider = back.providers.get("deepseek").expect("provider 应回读").clone();
        assert_eq!(provider.provider_config.base_url.as_str(), "https://api.deepseek.com");
        assert_eq!(provider.default_model_config.max_tokens_usage, 128000);
    }

    #[tokio::test]
    async fn add_remove_admin_missing_channel_errors() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        // channel 不存在：add_admin / remove_admin 都应返回 ConfigNotFound 而非静默成功
        let admin = ChannelUser { messenger_id: ArcStr::from("m1"), user_id: ArcStr::from("u1") };
        let err = manager.add_admin("nope", Arc::new(admin)).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        let err = manager.remove_admin("nope", &ChannelUser { messenger_id: ArcStr::from("m1"), user_id: ArcStr::from("u1") }).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
    }

    fn sample_channel(id: &str) -> ChannelConfig {
        ChannelConfig {
            channel_id: ArcStr::from(id),
            ws_url: ArcStr::from("ws://127.0.0.1:8201"),
            admins: Arc::new(HashSet::new()),
            enabled: true,
        }
    }

    #[test]
    fn channel_config_serde_roundtrip() {
        let ch = sample_channel("web-main");
        let json = serde_json::to_string(&ch).unwrap();
        assert!(json.contains("\"channel_id\""));
        assert!(json.contains("\"ws_url\""));
        assert!(json.contains("\"admins\""));
        assert!(json.contains("\"enabled\""));
        assert!(!json.contains("\"bind_users\""), "绑定身份由 pipeline input_channels 承载");
        assert!(!json.contains("\"agent_id\""), "channel 不再绑定 agent");
        assert!(!json.contains("\"role_name\""), "channel 不再绑定 role");
        let back: ChannelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.channel_id, "web-main");
        assert!(back.enabled);
    }

    #[test]
    fn channel_config_ignores_legacy_binding_fields() {
        // 旧配置残留的绑定字段不参与解析（绑定改由 pipeline 的 input_channels 决定）
        let json = r#"{
            "channel_id": "c1",
            "ws_url": "ws://127.0.0.1:8201",
            "admins": [],
            "enabled": true,
            "bind_users": [{ "messenger_id": "web", "user_id": "u1" }],
            "agent_id": "a1",
            "role_name": "r1"
        }"#;
        let ch: ChannelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(ch.channel_id, "c1");
        assert!(ch.admins.is_empty());
    }

    #[test]
    fn channel_user_hash_eq_by_value() {
        let a = ChannelUser { messenger_id: ArcStr::from("m1"), user_id: ArcStr::from("u1") };
        let b = ChannelUser { messenger_id: ArcStr::from("m1"), user_id: ArcStr::from("u1") };
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 ChannelUser 应命中 HashSet");
    }

    #[test]
    fn nexus_repo_serde_roundtrip() {
        // NexusRepo 的形状：channels/providers/memory_structs/agent_roles/sessions
        // （模型由 provider 配置 + (agent, role) llm 配置表达）
        let mut repo = NexusRepo::default();
        {
            let map = Arc::make_mut(&mut repo.providers);
            map.insert(ArcStr::from("deepseek"), Arc::new(sample_provider()));
            let pipelines = Arc::make_mut(&mut repo.pipelines);
            pipelines.insert(ArcStr::from("a1"), Arc::new(HashMap::from([
                (ArcStr::from(""), Arc::new(PipelineConfig {
                    pipeline_name: ArcStr::from("default_memory"),
                })),
            ])));
        }
        let json = serde_json::to_string(&repo).unwrap();
        assert!(json.contains("\"providers\""), "应序列化 providers");
        assert!(json.contains("\"pipelines\""), "应序列化 pipelines");
        assert!(json.contains("\"sessions\""), "应序列化 sessions");
        assert!(!json.contains("\"memory_sessions\""), "NexusRepo 无 memory_sessions 字段");
        assert!(!json.contains("\"raw_sessions\""), "NexusRepo 无 raw_sessions 字段");
        assert!(!json.contains("\"default_model\""), "NexusRepo 无 default_model 字段");
        assert!(!json.contains("\"default_system_prompt\""), "NexusRepo 无 default_system_prompt 字段");
        let back: NexusRepo = serde_json::from_str(&json).unwrap();
        let provider = back.providers.get("deepseek").expect("provider 应回读").clone();
        assert_eq!(provider.provider_config.provider_type.as_str(), "openai");
        assert_eq!(provider.default_model_config.max_tokens_usage, 128000);
        let pipeline = back.pipelines.get("a1").and_then(|roles| roles.get(""))
            .expect("pipeline 应回读");
        assert_eq!(pipeline.pipeline_name.as_str(), "default_memory");
    }

    #[test]
    fn nexus_repo_default_empty() {
        let repo = NexusRepo::default();
        assert!(repo.channels.is_empty());
        assert!(repo.providers.is_empty());
        assert!(repo.memory_structs.is_empty());
        assert!(repo.agent_roles.is_empty(), "agent_roles 初始为空 map");
        assert!(repo.pipelines.is_empty(), "pipelines 初始为空 map");
        assert!(repo.sessions.is_empty(), "sessions 初始为空 map");
    }

    #[test]
    fn nexus_repo_missing_sessions_defaults_empty() {
        // 旧配置没有 sessions / pipelines 字段时应能加载（pipeline 集合为空，不装配任何 pipeline）
        let json = r#"{
            "channels": {},
            "providers": {},
            "memory_structs": {},
            "agent_roles": {},
            "memory_sessions": [],
            "raw_sessions": {}
        }"#;
        let repo: NexusRepo = serde_json::from_str(json).unwrap();
        assert!(repo.sessions.is_empty());
        assert!(repo.pipelines.is_empty());
    }

    // ---------- Provider 配置 ----------

    /// provider 配置（provider_config + default_model_config；model_configs 默认空）。
    /// provider 名由调用方作为 map key 提供（ProviderModelConfig 已无 name 字段）
    fn sample_provider() -> ProviderModelConfig {
        ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: ArcStr::from("openai"),
                base_url: ArcStr::from("https://api.deepseek.com"),
                api_key: ArcStr::from("sk-test"),
            }),
            default_model_config: ModelConfig {
                max_tokens_usage: 128000,
                timeout_secs: Some(60),
                retry_count: Some(3),
            },
            model_configs: Arc::new(HashMap::new()),
        }
    }

    /// 往 provider 的 model_configs 写入一条 model 覆盖（写时复制）
    fn with_model(mut provider: ProviderModelConfig, model: &str, config: ModelConfig) -> ProviderModelConfig {
        {
            let map = Arc::make_mut(&mut provider.model_configs);
            map.insert(ArcStr::from(model), Arc::new(config));
        }
        provider
    }

    #[test]
    fn provider_config_old_shape_no_longer_parses() {
        // 旧扁平格式（name/provider_type/base_url/api_key/default_*）无 provider_config 段 →
        // 解析失败（破坏性变更，配置文件需迁移）
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
        assert!(serde_json::from_str::<ProviderModelConfig>(old).is_err(), "旧格式缺 provider_config 应解析失败");
    }

    #[test]
    fn provider_config_missing_max_tokens_usage_fails() {
        // 必填语义：default_model_config 段内缺 max_tokens_usage → 解析失败
        let json = r#"{
            "provider_config": { "provider_type": "openai", "base_url": "https://api.deepseek.com", "api_key": "sk-test" },
            "default_model_config": { "timeout_secs": 60 },
            "model_configs": {}
        }"#;
        assert!(serde_json::from_str::<ProviderModelConfig>(json).is_err(), "缺 max_tokens_usage 应解析失败");
    }

    #[test]
    fn provider_config_nested_roundtrip() {
        // 嵌套格式序列化/反序列化往返（锁定 provider_config + default_model_config + model_configs 契约）
        let pc = with_model(sample_provider(), "deepseek-4-flash", ModelConfig {
            max_tokens_usage: 131072,
            timeout_secs: Some(30),
            retry_count: Some(2),
        });
        let json = serde_json::to_string(&pc).unwrap();
        let back: ProviderModelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider_config.provider_type.as_str(), "openai");
        assert_eq!(back.provider_config.base_url.as_str(), "https://api.deepseek.com");
        assert_eq!(back.provider_config.api_key.as_str(), "sk-test");
        assert_eq!(back.default_model_config.max_tokens_usage, 128000);
        assert_eq!(back.default_model_config.timeout_secs, Some(60));
        assert_eq!(back.default_model_config.retry_count, Some(3));
        let model = back.model_configs.get("deepseek-4-flash").expect("model 覆盖应回读").clone();
        assert_eq!(model.max_tokens_usage, 131072);
        assert_eq!(model.timeout_secs, Some(30));
    }

    #[test]
    fn get_effective_config_missing_model_uses_provider_default() {
        // model 未在 model_configs 配置（含 model_configs={} 极端情况）→ 全取 provider 默认值
        let eff = sample_provider().get_effective_config("unconfigured-model");
        assert_eq!(eff.max_tokens_usage, 128000, "未配置参数取 provider 默认值");
        assert_eq!(eff.timeout_secs, 60);
        assert_eq!(eff.retry_count, 3);
        assert_eq!(eff.provider_config.provider_type.as_str(), "openai");
    }

    #[test]
    fn merge_model_provider_partial_defaults_fall_back_to_globals() {
        // provider 默认仅配部分字段：未配字段回落全局常量；model 覆盖仍生效
        let mut provider = sample_provider();
        provider.default_model_config = ModelConfig {
            max_tokens_usage: 128000,
            timeout_secs: None,
            retry_count: None,
        };
        let provider = with_model(provider, "deepseek-4-flash", ModelConfig {
            max_tokens_usage: 128000,
            timeout_secs: Some(30),
            retry_count: None,
        });
        let eff = provider.get_effective_config("deepseek-4-flash");
        assert_eq!(eff.max_tokens_usage, 128000, "provider 默认值生效");
        assert_eq!(eff.timeout_secs, 30, "model 覆盖 provider");
        assert_eq!(eff.retry_count, DEFAULT_RETRY_COUNT, "两级都未配回落全局常量");
    }

    #[test]
    fn merge_model_provider_all_default_and_model_overrides() {
        // provider 全缺省（ModelConfig::default()）+ model 覆盖
        let mut provider = sample_provider();
        provider.default_model_config = ModelConfig::default();
        let provider = with_model(provider, "deepseek-4-flash", ModelConfig {
            max_tokens_usage: 262144,
            timeout_secs: None,
            retry_count: None,
        });
        let eff = provider.get_effective_config("deepseek-4-flash");
        assert_eq!(eff.max_tokens_usage, 262144, "model 覆盖生效");
        assert_eq!(eff.timeout_secs, DEFAULT_TIMEOUT_SECS, "provider 全缺省回落全局常量");
        assert_eq!(eff.retry_count, DEFAULT_RETRY_COUNT);
    }

    #[tokio::test]
    async fn provider_model_config_merges_provider_and_model() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        // provider 默认（128000/60/3）+ model 覆盖（131072/30/2）
        let provider = with_model(sample_provider(), "deepseek-4-flash", ModelConfig {
            max_tokens_usage: 131072,
            timeout_secs: Some(30),
            retry_count: Some(2),
        });
        manager.add_provider("deepseek", provider).await.unwrap();
        let eff = manager.provider_model_config("deepseek", "deepseek-4-flash").await.expect("应能合成");
        assert_eq!(eff.provider_config.provider_type.as_str(), "openai");
        assert_eq!(eff.provider_config.base_url.as_str(), "https://api.deepseek.com");
        assert_eq!(eff.provider_config.api_key.as_str(), "sk-test");
        assert_eq!(eff.timeout_secs, 30, "model 覆盖 provider");
        assert_eq!(eff.retry_count, 2, "model 覆盖 provider");
        assert_eq!(eff.max_tokens_usage, 131072, "model 的 max_tokens_usage 应生效");
    }

    #[tokio::test]
    async fn provider_model_config_inherits_provider_defaults() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        // model 只配 max_tokens_usage，其余继承 provider 默认
        let provider = with_model(sample_provider(), "deepseek-4-flash", ModelConfig {
            max_tokens_usage: 131072,
            timeout_secs: None,
            retry_count: None,
        });
        manager.add_provider("deepseek", provider).await.unwrap();
        let eff = manager.provider_model_config("deepseek", "deepseek-4-flash").await.expect("应能合成");
        assert_eq!(eff.timeout_secs, 60, "缺省继承 provider 默认");
        assert_eq!(eff.retry_count, 3, "缺省继承 provider 默认");
        assert_eq!(eff.max_tokens_usage, 131072, "model 覆盖 max_tokens_usage 应生效");
    }

    #[tokio::test]
    async fn provider_model_config_missing_returns_none() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        // provider 不存在 → None（调用方按 ModelProviderNotFound 处理）
        assert!(manager.provider_model_config("nope", "m").await.is_none());
        // provider 存在但 model 未配置 → Some（用 provider 默认值合成）
        manager.add_provider("deepseek", sample_provider()).await.unwrap();
        let eff = manager.provider_model_config("deepseek", "nope").await.expect("model 未配置也应合成");
        assert_eq!(eff.timeout_secs, 60, "未配置参数取 provider 默认值");
        assert_eq!(eff.max_tokens_usage, 128000, "model 未配置时用 provider 默认");
    }

    #[tokio::test]
    async fn provider_config_by_name_getter() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        // 未添加前 None
        assert!(manager.provider_config("deepseek").await.is_none());
        manager.add_provider("deepseek", sample_provider()).await.unwrap();
        let pc = manager.provider_config("deepseek").await.expect("应能查到");
        assert_eq!(pc.provider_type.as_str(), "openai");
        assert_eq!(pc.base_url.as_str(), "https://api.deepseek.com");
        assert_eq!(pc.api_key.as_str(), "sk-test");
        assert!(manager.provider_config("nope").await.is_none());
    }

    #[tokio::test]
    async fn provider_crud_and_snapshot() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        // add_provider → 合成可见
        manager.add_provider("deepseek", sample_provider()).await.unwrap();
        let eff = manager.provider_model_config("deepseek", "deepseek-4-flash").await
            .expect("provider 无 model 覆盖时 model 未配置也应合成（取 provider 默认值）");
        assert_eq!(eff.timeout_secs, 60, "未配置参数取 provider 默认值");
        // 带 model 覆盖的 provider
        let provider = with_model(sample_provider(), "gpt-4o", ModelConfig {
            max_tokens_usage: 262144,
            timeout_secs: None,
            retry_count: None,
        });
        manager.add_provider("openai", provider).await.unwrap();
        // 重名报错
        let err = manager.add_provider("openai", sample_provider()).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        let eff = manager.provider_model_config("openai", "gpt-4o").await.expect("model 覆盖应生效");
        assert_eq!(eff.max_tokens_usage, 262144);
        // remove_provider → 合成返回 None；不存在报错
        manager.remove_provider("openai").await.unwrap();
        assert!(manager.provider_model_config("openai", "gpt-4o").await.is_none());
        let err = manager.remove_provider("nope").await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        // nexus_snapshot 反映变更
        let snap = manager.nexus_snapshot().await;
        assert!(snap.providers.contains_key("deepseek"));
        assert!(!snap.providers.contains_key("openai"));
    }

    #[test]
    fn station_repo_new_shape_serde_roundtrip() {
        // StationRepo 形状：station_id + groups（key = group 名，value = 工具元数据）+ sub_stations
        let mut repo = StationRepo::default();
        repo.station_id = ArcStr::from("station-self");
        repo.groups = vec![ArcStr::from("filesystem")];
        {
            let map = Arc::make_mut(&mut repo.sub_stations);
            map.insert(ArcStr::from("station-a"), Arc::new(SubStationConfig {
                station_id: ArcStr::from("station-a"),
                base_url: ArcStr::from("http://127.0.0.1:9001"),
                timeout_secs: 30,
                prefix: Some(ArcStr::from("a/")),
            }));
        }
        let json = serde_json::to_string(&repo).unwrap();
        assert!(json.contains("\"station_id\"") && json.contains("\"groups\"") && json.contains("\"sub_stations\""), "形状字段");
        let back: StationRepo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.station_id.as_str(), "station-self");
        assert_eq!(back.groups, vec![ArcStr::from("filesystem")], "group 列表回读");
        let sub = back.sub_stations.get("station-a").unwrap().clone();
        assert_eq!(sub.base_url.as_str(), "http://127.0.0.1:9001");
        assert_eq!(sub.prefix.as_deref(), Some("a/"), "前缀回读");
        // 前缀可缺省
        let minimal: SubStationConfig = serde_json::from_str(
            r#"{"station_id":"s1","base_url":"http://127.0.0.1:9002","timeout_secs":5}"#,
        )
        .unwrap();
        assert!(minimal.prefix.is_none());

        // station_id 为必填：空对象 {} 应解析失败
        assert!(serde_json::from_str::<StationRepo>("{}").is_err(), "station_id 必填");
    }

    // ---- (agent, role) 配置继承与 session 配置 ----

    /// LLM 配置助手：provider/model 已配，其余字段按需覆盖
    fn llm_config(provider: &str, model: &str) -> LLMConfig {
        LLMConfig {
            provider: Some(ArcStr::from(provider)),
            model: Some(ArcStr::from(model)),
            ..LLMConfig::default()
        }
    }

    #[test]
    fn agent_role_config_build_returns_defaults_when_unconfigured() {
        let agent = AgentRoleConfig::default();
        let llm: LLMConfig = agent.build("");
        assert!(llm.provider.is_none() && llm.model.is_none(), "未配置 provider/model");
        assert!(!llm.has_max_tokens && !llm.has_temperature && !llm.has_thinking && !llm.has_reasoning_effort,
            "未配字段 has_* 均为 false（区分「未配」与「显式置空」）");
    }

    #[test]
    fn role_config_overrides_agent_config() {
        // role_name 空串 = agent 级默认；非空 = role 覆盖
        let mut agent = AgentRoleConfig::default();
        let mut agent_llm = llm_config("deepseek", "deepseek-chat");
        agent_llm.set_max_tokens(Some(1024));
        agent.set::<LLMConfig>(ArcStr::from(""), Arc::new(agent_llm));
        // role 只配 model：其余继承 agent
        let mut role_llm = LLMConfig::default();
        role_llm.model = Some(ArcStr::from("deepseek-reasoner"));
        agent.set::<LLMConfig>(ArcStr::from("r1"), Arc::new(role_llm));

        let llm: LLMConfig = agent.build("r1");
        assert_eq!(llm.provider.as_deref(), Some("deepseek"), "provider 继承 agent");
        assert_eq!(llm.model.as_deref(), Some("deepseek-reasoner"), "model 被 role 覆盖");
        assert_eq!(llm.max_tokens, Some(1024), "max_tokens 继承 agent");
        assert!(llm.has_max_tokens, "继承后 has_* 保留");

        // 未配置的 role 只拿 agent 默认
        let other: LLMConfig = agent.build("r2");
        assert_eq!(other.model.as_deref(), Some("deepseek-chat"), "未配置 role 继承 agent");
    }

    #[test]
    fn has_flag_distinguishes_unset_from_explicit_none() {
        // set_* = 显式配置（取值可为 None，用于显式清空）；unset_* = 撤销配置（回落继承）
        let mut agent = AgentRoleConfig::default();
        let mut agent_llm = LLMConfig::default();
        agent_llm.set_temperature(Some(0.7));
        agent.set::<LLMConfig>(ArcStr::from(""), Arc::new(agent_llm));

        // role 显式置空：has_* 为 true → 覆盖 agent 的取值
        let mut role_clear = LLMConfig::default();
        role_clear.set_temperature(None);
        agent.set::<LLMConfig>(ArcStr::from("r1"), Arc::new(role_clear));
        let llm: LLMConfig = agent.build("r1");
        assert!(llm.has_temperature, "显式 set（取值可为 None）");
        assert_eq!(llm.temperature, None, "显式置空覆盖 agent 取值");

        // role 撤销配置：has_* 为 false → 该字段回落继承 agent
        let mut role_unset = LLMConfig::default();
        role_unset.unset_temperature();
        agent.set::<LLMConfig>(ArcStr::from("r2"), Arc::new(role_unset));
        let llm: LLMConfig = agent.build("r2");
        assert_eq!(llm.temperature, Some(0.7), "unset 视为未配置 → 回落继承 agent");
    }

    #[test]
    fn group_set_config_merges_as_union() {
        // GroupSetConfig::merge 为并集（agent 与 role 的 group 累加，区别于 LLMConfig 的覆盖语义）
        let mut agent = AgentRoleConfig::default();
        let mut agent_tk = GroupSetConfig::default();
        agent_tk.group_set = Arc::new(HashSet::from([ArcStr::from("filesystem")]));
        agent.set::<GroupSetConfig>(ArcStr::from(""), Arc::new(agent_tk));
        let mut role_tk = GroupSetConfig::default();
        role_tk.group_set = Arc::new(HashSet::from([ArcStr::from("web")]));
        agent.set::<GroupSetConfig>(ArcStr::from("r1"), Arc::new(role_tk));

        let tk: GroupSetConfig = agent.build("r1");
        assert!(tk.group_set.contains("filesystem") && tk.group_set.contains("web"), "agent + role 并集");
    }

    #[test]
    fn agent_role_config_serde_roundtrip() {
        let mut agent = AgentRoleConfig::default();
        let mut agent_llm = llm_config("deepseek", "deepseek-chat");
        agent_llm.set_max_tokens(Some(2048));
        agent.set::<LLMConfig>(ArcStr::from(""), Arc::new(agent_llm));
        let mut role_tk = GroupSetConfig::default();
        role_tk.group_set = Arc::new(HashSet::from([ArcStr::from("filesystem")]));
        agent.set::<GroupSetConfig>(ArcStr::from("r1"), Arc::new(role_tk));

        let json = serde_json::to_string(&agent).unwrap();
        let back: AgentRoleConfig = serde_json::from_str(&json).unwrap();
        let llm: LLMConfig = back.build("");
        assert_eq!(llm.provider.as_deref(), Some("deepseek"));
        assert_eq!(llm.max_tokens, Some(2048), "has_max_tokens 应随取值一起持久化");
        let tk: GroupSetConfig = back.build("r1");
        assert!(tk.group_set.contains("filesystem"), "role 级配置应持久化");
    }

    #[tokio::test]
    async fn set_agent_role_config_merges_into_existing() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        manager.set_agent_role_config::<LLMConfig>(ArcStr::from("a1"), ArcStr::from(""), Arc::new(llm_config("deepseek", "deepseek-chat"))).await.unwrap();
        // 第二次只配 max_tokens：应合并进同一条 agent 配置（不丢 provider/model）
        let mut more = LLMConfig::default();
        more.set_max_tokens(Some(4096));
        manager.set_agent_role_config::<LLMConfig>(ArcStr::from("a1"), ArcStr::from(""), Arc::new(more)).await.unwrap();

        let snap = manager.nexus_snapshot().await;
        let agent = snap.agent_roles.get("a1").expect("agent 条目应懒建");
        let llm: LLMConfig = agent.build("");
        assert_eq!(llm.provider.as_deref(), Some("deepseek"), "已有取值不被覆盖");
        assert_eq!(llm.model.as_deref(), Some("deepseek-chat"));
        assert_eq!(llm.max_tokens, Some(4096), "第二次配置合并进同一条");
        // 落盘可见（agent_roles 的 key 为字符串，可正常序列化）
        let saved = read_nexus_json(&dir);
        assert_eq!(saved["agent_roles"]["a1"]["config_map"]["llm"]["model"], "deepseek-chat");
    }

    /// LLM 有效配置助手（session 配置表存的是 Effective 形态）
    fn effective_llm(provider: &str, model: &str) -> EffectiveLLMConfig {
        llm_config(provider, model).get_effective_config()
    }

    #[tokio::test]
    async fn session_config_creates_default_item_when_absent() {
        // session 配置不继承 agent/role；缺失时用默认有效配置建项并缓存
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        manager.set_agent_role_config::<LLMConfig>(
            ArcStr::from("a1"), ArcStr::from("r1"),
            Arc::new(llm_config("deepseek", "deepseek-chat"))).await.unwrap();

        let sid = ArcStr::from("a1-r1");
        let cfg = manager.session_config::<LLMConfig, EffectiveLLMConfig>(sid.clone()).await;
        assert!(cfg.provider.is_empty() && cfg.model.is_empty(), "不继承 agent/role");
        let again = manager.session_config::<LLMConfig, EffectiveLLMConfig>(sid).await;
        assert!(Arc::ptr_eq(&cfg, &again), "缺失时创建的默认项应缓存");

        let saved = serde_json::to_value(manager.nexus_snapshot().await).unwrap();
        assert!(saved["sessions"]["a1-r1"]["llm"].is_object(), "默认项应写入 session 配置");
    }

    #[tokio::test]
    async fn set_session_config_persists_one_item() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            ArcStr::from("a1-r1"), &llm_config("deepseek", "deepseek-chat")).await.unwrap();

        let got = manager.session_config::<LLMConfig, EffectiveLLMConfig>(ArcStr::from("a1-r1")).await;
        assert_eq!(got.provider.as_str(), "deepseek");
        assert_eq!(got.model.as_str(), "deepseek-chat");

        // 只写入该配置项，其它项仍为空
        let saved = read_nexus_json(&dir);
        assert_eq!(saved["sessions"]["a1-r1"]["llm"]["model"], "deepseek-chat");
        assert!(saved["sessions"]["a1-r1"]["compress"].is_null(), "未写入的配置项不生成");
    }

    #[tokio::test]
    async fn set_session_config_merges_into_existing() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let sid = ArcStr::from("a1-r1");
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            sid.clone(), &llm_config("deepseek", "deepseek-chat")).await.unwrap();

        // 第二次只改 model：provider 保留
        let mut more = LLMConfig::default();
        more.model = Some(ArcStr::from("deepseek-reasoner"));
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(sid.clone(), &more).await.unwrap();

        let got = manager.session_config::<LLMConfig, EffectiveLLMConfig>(sid).await;
        assert_eq!(got.provider.as_str(), "deepseek", "未覆盖字段保留");
        assert_eq!(got.model.as_str(), "deepseek-reasoner", "覆盖字段生效");
    }

    #[tokio::test]
    async fn set_session_config_creates_missing_item_from_override() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let sid = ArcStr::from("a1-r1");
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            sid.clone(), &llm_config("openai", "gpt-4o")).await.unwrap();

        let got = manager.session_config::<LLMConfig, EffectiveLLMConfig>(sid).await;
        assert_eq!(got.provider.as_str(), "openai");
        assert_eq!(got.model.as_str(), "gpt-4o");
    }

    #[tokio::test]
    async fn session_config_is_scoped_by_session_id() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            ArcStr::from("a1-r1"), &llm_config("deepseek", "deepseek-chat")).await.unwrap();
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            ArcStr::from("a1-r1-role-weather"), &llm_config("openai", "gpt-4o")).await.unwrap();

        let one = manager.session_config::<LLMConfig, EffectiveLLMConfig>(ArcStr::from("a1-r1")).await;
        assert_eq!(one.model.as_str(), "deepseek-chat");
        let other = manager.session_config::<LLMConfig, EffectiveLLMConfig>(ArcStr::from("a1-r1-role-weather")).await;
        assert_eq!(other.model.as_str(), "gpt-4o", "不同 session 互不影响");
        let default = manager.session_config::<LLMConfig, EffectiveLLMConfig>(ArcStr::from("no-such-session")).await;
        assert!(default.model.is_empty(), "未知 session 用默认值");
    }

    #[tokio::test]
    async fn set_session_config_if_not_exists_keeps_existing() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let sid = ArcStr::from("a1-r1");
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            sid.clone(), &llm_config("deepseek", "deepseek-chat")).await.unwrap();
        manager.set_session_config_if_not_exists(
            sid.clone(), Arc::new(effective_llm("openai", "gpt-4o"))).await.unwrap();

        let got = manager.session_config::<LLMConfig, EffectiveLLMConfig>(sid).await;
        assert_eq!(got.model.as_str(), "deepseek-chat", "已有配置不被覆盖");

        // 不存在时插入
        let other = ArcStr::from("a1-r2");
        manager.set_session_config_if_not_exists(
            other.clone(), Arc::new(effective_llm("openai", "gpt-4o"))).await.unwrap();
        let got = manager.session_config::<LLMConfig, EffectiveLLMConfig>(other).await;
        assert_eq!(got.model.as_str(), "gpt-4o");
    }

    #[tokio::test]
    async fn copy_session_config_copies_all_items_and_keeps_both_sides_independent() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let from = ArcStr::from("a1-r1");
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            from.clone(), &llm_config("deepseek", "deepseek-chat")).await.unwrap();
        manager.set_session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(
            from.clone(), &MemoryPipelineConfig { load_memory: Some(false) }).await.unwrap();

        let to = ArcStr::from("a1-r1-new");
        manager.copy_session_config(from.clone(), to.clone()).await.unwrap();

        let llm = manager.session_config::<LLMConfig, EffectiveLLMConfig>(to.clone()).await;
        assert_eq!(llm.model.as_str(), "deepseek-chat", "LLM 配置整体复制");
        let memory = manager.session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(to.clone()).await;
        assert!(!memory.load_memory, "其它配置项同样复制");
        let saved = read_nexus_json(&dir);
        assert_eq!(saved["sessions"]["a1-r1-new"]["llm"]["model"], "deepseek-chat", "复制结果落盘");

        // 复制后两边独立：改新 session 不影响原 session
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            to.clone(), &llm_config("openai", "gpt-4o")).await.unwrap();
        let origin = manager.session_config::<LLMConfig, EffectiveLLMConfig>(from).await;
        assert_eq!(origin.model.as_str(), "deepseek-chat", "原 session 不受影响");
    }

    #[tokio::test]
    async fn session_config_is_independent_snapshot() {
        // session 配置是独立快照：写入后再改 agent/role 不影响已存值
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let sid = ArcStr::from("a1-r1");
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(
            sid.clone(), &llm_config("deepseek", "deepseek-chat")).await.unwrap();
        manager.set_agent_role_config::<LLMConfig>(
            ArcStr::from("a1"), ArcStr::from("r1"),
            Arc::new(llm_config("openai", "gpt-4o"))).await.unwrap();

        let got = manager.session_config::<LLMConfig, EffectiveLLMConfig>(sid).await;
        assert_eq!(got.model.as_str(), "deepseek-chat", "已有 session 配置不被 agent/role 变化覆盖");
    }

    #[tokio::test]
    async fn pipelines_are_listed_sorted_and_removed_by_role_key() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        manager.set_pipeline_config(ArcStr::from("a2"), ArcStr::from(""), Arc::new(PipelineConfig {
            pipeline_name: ArcStr::from("default"),
        })).await.unwrap();
        manager.set_pipeline_config(ArcStr::from("a1"), ArcStr::from("r2"), Arc::new(PipelineConfig {
            pipeline_name: ArcStr::from("memory_summary"),
        })).await.unwrap();
        manager.set_pipeline_config(ArcStr::from("a1"), ArcStr::from(""), Arc::new(PipelineConfig {
            pipeline_name: ArcStr::from("default_memory"),
        })).await.unwrap();

        let list = manager.pipelines().await;
        let keys: Vec<(String, String)> = list.iter()
            .map(|(key, _)| (key.agent_id.to_string(), key.role_name.to_string()))
            .collect();
        assert_eq!(keys, vec![
            ("a1".to_string(), "".to_string()),
            ("a1".to_string(), "r2".to_string()),
            ("a2".to_string(), "".to_string()),
        ], "按 agent_id / role_name 排序，保证装配顺序稳定");
        assert_eq!(list[1].1.pipeline_name.as_str(), "memory_summary");

        // 落盘可见
        let saved = read_nexus_json(&dir);
        assert_eq!(saved["pipelines"]["a1"]["r2"]["pipeline_name"], "memory_summary");

        manager.remove_pipeline_config("a1", "r2").await.unwrap();
        assert_eq!(manager.pipelines().await.len(), 2, "删除后不再出现在装配集合中");
    }

    #[tokio::test]
    async fn input_channels_role_replaces_agent_and_falls_back_when_unconfigured() {
        /// 生产代码读取 input_channels 的方式：直接走 agent_role_config 的合并结果
        async fn input_channels_of(manager: &ConfigManager, agent_id: &str, role_name: &str) -> Arc<Vec<Arc<InputChannel>>> {
            manager.agent_role_config::<InputChannelsConfig>(agent_id, role_name).await
                .map(|config| config.input_channels)
                .unwrap_or_default()
        }

        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let agent = ArcStr::from("a1");
        manager.set_agent_role_config::<InputChannelsConfig>(
            agent.clone(), ArcStr::from(""),
            Arc::new(InputChannelsConfig {
                input_channels: Arc::new(vec![Arc::new(InputChannel {
                    channel_id: ArcStr::from("web1"),
                    messenger_id: ArcStr::from("web"),
                    user_id: ArcStr::from("u1"),
                })]),
            })).await.unwrap();

        // role "" 读 agent 级
        let empty_role = input_channels_of(&manager, agent.as_str(), "").await;
        assert_eq!(empty_role.len(), 1);
        assert_eq!(empty_role[0].user_id.as_str(), "u1");

        // role 配置整体替换 agent 级（不是叠加）
        manager.set_agent_role_config::<InputChannelsConfig>(
            agent.clone(), ArcStr::from("r1"),
            Arc::new(InputChannelsConfig {
                input_channels: Arc::new(vec![Arc::new(InputChannel {
                    channel_id: ArcStr::from("web1"),
                    messenger_id: ArcStr::from("web"),
                    user_id: ArcStr::from("u2"),
                })]),
            })).await.unwrap();
        let r1 = input_channels_of(&manager, agent.as_str(), "r1").await;
        assert_eq!(r1.len(), 1);
        assert_eq!(r1[0].user_id.as_str(), "u2", "role 列表替换 agent 列表");

        // role 未配置 input_channels：沿用 agent 级
        let r2 = input_channels_of(&manager, agent.as_str(), "r2").await;
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].user_id.as_str(), "u1", "未配置的 role 沿用 agent 级");

        // role 显式配置空列表：替换为空
        manager.set_agent_role_config::<InputChannelsConfig>(
            agent.clone(), ArcStr::from("r3"),
            Arc::new(InputChannelsConfig { input_channels: Arc::new(vec![]) })).await.unwrap();
        assert!(input_channels_of(&manager, agent.as_str(), "r3").await.is_empty());
        // agent 不存在
        assert!(input_channels_of(&manager, "no-such-agent", "").await.is_empty());
    }

    #[tokio::test]
    async fn memory_pipeline_config_inherits_agent_and_role_then_session_overrides() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let agent_id = ArcStr::from("a-memory");
        let role_name = ArcStr::from("r-memory");
        // agent 关、role 开：role 覆盖 agent
        manager.set_agent_role_config::<MemoryPipelineConfig>(
            agent_id.clone(), ArcStr::from(""),
            Arc::new(MemoryPipelineConfig { load_memory: Some(false) })).await.unwrap();
        manager.set_agent_role_config::<MemoryPipelineConfig>(
            agent_id.clone(), role_name.clone(),
            Arc::new(MemoryPipelineConfig { load_memory: Some(true) })).await.unwrap();
        let inherited = manager.agent_role_config::<MemoryPipelineConfig>(agent_id.as_str(), role_name.as_str())
            .await.expect("agent/role 应能读到 memory_pipeline");
        assert_eq!(inherited.load_memory, Some(true), "role 覆盖 agent");
        assert!(inherited.get_effective_config().load_memory);

        // 建立 session 时按 agent/role 导入；之后 session 级覆盖优先
        let session_id = ArcStr::from("a-memory-r-memory");
        manager.set_session_config_if_not_exists(
            session_id.clone(), Arc::new(inherited.get_effective_config())).await.unwrap();
        assert!(manager.session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(session_id.clone())
            .await.load_memory, "新建 session 继承 agent/role");
        manager.set_session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(
            session_id.clone(), &MemoryPipelineConfig { load_memory: Some(false) }).await.unwrap();
        assert!(!manager.session_config::<MemoryPipelineConfig, EffectiveMemoryPipelineConfig>(session_id)
            .await.load_memory, "session 覆盖优先");
    }
}
