//! 作用域：`(agent, 记忆后缀)` 的标识、目录布局、事件树/各通道当前指针/各通道水位的加载与持久化。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use kissbot_api::{ChannelEventPointer, ChannelKey, ChannelWatermark, EventMode, EventNode, Watermark};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::STRUCT_DIR_NAME;
use crate::error::{Error, Result};
use crate::tree::{Tree, split_event_suffix};

/// 作用域标识：agent + 记忆后缀（角色模式为角色名，事件模式为 `角色名-事件id`）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeId {
    pub agent_id: String,
    pub role: String,
}

impl ScopeId {
    pub fn new(agent_id: impl Into<String>, role: impl Into<String>) -> Self {
        Self { agent_id: agent_id.into(), role: role.into() }
    }

    /// 记忆后缀中的事件模式信息：`(角色名, 事件标识)`
    pub fn event_suffix(&self) -> Option<(&str, &str)> {
        split_event_suffix(&self.role)
    }

    /// 按记忆后缀判定的模式（树上已有节点时以树上记录为准）
    pub fn mode(&self) -> EventMode {
        match self.event_suffix() {
            Some(_) => EventMode::Event,
            None => EventMode::Role,
        }
    }

    /// 事件模式下由后缀携带的事件标识
    pub fn suffix_event_id(&self) -> Option<String> {
        self.event_suffix().map(|(_, event_id)| event_id.to_string())
    }

    /// LLM 会话标识（文件名安全；空作用域用 `_default`）
    pub fn session_id(&self) -> String {
        let mut id: String = self
            .role
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
            .collect();
        if id.is_empty() {
            id = "_default".to_string();
        }
        id
    }

    /// DashMap 键（`agent/role`）
    pub fn key(&self) -> String {
        format!("{}/{}", self.agent_id, self.role)
    }
}

impl fmt::Display for ScopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.agent_id, self.role)
    }
}

/// 作用域运行态：事件树 + 各通道的当前事件指针 + 各通道水位 + 待处理标记
#[derive(Debug, Clone, Default)]
pub struct ScopeState {
    pub tree: Tree,
    /// 各通道的当前事件指针（通道 → 事件 id）
    pub current_events: BTreeMap<ChannelKey, String>,
    /// 各通道的已处理水位
    pub watermarks: BTreeMap<ChannelKey, Watermark>,
    /// 待处理标记：通知只登记「哪个作用域哪个通道变了」，不计算；
    /// 查询时按作用域把全部通道各处理一次。`dirty_all` 表示通道未知
    /// （思考/工具通知、首次加载、通知重连——此时按作用域全部通道处理）
    pub dirty_all: bool,
    pub dirty_channels: BTreeSet<ChannelKey>,
}

impl ScopeState {
    /// 标记某通道有新数据
    pub fn mark_channel(&mut self, channel: &ChannelKey) {
        self.dirty_channels.insert(channel.clone());
    }

    /// 标记整个作用域有新数据（通道未知）
    pub fn mark_all(&mut self) {
        self.dirty_all = true;
    }

    /// 是否有待处理的新数据（查询时据此决定是否做一次归档计算）
    pub fn is_dirty(&self) -> bool {
        self.dirty_all || !self.dirty_channels.is_empty()
    }

    /// 处理成功后清除本轮标记
    pub fn clear_dirty(&mut self, all: bool, processed: &BTreeSet<ChannelKey>) {
        if all {
            self.dirty_all = false;
            self.dirty_channels.clear();
            return;
        }
        for channel in processed {
            self.dirty_channels.remove(channel);
        }
    }
}

/// current.json 的文件形状：各通道的当前事件指针
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentFile {
    #[serde(default)]
    channels: Vec<ChannelEventPointer>,
}

/// watermark.json 的文件形状：各通道的水位
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WatermarkFile {
    #[serde(default)]
    channels: Vec<ChannelWatermark>,
}

/// 作用域句柄：目录、固定模式与状态互斥（归档/定位按作用域串行）
pub struct ScopeHandle {
    pub id: ScopeId,
    pub dir: PathBuf,
    pub mode: EventMode,
    /// 事件模式的作用域事件标识
    pub event_id: Option<String>,
    pub state: Mutex<ScopeState>,
}

impl ScopeHandle {
    pub fn tree_path(&self) -> PathBuf {
        self.dir.join("tree.json")
    }

    pub fn current_path(&self) -> PathBuf {
        self.dir.join("current.json")
    }

    pub fn watermark_path(&self) -> PathBuf {
        self.dir.join("watermark.json")
    }

    pub fn events_dir(&self) -> PathBuf {
        self.dir.join("events")
    }

    pub fn event_dir(&self, event_id: &str) -> PathBuf {
        crate::records::event_dir(&self.dir, event_id)
    }

    /// 持久化事件树
    pub async fn save_tree(&self, tree: &Tree) -> Result<()> {
        let nodes: BTreeMap<String, EventNode> = tree
            .nodes()
            .into_iter()
            .map(|n| (n.event_id.as_str().to_string(), n.clone()))
            .collect();
        write_json(&self.tree_path(), &serde_json::json!({ "nodes": nodes })).await
    }

    /// 持久化各通道的当前事件指针（无通道时删除文件）
    pub async fn save_current(&self, current: &BTreeMap<ChannelKey, String>) -> Result<()> {
        if current.is_empty() {
            return remove_file(&self.current_path()).await;
        }
        let channels: Vec<ChannelEventPointer> = current
            .iter()
            .map(|(channel, event_id)| ChannelEventPointer {
                channel: channel.clone(),
                event_id: Arc::new(event_id.clone()),
            })
            .collect();
        write_json(&self.current_path(), &serde_json::json!({ "channels": channels })).await
    }

    /// 持久化各通道水位
    pub async fn save_watermark(&self, watermarks: &BTreeMap<ChannelKey, Watermark>) -> Result<()> {
        let channels: Vec<ChannelWatermark> = watermarks
            .iter()
            .map(|(channel, watermark)| ChannelWatermark {
                channel: channel.clone(),
                watermark: Arc::new(watermark.clone()),
            })
            .collect();
        write_json(&self.watermark_path(), &serde_json::json!({ "channels": channels })).await
    }

    /// 持久化作用域状态：事件树 + 各通道当前事件指针 + 各通道水位（归档逐块落盘用）
    pub async fn save_state(&self, state: &ScopeState) -> Result<()> {
        self.save_tree(&state.tree).await?;
        self.save_current(&state.current_events).await?;
        self.save_watermark(&state.watermarks).await
    }
}

async fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(value)?).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

async fn remove_file(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// 作用域集合：按需加载、缓存句柄
pub struct ScopeManager {
    root: PathBuf,
    scopes: DashMap<String, Arc<ScopeHandle>>,
}

impl ScopeManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), scopes: DashMap::new() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 作用域目录：`{root}/{agent}/memory-struct-abstract/{role}`
    pub fn scope_dir(&self, id: &ScopeId) -> PathBuf {
        self.root.join(&id.agent_id).join(STRUCT_DIR_NAME).join(&id.role)
    }

    pub fn get(&self, id: &ScopeId) -> Option<Arc<ScopeHandle>> {
        self.scopes.get(&id.key()).map(|h| h.clone())
    }

    /// 打开（或复用）作用域：从磁盘加载事件树、当前指针与水位
    pub async fn open(&self, id: ScopeId) -> Result<Arc<ScopeHandle>> {
        if let Some(handle) = self.get(&id) {
            return Ok(handle);
        }
        let handle = Arc::new(self.load(id).await?);
        let entry = self.scopes.entry(handle.id.key()).or_insert_with(|| handle.clone());
        Ok(entry.clone())
    }

    async fn load(&self, id: ScopeId) -> Result<ScopeHandle> {
        let dir = self.scope_dir(&id);
        let tree = match read_json(&dir.join("tree.json")).await? {
            Some(value) => {
                let nodes = value
                    .get("nodes")
                    .and_then(|n| n.as_object())
                    .map(|map| {
                        map.values()
                            .filter_map(|v| serde_json::from_value::<EventNode>(v.clone()).ok())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Tree::from_nodes(nodes)
            }
            None => Tree::new(),
        };
        let current_events = match read_json(&dir.join("current.json")).await? {
            Some(value) => serde_json::from_value::<CurrentFile>(value)
                .map_err(|e| Error::StateError(format!("当前事件指针文件损坏: {}", e)))?
                .channels
                .into_iter()
                .map(|pointer| (pointer.channel, pointer.event_id.as_str().to_string()))
                .collect(),
            None => BTreeMap::new(),
        };
        let watermarks = match read_json(&dir.join("watermark.json")).await? {
            Some(value) => serde_json::from_value::<WatermarkFile>(value)
                .map_err(|e| Error::StateError(format!("水位文件损坏: {}", e)))?
                .channels
                .into_iter()
                .map(|entry| (entry.channel, entry.watermark.as_ref().clone()))
                .collect(),
            None => BTreeMap::new(),
        };

        // 模式以树上记录为准（已有事件），否则按记忆后缀判定
        let mode = tree.scope_mode().unwrap_or_else(|| id.mode());
        let event_id = match mode {
            EventMode::Event => tree
                .nodes()
                .first()
                .map(|n| n.event_id.as_str().to_string())
                .or_else(|| id.suffix_event_id()),
            EventMode::Role => None,
        };

        Ok(ScopeHandle {
            id,
            dir,
            mode,
            event_id,
            state: Mutex::new(ScopeState {
                tree,
                current_events,
                watermarks,
                // 首次加载：磁盘上的原始记录与水位的关系未知，按「全部通道待处理」处理
                dirty_all: true,
                dirty_channels: BTreeSet::new(),
            }),
        })
    }

    /// 扫描磁盘上已有的作用域（服务启动时恢复）
    pub async fn load_all(&self) -> Result<Vec<Arc<ScopeHandle>>> {
        let mut handles = Vec::new();
        let mut agents = match tokio::fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(handles),
            Err(e) => return Err(e.into()),
        };
        while let Some(agent_entry) = agents.next_entry().await? {
            if !agent_entry.file_type().await?.is_dir() {
                continue;
            }
            let agent_id = agent_entry.file_name().to_string_lossy().to_string();
            let struct_root = agent_entry.path().join(STRUCT_DIR_NAME);
            let mut roles = match tokio::fs::read_dir(&struct_root).await {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            while let Some(role_entry) = roles.next_entry().await? {
                if !role_entry.file_type().await?.is_dir() {
                    continue;
                }
                let role = role_entry.file_name().to_string_lossy().to_string();
                let handle = self.open(ScopeId::new(agent_id.clone(), role)).await?;
                handles.push(handle);
            }
        }
        handles.sort_by(|a, b| a.id.key().cmp(&b.id.key()));
        Ok(handles)
    }

    /// 全部已加载作用域
    pub fn loaded(&self) -> Vec<Arc<ScopeHandle>> {
        let mut handles: Vec<Arc<ScopeHandle>> = self.scopes.iter().map(|h| h.value().clone()).collect();
        handles.sort_by(|a, b| a.id.key().cmp(&b.id.key()));
        handles
    }

    /// 删除某作用域的事件数据（重建前调用）
    pub async fn reset_scope_data(&self, handle: &ScopeHandle) -> Result<()> {
        crate::records::clear_event_records(&handle.events_dir()).await?;
        remove_file(&handle.tree_path()).await?;
        remove_file(&handle.current_path()).await?;
        remove_file(&handle.watermark_path()).await?;
        Ok(())
    }
}

async fn read_json(path: &Path) -> Result<Option<Value>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let value = serde_json::from_slice(&bytes)
                .map_err(|e| Error::StateError(format!("状态文件损坏 {}: {}", path.display(), e)))?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{RecordBody, RecordEntry};
    use kissbot_api::{ChannelKey, ChannelRecord, Content, RecordKind};

    fn channel(time: &str, sn: u64) -> RecordEntry {
        RecordEntry {
            kind: RecordKind::Channel,
            time: Arc::new(time.to_string()),
            sn,
            body: RecordBody::Channel(Arc::new(ChannelRecord {
                user_id: Arc::new("u1".into()),
                self_user_id: Arc::new("self".into()),
                messenger_id: Arc::new("web".into()),
                group_id: Arc::new("g1".into()),
                is_self: 0,
                msg_id: Arc::new(String::new()),
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new("alice".into()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new("hi".into())),
                time: Arc::new(time.to_string()),
                sn,
            })),
        }
    }

    #[test]
    fn scope_id_mode_and_session_id() {
        let role = ScopeId::new("0", "dev");
        assert_eq!(role.mode(), EventMode::Role);
        assert_eq!(role.suffix_event_id(), None);
        assert_eq!(role.session_id(), "dev");

        let event = ScopeId::new("0", "dev-e12");
        assert_eq!(event.mode(), EventMode::Event);
        assert_eq!(event.suffix_event_id().as_deref(), Some("e12"));
        assert_eq!(event.session_id(), "dev-e12");

        let empty = ScopeId::new("0", "");
        assert_eq!(empty.session_id(), "_default");
        let weird = ScopeId::new("0", "a/b c");
        assert_eq!(weird.session_id(), "a_b_c");
    }

    #[tokio::test]
    async fn scope_state_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let id = ScopeId::new("0", "dev");
        let key = ChannelKey::new("web", "self1", "g1");

        let handle = manager.open(id.clone()).await.unwrap();
        assert_eq!(handle.mode, EventMode::Role);
        assert!(handle.state.lock().await.tree.is_empty());

        {
            let mut state = handle.state.lock().await;
            state.tree.create_node("e1", None, EventMode::Role, "2026-09-22 10:05:00");
            state.tree.apply_records("e1", &[channel("2026-09-22 10:00:00", 1)], "2026-09-22 10:06:00");
            state.tree.set_summary("e1", "聊了天气", "2026-09-22 10:06:00");
            state.current_events.insert(key.clone(), "e1".into());
            let watermark = state.watermarks.entry(key.clone()).or_default();
            watermark.channel.time = Arc::new("2026-09-22 10:00:00".into());
            watermark.channel.sn = 1;
            handle.save_tree(&state.tree).await.unwrap();
            handle.save_current(&state.current_events).await.unwrap();
            handle.save_watermark(&state.watermarks).await.unwrap();
        }

        // 新管理器重新加载 → 状态一致
        let reloaded = ScopeManager::new(temp.path()).open(id).await.unwrap();
        let state = reloaded.state.lock().await;
        assert_eq!(state.current_events.get(&key).map(|id| id.as_str()), Some("e1"));
        assert_eq!(state.watermarks.get(&key).unwrap().channel.sn, 1);
        let node = state.tree.get("e1").unwrap();
        assert_eq!(node.summary.as_str(), "聊了天气");
        assert_eq!(node.record_count, 1);
        assert_eq!(node.start_time.as_str(), "2026-09-22 10:00:00");

        // tree.json 形状符合文档
        let raw: Value = serde_json::from_slice(&tokio::fs::read(reloaded.tree_path()).await.unwrap()).unwrap();
        assert_eq!(raw["nodes"]["e1"]["mode"], "role");
        assert_eq!(raw["nodes"]["e1"]["children"], serde_json::json!([]));

        // current.json / watermark.json 按通道分组
        let raw: Value = serde_json::from_slice(&tokio::fs::read(reloaded.current_path()).await.unwrap()).unwrap();
        assert_eq!(raw["channels"][0]["channel"]["self_user_id"], "self1");
        assert_eq!(raw["channels"][0]["event_id"], "e1");
        let raw: Value = serde_json::from_slice(&tokio::fs::read(reloaded.watermark_path()).await.unwrap()).unwrap();
        assert_eq!(raw["channels"][0]["channel"]["messenger_id"], "web");
        assert_eq!(raw["channels"][0]["watermark"]["channel"]["sn"], 1);
    }

    #[tokio::test]
    async fn load_all_discovers_existing_scopes() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let role = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        role.save_tree(&Tree::new()).await.unwrap();
        let event = manager.open(ScopeId::new("0", "dev-e1")).await.unwrap();
        event.save_tree(&Tree::new()).await.unwrap();

        let fresh = ScopeManager::new(temp.path());
        let handles = fresh.load_all().await.unwrap();
        let keys: Vec<String> = handles.iter().map(|h| h.id.key()).collect();
        assert_eq!(keys, vec!["0/dev".to_string(), "0/dev-e1".to_string()]);
        assert_eq!(handles[1].mode, EventMode::Event);
        assert_eq!(handles[1].event_id.as_deref(), Some("e1"));
    }

    #[tokio::test]
    async fn reset_scope_data_clears_state() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        {
            let mut state = handle.state.lock().await;
            state.tree.create_node("e1", None, EventMode::Role, "2026-09-22 10:05:00");
            state.current_events.insert(ChannelKey::new("web", "self1", "g1"), "e1".into());
            handle.save_tree(&state.tree).await.unwrap();
            handle.save_current(&state.current_events).await.unwrap();
            handle.save_watermark(&state.watermarks).await.unwrap();
        }
        manager.reset_scope_data(&handle).await.unwrap();
        assert!(!handle.tree_path().exists());
        assert!(!handle.current_path().exists());
        assert!(!handle.watermark_path().exists());
    }

    #[tokio::test]
    async fn save_current_without_channels_removes_pointer() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let mut current = BTreeMap::new();
        current.insert(ChannelKey::new("web", "self1", "g1"), "e1".to_string());
        handle.save_current(&current).await.unwrap();
        assert!(handle.current_path().exists());
        handle.save_current(&BTreeMap::new()).await.unwrap();
        assert!(!handle.current_path().exists());
    }

    #[tokio::test]
    async fn corrupted_watermark_is_state_error() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        tokio::fs::create_dir_all(handle.dir.as_path()).await.unwrap();
        // 旧版按类型平铺的水位文件：形状不再匹配 → 报错而非静默丢水位
        tokio::fs::write(handle.watermark_path(), b"{\"channel\":{\"time\":\"t\",\"sn\":1}}").await.unwrap();
        let fresh = ScopeManager::new(temp.path());
        assert!(fresh.open(ScopeId::new("0", "dev")).await.is_err());
    }
}
