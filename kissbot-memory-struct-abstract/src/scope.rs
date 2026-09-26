//! 作用域：**角色**（`agent + 角色名`）标识、目录布局，以及 case 树 + 各记忆后缀的当前指针/水位/待处理标记的加载与持久化。
//!
//! - 角色（[`AgentRole`]）决定 case 树：同一个角色无论角色模式还是事件模式都共用一棵树；
//! - 记忆后缀（[`RoleEvent`]，即外部传入的 `role`）决定输入与状态：原始记录、水位、当前指针
//!   与模型会话都按后缀区分（事件模式的后缀为 `角色名-caseid`）。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use kissbot_api::{CaseNode, ChannelCasePointer, ChannelKey, ChannelWatermark, Watermark};
#[cfg(test)]
use kissbot_api::CaseMode;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::STRUCT_DIR_NAME;
use crate::error::{Error, Result};
use crate::records::list_store_suffixes;
use crate::tree::{CaseTree, split_role_event};

/// 角色：作用域标识（agent + 角色名，**不含 event**）。同一角色共用一棵 case 树。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentRole {
    pub agent_id: String,
    pub role: String,
}

impl AgentRole {
    pub fn new(agent_id: impl Into<String>, role: impl Into<String>) -> Self {
        Self { agent_id: agent_id.into(), role: role.into() }
    }

    /// DashMap 键（`agent/role`）
    pub fn key(&self) -> String {
        format!("{}/{}", self.agent_id, self.role)
    }
}

impl fmt::Display for AgentRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.agent_id, self.role)
    }
}

/// 记忆后缀：外部传入的 `role`。无 `-` 为角色模式（纯角色名），含 `-` 为事件模式（`角色名-caseid`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RoleEvent {
    /// 角色名（不含 event）
    pub role: String,
    /// 事件标识（事件模式）；角色模式为 None
    pub event_id: Option<String>,
}

impl RoleEvent {
    /// 解析记忆后缀：按第一个 `-` 拆分（角色名不允许含 `-`）
    pub fn parse(suffix: &str) -> Self {
        match split_role_event(suffix) {
            Some((role, event_id)) => Self { role: role.to_string(), event_id: Some(event_id.to_string()) },
            None => Self { role: suffix.to_string(), event_id: None },
        }
    }

    /// 记忆后缀原样（记忆存储目录、水位/指针与模型会话都按它区分）
    pub fn suffix(&self) -> String {
        match &self.event_id {
            Some(event_id) => format!("{}-{}", self.role, event_id),
            None => self.role.clone(),
        }
    }

    /// 后缀的 case 模式（测试断言用；生产代码直接判 `event_case_name()`）
    #[cfg(test)]
    pub fn mode(&self) -> CaseMode {
        match self.event_id {
            Some(_) => CaseMode::Event,
            None => CaseMode::Role,
        }
    }

    /// 所属角色（case 树的作用域）
    pub fn agent_role(&self, agent_id: &str) -> AgentRole {
        AgentRole::new(agent_id, self.role.clone())
    }

    /// 事件模式固定的顶层叶子 case 名称：就是事件标识本身（case 名称即唯一标识）
    pub fn event_case_name(&self) -> Option<String> {
        self.event_id.clone()
    }

    /// LLM 会话标识（文件名安全；空后缀用 `_default`）
    pub fn session_id(&self) -> String {
        let mut id: String = self
            .suffix()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
            .collect();
        if id.is_empty() {
            id = "_default".to_string();
        }
        id
    }
}

impl fmt::Display for RoleEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.suffix())
    }
}

/// 状态键：记忆后缀 + 通道
pub type SuffixChannel = (RoleEvent, ChannelKey);

/// 判定作用域：agent + 记忆后缀。模型会话按它隔离（同一记忆后缀的连续判定共享上下文）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelScope {
    pub agent_id: String,
    pub role_event: RoleEvent,
}

impl ModelScope {
    pub fn new(role: &AgentRole, role_event: &RoleEvent) -> Self {
        Self { agent_id: role.agent_id.clone(), role_event: role_event.clone() }
    }
}

impl fmt::Display for ModelScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.agent_id, self.role_event)
    }
}

/// 角色运行态：case 树 + 各记忆后缀的当前 case 指针/水位 + 待处理标记
#[derive(Debug, Clone, Default)]
pub struct ScopeState {
    pub tree: CaseTree,
    /// 各 `(记忆后缀, 通道)` 的当前 case 指针
    pub current_cases: BTreeMap<SuffixChannel, String>,
    /// 各 `(记忆后缀, 通道)` 的已处理水位
    pub watermarks: BTreeMap<SuffixChannel, Watermark>,
    /// 已从磁盘加载过状态文件的后缀（避免重复读盘）
    loaded_suffixes: BTreeSet<RoleEvent>,
    /// 待处理标记：通知只登记「哪个后缀哪个通道变了」，不计算。
    /// `dirty_roles` 表示某后缀的**全部通道**（思考/工具通知、后缀未知通道、首次加载、通知重连）
    pub dirty_roles: BTreeSet<RoleEvent>,
    pub dirty_channels: BTreeSet<SuffixChannel>,
}

impl ScopeState {
    /// 标记某后缀的某通道有新数据
    pub fn mark_channel(&mut self, role_event: &RoleEvent, channel: &ChannelKey) {
        self.dirty_channels.insert((role_event.clone(), channel.clone()));
    }

    /// 标记某后缀的全部通道有新数据（通道未知）
    pub fn mark_role(&mut self, role_event: &RoleEvent) {
        self.dirty_roles.insert(role_event.clone());
    }

    /// 某后缀是否有待处理的新数据（据此决定是否做一次归档计算）
    pub fn is_dirty(&self, role_event: &RoleEvent) -> bool {
        self.dirty_roles.contains(role_event) || self.dirty_channels.iter().any(|(suffix, _)| suffix == role_event)
    }

    /// 某后缀被显式标记过待处理的通道
    pub fn dirty_channels_of(&self, role_event: &RoleEvent) -> BTreeSet<ChannelKey> {
        self.dirty_channels
            .iter()
            .filter(|(suffix, _)| suffix == role_event)
            .map(|(_, channel)| channel.clone())
            .collect()
    }

    /// 处理成功后清除该后缀的本轮标记
    pub fn clear_dirty(&mut self, role_event: &RoleEvent, processed: &BTreeSet<ChannelKey>) {
        self.dirty_roles.remove(role_event);
        self.dirty_channels
            .retain(|(suffix, channel)| suffix != role_event || !processed.contains(channel));
    }

    /// 某后缀在磁盘上的状态是否已加载（返回 true 表示本次需要加载）
    fn take_unloaded(&mut self, role_event: &RoleEvent) -> bool {
        self.loaded_suffixes.insert(role_event.clone())
    }

    /// 某后缀的当前 case 指针（按通道）
    pub fn current_of(&self, role_event: &RoleEvent) -> BTreeMap<ChannelKey, String> {
        self.current_cases
            .iter()
            .filter(|((suffix, _), _)| suffix == role_event)
            .map(|((_, channel), case_name)| (channel.clone(), case_name.clone()))
            .collect()
    }

    /// 某后缀的水位（按通道）
    pub fn watermarks_of(&self, role_event: &RoleEvent) -> BTreeMap<ChannelKey, Watermark> {
        self.watermarks
            .iter()
            .filter(|((suffix, _), _)| suffix == role_event)
            .map(|((_, channel), watermark)| (channel.clone(), watermark.clone()))
            .collect()
    }

    /// 某后缀某通道的当前 case
    pub fn current_case(&self, role_event: &RoleEvent, channel: &ChannelKey) -> Option<&String> {
        self.current_cases.get(&(role_event.clone(), channel.clone()))
    }

    /// 某后缀某通道的水位
    pub fn watermark(&self, role_event: &RoleEvent, channel: &ChannelKey) -> Watermark {
        self.watermarks.get(&(role_event.clone(), channel.clone())).cloned().unwrap_or_default()
    }

    /// 清除某后缀的指针与水位（重建前调用）
    pub fn clear_role_event(&mut self, role_event: &RoleEvent) {
        self.current_cases.retain(|(suffix, _), _| suffix != role_event);
        self.watermarks.retain(|(suffix, _), _| suffix != role_event);
    }
}

/// current.json 的文件形状：各通道的当前 case 指针
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentFile {
    #[serde(default)]
    channels: Vec<ChannelCasePointer>,
}

/// watermark.json 的文件形状：各通道的水位
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WatermarkFile {
    #[serde(default)]
    channels: Vec<ChannelWatermark>,
}

/// 角色句柄：目录与状态互斥（归档/定位按角色串行）
pub struct ScopeHandle {
    pub id: AgentRole,
    /// 角色目录（`{root}/{agent}/memory-struct-abstract/{角色名}`）：case 树与原文副本
    pub dir: PathBuf,
    pub state: Mutex<ScopeState>,
}

impl ScopeHandle {
    pub fn tree_path(&self) -> PathBuf {
        self.dir.join("tree.json")
    }

    /// 记忆后缀目录（`{root}/{agent}/memory-struct-abstract/{后缀}`）：水位与当前指针。
    /// 角色模式下后缀即角色名，与角色目录相同。
    pub fn suffix_dir(&self, role_event: &RoleEvent) -> PathBuf {
        self.dir.parent().map(|parent| parent.join(role_event.suffix())).unwrap_or_else(|| self.dir.join(role_event.suffix()))
    }

    pub fn current_path(&self, role_event: &RoleEvent) -> PathBuf {
        self.suffix_dir(role_event).join("current.json")
    }

    pub fn watermark_path(&self, role_event: &RoleEvent) -> PathBuf {
        self.suffix_dir(role_event).join("watermark.json")
    }

    /// 全部 case 原文副本目录（测试断言用）
    #[cfg(test)]
    pub fn cases_dir(&self) -> PathBuf {
        self.dir.join("cases")
    }

    pub fn case_dir(&self, case_name: &str) -> PathBuf {
        crate::records::case_dir(&self.dir, case_name)
    }

    /// 持久化 case 树
    pub async fn save_tree(&self, tree: &CaseTree) -> Result<()> {
        let nodes: BTreeMap<String, CaseNode> = tree
            .nodes()
            .into_iter()
            .map(|n| (n.case_name.as_str().to_string(), n.clone()))
            .collect();
        write_json(&self.tree_path(), &serde_json::json!({ "nodes": nodes })).await
    }

    /// 持久化某后缀各通道的当前 case 指针（无通道时删除文件）
    pub async fn save_current(&self, role_event: &RoleEvent, current: &BTreeMap<ChannelKey, String>) -> Result<()> {
        if current.is_empty() {
            return remove_file(&self.current_path(role_event)).await;
        }
        let channels: Vec<ChannelCasePointer> = current
            .iter()
            .map(|(channel, case_name)| ChannelCasePointer {
                channel: channel.clone(),
                case_name: Arc::new(case_name.clone()),
            })
            .collect();
        write_json(&self.current_path(role_event), &serde_json::json!({ "channels": channels })).await
    }

    /// 持久化某后缀各通道水位
    pub async fn save_watermark(&self, role_event: &RoleEvent, watermarks: &BTreeMap<ChannelKey, Watermark>) -> Result<()> {
        let channels: Vec<ChannelWatermark> = watermarks
            .iter()
            .map(|(channel, watermark)| ChannelWatermark {
                channel: channel.clone(),
                watermark: Arc::new(watermark.clone()),
            })
            .collect();
        write_json(&self.watermark_path(role_event), &serde_json::json!({ "channels": channels })).await
    }

    /// 持久化角色状态：case 树 + 该后缀的当前 case 指针与水位（归档逐块落盘用）
    pub async fn save_state(&self, role_event: &RoleEvent, state: &ScopeState) -> Result<()> {
        self.save_tree(&state.tree).await?;
        self.save_current(role_event, &state.current_of(role_event)).await?;
        self.save_watermark(role_event, &state.watermarks_of(role_event)).await
    }

    /// 确保某后缀的磁盘状态已加载（首次使用时读一次 current.json / watermark.json）
    pub async fn ensure_suffix(&self, state: &mut ScopeState, role_event: &RoleEvent) -> Result<()> {
        if !state.take_unloaded(role_event) {
            return Ok(());
        }
        let dir = self.suffix_dir(role_event);
        let current = match read_json(&dir.join("current.json")).await? {
            Some(value) => serde_json::from_value::<CurrentFile>(value)
                .map_err(|e| Error::StateError(format!("当前 case 指针文件损坏: {}", e)))?
                .channels,
            None => Vec::new(),
        };
        for pointer in current {
            state.current_cases.insert((role_event.clone(), pointer.channel), pointer.case_name.as_str().to_string());
        }
        let watermarks = match read_json(&dir.join("watermark.json")).await? {
            Some(value) => serde_json::from_value::<WatermarkFile>(value)
                .map_err(|e| Error::StateError(format!("水位文件损坏: {}", e)))?
                .channels,
            None => Vec::new(),
        };
        for entry in watermarks {
            state.watermarks.insert((role_event.clone(), entry.channel), entry.watermark.as_ref().clone());
        }
        Ok(())
    }

    /// 删除某后缀的状态文件（重建前调用）
    pub async fn reset_role_event_state(&self, role_event: &RoleEvent) -> Result<()> {
        remove_file(&self.current_path(role_event)).await?;
        remove_file(&self.watermark_path(role_event)).await
    }

    /// 删除某 case 的原文副本目录（重建前调用）
    pub async fn reset_case_records(&self, case_name: &str) -> Result<()> {
        crate::records::clear_case_records(&self.case_dir(case_name)).await
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

/// 角色集合：按需加载、缓存句柄
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

    /// 角色目录：`{root}/{agent}/memory-struct-abstract/{role}`
    pub fn scope_dir(&self, id: &AgentRole) -> PathBuf {
        self.root.join(&id.agent_id).join(STRUCT_DIR_NAME).join(&id.role)
    }

    pub fn get(&self, id: &AgentRole) -> Option<Arc<ScopeHandle>> {
        self.scopes.get(&id.key()).map(|h| h.clone())
    }

    /// 打开（或复用）角色：从磁盘加载 case 树；记忆存储里出现过的后缀登记为待处理
    pub async fn open(&self, id: AgentRole) -> Result<Arc<ScopeHandle>> {
        if let Some(handle) = self.get(&id) {
            return Ok(handle);
        }
        let handle = Arc::new(self.load(id).await?);
        let entry = self.scopes.entry(handle.id.key()).or_insert_with(|| handle.clone());
        Ok(entry.clone())
    }

    async fn load(&self, id: AgentRole) -> Result<ScopeHandle> {
        let dir = self.scope_dir(&id);
        let tree = match read_json(&dir.join("tree.json")).await? {
            Some(value) => {
                let nodes = value
                    .get("nodes")
                    .and_then(|n| n.as_object())
                    .map(|map| {
                        map.values()
                            .filter_map(|v| serde_json::from_value::<CaseNode>(v.clone()).ok())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                CaseTree::from_nodes(nodes)
            }
            None => CaseTree::new(),
        };

        let mut state = ScopeState { tree, ..ScopeState::default() };
        // 首次加载：磁盘上的原始记录与水位的关系未知，按「记忆存储里出现过的后缀全部通道待处理」处理
        for suffix in list_store_suffixes(&self.root, &id.agent_id, &id.role).await? {
            state.mark_role(&RoleEvent::parse(&suffix));
        }

        Ok(ScopeHandle {
            id,
            dir,
            state: Mutex::new(state),
        })
    }

    /// 扫描磁盘上已有的角色（服务启动时恢复）：目录名即记忆后缀，角色名取第一个 `-` 之前的部分
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
            let mut suffixes = match tokio::fs::read_dir(&struct_root).await {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            while let Some(suffix_entry) = suffixes.next_entry().await? {
                if !suffix_entry.file_type().await?.is_dir() {
                    continue;
                }
                let suffix = suffix_entry.file_name().to_string_lossy().to_string();
                if suffix == "llm" && !suffix_entry.path().join("tree.json").exists() {
                    // 模型会话目录不是记忆后缀（除非真有一个叫 llm 的角色目录）
                    continue;
                }
                let role = RoleEvent::parse(&suffix).role;
                let handle = self.open(AgentRole::new(agent_id.clone(), role)).await?;
                if !handles.iter().any(|h: &Arc<ScopeHandle>| h.id.key() == handle.id.key()) {
                    handles.push(handle);
                }
            }
        }
        handles.sort_by(|a, b| a.id.key().cmp(&b.id.key()));
        Ok(handles)
    }

    /// 全部已加载角色
    pub fn loaded(&self) -> Vec<Arc<ScopeHandle>> {
        let mut handles: Vec<Arc<ScopeHandle>> = self.scopes.iter().map(|h| h.value().clone()).collect();
        handles.sort_by(|a, b| a.id.key().cmp(&b.id.key()));
        handles
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
    use kissbot_api::{CaseMode, ChannelKey, ChannelRecord, Content, RecordKind};

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
    fn role_event_parses_suffix_and_mode() {
        let role = RoleEvent::parse("dev");
        assert_eq!(role.mode(), CaseMode::Role);
        assert_eq!(role.event_case_name(), None);
        assert_eq!(role.suffix(), "dev");
        assert_eq!(role.session_id(), "dev");
        assert_eq!(role.agent_role("0"), AgentRole::new("0", "dev"));

        let event = RoleEvent::parse("dev-e12");
        assert_eq!(event.mode(), CaseMode::Event);
        assert_eq!(event.event_case_name().as_deref(), Some("e12"));
        assert_eq!(event.suffix(), "dev-e12");
        assert_eq!(event.session_id(), "dev-e12");

        // 事件标识本身可以含 '-'（UUID）：只按第一个 '-' 拆分
        assert_eq!(RoleEvent::parse("dev-3f2a-c3d4").event_id.as_deref(), Some("3f2a-c3d4"));

        let empty = RoleEvent::parse("");
        assert_eq!(empty.session_id(), "_default");
        let weird = RoleEvent::parse("a/b c");
        assert_eq!(weird.session_id(), "a_b_c");
    }

    #[tokio::test]
    async fn role_state_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let id = AgentRole::new("0", "dev");
        let key = ChannelKey::new("web", "self1", "g1");
        let role_event = RoleEvent::parse("dev");

        let handle = manager.open(id.clone()).await.unwrap();
        assert!(handle.state.lock().await.tree.is_empty());

        {
            let mut state = handle.state.lock().await;
            state.tree.create_node("role-1", None, CaseMode::Role, "2026-09-22 10:05:00");
            state.tree.apply_records("role-1", &[channel("2026-09-22 10:00:00", 1)], "2026-09-22 10:06:00");
            state.tree.set_summary("role-1", "聊了天气", "2026-09-22 10:06:00");
            state.current_cases.insert((role_event.clone(), key.clone()), "role-1".into());
            let watermark = state.watermarks.entry((role_event.clone(), key.clone())).or_default();
            watermark.channel.time = Arc::new("2026-09-22 10:00:00".into());
            watermark.channel.sn = 1;
            handle.save_state(&role_event, &state).await.unwrap();
        }

        // 新管理器重新加载 → 状态一致
        let reloaded = ScopeManager::new(temp.path()).open(id).await.unwrap();
        let mut state = reloaded.state.lock().await;
        reloaded.ensure_suffix(&mut state, &role_event).await.unwrap();
        assert_eq!(state.current_of(&role_event).get(&key).map(|id| id.as_str()), Some("role-1"));
        assert_eq!(state.watermark(&role_event, &key).channel.sn, 1);
        let node = state.tree.get("role-1").unwrap();
        assert_eq!(node.summary.as_str(), "聊了天气");
        assert_eq!(node.record_count, 1);
        assert_eq!(node.start_time.as_str(), "2026-09-22 10:00:00");

        // tree.json 形状符合文档
        let raw: Value = serde_json::from_slice(&tokio::fs::read(reloaded.tree_path()).await.unwrap()).unwrap();
        assert_eq!(raw["nodes"]["role-1"]["mode"], "role");
        assert_eq!(raw["nodes"]["role-1"]["children"], serde_json::json!([]));

        // current.json / watermark.json 按通道分组，放在记忆后缀目录下
        let raw: Value = serde_json::from_slice(&tokio::fs::read(reloaded.current_path(&role_event)).await.unwrap()).unwrap();
        assert_eq!(raw["channels"][0]["channel"]["self_user_id"], "self1");
        assert_eq!(raw["channels"][0]["case_name"], "role-1");
        let raw: Value = serde_json::from_slice(&tokio::fs::read(reloaded.watermark_path(&role_event)).await.unwrap()).unwrap();
        assert_eq!(raw["channels"][0]["channel"]["messenger_id"], "web");
        assert_eq!(raw["channels"][0]["watermark"]["channel"]["sn"], 1);
    }

    #[tokio::test]
    async fn event_suffix_shares_role_tree_and_keeps_own_state() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let event = RoleEvent::parse("dev-e1");
        let key = ChannelKey::new("web", "self1", "g1");

        let role = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        {
            let mut state = role.state.lock().await;
            state.tree.create_node("role-1", None, CaseMode::Role, "t");
            state.current_cases.insert((RoleEvent::parse("dev"), key.clone()), "role-1".into());
            role.save_state(&RoleEvent::parse("dev"), &state).await.unwrap();
        }
        // 事件后缀：同一棵树（能看到角色 case），状态文件独立
        let same = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        let mut state = same.state.lock().await;
        same.ensure_suffix(&mut state, &event).await.unwrap();
        assert!(state.tree.contains("role-1"), "角色与事件后缀共用一棵 case 树");
        assert!(state.current_of(&event).is_empty(), "事件后缀有自己的当前指针");
        assert_eq!(same.suffix_dir(&event), temp.path().join("0").join(STRUCT_DIR_NAME).join("dev-e1"));
        assert_eq!(same.suffix_dir(&RoleEvent::parse("dev")), same.dir, "角色模式下后缀目录即角色目录");
    }

    #[tokio::test]
    async fn load_all_discovers_roles_from_suffix_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let role = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        role.save_tree(&CaseTree::new()).await.unwrap();
        let event = RoleEvent::parse("dev-e1");
        {
            let mut state = role.state.lock().await;
            state.current_cases.insert((event.clone(), ChannelKey::new("web", "self1", "g1")), "e1".into());
            role.save_state(&event, &state).await.unwrap();
        }

        let fresh = ScopeManager::new(temp.path());
        let handles = fresh.load_all().await.unwrap();
        let keys: Vec<String> = handles.iter().map(|h| h.id.key()).collect();
        assert_eq!(keys, vec!["0/dev".to_string()], "dev 与 dev-e1 是同一个角色");
    }

    #[tokio::test]
    async fn reset_role_event_state_clears_state_files() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        let role_event = RoleEvent::parse("dev");
        {
            let mut state = handle.state.lock().await;
            state.tree.create_node("role-1", None, CaseMode::Role, "t");
            state.current_cases.insert((role_event.clone(), ChannelKey::new("web", "self1", "g1")), "role-1".into());
            handle.save_state(&role_event, &state).await.unwrap();
        }
        assert!(handle.current_path(&role_event).exists());
        handle.reset_role_event_state(&role_event).await.unwrap();
        assert!(!handle.current_path(&role_event).exists());
        assert!(!handle.watermark_path(&role_event).exists());
        assert!(handle.tree_path().exists(), "case 树不属于单个后缀");
    }

    #[tokio::test]
    async fn save_current_without_channels_removes_pointer() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        let role_event = RoleEvent::parse("dev");
        let mut current = BTreeMap::new();
        current.insert(ChannelKey::new("web", "self1", "g1"), "role-1".to_string());
        handle.save_current(&role_event, &current).await.unwrap();
        assert!(handle.current_path(&role_event).exists());
        handle.save_current(&role_event, &BTreeMap::new()).await.unwrap();
        assert!(!handle.current_path(&role_event).exists());
    }

    #[tokio::test]
    async fn corrupted_watermark_is_state_error() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        let role_event = RoleEvent::parse("dev");
        tokio::fs::create_dir_all(handle.suffix_dir(&role_event)).await.unwrap();
        // 旧版按类型平铺的水位文件：形状不再匹配 → 报错而非静默丢水位
        tokio::fs::write(handle.watermark_path(&role_event), b"{\"channel\":{\"time\":\"t\",\"sn\":1}}").await.unwrap();
        let fresh = ScopeManager::new(temp.path());
        let handle = fresh.open(AgentRole::new("0", "dev")).await.unwrap();
        let mut state = handle.state.lock().await;
        assert!(handle.ensure_suffix(&mut state, &role_event).await.is_err());
    }

    #[tokio::test]
    async fn dirty_marks_are_kept_per_suffix() {
        let mut state = ScopeState::default();
        let role_event = RoleEvent::parse("dev-e1");
        let other = RoleEvent::parse("dev-e2");
        let channel = ChannelKey::new("web", "self1", "g1");

        state.mark_channel(&role_event, &channel);
        assert!(state.is_dirty(&role_event));
        assert!(!state.is_dirty(&other), "标记按后缀隔离");
        assert_eq!(state.dirty_channels_of(&role_event), [channel.clone()].into_iter().collect());

        state.mark_role(&other);
        state.clear_dirty(&role_event, &[channel].into_iter().collect());
        assert!(!state.is_dirty(&role_event));
        assert!(state.is_dirty(&other), "清除只影响本后缀");
    }
}
