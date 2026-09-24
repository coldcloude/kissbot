//! 事件树：节点、层级、当前事件指针的读写与持久化支撑。
//!
//! 树以 `(agent, 角色)` 为作用域组织，跨年份持续累积；节点即 kissbot-api 的 [`EventNode`]。

use std::collections::BTreeMap;
use std::sync::Arc;

use kissbot_api::{EventBrief, EventMode, EventNode};

use crate::records::RecordEntry;

/// 事件 id 前缀（角色模式由本组件分配：`e1`、`e2`…）
pub const EVENT_ID_PREFIX: &str = "e";

#[derive(Debug, Clone, Default)]
pub struct Tree {
    nodes: BTreeMap<String, EventNode>,
}

impl Tree {
    pub fn new() -> Self {
        Self { nodes: BTreeMap::new() }
    }

    pub fn from_nodes(nodes: Vec<EventNode>) -> Self {
        Self {
            nodes: nodes.into_iter().map(|n| (n.event_id.as_str().to_string(), n)).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn get(&self, event_id: &str) -> Option<&EventNode> {
        self.nodes.get(event_id)
    }

    /// 全部节点（按 created_at、event_id 升序）
    pub fn nodes(&self) -> Vec<&EventNode> {
        let mut nodes: Vec<&EventNode> = self.nodes.values().collect();
        nodes.sort_by(|a, b| (a.created_at.as_str(), a.event_id.as_str()).cmp(&(b.created_at.as_str(), b.event_id.as_str())));
        nodes
    }

    pub fn contains(&self, event_id: &str) -> bool {
        self.nodes.contains_key(event_id)
    }

    /// 顶层事件 id（按 created_at、event_id 升序）
    pub fn top_level_ids(&self) -> Vec<String> {
        self.nodes()
            .into_iter()
            .filter(|n| n.parent_id.is_none())
            .map(|n| n.event_id.as_str().to_string())
            .collect()
    }

    /// 某节点的子事件 id（按 created_at、event_id 升序）
    pub fn children_ids(&self, event_id: &str) -> Vec<String> {
        let Some(node) = self.nodes.get(event_id) else {
            return Vec::new();
        };
        let mut ids = node.children.clone();
        ids.sort_by(|a, b| {
            let ca = self.nodes.get(a.as_str()).map(|n| n.created_at.as_str()).unwrap_or("");
            let cb = self.nodes.get(b.as_str()).map(|n| n.created_at.as_str()).unwrap_or("");
            (ca, a.as_str()).cmp(&(cb, b.as_str()))
        });
        ids.into_iter().map(|id| id.as_str().to_string()).collect()
    }

    pub fn is_leaf(&self, event_id: &str) -> bool {
        self.nodes.get(event_id).map(|n| n.is_leaf()).unwrap_or(false)
    }

    /// 作用域模式：树上第一个节点的模式（空树由调用方按记忆后缀判定）
    pub fn scope_mode(&self) -> Option<EventMode> {
        self.nodes().first().map(|n| n.mode)
    }

    pub fn brief(&self, event_id: &str) -> Option<EventBrief> {
        self.nodes.get(event_id).map(EventBrief::from)
    }

    pub fn briefs(&self, ids: &[String]) -> Vec<EventBrief> {
        ids.iter().filter_map(|id| self.brief(id)).collect()
    }

    /// 父链（自近到远）
    pub fn ancestors(&self, event_id: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cursor = self.nodes.get(event_id).and_then(|n| n.parent_id.clone());
        while let Some(parent) = cursor {
            chain.push(parent.as_str().to_string());
            cursor = self.nodes.get(parent.as_str()).and_then(|n| n.parent_id.clone());
        }
        chain
    }

    /// 新建节点并挂到父节点下（`parent_id` 为 None 即顶层）
    pub fn create_node(
        &mut self,
        event_id: &str,
        parent_id: Option<&str>,
        mode: EventMode,
        now: &str,
    ) -> EventNode {
        let node = EventNode {
            event_id: Arc::new(event_id.to_string()),
            parent_id: parent_id.map(|p| Arc::new(p.to_string())),
            children: Vec::new(),
            mode,
            summary: Arc::new(String::new()),
            record_count: 0,
            start_time: Arc::new(now.to_string()),
            end_time: Arc::new(now.to_string()),
            created_at: Arc::new(now.to_string()),
            updated_at: Arc::new(now.to_string()),
        };
        if let Some(parent) = parent_id {
            if let Some(parent_node) = self.nodes.get_mut(parent) {
                parent_node.children.push(Arc::new(event_id.to_string()));
            }
        }
        self.nodes.insert(event_id.to_string(), node.clone());
        node
    }

    /// 事件内容变化后更新节点统计（记录条数、起止时间、更新时间）
    pub fn apply_records(&mut self, event_id: &str, entries: &[RecordEntry], now: &str) -> Option<&EventNode> {
        let node = self.nodes.get_mut(event_id)?;
        if !entries.is_empty() {
            let start = entries.iter().map(|e| e.time.as_str()).min().unwrap_or("").to_string();
            let end = entries.iter().map(|e| e.time.as_str()).max().unwrap_or("").to_string();
            if node.record_count == 0 || start.as_str() < node.start_time.as_str() {
                node.start_time = Arc::new(start);
            }
            if node.record_count == 0 || end.as_str() > node.end_time.as_str() {
                node.end_time = Arc::new(end);
            }
            node.record_count += entries.len() as u64;
        }
        node.updated_at = Arc::new(now.to_string());
        self.nodes.get(event_id)
    }

    pub fn set_summary(&mut self, event_id: &str, summary: &str, now: &str) {
        if let Some(node) = self.nodes.get_mut(event_id) {
            node.summary = Arc::new(summary.to_string());
            node.updated_at = Arc::new(now.to_string());
        }
    }

    /// 分配角色模式的新事件 id（作用域内递增）
    pub fn allocate_event_id(&self) -> String {
        let max = self
            .nodes
            .keys()
            .filter_map(|id| id.strip_prefix(EVENT_ID_PREFIX))
            .filter_map(|n| n.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("{}{}", EVENT_ID_PREFIX, max + 1)
    }

}

/// 记忆后缀的模式拆分：`{role_name}-{event_id}`（以**第一个** `-` 分隔）。
///
/// 角色名不允许包含 `-`，因此只要出现 `-` 即视为事件模式；无 `-` 为角色模式。
/// 角色名为空时后缀形如 `-e1`，拆分出空角色名。
pub fn split_event_suffix(role: &str) -> Option<(&str, &str)> {
    role.split_once('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{RecordBody, RecordEntry};
    use kissbot_api::ChannelRecord;

    const NOW: &str = "2026-09-22 13:49:00";

    fn entry(time: &str, sn: u64) -> RecordEntry {
        RecordEntry {
            kind: kissbot_api::RecordKind::Channel,
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
                content: kissbot_api::Content::Text(Arc::new("hi".into())),
                time: Arc::new(time.to_string()),
                sn,
            })),
        }
    }

    #[test]
    fn create_node_links_parent_and_lists_levels() {
        let mut tree = Tree::new();
        tree.create_node("e1", None, EventMode::Role, NOW);
        tree.create_node("e2", Some("e1"), EventMode::Role, NOW);
        tree.create_node("e3", Some("e1"), EventMode::Role, NOW);

        assert_eq!(tree.top_level_ids(), vec!["e1"]);
        assert_eq!(tree.children_ids("e1"), vec!["e2", "e3"]);
        assert!(tree.is_leaf("e2"));
        assert!(!tree.is_leaf("e1"));
        assert_eq!(tree.ancestors("e3"), vec!["e1"]);
        assert!(tree.ancestors("e1").is_empty());
        assert_eq!(tree.get("e2").unwrap().parent_id.as_ref().unwrap().as_str(), "e1");
    }

    #[test]
    fn apply_records_tracks_count_and_time_bounds() {
        let mut tree = Tree::new();
        tree.create_node("e1", None, EventMode::Role, NOW);
        tree.apply_records("e1", &[entry("2026-09-22 10:00:00", 1), entry("2026-09-22 10:05:00", 2)], NOW);
        let node = tree.get("e1").unwrap();
        assert_eq!(node.record_count, 2);
        assert_eq!(node.start_time.as_str(), "2026-09-22 10:00:00");
        assert_eq!(node.end_time.as_str(), "2026-09-22 10:05:00");

        // 追加更早与更晚的记录：边界随内容扩展
        tree.apply_records("e1", &[entry("2026-09-21 09:00:00", 3)], NOW);
        tree.apply_records("e1", &[entry("2026-09-23 09:00:00", 4)], NOW);
        let node = tree.get("e1").unwrap();
        assert_eq!(node.record_count, 4);
        assert_eq!(node.start_time.as_str(), "2026-09-21 09:00:00");
        assert_eq!(node.end_time.as_str(), "2026-09-23 09:00:00");
    }

    #[test]
    fn allocate_event_id_increments_within_scope() {
        let mut tree = Tree::new();
        assert_eq!(tree.allocate_event_id(), "e1");
        tree.create_node("e1", None, EventMode::Role, NOW);
        tree.create_node("e2", Some("e1"), EventMode::Role, NOW);
        assert_eq!(tree.allocate_event_id(), "e3");
        // 非 e{n} 形状的 id（事件模式）不参与递增
        tree.create_node("custom", None, EventMode::Event, NOW);
        assert_eq!(tree.allocate_event_id(), "e3");
        // 数字取最大值而非最大值+1（避免删除后重号）
        tree.create_node("e10", None, EventMode::Role, NOW);
        assert_eq!(tree.allocate_event_id(), "e11");
    }

    #[test]
    fn from_nodes_keeps_all() {
        let mut tree = Tree::new();
        tree.create_node("e1", None, EventMode::Role, NOW);
        let nodes = tree.nodes().into_iter().cloned().collect::<Vec<_>>();
        let restored = Tree::from_nodes(nodes);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored.top_level_ids(), vec!["e1"]);
        assert_eq!(restored.scope_mode(), Some(EventMode::Role));
    }

    #[test]
    fn briefs_skip_missing_and_keep_order() {
        let mut tree = Tree::new();
        tree.create_node("e1", None, EventMode::Role, NOW);
        tree.set_summary("e1", "摘要一", NOW);
        tree.create_node("e2", None, EventMode::Role, NOW);
        tree.set_summary("e2", "摘要二", NOW);
        let briefs = tree.briefs(&["e2".to_string(), "missing".to_string(), "e1".to_string()]);
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].event_id.as_str(), "e2");
        assert_eq!(briefs[0].summary.as_str(), "摘要二");
        assert_eq!(briefs[1].event_id.as_str(), "e1");
    }

    #[test]
    fn event_suffix_detection() {
        // 以第一个 '-' 分隔：前半为角色名、后半为事件标识（角色名不允许含 '-'）
        assert_eq!(split_event_suffix("dev-e1"), Some(("dev", "e1")));
        assert_eq!(split_event_suffix("-e12"), Some(("", "e12")));
        assert_eq!(split_event_suffix("dev"), None);
        assert_eq!(split_event_suffix("my-role"), Some(("my", "role")));
        assert_eq!(split_event_suffix("dev-e1-x"), Some(("dev", "e1-x")), "只按第一个 '-' 拆分");
    }

}
