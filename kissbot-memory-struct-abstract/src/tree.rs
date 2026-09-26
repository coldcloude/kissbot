//! Case 树：节点、层级、当前 case 指针的读写与持久化支撑。
//!
//! 树以**角色**（`agent + 角色名`）为作用域组织，跨年份持续累积；节点即 kissbot-api 的
//! [`CaseNode`]。同一个角色的角色模式与事件模式共用这一棵树，但各走各的子树、互不并入：
//! 角色模式的 case 由模型判定划分（id 前缀 `role-`），事件模式的 case 是外部 case 固定的
//! 顶层叶子（id 前缀 `event-`，见 [`EVENT_CASE_PREFIX`]）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use kissbot_api::{CaseBrief, CaseMode, CaseNode};

use crate::records::RecordEntry;

/// 角色模式 case id 前缀（本组件分配：`role-1`、`role-2`…）
pub const ROLE_CASE_PREFIX: &str = "role-";

/// 事件模式 case id 前缀（外部事件标识拼在其后：`event-{event id}`）
pub const EVENT_CASE_PREFIX: &str = "event-";

#[derive(Debug, Clone, Default)]
pub struct CaseTree {
    nodes: BTreeMap<String, CaseNode>,
}

impl CaseTree {
    pub fn new() -> Self {
        Self { nodes: BTreeMap::new() }
    }

    pub fn from_nodes(nodes: Vec<CaseNode>) -> Self {
        Self {
            nodes: nodes.into_iter().map(|n| (n.case_id.as_str().to_string(), n)).collect(),
        }
    }

    /// Case 树是否为空（测试断言用）
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn get(&self, case_id: &str) -> Option<&CaseNode> {
        self.nodes.get(case_id)
    }

    /// 全部节点（按 created_at、case_id 升序）
    pub fn nodes(&self) -> Vec<&CaseNode> {
        let mut nodes: Vec<&CaseNode> = self.nodes.values().collect();
        nodes.sort_by(|a, b| (a.created_at.as_str(), a.case_id.as_str()).cmp(&(b.created_at.as_str(), b.case_id.as_str())));
        nodes
    }

    pub fn contains(&self, case_id: &str) -> bool {
        self.nodes.contains_key(case_id)
    }

    /// 某模式的全部 case id
    pub fn ids_of_mode(&self, mode: CaseMode) -> BTreeSet<String> {
        self.nodes
            .values()
            .filter(|n| n.mode == mode)
            .map(|n| n.case_id.as_str().to_string())
            .collect()
    }

    /// 顶层 case id（两种子树都在；测试断言用）
    #[cfg(test)]
    pub fn top_level_ids(&self) -> Vec<String> {
        self.ids_of(None)
    }

    /// 角色模式顶层 case id（逐层判定的候选；事件模式的固定 case 不参与）
    pub fn role_top_level_ids(&self) -> Vec<String> {
        self.ids_of(Some(CaseMode::Role))
    }

    fn ids_of(&self, mode: Option<CaseMode>) -> Vec<String> {
        self.nodes()
            .into_iter()
            .filter(|n| n.parent_id.is_none())
            .filter(|n| mode.is_none_or(|mode| n.mode == mode))
            .map(|n| n.case_id.as_str().to_string())
            .collect()
    }

    /// 某节点的子 case id（按 created_at、case_id 升序）
    pub fn children_ids(&self, case_id: &str) -> Vec<String> {
        let Some(node) = self.nodes.get(case_id) else {
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

    /// 角色模式的子 case id（逐层判定的候选）
    pub fn role_children_ids(&self, case_id: &str) -> Vec<String> {
        self.children_ids(case_id)
            .into_iter()
            .filter(|id| self.get(id).map(|n| n.mode == CaseMode::Role).unwrap_or(false))
            .collect()
    }

    pub fn is_leaf(&self, case_id: &str) -> bool {
        self.nodes.get(case_id).map(|n| n.is_leaf()).unwrap_or(false)
    }

    pub fn brief(&self, case_id: &str) -> Option<CaseBrief> {
        self.nodes.get(case_id).map(CaseBrief::from)
    }

    pub fn briefs(&self, ids: &[String]) -> Vec<CaseBrief> {
        ids.iter().filter_map(|id| self.brief(id)).collect()
    }

    /// 父链（自近到远）
    pub fn ancestors(&self, case_id: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cursor = self.nodes.get(case_id).and_then(|n| n.parent_id.clone());
        while let Some(parent) = cursor {
            chain.push(parent.as_str().to_string());
            cursor = self.nodes.get(parent.as_str()).and_then(|n| n.parent_id.clone());
        }
        chain
    }

    /// 新建节点并挂到父节点下（`parent_id` 为 None 即顶层）
    pub fn create_node(&mut self, case_id: &str, parent_id: Option<&str>, mode: CaseMode, now: &str) -> CaseNode {
        let node = CaseNode {
            case_id: Arc::new(case_id.to_string()),
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
                parent_node.children.push(Arc::new(case_id.to_string()));
            }
        }
        self.nodes.insert(case_id.to_string(), node.clone());
        node
    }

    /// 删除若干 case（并从父节点的 children 摘除）；重建时按模式清空子树用
    pub fn remove_nodes(&mut self, case_ids: &BTreeSet<String>) {
        for case_id in case_ids {
            self.nodes.remove(case_id);
        }
        for node in self.nodes.values_mut() {
            node.children.retain(|child| !case_ids.contains(child.as_str()));
        }
    }

    /// case 内容变化后更新节点统计（记录条数、起止时间、更新时间）
    pub fn apply_records(&mut self, case_id: &str, entries: &[RecordEntry], now: &str) -> Option<&CaseNode> {
        let node = self.nodes.get_mut(case_id)?;
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
        self.nodes.get(case_id)
    }

    pub fn set_summary(&mut self, case_id: &str, summary: &str, now: &str) {
        if let Some(node) = self.nodes.get_mut(case_id) {
            node.summary = Arc::new(summary.to_string());
            node.updated_at = Arc::new(now.to_string());
        }
    }

    /// 分配角色模式的新 case id（角色树内递增：`role-1`、`role-2`…）
    pub fn allocate_case_id(&self) -> String {
        let max = self
            .nodes
            .keys()
            .filter_map(|id| id.strip_prefix(ROLE_CASE_PREFIX))
            .filter_map(|n| n.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("{}{}", ROLE_CASE_PREFIX, max + 1)
    }
}

/// 记忆后缀的拆分：`{角色名}-{caseid}`（以**第一个** `-` 分隔）。
///
/// 角色名不允许包含 `-`，因此只要出现 `-` 即为「含 event」；无 `-` 为纯角色。
/// 角色名为空时后缀形如 `-e1`，拆分出空角色名。
pub fn split_role_event(suffix: &str) -> Option<(&str, &str)> {
    suffix.split_once('-')
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
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW);
        tree.create_node("role-2", Some("role-1"), CaseMode::Role, NOW);
        tree.create_node("role-3", Some("role-1"), CaseMode::Role, NOW);

        assert_eq!(tree.top_level_ids(), vec!["role-1"]);
        assert_eq!(tree.children_ids("role-1"), vec!["role-2", "role-3"]);
        assert!(tree.is_leaf("role-2"));
        assert!(!tree.is_leaf("role-1"));
        assert_eq!(tree.ancestors("role-3"), vec!["role-1"]);
        assert!(tree.ancestors("role-1").is_empty());
        assert_eq!(tree.get("role-2").unwrap().parent_id.as_ref().unwrap().as_str(), "role-1");
    }

    #[test]
    fn role_candidates_exclude_event_cases() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW);
        tree.create_node("role-2", Some("role-1"), CaseMode::Role, NOW);
        tree.create_node("event-e12", None, CaseMode::Event, NOW);

        assert_eq!(tree.top_level_ids(), vec!["event-e12", "role-1"], "查询可见全部顶层 case（按 created_at、case_id 升序）");
        assert_eq!(tree.role_top_level_ids(), vec!["role-1"], "角色模式判定只看角色子树");
        assert_eq!(tree.role_children_ids("role-1"), vec!["role-2"]);
        assert_eq!(tree.ids_of_mode(CaseMode::Event), ["event-e12".to_string()].into_iter().collect());
    }

    #[test]
    fn remove_nodes_unlinks_parents() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW);
        tree.create_node("role-2", Some("role-1"), CaseMode::Role, NOW);
        tree.create_node("event-e12", None, CaseMode::Event, NOW);

        tree.remove_nodes(&tree.ids_of_mode(CaseMode::Event));
        assert!(!tree.contains("event-e12"));
        assert_eq!(tree.top_level_ids(), vec!["role-1"]);

        tree.remove_nodes(&tree.ids_of_mode(CaseMode::Role));
        assert!(tree.is_empty(), "角色子树整体删除（含子节点）");
    }

    #[test]
    fn apply_records_tracks_count_and_time_bounds() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW);
        tree.apply_records("role-1", &[entry("2026-09-22 10:00:00", 1), entry("2026-09-22 10:05:00", 2)], NOW);
        let node = tree.get("role-1").unwrap();
        assert_eq!(node.record_count, 2);
        assert_eq!(node.start_time.as_str(), "2026-09-22 10:00:00");
        assert_eq!(node.end_time.as_str(), "2026-09-22 10:05:00");

        // 追加更早与更晚的记录：边界随内容扩展
        tree.apply_records("role-1", &[entry("2026-09-21 09:00:00", 3)], NOW);
        tree.apply_records("role-1", &[entry("2026-09-23 09:00:00", 4)], NOW);
        let node = tree.get("role-1").unwrap();
        assert_eq!(node.record_count, 4);
        assert_eq!(node.start_time.as_str(), "2026-09-21 09:00:00");
        assert_eq!(node.end_time.as_str(), "2026-09-23 09:00:00");
    }

    #[test]
    fn allocate_case_id_increments_within_role() {
        let mut tree = CaseTree::new();
        assert_eq!(tree.allocate_case_id(), "role-1");
        tree.create_node("role-1", None, CaseMode::Role, NOW);
        tree.create_node("role-2", Some("role-1"), CaseMode::Role, NOW);
        assert_eq!(tree.allocate_case_id(), "role-3");
        // 事件模式的固定 case 不参与角色编号
        tree.create_node("event-e1", None, CaseMode::Event, NOW);
        assert_eq!(tree.allocate_case_id(), "role-3", "event-* 不参与角色编号");
        // 数字取最大值而非最大值+1（避免删除后重号）
        tree.create_node("role-10", None, CaseMode::Role, NOW);
        assert_eq!(tree.allocate_case_id(), "role-11");
    }

    #[test]
    fn from_nodes_keeps_all() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW);
        let nodes = tree.nodes().into_iter().cloned().collect::<Vec<_>>();
        let restored = CaseTree::from_nodes(nodes);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored.top_level_ids(), vec!["role-1"]);
    }

    #[test]
    fn briefs_skip_missing_and_keep_order() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW);
        tree.set_summary("role-1", "摘要一", NOW);
        tree.create_node("role-2", None, CaseMode::Role, NOW);
        tree.set_summary("role-2", "摘要二", NOW);
        let briefs = tree.briefs(&["role-2".to_string(), "missing".to_string(), "role-1".to_string()]);
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].case_id.as_str(), "role-2");
        assert_eq!(briefs[0].summary.as_str(), "摘要二");
        assert_eq!(briefs[1].case_id.as_str(), "role-1");
    }

    #[test]
    fn role_event_detection() {
        // 以第一个 '-' 分隔：前半为角色名、后半为事件标识（角色名不允许含 '-'）
        assert_eq!(split_role_event("dev-e1"), Some(("dev", "e1")));
        assert_eq!(split_role_event("-e12"), Some(("", "e12")));
        assert_eq!(split_role_event("dev"), None);
        assert_eq!(split_role_event("my-role"), Some(("my", "role")));
        assert_eq!(split_role_event("dev-e1-x"), Some(("dev", "e1-x")), "只按第一个 '-' 拆分");
    }
}
