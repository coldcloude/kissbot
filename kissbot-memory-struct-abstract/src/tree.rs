//! Case 树：节点、层级、当前 case 指针的读写与持久化支撑。
//!
//! 树以**角色**（`agent + 角色名`）为作用域组织，跨年份持续累积；节点即 kissbot-api 的
//! [`CaseNode`]。同一个角色的角色模式与事件模式共用这一棵树，但各走各的子树、互不并入：
//! 角色模式的 case 由模型判定划分（名称 = `role-{模型给出的名称}`），事件模式的 case 是外部事件
//! 固定的顶层叶子（名称 = `event-{事件标识}`）。**case 名称就是 case 的唯一标识**，没有单独分配的
//! id；两种名称前缀不同，因此角色模式与事件模式天然不会重名。

use std::collections::{BTreeMap, BTreeSet};

use arcstr::ArcStr;
use kissbot_api::{CaseBrief, CaseMode, CaseNode};

use crate::records::RecordEntry;

/// 角色模式 case 名前缀
pub const ROLE_CASE_PREFIX: ArcStr = arcstr::literal!("role-");

/// 事件模式 case 名前缀
pub const EVENT_CASE_PREFIX: ArcStr = arcstr::literal!("event-");

#[derive(Debug, Clone, Default)]
pub struct CaseTree {
    nodes: BTreeMap<ArcStr, CaseNode>,
}

impl CaseTree {
    pub fn new() -> Self {
        Self { nodes: BTreeMap::new() }
    }

    pub fn from_nodes(nodes: Vec<CaseNode>) -> Self {
        Self {
            nodes: nodes.into_iter().map(|n| (n.case_name.clone(), n)).collect(),
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

    pub fn get(&self, case_name: &str) -> Option<&CaseNode> {
        self.nodes.get(case_name)
    }

    /// 全部节点（按 created_at、case_name 升序）
    pub fn nodes(&self) -> Vec<&CaseNode> {
        let mut nodes: Vec<&CaseNode> = self.nodes.values().collect();
        nodes.sort_by(|a, b| (a.created_at.as_str(), a.case_name.as_str()).cmp(&(b.created_at.as_str(), b.case_name.as_str())));
        nodes
    }

    pub fn contains(&self, case_name: &str) -> bool {
        self.nodes.contains_key(case_name)
    }

    /// 某模式的全部 case id
    pub fn ids_of_mode(&self, mode: CaseMode) -> BTreeSet<ArcStr> {
        self.nodes
            .values()
            .filter(|n| n.mode == mode)
            .map(|n| n.case_name.clone())
            .collect()
    }

    /// 顶层 case id（两种子树都在；测试断言用）
    #[cfg(test)]
    pub fn top_level_ids(&self) -> Vec<ArcStr> {
        self.ids_of(None)
    }

    /// 角色模式顶层 case id（逐层判定的候选；事件模式的固定 case 不参与）
    pub fn role_top_level_ids(&self) -> Vec<ArcStr> {
        self.ids_of(Some(CaseMode::Role))
    }

    fn ids_of(&self, mode: Option<CaseMode>) -> Vec<ArcStr> {
        self.nodes()
            .into_iter()
            .filter(|n| n.parent_id.is_none())
            .filter(|n| mode.is_none_or(|mode| n.mode == mode))
            .map(|n| n.case_name.clone())
            .collect()
    }

    /// 某节点的子 case id（按 created_at、case_name 升序）
    pub fn children_ids(&self, case_name: &str) -> Vec<ArcStr> {
        let Some(node) = self.nodes.get(case_name) else {
            return Vec::new();
        };
        let mut ids = node.children.clone();
        ids.sort_by(|a, b| {
            let ca = self.nodes.get(a.as_str()).map(|n| n.created_at.as_str()).unwrap_or("");
            let cb = self.nodes.get(b.as_str()).map(|n| n.created_at.as_str()).unwrap_or("");
            (ca, a.as_str()).cmp(&(cb, b.as_str()))
        });
        ids
    }

    /// 角色模式的子 case id（逐层判定的候选）
    pub fn role_children_ids(&self, case_name: &str) -> Vec<ArcStr> {
        self.children_ids(case_name)
            .into_iter()
            .filter(|id| self.get(id).map(|n| n.mode == CaseMode::Role).unwrap_or(false))
            .collect()
    }

    pub fn is_leaf(&self, case_name: &str) -> bool {
        self.nodes.get(case_name).map(|n| n.is_leaf()).unwrap_or(false)
    }

    pub fn brief(&self, case_name: &str) -> Option<CaseBrief> {
        self.nodes.get(case_name).map(CaseBrief::from)
    }

    pub fn briefs(&self, ids: &[ArcStr]) -> Vec<CaseBrief> {
        ids.iter().filter_map(|id| self.brief(id)).collect()
    }

    /// 父链（自近到远）
    pub fn ancestors(&self, case_name: &str) -> Vec<ArcStr> {
        let mut chain = Vec::new();
        let mut cursor = self.nodes.get(case_name).and_then(|n| n.parent_id.clone());
        while let Some(parent) = cursor {
            chain.push(parent.clone());
            cursor = self.nodes.get(parent.as_str()).and_then(|n| n.parent_id.clone());
        }
        chain
    }

    /// 新建节点并挂到父节点下（`parent_id` 为 None 即顶层）
    pub fn create_node(&mut self, case_name: &str, parent_id: Option<&str>, mode: CaseMode, now: &str) -> CaseNode {
        let node = CaseNode {
            case_name: ArcStr::from(case_name),
            parent_id: parent_id.map(ArcStr::from),
            children: Vec::new(),
            mode,
            summary: ArcStr::new(),
            record_count: 0,
            start_time: ArcStr::from(now),
            end_time: ArcStr::from(now),
            created_at: ArcStr::from(now),
            updated_at: ArcStr::from(now),
        };
        if let Some(parent) = parent_id {
            if let Some(parent_node) = self.nodes.get_mut(parent) {
                parent_node.children.push(ArcStr::from(case_name));
            }
        }
        self.nodes.insert(node.case_name.clone(), node.clone());
        node
    }

    /// 删除若干 case（并从父节点的 children 摘除）；重建时按模式清空子树用
    pub fn remove_nodes(&mut self, case_names: &BTreeSet<ArcStr>) {
        for case_name in case_names {
            self.nodes.remove(case_name);
        }
        for node in self.nodes.values_mut() {
            node.children.retain(|child| !case_names.contains(child.as_str()));
        }
    }

    /// case 内容变化后更新节点统计（记录条数、起止时间、更新时间）
    pub fn apply_records(&mut self, case_name: &str, entries: &[RecordEntry], now: &str) -> Option<&CaseNode> {
        let node = self.nodes.get_mut(case_name)?;
        if !entries.is_empty() {
            let start = entries.iter().map(|e| e.time.as_str()).min().unwrap_or("");
            let end = entries.iter().map(|e| e.time.as_str()).max().unwrap_or("");
            if node.record_count == 0 || start < node.start_time.as_str() {
                node.start_time = ArcStr::from(start);
            }
            if node.record_count == 0 || end > node.end_time.as_str() {
                node.end_time = ArcStr::from(end);
            }
            node.record_count += entries.len() as u64;
        }
        node.updated_at = ArcStr::from(now);
        self.nodes.get(case_name)
    }

    pub fn set_summary(&mut self, case_name: &str, summary: &str, now: &str) {
        if let Some(node) = self.nodes.get_mut(case_name) {
            node.summary = ArcStr::from(summary);
            node.updated_at = ArcStr::from(now);
        }
    }

    /// 角色模式的新 case 名：`role-{模型给出的名称}`；同一角色树里重名时追加 `-2`、`-3`…
    pub fn unique_role_case_name(&self, name: &str) -> ArcStr {
        let base = format!("{}{}", ROLE_CASE_PREFIX, name);
        if !self.contains(&base) {
            return ArcStr::from(base);
        }
        for suffix in 2u64.. {
            let candidate = format!("{}-{}", base, suffix);
            if !self.contains(&candidate) {
                return ArcStr::from(candidate);
            }
        }
        unreachable!("u64 后缀不可能被用尽")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::records::{RecordBody, RecordEntry};
    use kissbot_api::ChannelRecord;

    const NOW: ArcStr = arcstr::literal!("2026-09-22 13:49:00");

    fn entry(time: &str, sn: u64) -> RecordEntry {
        RecordEntry {
            kind: kissbot_api::RecordKind::Channel,
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
                content: kissbot_api::Content::Text(ArcStr::from("hi")),
                time: ArcStr::from(time),
                sn,
            })),
        }
    }

    #[test]
    fn create_node_links_parent_and_lists_levels() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW.as_str());
        tree.create_node("role-2", Some("role-1"), CaseMode::Role, NOW.as_str());
        tree.create_node("role-3", Some("role-1"), CaseMode::Role, NOW.as_str());

        assert_eq!(tree.top_level_ids(), vec![ArcStr::from("role-1")]);
        assert_eq!(tree.children_ids("role-1"), vec![ArcStr::from("role-2"), ArcStr::from("role-3")]);
        assert!(tree.is_leaf("role-2"));
        assert!(!tree.is_leaf("role-1"));
        assert_eq!(tree.ancestors("role-3"), vec![ArcStr::from("role-1")]);
        assert!(tree.ancestors("role-1").is_empty());
        assert_eq!(tree.get("role-2").unwrap().parent_id.as_ref().unwrap().as_str(), "role-1");
    }

    #[test]
    fn role_candidates_exclude_event_cases() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW.as_str());
        tree.create_node("role-2", Some("role-1"), CaseMode::Role, NOW.as_str());
        tree.create_node("event-e12", None, CaseMode::Event, NOW.as_str());

        assert_eq!(tree.top_level_ids(), vec![ArcStr::from("event-e12"), ArcStr::from("role-1")], "查询可见全部顶层 case（按 created_at、case_name 升序）");
        assert_eq!(tree.role_top_level_ids(), vec![ArcStr::from("role-1")], "角色模式判定只看角色子树");
        assert_eq!(tree.role_children_ids("role-1"), vec![ArcStr::from("role-2")]);
        assert_eq!(tree.ids_of_mode(CaseMode::Event), [ArcStr::from("event-e12")].into_iter().collect());
    }

    #[test]
    fn remove_nodes_unlinks_parents() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW.as_str());
        tree.create_node("role-2", Some("role-1"), CaseMode::Role, NOW.as_str());
        tree.create_node("event-e12", None, CaseMode::Event, NOW.as_str());

        tree.remove_nodes(&tree.ids_of_mode(CaseMode::Event));
        assert!(!tree.contains("event-e12"));
        assert_eq!(tree.top_level_ids(), vec![ArcStr::from("role-1")]);

        tree.remove_nodes(&tree.ids_of_mode(CaseMode::Role));
        assert!(tree.is_empty(), "角色子树整体删除（含子节点）");
    }

    #[test]
    fn apply_records_tracks_count_and_time_bounds() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW.as_str());
        tree.apply_records("role-1", &[entry("2026-09-22 10:00:00", 1), entry("2026-09-22 10:05:00", 2)], NOW.as_str());
        let node = tree.get("role-1").unwrap();
        assert_eq!(node.record_count, 2);
        assert_eq!(node.start_time.as_str(), "2026-09-22 10:00:00");
        assert_eq!(node.end_time.as_str(), "2026-09-22 10:05:00");

        // 追加更早与更晚的记录：边界随内容扩展
        tree.apply_records("role-1", &[entry("2026-09-21 09:00:00", 3)], NOW.as_str());
        tree.apply_records("role-1", &[entry("2026-09-23 09:00:00", 4)], NOW.as_str());
        let node = tree.get("role-1").unwrap();
        assert_eq!(node.record_count, 4);
        assert_eq!(node.start_time.as_str(), "2026-09-21 09:00:00");
        assert_eq!(node.end_time.as_str(), "2026-09-23 09:00:00");
    }

    #[test]
    fn unique_role_case_name_appends_suffix_on_conflict() {
        let mut tree = CaseTree::new();
        assert_eq!(tree.unique_role_case_name("weather-chat"), "role-weather-chat");

        // 重名（含模型本批内给出同名）时追加 -2、-3…，名称仍然合法
        tree.create_node("role-weather-chat", None, CaseMode::Role, NOW.as_str());
        assert_eq!(tree.unique_role_case_name("weather-chat"), "role-weather-chat-2");
        tree.create_node("role-weather-chat-2", None, CaseMode::Role, NOW.as_str());
        assert_eq!(tree.unique_role_case_name("weather-chat"), "role-weather-chat-3");
        // 事件 case 名带 event- 前缀，与角色名天然不会重名
        tree.create_node("event-e1", None, CaseMode::Event, NOW.as_str());
        assert_eq!(tree.unique_role_case_name("e1"), "role-e1");
    }

    #[test]
    fn from_nodes_keeps_all() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW.as_str());
        let nodes = tree.nodes().into_iter().cloned().collect::<Vec<_>>();
        let restored = CaseTree::from_nodes(nodes);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored.top_level_ids(), vec![ArcStr::from("role-1")]);
    }

    #[test]
    fn briefs_skip_missing_and_keep_order() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, NOW.as_str());
        tree.set_summary("role-1", "摘要一", NOW.as_str());
        tree.create_node("role-2", None, CaseMode::Role, NOW.as_str());
        tree.set_summary("role-2", "摘要二", NOW.as_str());
        let briefs = tree.briefs(&[ArcStr::from("role-2"), ArcStr::from("missing"), ArcStr::from("role-1")]);
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].case_name.as_str(), "role-2");
        assert_eq!(briefs[0].summary.as_str(), "摘要二");
        assert_eq!(briefs[1].case_name.as_str(), "role-1");
    }
}
