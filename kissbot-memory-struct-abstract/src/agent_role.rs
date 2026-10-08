//! 角色：**AgentRole**（`agent_id + 角色名`）是组件的身份与 case 树归属者——一个 AgentRole 一棵 case 树；
//! 句柄（[`AgentRoleHandle`]）持有角色目录与运行态（[`AgentRoleState`]），由 [`AgentRoleManager`] 按角色缓存。
//!
//! - case 树按 [`AgentRole`] 组织：同一角色的角色模式与事件模式共用这一棵树；
//! - 输入与状态按记忆后缀区分（[`RoleEvent`]）：原始记录、水位、当前指针与模型会话都按后缀区分
//!   （事件模式的后缀为 `角色名-事件标识`）。后缀字符串只在构造文件/目录路径时由 kissbot-memory 的
//!   `role_event_suffix` 拼出，本模块不做拆分。
//! - `event_id`（`Option<ArcStr>`）是独立参数：`None` 为角色模式，`Some` 为事件模式（按它直接找
//!   `event-{event_id}` 顶层叶子 case）。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arcstr::ArcStr;
use dashmap::DashMap;
use kissbot_api::{CaseNode, ChannelCasePointer, ChannelKey, ChannelWatermark, RoleEvent, Watermark};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::STRUCT_DIR_NAME;
use crate::error::{Error, Result};
use crate::records::list_store_suffixes;
use crate::tree::CaseTree;

/// 角色：case 树的归属者（`agent_id + 角色名`，**不含 event**）。一个 AgentRole 一棵 case 树。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentRole {
    pub agent_id: ArcStr,
    pub role: ArcStr,
}

impl AgentRole {
    pub fn new(agent_id: impl Into<ArcStr>, role: impl Into<ArcStr>) -> Self {
        Self { agent_id: agent_id.into(), role: role.into() }
    }
}

impl fmt::Display for AgentRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.agent_id, self.role)
    }
}

/// 记忆后缀（`RoleEvent`）：由角色（`AgentRole` 的角色名）与独立的 `event_id` 组装，
/// 用于状态键、磁盘路径与会话标识。
pub fn role_event_of(agent_role: &AgentRole, event_id: Option<&ArcStr>) -> RoleEvent {
    RoleEvent { role_name: agent_role.role.clone(), event_id: event_id.cloned() }
}

/// LLM 会话标识（文件名安全；空后缀用 `_default`）：基于 kissbot-memory 的记忆后缀做字符替换。
pub fn session_id(role_event: &RoleEvent) -> String {
    let mut id = kissbot_memory::role_event_suffix(role_event)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect::<String>();
    if id.is_empty() {
        id = "_default".to_string();
    }
    id
}

/// 状态键：记忆后缀（`RoleEvent`）+ 通道
pub type RoleEventChannel = (RoleEvent, ChannelKey);

/// 角色运行态：case 树 + 各记忆后缀的当前 case 指针/水位 + 待处理标记
#[derive(Debug, Clone, Default)]
pub struct AgentRoleState {
    pub tree: CaseTree,
    /// 各 `(记忆后缀, 通道)` 的当前 case 指针
    pub current_cases: BTreeMap<RoleEventChannel, ArcStr>,
    /// 各 `(记忆后缀, 通道)` 的已处理水位
    pub watermarks: BTreeMap<RoleEventChannel, Watermark>,
    /// 已从磁盘加载过状态文件的 role_event（避免重复读盘）
    loaded_role_events: BTreeSet<Arc<RoleEvent>>,
    /// 待处理标记：通知只登记「哪个后缀哪个通道变了」，不计算。
    /// `dirty_role_event_suffixes` 表示某后缀的**全部通道**（思考/工具通知、后缀未知通道、首次加载、通知重连），
    /// 按记忆后缀字符串登记（拼接见 [`kissbot_memory::role_event_suffix`]），因此磁盘扫描出的目录名可直接登记。
    pub dirty_role_event_suffixes: BTreeSet<ArcStr>,
    pub dirty_channels: BTreeSet<RoleEventChannel>,
}

impl AgentRoleState {
    /// 标记某后缀的某通道有新数据
    pub fn mark_channel(&mut self, role_event: &RoleEvent, channel: &ChannelKey) {
        self.dirty_channels.insert((role_event.clone(), channel.clone()));
    }

    /// 标记某后缀的全部通道有新数据（通道未知）
    pub fn mark_role_event(&mut self, role_event: &RoleEvent) {
        self.dirty_role_event_suffixes.insert(kissbot_memory::role_event_suffix(role_event));
    }

    /// 某后缀是否有待处理的新数据（据此决定是否做一次归档计算）
    pub fn is_dirty(&self, role_event: &RoleEvent) -> bool {
        self.dirty_role_event_suffixes.contains(&kissbot_memory::role_event_suffix(role_event))
            || self.dirty_channels.iter().any(|(key, _)| key == role_event)
    }

    /// 某后缀被显式标记过待处理的通道
    pub fn dirty_channels_of(&self, role_event: &RoleEvent) -> BTreeSet<Arc<ChannelKey>> {
        self.dirty_channels
            .iter()
            .filter(|(key, _)| key == role_event)
            .map(|(_, channel)| Arc::new(channel.clone()))
            .collect()
    }

    /// 处理成功后清除该后缀的本轮标记
    pub fn clear_dirty(&mut self, role_event: &RoleEvent, processed: &BTreeSet<Arc<ChannelKey>>) {
        self.dirty_role_event_suffixes.remove(&kissbot_memory::role_event_suffix(role_event));
        self.dirty_channels
            .retain(|(key, channel)| key != role_event || !processed.contains(channel));
    }

    /// 某后缀在磁盘上的状态是否已加载（返回 true 表示本次需要加载）
    fn take_unloaded(&mut self, role_event: &RoleEvent) -> bool {
        self.loaded_role_events.insert(Arc::new(role_event.clone()))
    }

    /// 某后缀的当前 case 指针（按通道）
    pub fn current_of(&self, role_event: &RoleEvent) -> BTreeMap<Arc<ChannelKey>, ArcStr> {
        self.current_cases
            .iter()
            .filter(|((key, _), _)| key == role_event)
            .map(|((_, channel), case_name)| (Arc::new(channel.clone()), case_name.clone()))
            .collect()
    }

    /// 某后缀的水位（按通道）
    pub fn watermarks_of(&self, role_event: &RoleEvent) -> BTreeMap<Arc<ChannelKey>, Watermark> {
        self.watermarks
            .iter()
            .filter(|((key, _), _)| key == role_event)
            .map(|((_, channel), watermark)| (Arc::new(channel.clone()), watermark.clone()))
            .collect()
    }

    /// 某后缀某通道的当前 case
    pub fn current_case(&self, role_event: &RoleEvent, channel: &ChannelKey) -> Option<&ArcStr> {
        self.current_cases.get(&(role_event.clone(), channel.clone()))
    }

    /// 某后缀某通道的水位
    pub fn watermark(&self, role_event: &RoleEvent, channel: &ChannelKey) -> Watermark {
        self.watermarks.get(&(role_event.clone(), channel.clone())).cloned().unwrap_or_default()
    }

    /// 清除某后缀的指针与水位（重建前调用）
    pub fn clear_role_event(&mut self, role_event: &RoleEvent) {
        self.current_cases.retain(|(key, _), _| key != role_event);
        self.watermarks.retain(|(key, _), _| key != role_event);
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
pub struct AgentRoleHandle {
    pub id: AgentRole,
    /// 角色目录（`{root}/{agent}/memory-struct-abstract/{角色名}`）：case 树与原文副本
    pub dir: PathBuf,
    pub state: Mutex<AgentRoleState>,
}

impl AgentRoleHandle {
    pub fn tree_path(&self) -> PathBuf {
        self.dir.join("tree.json")
    }

    /// 记忆后缀目录（`{root}/{agent}/memory-struct-abstract/{后缀}`）：水位与当前指针。
    /// 角色模式下后缀即角色名，与角色目录相同。
    pub fn role_event_dir(&self, role_event: &RoleEvent) -> PathBuf {
        let suffix = kissbot_memory::role_event_suffix(role_event);
        self.dir.parent().map(|parent| parent.join(suffix.as_str())).unwrap_or_else(|| self.dir.join(suffix.as_str()))
    }

    pub fn current_path(&self, role_event: &RoleEvent) -> PathBuf {
        self.role_event_dir(role_event).join("current.json")
    }

    pub fn watermark_path(&self, role_event: &RoleEvent) -> PathBuf {
        self.role_event_dir(role_event).join("watermark.json")
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
        let nodes: BTreeMap<ArcStr, CaseNode> = tree
            .nodes()
            .into_iter()
            .map(|n| (n.case_name.clone(), n.clone()))
            .collect();
        write_json(&self.tree_path(), &serde_json::json!({ "nodes": nodes })).await
    }

    /// 持久化某后缀各通道的当前 case 指针（无通道时删除文件）
    pub async fn save_current(&self, role_event: &RoleEvent, current: &BTreeMap<Arc<ChannelKey>, ArcStr>) -> Result<()> {
        if current.is_empty() {
            return remove_file(&self.current_path(role_event)).await;
        }
        let channels: Vec<ChannelCasePointer> = current
            .iter()
            .map(|(channel, case_name)| ChannelCasePointer {
                channel: channel.as_ref().clone(),
                case_name: case_name.clone(),
            })
            .collect();
        write_json(&self.current_path(role_event), &serde_json::json!({ "channels": channels })).await
    }

    /// 持久化某后缀各通道水位
    pub async fn save_watermark(&self, role_event: &RoleEvent, watermarks: &BTreeMap<Arc<ChannelKey>, Watermark>) -> Result<()> {
        let channels: Vec<ChannelWatermark> = watermarks
            .iter()
            .map(|(channel, watermark)| ChannelWatermark {
                channel: channel.as_ref().clone(),
                watermark: Arc::new(watermark.clone()),
            })
            .collect();
        write_json(&self.watermark_path(role_event), &serde_json::json!({ "channels": channels })).await
    }

    /// 持久化角色状态：case 树 + 该后缀的当前 case 指针与水位（归档逐块落盘用）
    pub async fn save_state(&self, role_event: &RoleEvent, state: &AgentRoleState) -> Result<()> {
        self.save_tree(&state.tree).await?;
        self.save_current(role_event, &state.current_of(role_event)).await?;
        self.save_watermark(role_event, &state.watermarks_of(role_event)).await
    }

    /// 确保某后缀的磁盘状态已加载（首次使用时读一次 current.json / watermark.json）
    pub async fn ensure_role_event(&self, state: &mut AgentRoleState, role_event: &RoleEvent) -> Result<()> {
        if !state.take_unloaded(role_event) {
            return Ok(());
        }
        let dir = self.role_event_dir(role_event);
        let current = match read_json(&dir.join("current.json")).await? {
            Some(value) => serde_json::from_value::<CurrentFile>(value)
                .map_err(|e| Error::StateError(format!("当前 case 指针文件损坏: {}", e).into()))?
                .channels,
            None => Vec::new(),
        };
        for pointer in current {
            state.current_cases.insert((role_event.clone(), pointer.channel), pointer.case_name.clone());
        }
        let watermarks = match read_json(&dir.join("watermark.json")).await? {
            Some(value) => serde_json::from_value::<WatermarkFile>(value)
                .map_err(|e| Error::StateError(format!("水位文件损坏: {}", e).into()))?
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

/// 角色集合：按需加载、按 AgentRole 缓存句柄
pub struct AgentRoleManager {
    root: PathBuf,
    roles: DashMap<AgentRole, Arc<AgentRoleHandle>>,
}

impl AgentRoleManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), roles: DashMap::new() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 角色目录：`{root}/{agent}/memory-struct-abstract/{role}`
    pub fn role_dir(&self, id: &AgentRole) -> PathBuf {
        self.root.join(id.agent_id.as_str()).join(STRUCT_DIR_NAME.as_str()).join(id.role.as_str())
    }

    pub fn get(&self, id: &AgentRole) -> Option<Arc<AgentRoleHandle>> {
        self.roles.get(id).map(|h| h.clone())
    }

    /// 打开（或复用）角色：从磁盘加载 case 树；记忆存储里出现过的后缀登记为待处理
    pub async fn open(&self, id: AgentRole) -> Result<Arc<AgentRoleHandle>> {
        if let Some(handle) = self.get(&id) {
            return Ok(handle);
        }
        let handle = Arc::new(self.load(id).await?);
        let entry = self.roles.entry(handle.id.clone()).or_insert_with(|| handle.clone());
        Ok(entry.clone())
    }

    async fn load(&self, id: AgentRole) -> Result<AgentRoleHandle> {
        let dir = self.role_dir(&id);
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

        let mut state = AgentRoleState { tree, ..AgentRoleState::default() };
        // 首次加载：磁盘上的原始记录与水位的关系未知，按「记忆存储里出现过的后缀全部通道待处理」处理
        // （后缀字符串可直接作待处理标记的键，无需还原成 RoleEvent）
        state.dirty_role_event_suffixes.extend(list_store_suffixes(&self.root, &id.agent_id, &id.role).await?);

        Ok(AgentRoleHandle {
            id,
            dir,
            state: Mutex::new(state),
        })
    }

    /// 扫描磁盘上已有的角色（服务启动时恢复）：角色目录即角色名（含 `tree.json` 的目录），
    /// 事件后缀目录只放水位/指针、没有 `tree.json`，不会作为角色被打开
    pub async fn load_all(&self) -> Result<Vec<Arc<AgentRoleHandle>>> {
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
            let struct_root = agent_entry.path().join(STRUCT_DIR_NAME.as_str());
            let mut roles = match tokio::fs::read_dir(&struct_root).await {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            while let Some(role_entry) = roles.next_entry().await? {
                if !role_entry.file_type().await?.is_dir() || !role_entry.path().join("tree.json").exists() {
                    // 模型会话目录与事件后缀目录都没有 tree.json，不是角色目录
                    continue;
                }
                let role = role_entry.file_name().to_string_lossy().to_string();
                let handle = self.open(AgentRole::new(agent_id.clone(), role)).await?;
                if !handles.iter().any(|h: &Arc<AgentRoleHandle>| h.id == handle.id) {
                    handles.push(handle);
                }
            }
        }
        handles.sort_by(|a, b| role_order(&a.id).cmp(&role_order(&b.id)));
        Ok(handles)
    }

    /// 全部已加载角色
    pub fn loaded(&self) -> Vec<Arc<AgentRoleHandle>> {
        let mut handles: Vec<Arc<AgentRoleHandle>> = self.roles.iter().map(|h| h.value().clone()).collect();
        handles.sort_by(|a, b| role_order(&a.id).cmp(&role_order(&b.id)));
        handles
    }
}

/// 角色排序键（`agent_id` → `role`）
fn role_order(id: &AgentRole) -> (&str, &str) {
    (id.agent_id.as_str(), id.role.as_str())
}

async fn read_json(path: &Path) -> Result<Option<Value>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let value = serde_json::from_slice(&bytes)
                .map_err(|e| Error::StateError(format!("状态文件损坏 {}: {}", path.display(), e).into()))?;
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
            time: ArcStr::from(time),
            sn,
            body: RecordBody::Channel(Arc::new(ChannelRecord {
                user_id: ArcStr::from("u1"),
                self_user_id: ArcStr::from("self"),
                messenger_id: ArcStr::from("web"),
                group_id: ArcStr::from("g1"),
                is_self: 0,
                msg_id: ArcStr::new(),
                messenger_name: ArcStr::new(),
                user_name: ArcStr::from("alice"),
                group_name: ArcStr::new(),
                content: Content::Text(ArcStr::from("hi")),
                time: ArcStr::from(time),
                sn,
            })),
        }
    }

    fn role_event(role_name: &str, event_id: Option<&str>) -> RoleEvent {
        RoleEvent {
            role_name: ArcStr::from(role_name),
            event_id: event_id.map(ArcStr::from),
        }
    }

    #[test]
    fn agent_role_and_role_event_of() {
        // AgentRole 不含 event：由 agent_id + 角色名构成，是 case 树的归属者
        assert_eq!(AgentRole::new("0", "dev").to_string(), "0/dev");

        let role = role_event_of(&AgentRole::new("0", "dev"), None);
        assert_eq!(role, role_event("dev", None));
        assert_eq!(kissbot_memory::role_event_suffix(&role), "dev");
        assert_eq!(session_id(&role), "dev");

        let event_id = ArcStr::from("e12");
        let event = role_event_of(&AgentRole::new("0", "dev"), Some(&event_id));
        assert_eq!(event, role_event("dev", Some("e12")));
        assert_eq!(kissbot_memory::role_event_suffix(&event), "dev-e12");
        assert_eq!(session_id(&event), "dev-e12");

        // 事件标识本身可以含 '-'（UUID）：RoleEvent 显式区分角色名与事件标识，后缀拼接不做拆分
        assert_eq!(role_event("dev", Some("3f2a-c3d4")).event_id.as_deref(), Some("3f2a-c3d4"));

        let empty = role_event("", None);
        assert_eq!(session_id(&empty), "_default");
        let weird = RoleEvent::new("a/b c");
        assert_eq!(session_id(&weird), "a_b_c");
    }

    #[tokio::test]
    async fn role_state_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let id = AgentRole::new("0", "dev");
        let key = ChannelKey::new("web", "self1", "g1");
        let role_event = role_event("dev", None);

        let handle = manager.open(id.clone()).await.unwrap();
        assert!(handle.state.lock().await.tree.is_empty());

        {
            let mut state = handle.state.lock().await;
            state.tree.create_node("role-1", None, "2026-09-22 10:05:00");
            state.tree.apply_records("role-1", &[channel("2026-09-22 10:00:00", 1)], "2026-09-22 10:06:00");
            state.tree.set_summary("role-1", "聊了天气", "2026-09-22 10:06:00");
            state.current_cases.insert((role_event.clone(), key.clone()), "role-1".into());
            let watermark = state.watermarks.entry((role_event.clone(), key.clone())).or_default();
            watermark.channel.time = ArcStr::from("2026-09-22 10:00:00");
            watermark.channel.sn = 1;
            handle.save_state(&role_event, &state).await.unwrap();
        }

        // 新管理器重新加载 → 状态一致
        let reloaded = AgentRoleManager::new(temp.path()).open(id).await.unwrap();
        let mut state = reloaded.state.lock().await;
        reloaded.ensure_role_event(&mut state, &role_event).await.unwrap();
        assert_eq!(state.current_of(&role_event).get(&key).map(|id| id.as_str()), Some("role-1"));
        assert_eq!(state.watermark(&role_event, &key).channel.sn, 1);
        let node = state.tree.get("role-1").unwrap();
        assert_eq!(node.summary.as_str(), "聊了天气");
        assert_eq!(node.record_count, 1);
        assert_eq!(node.start_time.as_str(), "2026-09-22 10:00:00");

        // tree.json 形状符合文档
        let raw: Value = serde_json::from_slice(&tokio::fs::read(reloaded.tree_path()).await.unwrap()).unwrap();
        assert!(raw["nodes"]["role-1"].get("mode").is_none(), "tree.json 的节点不带模式/事件信息");
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
    async fn event_mode_shares_role_tree_and_keeps_own_state() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let event = role_event("dev", Some("e1"));
        let key = ChannelKey::new("web", "self1", "g1");

        let role = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        {
            let mut state = role.state.lock().await;
            state.tree.create_node("role-1", None, "t");
            state.current_cases.insert((role_event("dev", None), key.clone()), "role-1".into());
            role.save_state(&role_event("dev", None), &state).await.unwrap();
        }
        // 事件后缀：同一棵树（能看到角色 case），状态文件独立
        let same = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        let mut state = same.state.lock().await;
        same.ensure_role_event(&mut state, &event).await.unwrap();
        assert!(state.tree.contains("role-1"), "角色与事件后缀共用一棵 case 树");
        assert!(state.current_of(&event).is_empty(), "事件后缀有自己的当前指针");
        assert_eq!(same.role_event_dir(&event), temp.path().join("0").join(STRUCT_DIR_NAME.as_str()).join("dev-e1"));
        assert_eq!(same.role_event_dir(&role_event("dev", None)), same.dir, "角色模式下后缀目录即角色目录");
    }

    #[tokio::test]
    async fn load_all_discovers_roles_from_role_event_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let role = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        role.save_tree(&CaseTree::new()).await.unwrap();
        let event = role_event("dev", Some("e1"));
        {
            let mut state = role.state.lock().await;
            state.current_cases.insert((event.clone(), ChannelKey::new("web", "self1", "g1")), "event-e1".into());
            role.save_state(&event, &state).await.unwrap();
        }

        let fresh = AgentRoleManager::new(temp.path());
        let handles = fresh.load_all().await.unwrap();
        let keys: Vec<String> = handles.iter().map(|h| h.id.to_string()).collect();
        assert_eq!(keys, vec!["0/dev".to_string()], "dev 与 dev-e1 是同一个角色");
    }

    #[tokio::test]
    async fn reset_role_event_state_clears_state_files() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        let role_event = role_event("dev", None);
        {
            let mut state = handle.state.lock().await;
            state.tree.create_node("role-1", None, "t");
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
        let manager = AgentRoleManager::new(temp.path());
        let handle = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        let role_event = role_event("dev", None);
        let mut current = BTreeMap::new();
        current.insert(Arc::new(ChannelKey::new("web", "self1", "g1")), ArcStr::from("role-1"));
        handle.save_current(&role_event, &current).await.unwrap();
        assert!(handle.current_path(&role_event).exists());
        handle.save_current(&role_event, &BTreeMap::new()).await.unwrap();
        assert!(!handle.current_path(&role_event).exists());
    }

    #[tokio::test]
    async fn corrupted_watermark_is_state_error() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = manager.open(AgentRole::new("0", "dev")).await.unwrap();
        let role_event = role_event("dev", None);
        tokio::fs::create_dir_all(handle.role_event_dir(&role_event)).await.unwrap();
        // 旧版按类型平铺的水位文件：形状不再匹配 → 报错而非静默丢水位
        tokio::fs::write(handle.watermark_path(&role_event), b"{\"channel\":{\"time\":\"t\",\"sn\":1}}").await.unwrap();
        let fresh = AgentRoleManager::new(temp.path());
        let handle = fresh.open(AgentRole::new("0", "dev")).await.unwrap();
        let mut state = handle.state.lock().await;
        assert!(handle.ensure_role_event(&mut state, &role_event).await.is_err());
    }

    #[tokio::test]
    async fn dirty_marks_are_kept_per_role_event() {
        let mut state = AgentRoleState::default();
        let first = role_event("dev", Some("e1"));
        let other = role_event("dev", Some("e2"));
        let channel = ChannelKey::new("web", "self1", "g1");

        state.mark_channel(&first, &channel);
        assert!(state.is_dirty(&first));
        assert!(!state.is_dirty(&other), "标记按后缀隔离");
        assert_eq!(state.dirty_channels_of(&first), [Arc::new(channel.clone())].into_iter().collect());

        state.mark_role_event(&other);
        state.clear_dirty(&first, &[Arc::new(channel.clone())].into_iter().collect());
        assert!(!state.is_dirty(&first));
        assert!(state.is_dirty(&other), "清除只影响本后缀");
    }
}
