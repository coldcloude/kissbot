use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use arc_swap::ArcSwap;
use kissbot_api::ChannelUser;
use tokio::sync::RwLock;

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
    nexus_path: String,
    station_path: String,
}

#[async_trait]
pub trait SessionConfigField<C: MergeSelf + MergeEffectiveConfig<E>, E: MergeBy<C>> {
    async fn session_config(&self, session_key: &SessionKey) -> Arc<E>;
    async fn set_agent_role_config(&self, agent_id: &str, role_name: &str, config: Arc<C>) -> Result<()>;
    async fn set_session_config(&self, session_key: &SessionKey, config: &C) -> Result<()>;
}

macro_rules! impl_session_config_field {
    ($ct:ty, $et:ty) => {
        #[async_trait]
        impl SessionConfigField<$ct, $et> for ConfigManager {
            /// 按 SessionKey 取 session 运行配置，无运行配置回退到 agent role 配置
            async fn session_config(&self, session_key: &SessionKey) -> Arc<$et> {
                // 先尝试获取 session 配置，只读repo
                {
                    let repo = self.nexus_repo.read().await;
                    if let Some(session_config_map) = repo.sessions.get(session_key) {
                        if let Some(session_config) = session_config_map.as_ref().get::<$et>() {
                            return session_config;
                        }
                    }
                }

                // 需要新建 session 配置，需要写权限
                let mut repo = self.nexus_repo.write().await;
                // 新建 session 配置
                let config = Arc::new(SessionConfigMap::create::<$ct, $et>(repo.agents.as_ref(), session_key));
                // 保存 session config
                let sessions = Arc::make_mut(&mut repo.sessions);
                let config_map = if let Some(mut config_map) = sessions.remove(session_key) {
                    let config_map_mut = Arc::make_mut(&mut config_map);
                    config_map_mut.insert::<$et>(config.clone());
                    config_map
                } else {
                    let mut config_map = SessionConfigMap::default();
                    config_map.insert::<$et>(config.clone());
                    Arc::new(config_map)
                };
                sessions.insert(session_key.clone(), config_map);
                config
            }

            /// 设置 role 配置，role_name 为空时设置 agent配置
            async fn set_agent_role_config(&self, agent_id: &str, role_name: &str, config: Arc<$ct>) -> Result<()> {
                self.write_nexus_config(|repo| {
                    let agents = Arc::make_mut(&mut repo.agents);
                    // agent 条目不存在则懒建
                    let agent_role_config = if let Some(mut agent_role_config) = agents.remove(agent_id) {
                        let agent_role_config_mut = Arc::make_mut(&mut agent_role_config);
                        agent_role_config_mut.set::<$ct>(role_name, config);
                        agent_role_config
                    } else {
                        let mut agent_role_config = AgentRoleConfig::default();
                        agent_role_config.set::<$ct>(role_name, config);
                        Arc::new(agent_role_config)
                    };
                    agents.insert(agent_id.to_string(), agent_role_config);
                    Ok(())
                }).await
            }

            /// 设置 session 配置
            async fn set_session_config(&self, session_key: &SessionKey, config: &$ct) -> Result<()> {
                self.write_nexus_config(|repo| {
                    let sessions = Arc::make_mut(&mut repo.sessions);
                    let config_map = if let Some(mut config_map) = sessions.remove(session_key) {
                        let config_map_mut = Arc::make_mut(&mut config_map);
                        let eff_config = if let Some(mut eff_config) = config_map_mut.take::<$et>() {
                            let eff_config_mut = Arc::make_mut(&mut eff_config);
                            eff_config_mut.merge(config);
                            eff_config
                        } else {
                            // 新建 session 配置
                            let mut eff_config = SessionConfigMap::create::<$ct, $et>(repo.agents.as_ref(), session_key);
                            eff_config.merge(config);
                            Arc::new(eff_config)
                        };
                        config_map_mut.insert::<$et>(eff_config);
                        config_map
                    } else {
                        // 新建 session 配置
                        let mut eff_config = SessionConfigMap::create::<$ct, $et>(repo.agents.as_ref(), session_key);
                        eff_config.merge(config);
                        // 新建 session map
                        let mut config_map = SessionConfigMap::default();
                        config_map.insert::<$et>(Arc::new(eff_config));
                        Arc::new(config_map)
                    };
                    sessions.insert(session_key.clone(), config_map);
                    Ok(())
                }).await
            }
        }
    };
}

impl_session_config_field!(LLMConfig, EffectiveLLMConfig);
impl_session_config_field!(CompressConfig, EffectiveCompressConfig);
impl_session_config_field!(MemoryRecoverConfig, EffectiveMemoryRecoverConfig);
impl_session_config_field!(ChannelBatchConfig, ChannelBatchConfig);
impl_session_config_field!(OutChannelConfig, OutChannelConfig);
impl_session_config_field!(ToolkitSetConfig, ToolkitSetConfig);
impl_session_config_field!(PipelineConfig, PipelineConfig);

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

        let manager = Self {
            agent_config,
            nexus_repo: Arc::new(RwLock::new(nexus_repo)),
            station_repo: Arc::new(RwLock::new(station_repo)),
            nexus_path,
            station_path,
        };
        // 注册全局单例（此后 get() 可用；重复调用幂等，第二次 set 被忽略，与 Nexus 一致）
        let _ = INSTANCE.set(manager);
        Ok(())
    }

    async fn load_or_create_nexus(path: &str) -> Result<NexusRepo> {
        if std::path::Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await
                .map_err(|e| Error::ConfigNotFound(format!("{}: {}", path, e)))?;
            let repo: NexusRepo = serde_json::from_str(&content)
                .map_err(|e| Error::ConfigParseError(e.to_string()))?;
            Ok(repo)
        } else {
            // 首次创建：默认空配置（channels/providers/(agent, role) 配置由 nexus.json 模板或管理 API 填写）
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

    // ========== NexusRepo CRUD ==========

    // ---------- channels ----------
    /// 返回所有 channel 配置快照（channel_id -> Arc<ChannelConfig>）
    pub async fn channels(&self) -> Vec<(String, Arc<ChannelConfig>)> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter().map(|(k, v)| (k.clone(), v.load().clone())).collect()
    }
    /// 返回 StationRepo 快照（Station 单例构建使用）
    pub async fn station_repo_snapshot(&self) -> StationRepo {
        self.station_repo.read().await.clone()
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

    /// 修改 channel 配置并落盘（绑定/agent/role 等运行时回写统一入口）
    /// channel 不存在返回 ConfigNotFound
    pub async fn update_channel<F>(&self, channel_id: &str, f: F) -> Result<()>
    where
        F: FnOnce(&mut ChannelConfig) + Send,
    {
        self.write_nexus_config(|repo| {
            let swap = repo.channels.get(channel_id)
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let mut ch = swap.load_full();
            let ch_mut = Arc::make_mut(&mut ch);
            f(ch_mut);
            swap.store(ch);
            Ok(())
        }).await
    }

    // ---------- providers ----------
    /// 合成 provider 默认 + model 覆盖的有效参数（每次调用现场合成，配置永远最新）
    /// model 未在 provider.model_configs 配置时用 provider 默认值合成（极端 model_configs={} 也可用）
    pub async fn provider_model_config(&self, provider: &str, model: &str) -> Option<EffectiveModelConfig> {
        let repo = self.nexus_repo.read().await;
        let provider = repo.providers.get(provider)?;
        Some(provider.load().get_effective_config(model))
    }

    /// 按名取 provider 配置（Arc 快照），供 provider 构造（ProviderManager::list_models 使用）
    /// 不存在返回 None
    pub async fn provider_config(&self, name: &str) -> Option<Arc<ProviderConfig>> {
        let repo = self.nexus_repo.read().await;
        let provider_model_config = repo.providers.get(name);
        if let Some(config) = provider_model_config {
            let provider_config = config.load().provider_config.clone();
            Some(provider_config)
        } else {
            None
        }
    }

    pub async fn session_config<C,E>(&self, session_key: &SessionKey) -> Arc<E>
    where
        C: MergeSelf + MergeEffectiveConfig<E>,
        E: MergeBy<C>,
        Self: SessionConfigField<C,E>,
    {
        <Self as SessionConfigField<C,E>>::session_config(self, session_key).await
    }

    pub async fn set_agent_role_config<C,E>(&self, agent_id: &str, role_name: &str, config: Arc<C>) -> Result<()>
    where
        C: MergeSelf + MergeEffectiveConfig<E>,
        E: MergeBy<C>,
        Self: SessionConfigField<C,E>,
    {
        <Self as SessionConfigField<C,E>>::set_agent_role_config(self, agent_id, role_name, config).await
    }

    pub async fn set_session_config<C,E>(&self, session_key: &SessionKey, config: &C) -> Result<()>
    where
        C: MergeSelf + MergeEffectiveConfig<E>,
        E: MergeBy<C>,
        Self: SessionConfigField<C,E>,
    {
        <Self as SessionConfigField<C,E>>::set_session_config(self, session_key, config).await
    }

    /// 设置 (agent, role) 的 out_channel（/bind-outgoing、/unbind-outgoing：role 空写 agent 默认，
    /// 非空写 role 覆盖；None 清除；write_nexus_config 单次原子，无需串行队列；agent 条目懒建）
    /// Task 2 已接线（命令调用），allow(dead_code) 随之移除
    pub async fn set_out_channel(&self, agent_id: &str, role_name: &str, out: Option<Arc<OutChannel>>) -> Result<()> {
        let config = OutChannelConfig {
            out_channel: out,
        };
        self.set_agent_role_config::<OutChannelConfig,OutChannelConfig>(agent_id, role_name, Arc::new(config)).await
    }

    // ---------- providers CRUD（管理 API 使用，落盘） ----------
    /// 添加 provider（重名报 ConfigNotFound），落盘
    pub async fn add_provider(&self, name: &str, cfg: ProviderModelConfig) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.providers);
            if map.contains_key(name) {
                return Err(Error::ConfigNotFound(format!("provider 已存在: {}", name.to_string())));
            }
            map.insert(name.to_string(), ArcSwap::new(Arc::new(cfg)));
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

    // ===== admins（永久操作：聚合 + NexusRepo 回写，check_admin 使用）=====
    /// 聚合所有 channel 的 admins（当前无消费方）
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
    use std::collections::HashSet;

    use super::*;
    use kissbot_api::ArcSwapHashMap;
    use tempfile::tempdir;

    fn agent_config(data_dir: &str) -> AgentConfig {
        AgentConfig {
            data_dir: Arc::new(data_dir.into()),
            mgmt_host: Arc::new("127.0.0.1".into()),
            mgmt_port: 9090,
            station_host: Arc::new("127.0.0.1".into()),
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
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
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
        assert!(repo.agents.is_empty());
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
            providers.insert("deepseek".to_string(), ArcSwap::new(Arc::new(sample_provider())));
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
            map.insert("deepseek".to_string(), ArcSwap::new(Arc::new(sample_provider())));
        }
        // 模拟写回再读
        let json = serde_json::to_string_pretty(&repo).unwrap();
        std::fs::write(&path, json).unwrap();
        let back: NexusRepo = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let provider = back.providers.get("deepseek").expect("provider 应回读").load_full();
        assert_eq!(provider.provider_config.base_url.as_str(), "https://api.deepseek.com");
        assert_eq!(provider.default_model_config.max_tokens_usage, 128000);
    }

    #[tokio::test]
    async fn write_config_op_error_skips_persist() {
        // write_nexus_config 模式语义：op 返回 Err 时不应序列化写盘（文件保持不变）
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        manager.add_channel(sample_channel("web-main")).await.unwrap();
        let before = std::fs::read_to_string(dir.path().join("nexus.json")).unwrap();

        // update_channel 的 op 前校验失败（channel 不存在）→ 返回 Err 且不落盘
        let err = manager.update_channel("nope", |_| {}).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        let after = std::fs::read_to_string(dir.path().join("nexus.json")).unwrap();
        assert_eq!(before, after, "op 失败不应写入文件");

        // 成功路径：落盘可见
        manager.update_channel("web-main", |c| c.agent_id = Arc::new("a1".into())).await.unwrap();
        let saved = read_nexus_json(&dir);
        assert_eq!(saved["channels"]["web-main"]["agent_id"], "a1");
    }

    #[tokio::test]
    async fn add_remove_admin_missing_channel_errors() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
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
            bind_users: Arc::new(HashSet::from([ChannelUser { messenger_id: "web".into(), user_id: "u1".into() }])),
            agent_id: Arc::new("0".into()),
            role_name: Arc::new("".into()),
            enabled: true,
        }
    }

    #[test]
    fn channel_config_bind_users_roundtrip() {
        let ch = ChannelConfig {
            channel_id: Arc::new("c1".into()),
            ws_url: Arc::new("ws://127.0.0.1:8201".into()),
            admins: Arc::new(HashSet::new()),
            bind_users: Arc::new(HashSet::from([
                ChannelUser { messenger_id: "web".into(), user_id: "u1".into() },
                ChannelUser { messenger_id: "web".into(), user_id: "u2".into() },
            ])),
            agent_id: Arc::new("a1".into()),
            role_name: Arc::new("r1".into()),
            enabled: true,
        };
        let json = serde_json::to_value(&ch).unwrap();
        // bind_users 为 HashSet，序列化数组顺序不定——按内容断言而非下标
        let json_bind_users = json["bind_users"].as_array().unwrap();
        assert!(json_bind_users.iter().any(|u| u["user_id"] == "u1"), "bind_users 应序列化 u1");
        assert!(json_bind_users.iter().any(|u| u["user_id"] == "u2"), "bind_users 应序列化 u2");
        assert!(json.get("is_send_channel").is_none(), "is_send_channel 已删除");
        let back: ChannelConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.bind_users.len(), 2);
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
        assert!(back.bind_users.contains(&ChannelUser { messenger_id: "web".into(), user_id: "u1".into() }), "bind_users 应包含 web/u1");
    }

    #[test]
    fn channel_config_old_shape_no_longer_parses() {
        // 旧格式（default_bind_user / enabled_by_default / is_send_channel）不兼容：
        // bind_users 为必填数组，无 serde alias（不兼容单值字段，需直接编辑配置文件）
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
        let manager = test_manager(&dir);
        manager.add_channel(sample_channel("web-main")).await.unwrap();

        // 修改 agent_id/role_name/bind_users（绑定为数组追加）
        manager.update_channel("web-main", |c| {
            c.agent_id = Arc::new("a1".into());
            c.role_name = Arc::new("r1".into());
            // HashSet 追加（Arc::make_mut 写时复制）
            Arc::make_mut(&mut c.bind_users).insert(ChannelUser { messenger_id: "web".into(), user_id: "u2".into() });
        }).await.unwrap();

        // 内存可见
        let ch = manager.channels().await.into_iter()
            .find(|(id, _)| id == "web-main").map(|(_, c)| c).unwrap();
        assert_eq!(*ch.agent_id, "a1");
        assert_eq!(ch.bind_users.len(), 2, "bind_users 追加应可见");

        // 落盘可见（重新读文件）
        let saved = read_nexus_json(&dir);
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
        // NexusRepo 现形状：channels/providers/memory_structs/agents/sessions
        // （default_model/default_system_prompt 已移除：模型由 provider 配置 + (agent, role) llm 配置表达）
        let mut repo = NexusRepo::default();
        {
            let map = Arc::make_mut(&mut repo.providers);
            map.insert("deepseek".to_string(), ArcSwap::new(Arc::new(sample_provider())));
        }
        let json = serde_json::to_string(&repo).unwrap();
        assert!(json.contains("\"providers\""), "应序列化 providers");
        assert!(json.contains("\"sessions\""), "应序列化 sessions");
        assert!(!json.contains("\"default_model\""), "default_model 已移除");
        assert!(!json.contains("\"default_system_prompt\""), "default_system_prompt 已移除");
        let back: NexusRepo = serde_json::from_str(&json).unwrap();
        let provider = back.providers.get("deepseek").expect("provider 应回读").load_full();
        assert_eq!(provider.provider_config.provider_type.as_str(), "openai");
        assert_eq!(provider.default_model_config.max_tokens_usage, 128000);
    }

    #[test]
    fn nexus_repo_default_empty() {
        let repo = NexusRepo::default();
        assert!(repo.channels.is_empty());
        assert!(repo.providers.is_empty());
        assert!(repo.memory_structs.is_empty());
        assert!(repo.agents.is_empty(), "agents 初始为空 map");
        assert!(repo.sessions.is_empty(), "sessions 初始为空 map");
    }

    // ---------- Provider 配置 ----------

    /// provider 配置（provider_config + default_model_config；model_configs 默认空）。
    /// provider 名由调用方作为 map key 提供（ProviderModelConfig 已无 name 字段）
    fn sample_provider() -> ProviderModelConfig {
        ProviderModelConfig {
            provider_config: Arc::new(ProviderConfig {
                provider_type: Arc::new("openai".into()),
                base_url: Arc::new("https://api.deepseek.com".into()),
                api_key: Arc::new("sk-test".into()),
            }),
            default_model_config: ModelConfig {
                max_tokens_usage: 128000,
                timeout_secs: Some(60),
                retry_count: Some(3),
            },
            model_configs: Arc::new(ArcSwapHashMap::new()),
        }
    }

    /// 往 provider 的 model_configs 写入一条 model 覆盖（ArcSwapHashMap 写时复制）
    fn with_model(mut provider: ProviderModelConfig, model: &str, config: ModelConfig) -> ProviderModelConfig {
        {
            let map = Arc::make_mut(&mut provider.model_configs);
            map.insert(model.to_string(), ArcSwap::new(Arc::new(config)));
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
        let model = back.model_configs.get("deepseek-4-flash").expect("model 覆盖应回读").load_full();
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
        // StationRepo 新形状：station_id + toolkits + sub_stations；McpConfig 占位序列化
        let mut repo = StationRepo::default();
        repo.station_id = Arc::new("station-self".into());
        {
            let map = Arc::make_mut(&mut repo.toolkits);
            map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(ToolkitConfig {
                tools: Arc::new(ArcSwapHashMap::new()),
                mcps: Arc::new({
                    let mut m = ArcSwapHashMap::new();
                    m.insert("mcp1".to_string(), ArcSwap::new(Arc::new(McpConfig {
                        name: Arc::new("mcp1".into()),
                        description: Arc::new("占位".into()),
                    })));
                    m
                }),
            })));
        }
        {
            let map = Arc::make_mut(&mut repo.sub_stations);
            map.insert("station-a".to_string(), ArcSwap::new(Arc::new(SubStationConfig {
                station_id: Arc::new("station-a".into()),
                base_url: Arc::new("http://127.0.0.1:9001".into()),
                timeout_secs: 30,
            })));
        }
        let json = serde_json::to_string(&repo).unwrap();
        assert!(json.contains("\"station_id\"") && json.contains("\"toolkits\"") && json.contains("\"sub_stations\""), "新形状字段");
        let back: StationRepo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.station_id.as_str(), "station-self");
        assert!(back.toolkits.contains_key("filesystem"));
        let tcfg = back.toolkits.get("filesystem").unwrap().load_full();
        assert_eq!(tcfg.mcps.get("mcp1").unwrap().load_full().name.as_str(), "mcp1");
        let sub = back.sub_stations.get("station-a").unwrap().load_full();
        assert_eq!(sub.base_url.as_str(), "http://127.0.0.1:9001");

        // station_id 为必填：空对象 {} 应解析失败
        assert!(serde_json::from_str::<StationRepo>("{}").is_err(), "station_id 必填");

        // ToolConfig 序列化（工具元数据契约）
        let tc = ToolConfig {
            name: Arc::new("read".into()),
            description: Arc::new("读取文本文件".into()),
            parameters: Arc::new(serde_json::json!({ "type": "object", "properties": { "path": { "type": "string" } } })),
        };
        let tj = serde_json::to_value(&tc).unwrap();
        assert_eq!(tj["name"], "read");
        assert_eq!(tj["parameters"]["properties"]["path"]["type"], "string");
    }

    // ---- (agent, role) 配置继承与 session 配置 ----

    fn session_key(agent_id: &str, role_name: &str) -> SessionKey {
        SessionKey { agent_id: agent_id.into(), role_name: role_name.into(), mode: Mode::Role }
    }

    /// LLM 配置助手：provider/model 已配，其余字段按需覆盖
    fn llm_config(provider: &str, model: &str) -> LLMConfig {
        LLMConfig {
            provider: Some(Arc::new(provider.into())),
            model: Some(Arc::new(model.into())),
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
        agent.set::<LLMConfig>("", Arc::new(agent_llm));
        // role 只配 model：其余继承 agent
        let mut role_llm = LLMConfig::default();
        role_llm.model = Some(Arc::new("deepseek-reasoner".into()));
        agent.set::<LLMConfig>("r1", Arc::new(role_llm));

        let llm: LLMConfig = agent.build("r1");
        assert_eq!(llm.provider.as_deref().map(|s| s.as_str()), Some("deepseek"), "provider 继承 agent");
        assert_eq!(llm.model.as_deref().map(|s| s.as_str()), Some("deepseek-reasoner"), "model 被 role 覆盖");
        assert_eq!(llm.max_tokens, Some(1024), "max_tokens 继承 agent");
        assert!(llm.has_max_tokens, "继承后 has_* 保留");

        // 未配置的 role 只拿 agent 默认
        let other: LLMConfig = agent.build("r2");
        assert_eq!(other.model.as_deref().map(|s| s.as_str()), Some("deepseek-chat"), "未配置 role 继承 agent");
    }

    #[test]
    fn has_flag_distinguishes_unset_from_explicit_none() {
        // set_* = 显式配置（取值可为 None，用于显式清空）；unset_* = 撤销配置（回落继承）
        let mut agent = AgentRoleConfig::default();
        let mut agent_llm = LLMConfig::default();
        agent_llm.set_temperature(Some(0.7));
        agent.set::<LLMConfig>("", Arc::new(agent_llm));

        // role 显式置空：has_* 为 true → 覆盖 agent 的取值
        let mut role_clear = LLMConfig::default();
        role_clear.set_temperature(None);
        agent.set::<LLMConfig>("r1", Arc::new(role_clear));
        let llm: LLMConfig = agent.build("r1");
        assert!(llm.has_temperature, "显式 set（取值可为 None）");
        assert_eq!(llm.temperature, None, "显式置空覆盖 agent 取值");

        // role 撤销配置：has_* 为 false → 该字段回落继承 agent
        let mut role_unset = LLMConfig::default();
        role_unset.unset_temperature();
        agent.set::<LLMConfig>("r2", Arc::new(role_unset));
        let llm: LLMConfig = agent.build("r2");
        assert_eq!(llm.temperature, Some(0.7), "unset 视为未配置 → 回落继承 agent");
    }

    #[test]
    fn toolkit_set_config_merges_as_union() {
        // ToolkitSetConfig::merge 为并集（agent 与 role 的 toolkit 累加，区别于 LLMConfig 的覆盖语义）
        let mut agent = AgentRoleConfig::default();
        let mut agent_tk = ToolkitSetConfig::default();
        agent_tk.toolkit_set = Arc::new(HashSet::from(["filesystem".to_string()]));
        agent.set::<ToolkitSetConfig>("", Arc::new(agent_tk));
        let mut role_tk = ToolkitSetConfig::default();
        role_tk.toolkit_set = Arc::new(HashSet::from(["web".to_string()]));
        agent.set::<ToolkitSetConfig>("r1", Arc::new(role_tk));

        let tk: ToolkitSetConfig = agent.build("r1");
        assert!(tk.toolkit_set.contains("filesystem") && tk.toolkit_set.contains("web"), "agent + role 并集");
    }

    #[test]
    fn agent_role_config_serde_roundtrip() {
        let mut agent = AgentRoleConfig::default();
        let mut agent_llm = llm_config("deepseek", "deepseek-chat");
        agent_llm.set_max_tokens(Some(2048));
        agent.set::<LLMConfig>("", Arc::new(agent_llm));
        let mut role_tk = ToolkitSetConfig::default();
        role_tk.toolkit_set = Arc::new(HashSet::from(["filesystem".to_string()]));
        agent.set::<ToolkitSetConfig>("r1", Arc::new(role_tk));

        let json = serde_json::to_string(&agent).unwrap();
        let back: AgentRoleConfig = serde_json::from_str(&json).unwrap();
        let llm: LLMConfig = back.build("");
        assert_eq!(llm.provider.as_deref().map(|s| s.as_str()), Some("deepseek"));
        assert_eq!(llm.max_tokens, Some(2048), "has_max_tokens 应随取值一起持久化");
        let tk: ToolkitSetConfig = back.build("r1");
        assert!(tk.toolkit_set.contains("filesystem"), "role 级配置应持久化");
    }

    #[tokio::test]
    async fn set_agent_role_config_merges_into_existing() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        manager.set_agent_role_config::<LLMConfig, EffectiveLLMConfig>("a1", "", Arc::new(llm_config("deepseek", "deepseek-chat"))).await.unwrap();
        // 第二次只配 max_tokens：应合并进同一条 agent 配置（不丢 provider/model）
        let mut more = LLMConfig::default();
        more.set_max_tokens(Some(4096));
        manager.set_agent_role_config::<LLMConfig, EffectiveLLMConfig>("a1", "", Arc::new(more)).await.unwrap();

        let snap = manager.nexus_snapshot().await;
        let agent = snap.agents.get("a1").expect("agent 条目应懒建");
        let llm: LLMConfig = agent.build("");
        assert_eq!(llm.provider.as_deref().map(|s| s.as_str()), Some("deepseek"), "已有取值不被覆盖");
        assert_eq!(llm.model.as_deref().map(|s| s.as_str()), Some("deepseek-chat"));
        assert_eq!(llm.max_tokens, Some(4096), "第二次配置合并进同一条");
        // 落盘可见（agents 的 key 为字符串，可正常序列化）
        let saved = read_nexus_json(&dir);
        assert_eq!(saved["agents"]["a1"]["config_map"]["llm"]["model"], "deepseek-chat");
    }

    #[tokio::test]
    async fn session_config_falls_back_to_agent_role_config() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        manager.set_agent_role_config::<LLMConfig, EffectiveLLMConfig>("a1", "", Arc::new(llm_config("deepseek", "deepseek-chat"))).await.unwrap();
        let mut role_llm = LLMConfig::default();
        role_llm.set_max_tokens(Some(4096));
        manager.set_agent_role_config::<LLMConfig, EffectiveLLMConfig>("a1", "r1", Arc::new(role_llm)).await.unwrap();

        // 无 session 配置 → 现场按 agent + role 合成（三层继承的最后一级）
        let eff = manager.session_config::<LLMConfig, EffectiveLLMConfig>(&session_key("a1", "r1")).await;
        assert_eq!(eff.provider.as_str(), "deepseek", "继承 agent 默认");
        assert_eq!(eff.model.as_str(), "deepseek-chat", "继承 agent 默认");
        assert_eq!(eff.max_tokens, Some(4096), "role 覆盖 agent");
        // 第二次取到同一份（读路径缓存命中，不重复合成）
        let again = manager.session_config::<LLMConfig, EffectiveLLMConfig>(&session_key("a1", "r1")).await;
        assert!(Arc::ptr_eq(&eff, &again), "同 key 返回缓存的同一份配置");
        // 未配置的 agent → 全默认
        let none = manager.session_config::<LLMConfig, EffectiveLLMConfig>(&session_key("nope", "r1")).await;
        assert!(none.provider.is_empty() && none.model.is_empty(), "未知 agent 用默认值");
    }

    #[tokio::test]
    async fn session_config_uses_global_defaults_for_compress() {
        // 三层都未配 → 回落全局默认常量
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let eff = manager.session_config::<CompressConfig, EffectiveCompressConfig>(&session_key("a1", "r1")).await;
        assert_eq!(eff.compress_threshold, DEFAULT_COMPRESS_THRESHOLD);
        assert_eq!(eff.compress_prompt.as_str(), DEFAULT_COMPRESS_PROMPT);

        // agent 级覆盖阈值 → 生效；未配的 prompt 保留默认
        let mut cfg = CompressConfig::default();
        cfg.compress_threshold = Some(0.5);
        manager.set_agent_role_config::<CompressConfig, EffectiveCompressConfig>("a2", "", Arc::new(cfg)).await.unwrap();
        let eff = manager.session_config::<CompressConfig, EffectiveCompressConfig>(&session_key("a2", "r1")).await;
        assert_eq!(eff.compress_threshold, 0.5, "agent 级覆盖生效");
        assert_eq!(eff.compress_prompt.as_str(), DEFAULT_COMPRESS_PROMPT, "未配字段保留默认");
    }

    #[tokio::test]
    async fn session_config_channel_batch_uses_default_interval() {
        // ChannelBatchConfig::default() 为零值 0，get_effective_config 回落 new() 的默认间隔
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let eff = manager.session_config::<ChannelBatchConfig, ChannelBatchConfig>(&session_key("a1", "r1")).await;
        assert_eq!(eff.channel_batch_interval_secs, DEFAULT_CHANNEL_BATCH_INTERVAL_SECS, "未配回落默认间隔");
    }

    #[tokio::test]
    async fn session_config_memory_recover_uses_defaults() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let eff = manager.session_config::<MemoryRecoverConfig, EffectiveMemoryRecoverConfig>(&session_key("a1", "r1")).await;
        assert_eq!(eff.memory_time_secs, DEFAULT_MEMORY_TIME_SECS);
        assert_eq!(eff.memory_count, DEFAULT_MEMORY_COUNT);
    }

    #[tokio::test]
    async fn set_session_config_overrides_agent_role_config() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        manager.set_agent_role_config::<LLMConfig, EffectiveLLMConfig>("a1", "r1", Arc::new(llm_config("deepseek", "deepseek-chat"))).await.unwrap();
        let key = session_key("a1", "r1");
        let before = manager.session_config::<LLMConfig, EffectiveLLMConfig>(&key).await;
        assert_eq!(before.model.as_str(), "deepseek-chat");

        // session 级覆盖 model（provider 未配 → 保留原值）
        let mut session_llm = LLMConfig::default();
        session_llm.model = Some(Arc::new("deepseek-reasoner".into()));
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(&key, &session_llm).await.unwrap();

        let after = manager.session_config::<LLMConfig, EffectiveLLMConfig>(&key).await;
        assert_eq!(after.model.as_str(), "deepseek-reasoner", "session 覆盖生效");
        assert_eq!(after.provider.as_str(), "deepseek", "未配字段保留原值");
    }

    #[tokio::test]
    async fn set_session_config_creates_entry_when_absent() {
        // session 条目不存在时：先按 agent+role 合成，再合并 session 级覆盖
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let key = session_key("a1", "r1");
        manager.set_session_config::<LLMConfig, EffectiveLLMConfig>(&key, &llm_config("openai", "gpt-4o")).await.unwrap();
        let eff = manager.session_config::<LLMConfig, EffectiveLLMConfig>(&key).await;
        assert_eq!(eff.provider.as_str(), "openai");
        assert_eq!(eff.model.as_str(), "gpt-4o");
        // sessions 落盘形状：SessionKey 是结构体，不能作 JSON map 键，故用 [[key, value], ...] 数组对
        let saved = read_nexus_json(&dir);
        let sessions = saved["sessions"].as_array().expect("sessions 应为数组对");
        assert_eq!(sessions.len(), 1, "一条 session 配置一个数组对");
        assert_eq!(sessions[0][0]["agent_id"], "a1", "数组对首项为 SessionKey");
        assert_eq!(sessions[0][0]["role_name"], "r1");
        assert_eq!(sessions[0][1]["llm"]["provider"], "openai", "数组对次项为 session 配置");
        assert_eq!(sessions[0][1]["llm"]["model"], "gpt-4o");
    }

    #[tokio::test]
    async fn set_out_channel_roundtrips_via_session_config() {
        // out_channel 由 ContextConfig 字段改为 (agent, role) 级 OutChannelConfig
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let out = Arc::new(OutChannel {
            channel_id: Arc::new("web-main".into()),
            user: Arc::new(ChannelUser { messenger_id: "web".into(), user_id: "u1".into() }),
            group_id: Arc::new("g1".into()),
        });
        manager.set_out_channel("a1", "", Some(out)).await.unwrap();
        let cfg = manager.session_config::<OutChannelConfig, OutChannelConfig>(&session_key("a1", "r1")).await;
        let got = cfg.out_channel.as_ref().expect("agent 级 out_channel 应被继承");
        assert_eq!(got.channel_id.as_str(), "web-main");
        assert_eq!(got.user.user_id.as_str(), "u1");

        // None 清除（换一个 session key：已合成的 session 配置是缓存快照）
        manager.set_out_channel("a1", "", None).await.unwrap();
        let cfg = manager.session_config::<OutChannelConfig, OutChannelConfig>(&session_key("a1", "r2")).await;
        assert!(cfg.out_channel.is_none(), "清除后不再继承");
    }
}
