//! Case 树：节点、层级、当前 case 指针的读写与持久化支撑。
//!
//! 树以**角色**（[`crate::agent_role::AgentRole`]，`agent + 角色名`）为归属组织，跨年份持续累积；
//! 节点即 kissbot-api 的 [`CaseNode`]。节点**不带模式/事件字段**：角色模式的 case 由模型判定划分
//! （名称 = `role-{模型给出的名称}`），事件模式的 case 是按 `event_id` 直接定位的顶层叶子
//! （名称 = `event-{事件标识}`）。**case 名称就是 case 的唯一标识**，没有单独分配的 id；两种名称
//! 前缀不同，因此角色模式与事件模式天然不会重名。

use std::collections::{BTreeMap, BTreeSet};

use arcstr::ArcStr;
use kissbot_api::{CaseBrief, CaseNode};

use crate::records::RecordEntry;

/// 角色模式 case 名前缀：角色 case 与事件 case **共用同一棵树**，两者靠名称前缀区分
/// （角色 `role-`、事件 `event-`），节点本身不记录模式字段。
pub const ROLE_CASE_PREFIX: ArcStr = arcstr::literal!("role-");

/// 事件模式 case 名前缀（与 `role-` 天然不同名，因此同一棵树上两种 case 不会混淆）
pub const EVENT_CASE_PREFIX: ArcStr = arcstr::literal!("event-");

/// 事件模式固定的顶层叶子 case 名：`event-{事件标识}`（事件 case 即按它定位/查找）。
pub fn event_case_name(event_id: &str) -> ArcStr {
    ArcStr::from(format!("{}{}", EVENT_CASE_PREFIX, event_id))
}

/// 是否是角色模式的 case：名称带 `role-` 前缀。
///
/// 两种 case 共用一棵树，判定候选一律**按前缀正选**（取 `role-` 前缀），
/// 而不是「排除 `event-` 前缀」的反选——避免任何非预期名称被当成角色 case 参与判定。
pub fn is_role_case(case_name: &str) -> bool {
    case_name.starts_with(ROLE_CASE_PREFIX.as_str())
}

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

    /// 事件模式固定的叶子 case（按 `event_id` 直接定位）
    pub fn event_case(&self, event_id: &str) -> Option<&CaseNode> {
        self.nodes.get(event_case_name(event_id).as_str())
    }

    /// 全部角色模式 case id（重建时清空的角色部分；只取 `role-` 前缀，事件叶子不在内）
    pub fn role_case_ids(&self) -> BTreeSet<ArcStr> {
        self.nodes
            .iter()
            .filter(|(case_name, _)| is_role_case(case_name))
            .map(|(case_name, _)| case_name.clone())
            .collect()
    }

    /// 顶层 case id（测试断言用）
    #[cfg(test)]
    pub fn top_level_ids(&self) -> Vec<ArcStr> {
        self.ids_of(false)
    }

    /// 角色模式顶层 case id（逐层判定的候选：只取 `role-` 前缀的 case，事件叶子 `event-` 不参与）
    pub fn role_top_level_ids(&self) -> Vec<ArcStr> {
        self.ids_of(true)
    }

    fn ids_of(&self, role_only: bool) -> Vec<ArcStr> {
        self.nodes()
            .into_iter()
            .filter(|n| n.parent_id.is_none())
            .filter(|n| !role_only || is_role_case(&n.case_name))
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

    /// 角色模式的子 case id（逐层判定的候选：只取 `role-` 前缀；事件叶子不参与）
    pub fn role_children_ids(&self, case_name: &str) -> Vec<ArcStr> {
        self.children_ids(case_name)
            .into_iter()
            .filter(|id| is_role_case(id))
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
    pub fn create_node(&mut self, case_name: &str, parent_id: Option<&str>, now: &str) -> CaseNode {
        let node = CaseNode {
            case_name: ArcStr::from(case_name),
            parent_id: parent_id.map(ArcStr::from),
            children: Vec::new(),
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

    /// 删除若干 case（并从父节点的 children 摘除）；重建时清空角色子树用
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
        tree.create_node("role-1", None, NOW.as_str());
        tree.create_node("role-2", Some("role-1"), NOW.as_str());
        tree.create_node("role-3", Some("role-1"), NOW.as_str());

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
        tree.create_node("role-1", None, NOW.as_str());
        tree.create_node("role-2", Some("role-1"), NOW.as_str());
        tree.create_node("event-e12", None, NOW.as_str());

        assert_eq!(tree.top_level_ids(), vec![ArcStr::from("event-e12"), ArcStr::from("role-1")], "查询可见全部顶层 case（按 created_at、case_name 升序）");
        assert_eq!(tree.role_top_level_ids(), vec![ArcStr::from("role-1")], "角色模式判定只看角色 case（只取 role- 前缀）");
        assert_eq!(tree.role_children_ids("role-1"), vec![ArcStr::from("role-2")]);
        assert_eq!(tree.role_case_ids(), [ArcStr::from("role-1"), ArcStr::from("role-2")].into_iter().collect());
        // 事件 case 按 event_id 直接定位（节点不带模式字段）
        assert_eq!(tree.event_case("e12").map(|n| n.case_name.as_str()), Some("event-e12"));
        assert!(tree.event_case("e13").is_none());
    }

    /// 前缀区分是**正选**：只有 `role-` 前缀的名称算角色 case，非两种前缀的名称不参与角色判定
    #[test]
    fn role_prefix_is_matched_positively() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, NOW.as_str());
        tree.create_node("event-e1", None, NOW.as_str());
        tree.create_node("stray", None, NOW.as_str());

        assert_eq!(tree.role_top_level_ids(), vec![ArcStr::from("role-1")], "非 role- 前缀不参与角色判定");
        assert_eq!(tree.role_case_ids(), [ArcStr::from("role-1")].into_iter().collect());
        assert!(is_role_case("role-x"));
        assert!(!is_role_case("event-e1"));
        assert!(!is_role_case("stray"));
    }

    #[test]
    fn remove_nodes_unlinks_parents() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, NOW.as_str());
        tree.create_node("role-2", Some("role-1"), NOW.as_str());
        tree.create_node("event-e12", None, NOW.as_str());

        tree.remove_nodes(&[ArcStr::from("event-e12")].into_iter().collect());
        assert!(!tree.contains("event-e12"));
        assert_eq!(tree.top_level_ids(), vec![ArcStr::from("role-1")]);

        tree.remove_nodes(&tree.role_case_ids());
        assert!(tree.is_empty(), "角色 case 整体删除（含子节点）");
    }

    #[test]
    fn apply_records_tracks_count_and_time_bounds() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, NOW.as_str());
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
        tree.create_node("role-weather-chat", None, NOW.as_str());
        assert_eq!(tree.unique_role_case_name("weather-chat"), "role-weather-chat-2");
        tree.create_node("role-weather-chat-2", None, NOW.as_str());
        assert_eq!(tree.unique_role_case_name("weather-chat"), "role-weather-chat-3");
        // 事件 case 名带 event- 前缀，与角色名天然不会重名
        tree.create_node("event-e1", None, NOW.as_str());
        assert_eq!(tree.unique_role_case_name("e1"), "role-e1");
    }

    #[test]
    fn from_nodes_keeps_all() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, NOW.as_str());
        let nodes = tree.nodes().into_iter().cloned().collect::<Vec<_>>();
        let restored = CaseTree::from_nodes(nodes);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored.top_level_ids(), vec![ArcStr::from("role-1")]);
    }

    #[test]
    fn briefs_skip_missing_and_keep_order() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, NOW.as_str());
        tree.set_summary("role-1", "摘要一", NOW.as_str());
        tree.create_node("role-2", None, NOW.as_str());
        tree.set_summary("role-2", "摘要二", NOW.as_str());
        let briefs = tree.briefs(&[ArcStr::from("role-2"), ArcStr::from("missing"), ArcStr::from("role-1")]);
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].case_name.as_str(), "role-2");
        assert_eq!(briefs[0].summary.as_str(), "摘要二");
        assert_eq!(briefs[1].case_name.as_str(), "role-1");
    }
}
