//! Case 树：节点、层级、当前 case 指针的读写与持久化支撑。
//!
//! 树以**角色**（[`crate::agent_role::AgentRole`]，`agent + 角色名`）为归属组织，跨年份持续累积；
//! 节点即 kissbot-api 的 [`CaseNode`]。节点**不带模式/事件字段**：角色模式的 case 由模型判定划分
//! （名称 = `role-{模型给出的名称}`），事件模式的 case 是按 `event_id` 直接定位的根节点
//! （名称 = `event-{事件标识}`）。**case 名称就是 case 的唯一标识**，没有单独分配的 id；两种名称
//! 前缀不同，因此角色模式与事件模式天然不会重名。
//!
//! 一个 AgentRole 的 case 组织成**森林**：角色模式一棵树（固定根 `__role_root__`，其子节点是角色
//! case），每个事件一棵树（根是 `event-{事件标识}`）。固定根只用于遍历，不是业务 case，对外查询
//! 隐藏；事件根是业务 case，对外可见。

use std::collections::{BTreeMap, BTreeSet};

use arcstr::ArcStr;
use kissbot_api::{CaseBrief, CaseNode};

use crate::records::RecordEntry;

/// 角色模式 case 名前缀：角色 case 与事件 case **共用同一片森林**，两者靠名称前缀区分
/// （角色 `role-`、事件 `event-`），节点本身不记录模式字段。
pub const ROLE_CASE_PREFIX: ArcStr = arcstr::literal!("role-");

/// 事件模式 case 名前缀（与 `role-` 天然不同名，因此同一片森林里两种 case 不会混淆）
pub const EVENT_CASE_PREFIX: ArcStr = arcstr::literal!("event-");

/// 事件模式的事件根名：`event-{事件标识}`（事件 case 即按它定位/查找；通常是叶子，非叶子时下潜）。
pub fn event_case_name(event_id: &str) -> ArcStr {
    ArcStr::from(format!("{}{}", EVENT_CASE_PREFIX, event_id))
}

/// 是否是角色模式的 case：名称带 `role-` 前缀。
///
/// 两种 case 共用一片森林，判定候选一律**按前缀正选**（取 `role-` 前缀），
/// 而不是「排除 `event-` 前缀」的反选——避免任何非预期名称被当成角色 case 参与判定。
pub fn is_role_case(case_name: &str) -> bool {
    case_name.starts_with(ROLE_CASE_PREFIX.as_str())
}

/// 角色模式的固定根 case 名：只用于遍历，不是业务 case，对外查询隐藏；摘要恒为空。
/// 名称不参与模型命名（`role-` / `event-` 前缀都不匹配），也不可能来自 meta（含 `_`）。
pub const ROLE_ROOT_CASE_NAME_STR: &str = "__role_root__";

pub const ROLE_ROOT_CASE_NAME: ArcStr = arcstr::literal!("__role_root__");

/// 是否是角色模式的固定根（隐藏节点）
pub fn is_role_root(case_name: &str) -> bool {
    case_name == ROLE_ROOT_CASE_NAME_STR
}

#[derive(Debug, Clone)]
pub struct CaseTree {
    nodes: BTreeMap<ArcStr, CaseNode>,
}

impl Default for CaseTree {
    fn default() -> Self {
        Self::new()
    }
}

impl CaseTree {
    /// 新建空森林：只包含角色模式的固定根（复合 case、摘要为空）
    pub fn new() -> Self {
        let now = crate::records::now_string();
        let mut nodes = BTreeMap::new();
        nodes.insert(ROLE_ROOT_CASE_NAME.clone(), Self::role_root_node(&now));
        Self { nodes }
    }

    /// 从持久化节点恢复：补齐/规范化角色根，并把旧数据里的角色顶层节点挂到角色根下。
    /// 事件根保持森林顶层（parent 为 None）。
    pub fn from_nodes(nodes: Vec<CaseNode>) -> Self {
        let mut map: BTreeMap<ArcStr, CaseNode> =
            nodes.into_iter().map(|n| (n.case_name.clone(), n)).collect();
        let now = crate::records::now_string();
        if !map.contains_key(ROLE_ROOT_CASE_NAME_STR) {
            map.insert(ROLE_ROOT_CASE_NAME.clone(), Self::role_root_node(&now));
        }
        if let Some(root) = map.get_mut(ROLE_ROOT_CASE_NAME_STR) {
            root.parent_id = None;
            root.summary = ArcStr::new();
            root.record_count = 0;
        }

        // 旧数据可能没有角色根，或父节点缺失：角色 case 统一挂到角色根下；
        // 事件根（以及无法识别的节点）保持/回到森林顶层。
        let names: Vec<ArcStr> = map.keys().cloned().collect();
        for name in &names {
            if is_role_root(name) {
                continue;
            }
            let parent = map.get(name).and_then(|node| node.parent_id.clone());
            let missing_parent = parent.as_ref().map(|parent| !map.contains_key(parent)).unwrap_or(true);
            if missing_parent {
                let fallback = if is_role_case(name) {
                    Some(ROLE_ROOT_CASE_NAME.clone())
                } else {
                    None
                };
                if let Some(node) = map.get_mut(name) {
                    node.parent_id = fallback;
                }
            }
        }

        // 由 parent_id 重建 children，避免旧文件里的 children 与 parent 不一致
        let links: Vec<(ArcStr, Option<ArcStr>)> =
            map.iter().map(|(name, node)| (name.clone(), node.parent_id.clone())).collect();
        for node in map.values_mut() {
            node.children.clear();
        }
        for (name, parent) in links {
            if let Some(parent_node) = parent.and_then(|parent| map.get_mut(&parent)) {
                parent_node.children.push(name);
            }
        }
        Self { nodes: map }
    }

    fn role_root_node(now: &str) -> CaseNode {
        CaseNode {
            case_name: ROLE_ROOT_CASE_NAME.clone(),
            parent_id: None,
            children: Vec::new(),
            summary: ArcStr::new(),
            record_count: 0,
            start_time: ArcStr::from(now),
            end_time: ArcStr::from(now),
            created_at: ArcStr::from(now),
            updated_at: ArcStr::from(now),
        }
    }

    /// 确保角色根存在并返回它（角色根摘要固定为空）
    pub fn ensure_role_root(&mut self, now: &str) -> CaseNode {
        if let Some(root) = self.nodes.get(ROLE_ROOT_CASE_NAME_STR) {
            return root.clone();
        }
        let root = Self::role_root_node(now);
        self.nodes.insert(ROLE_ROOT_CASE_NAME.clone(), root.clone());
        root
    }

    /// 森林是否为空（不含角色根；测试断言用）
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 业务 case 数（不含隐藏的角色根；事件根算业务 case）
    pub fn len(&self) -> usize {
        self.nodes.len().saturating_sub(1)
    }

    pub fn get(&self, case_name: &str) -> Option<&CaseNode> {
        self.nodes.get(case_name)
    }

    /// 对外的节点列表（不含隐藏的角色根；按 created_at、case_name 升序）
    pub fn nodes(&self) -> Vec<&CaseNode> {
        let mut nodes: Vec<&CaseNode> =
            self.nodes.values().filter(|node| !is_role_root(&node.case_name)).collect();
        nodes.sort_by(|a, b| (a.created_at.as_str(), a.case_name.as_str()).cmp(&(b.created_at.as_str(), b.case_name.as_str())));
        nodes
    }

    /// 持久化用的全部节点（含角色根）
    pub fn all_nodes(&self) -> Vec<&CaseNode> {
        let mut nodes: Vec<&CaseNode> = self.nodes.values().collect();
        nodes.sort_by(|a, b| (a.created_at.as_str(), a.case_name.as_str()).cmp(&(b.created_at.as_str(), b.case_name.as_str())));
        nodes
    }

    pub fn contains(&self, case_name: &str) -> bool {
        self.nodes.contains_key(case_name)
    }

    /// 事件模式固定的根 case（按 `event_id` 直接定位）
    pub fn event_case(&self, event_id: &str) -> Option<&CaseNode> {
        self.nodes.get(event_case_name(event_id).as_str())
    }

    /// 全部角色模式 case id（重建时清空的角色部分；只取 `role-` 前缀，角色根、事件 case 不在内）
    pub fn role_case_ids(&self) -> BTreeSet<ArcStr> {
        self.nodes
            .iter()
            .filter(|(case_name, _)| is_role_case(case_name))
            .map(|(case_name, _)| case_name.clone())
            .collect()
    }

    /// 森林顶层（角色根的子节点 + 事件根），对外查询用
    pub fn forest_top_level_ids(&self) -> Vec<ArcStr> {
        self.nodes()
            .into_iter()
            .filter(|node| {
                node.parent_id.is_none() || node.parent_id.as_deref() == Some(ROLE_ROOT_CASE_NAME_STR)
            })
            .map(|node| node.case_name.clone())
            .collect()
    }

    /// 森林顶层 case id（测试断言用，等价于 `forest_top_level_ids`）
    #[cfg(test)]
    pub fn top_level_ids(&self) -> Vec<ArcStr> {
        self.forest_top_level_ids()
    }

    /// 角色模式顶层 case id（角色根的直接子节点；事件根不参与）
    pub fn role_top_level_ids(&self) -> Vec<ArcStr> {
        self.children_ids(ROLE_ROOT_CASE_NAME_STR)
            .into_iter()
            .filter(|id| is_role_case(id))
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

    /// 角色模式的子 case id（逐层判定的候选：只取 `role-` 前缀；事件 case 不参与）
    pub fn role_children_ids(&self, case_name: &str) -> Vec<ArcStr> {
        self.children_ids(case_name)
            .into_iter()
            .filter(|id| is_role_case(id))
            .collect()
    }

    /// 叶子 case：没有子节点的业务 case；角色根永远是复合 case
    pub fn is_leaf(&self, case_name: &str) -> bool {
        !is_role_root(case_name)
            && self.nodes.get(case_name).map(|node| node.children.is_empty()).unwrap_or(false)
    }

    pub fn brief(&self, case_name: &str) -> Option<CaseBrief> {
        self.nodes.get(case_name).map(CaseBrief::from)
    }

    pub fn briefs(&self, ids: &[ArcStr]) -> Vec<CaseBrief> {
        ids.iter().filter_map(|id| self.brief(id)).collect()
    }

    /// 业务 case 的父链（自近到远，不包含隐藏的角色根）
    pub fn ancestors(&self, case_name: &str) -> Vec<ArcStr> {
        let mut chain = Vec::new();
        let mut cursor = self.nodes.get(case_name).and_then(|node| node.parent_id.clone());
        while let Some(parent) = cursor {
            if is_role_root(&parent) {
                break;
            }
            chain.push(parent.clone());
            cursor = self.nodes.get(parent.as_str()).and_then(|node| node.parent_id.clone());
        }
        chain
    }

    /// 新建节点并挂到父节点下。
    /// - `parent_id` 给定时挂到该父节点；
    /// - `parent_id` 为 None：角色 case（`role-` 前缀）挂到角色根下，事件根（`event-` 前缀）
    ///   及其他节点保持森林顶层。
    pub fn create_node(&mut self, case_name: &str, parent_id: Option<&str>, now: &str) -> CaseNode {
        if is_role_root(case_name) {
            return self.ensure_role_root(now);
        }
        self.ensure_role_root(now);
        let default_parent = if is_role_case(case_name) {
            Some(ROLE_ROOT_CASE_NAME.clone())
        } else {
            None
        };
        let parent = match parent_id {
            Some(parent) if self.nodes.contains_key(parent) => Some(ArcStr::from(parent)),
            Some(_) => default_parent,
            None => default_parent,
        };
        let node = CaseNode {
            case_name: ArcStr::from(case_name),
            parent_id: parent.clone(),
            children: Vec::new(),
            summary: ArcStr::new(),
            record_count: 0,
            start_time: ArcStr::from(now),
            end_time: ArcStr::from(now),
            created_at: ArcStr::from(now),
            updated_at: ArcStr::from(now),
        };
        if let Some(parent_node) = parent.and_then(|parent| self.nodes.get_mut(&parent)) {
            parent_node.children.push(ArcStr::from(case_name));
        }
        self.nodes.insert(node.case_name.clone(), node.clone());
        node
    }

    /// 删除若干 case 及其整棵子树（并从父节点的 children 摘除）；隐藏的角色根永远不会被删除。
    /// 重建角色子树 / 事件子树时使用，保证事件 case 的非叶子子节点也被清掉。
    pub fn remove_nodes(&mut self, case_names: &BTreeSet<ArcStr>) {
        let mut remove: BTreeSet<ArcStr> = BTreeSet::new();
        for case_name in case_names {
            self.collect_subtree(case_name, &mut remove);
        }
        for case_name in &remove {
            self.nodes.remove(case_name);
        }
        for node in self.nodes.values_mut() {
            node.children.retain(|child| !remove.contains(child));
        }
    }

    fn collect_subtree(&self, case_name: &str, remove: &mut BTreeSet<ArcStr>) {
        if is_role_root(case_name) {
            return;
        }
        let Some(node) = self.nodes.get(case_name) else {
            return;
        };
        remove.insert(ArcStr::from(case_name));
        for child in &node.children {
            self.collect_subtree(child, remove);
        }
    }

    /// case 内容变化后更新节点统计（记录条数、起止时间、更新时间）。
    /// 原始记录只能写入叶子 case；非叶子（含角色根）返回 None，由归档器按失败处理。
    pub fn apply_records(&mut self, case_name: &str, entries: &[RecordEntry], now: &str) -> Option<&CaseNode> {
        if !self.is_leaf(case_name) {
            return None;
        }
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
        // 角色根的摘要固定为空
        if is_role_root(case_name) {
            return;
        }
        if let Some(node) = self.nodes.get_mut(case_name) {
            node.summary = ArcStr::from(summary);
            node.updated_at = ArcStr::from(now);
        }
    }

    /// 角色模式的新 case 名：`role-{模型给出的名称}`；同一角色树里重名时追加 `-2`、`-3`…
    pub fn unique_role_case_name(&self, name: &str) -> ArcStr {
        self.unique_case_name(ROLE_CASE_PREFIX.as_str(), name)
    }

    /// Event case 非叶子、下潜遍历时的新子 case 名：`event-{事件标识}-{模型给出的名称}`。
    /// 与 `role-` 前缀天然不同名，角色模式的候选看不到它。
    pub fn unique_event_child_case_name(&self, event_id: &str, name: &str) -> ArcStr {
        let prefix = format!("{}-", event_case_name(event_id));
        self.unique_case_name(&prefix, name)
    }

    fn unique_case_name(&self, prefix: &str, name: &str) -> ArcStr {
        let base = format!("{}{}", prefix, name);
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
    use crate::records::RecordEntry;
    use kissbot_api::ChannelRecord;

    const NOW: ArcStr = arcstr::literal!("2026-09-22 13:49:00");

    fn entry(time: &str, sn: u64) -> RecordEntry {
        Arc::new(ChannelRecord {
            meta: None,
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
        })
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

    fn plain_node(name: &str, parent: Option<&str>) -> CaseNode {
        CaseNode {
            case_name: ArcStr::from(name),
            parent_id: parent.map(ArcStr::from),
            children: Vec::new(),
            summary: ArcStr::new(),
            record_count: 0,
            start_time: NOW.clone(),
            end_time: NOW.clone(),
            created_at: NOW.clone(),
            updated_at: NOW.clone(),
        }
    }

    #[test]
    fn role_root_is_hidden_and_role_cases_are_its_children() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, NOW.as_str());
        tree.create_node("event-e1", None, NOW.as_str());

        assert_eq!(tree.len(), 2, "业务 case 数不含角色根");
        assert_eq!(tree.nodes().len(), 2, "对外节点不含角色根");
        assert_eq!(tree.all_nodes().len(), 3, "持久化包含角色根");
        assert!(tree.get(ROLE_ROOT_CASE_NAME_STR).is_some());
        assert_eq!(tree.children_ids(ROLE_ROOT_CASE_NAME_STR), vec![ArcStr::from("role-1")], "角色 case 挂在角色根下");
        assert_eq!(tree.get("role-1").unwrap().parent_id.as_deref(), Some(ROLE_ROOT_CASE_NAME_STR));
        assert_eq!(tree.get("event-e1").unwrap().parent_id, None, "事件根是森林顶层");
        assert_eq!(
            tree.forest_top_level_ids(),
            vec![ArcStr::from("event-e1"), ArcStr::from("role-1")],
            "森林顶层 = 事件根 + 角色根下的角色 case"
        );
        assert_eq!(tree.role_top_level_ids(), vec![ArcStr::from("role-1")]);
        assert!(tree.is_leaf("role-1"));
        assert!(!tree.is_leaf(ROLE_ROOT_CASE_NAME_STR), "角色根固定是复合 case");
    }

    #[test]
    fn from_nodes_normalizes_old_data_and_keeps_event_roots() {
        let restored = CaseTree::from_nodes(vec![plain_node("role-1", None), plain_node("event-e1", None)]);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.all_nodes().len(), 3);
        assert_eq!(restored.get("role-1").unwrap().parent_id.as_deref(), Some(ROLE_ROOT_CASE_NAME_STR), "旧的角色顶层节点挂到角色根下");
        assert_eq!(restored.get("event-e1").unwrap().parent_id, None, "事件根保持在森林顶层");
        assert_eq!(
            restored.forest_top_level_ids(),
            vec![ArcStr::from("event-e1"), ArcStr::from("role-1")]
        );
    }

    #[test]
    fn remove_nodes_removes_subtree_and_keeps_role_root() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, NOW.as_str());
        tree.create_node("role-2", Some("role-1"), NOW.as_str());
        tree.create_node("event-e1", None, NOW.as_str());

        tree.remove_nodes(&tree.role_case_ids());
        assert!(tree.get(ROLE_ROOT_CASE_NAME_STR).is_some(), "角色根不删除");
        assert!(tree.children_ids(ROLE_ROOT_CASE_NAME_STR).is_empty(), "角色 subtree 整体删除");
        assert_eq!(tree.len(), 1, "事件根保留");
        tree.remove_nodes(&[ArcStr::from("event-e1")].into_iter().collect());
        assert_eq!(tree.len(), 0);
        assert!(tree.get(ROLE_ROOT_CASE_NAME_STR).is_some(), "删除事件根也不影响角色根");
    }

    #[test]
    fn apply_records_rejects_non_leaf() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, NOW.as_str());
        tree.create_node("role-2", Some("role-1"), NOW.as_str());
        assert!(tree.apply_records("role-1", &[entry("2026-09-22 10:00:00", 1)], NOW.as_str()).is_none(), "复合 case 不收记录");
        assert!(tree.apply_records(ROLE_ROOT_CASE_NAME_STR, &[entry("2026-09-22 10:00:00", 1)], NOW.as_str()).is_none(), "角色根不收记录");
        assert!(tree.apply_records("role-2", &[entry("2026-09-22 10:00:00", 1)], NOW.as_str()).is_some(), "叶子可以收记录");
    }

    #[test]
    fn unique_event_child_name_uses_event_prefix() {
        let mut tree = CaseTree::new();
        assert_eq!(tree.unique_event_child_case_name("e1", "topic"), "event-e1-topic");
        tree.create_node("event-e1-topic", Some("event-e1"), NOW.as_str());
        assert_eq!(tree.unique_event_child_case_name("e1", "topic"), "event-e1-topic-2");
        assert_eq!(tree.unique_event_child_case_name("e1", "other"), "event-e1-other");
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
