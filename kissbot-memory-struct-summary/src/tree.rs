//! Case 树：节点、层级、当前 case 指针的读写与持久化支撑。
//!
//! case 的唯一标识是**相对森林的完整路径**（见 [`crate::case_name`]）：角色 case 的路径由模型名
//! 逐层拼接，事件根的路径固定为 `[event]{event_id}`。同一父节点下最后一段相同即同一个 case；
//! 不同父节点下同名是不同 case。节点**不带模式/事件字段**，路径本身区分角色树与事件树。
//!
//! 一个 AgentRole 的 case 组织成**森林**：角色模式一棵树（隐藏固定根 `[role]`，其子节点是角色
//! case），每个事件一棵树（根是 `[event]{事件标识}`）。隐藏根只用于遍历，不是业务 case，对外查询
//! 隐藏；事件根是业务 case，对外可见。

use std::collections::{BTreeMap, BTreeSet};

use arcstr::ArcStr;
use kissbot_api::{CaseBrief, CaseNode};

use crate::records::RecordEntry;

pub use crate::case_name::{
    event_root_name as event_case_name, is_event_root, is_role_case, is_role_root,
    EVENT_ROOT_PREFIX, ROLE_ROOT_CASE_NAME, ROLE_ROOT_CASE_NAME_STR,
};

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

    /// 从持久化节点恢复：补齐/规范化角色根，按 `parent_id`（缺失时按路径）重建层级。
    /// 事件根保持在森林顶层；隐藏角色根永远是复合 case。
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

        // 旧数据可能没有角色根、或父节点缺失：优先用路径推导父节点，推导不出时角色 case 挂到
        // 角色根下、事件根保持森林顶层。事件根永远在顶层。
        let names: Vec<ArcStr> = map.keys().cloned().collect();
        for name in &names {
            if is_role_root(name) {
                continue;
            }
            let stored = map.get(name).and_then(|node| node.parent_id.clone());
            let derived = name.rsplit_once('/').map(|(parent, _)| ArcStr::from(parent));
            let parent = match stored {
                Some(parent) if map.contains_key(&parent) => Some(parent),
                _ => derived.filter(|parent| map.contains_key(parent)),
            };
            let parent = if is_event_root(name) {
                None
            } else {
                parent.or_else(|| is_role_case(name).then(|| ROLE_ROOT_CASE_NAME.clone()))
            };
            if let Some(node) = map.get_mut(name) {
                node.parent_id = parent;
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

    /// 事件模式固定的事件根（按 `event_id` 直接定位）
    pub fn event_case(&self, event_id: &str) -> Option<&CaseNode> {
        self.nodes.get(event_case_name(event_id).as_str())
    }

    /// 全部角色模式 case（含嵌套；角色根、事件树不在内）
    pub fn role_case_ids(&self) -> BTreeSet<ArcStr> {
        self.nodes.keys().filter(|case_name| is_role_case(case_name)).cloned().collect()
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

    /// 角色模式顶层 case id（角色根的直接子节点）
    pub fn role_top_level_ids(&self) -> Vec<ArcStr> {
        self.children_ids(ROLE_ROOT_CASE_NAME_STR)
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

    /// 按父路径 + 最后一段查找子 case（父为 None 表示角色顶层）
    pub fn child_id(&self, parent: Option<&str>, name: &str) -> Option<ArcStr> {
        let path = match parent {
            Some(parent) => format!("{}/{}", parent, name),
            None => name.to_string(),
        };
        self.nodes.contains_key(path.as_str()).then(|| ArcStr::from(path))
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

    /// 新建顶层 case：`[event]*` 建事件根，其余建角色顶层 case。
    /// 已存在时直接返回已有路径（同父同名即同一个 case）。
    pub fn create_top_level(&mut self, case_name: &str, now: &str) -> ArcStr {
        if is_role_root(case_name) {
            return self.ensure_role_root(now).case_name;
        }
        match case_name.strip_prefix(EVENT_ROOT_PREFIX) {
            Some(event_id) => self.create_event_root(event_id, now),
            None => self.create_role_case(None, case_name, now),
        }
    }

    /// 新建角色 case（`parent` 为 None 表示角色顶层）；同父同名直接复用已有节点。
    pub fn create_role_case(&mut self, parent: Option<&str>, name: &str, now: &str) -> ArcStr {
        self.ensure_role_root(now);
        let parent_id = match parent {
            Some(parent) if parent != ROLE_ROOT_CASE_NAME_STR && self.nodes.contains_key(parent) => {
                ArcStr::from(parent)
            }
            _ => ROLE_ROOT_CASE_NAME.clone(),
        };
        let path = if parent_id == ROLE_ROOT_CASE_NAME {
            ArcStr::from(name)
        } else {
            ArcStr::from(format!("{}/{}", parent_id, name))
        };
        if self.nodes.contains_key(&path) {
            return path;
        }
        self.insert_node(path.clone(), Some(parent_id), now);
        path
    }

    /// 新建事件根 `[event]{event_id}`（顶层）；已存在时直接复用。
    pub fn create_event_root(&mut self, event_id: &str, now: &str) -> ArcStr {
        let path = event_case_name(event_id);
        if self.nodes.contains_key(&path) {
            return path;
        }
        self.insert_node(path.clone(), None, now);
        path
    }

    /// 在已有父 case 下新建子 case；同父同名直接复用。
    pub fn create_child(&mut self, parent: &str, name: &str, now: &str) -> ArcStr {
        let path = ArcStr::from(format!("{}/{}", parent, name));
        if self.nodes.contains_key(&path) {
            return path;
        }
        debug_assert!(self.nodes.contains_key(parent), "create_child 的父 case 不存在: {}", parent);
        self.insert_node(path.clone(), Some(ArcStr::from(parent)), now);
        path
    }

    /// 确保一条完整路径存在（缺的祖先建成复合 case，最后一段建成叶子）。
    /// 调用方负责校验路径；已存在的段直接复用。
    pub fn ensure_path(&mut self, case_name: &str, now: &str) -> ArcStr {
        let mut segments = case_name.split('/');
        let first = segments.next().unwrap_or(case_name);
        let mut current = self.create_top_level(first, now);
        for segment in segments {
            current = self.create_child(&current, segment, now);
        }
        current
    }

    /// 测试构造用：按完整路径直接插入节点（父节点缺失时按路径推导）。
    #[cfg(test)]
    pub fn create_node(&mut self, case_name: &str, parent_id: Option<&str>, now: &str) {
        self.ensure_role_root(now);
        if self.nodes.contains_key(case_name) {
            return;
        }
        let derived = case_name.rsplit_once('/').map(|(parent, _)| ArcStr::from(parent));
        let parent = parent_id
            .map(ArcStr::from)
            .filter(|parent| self.nodes.contains_key(parent))
            .or_else(|| derived.filter(|parent| self.nodes.contains_key(parent)))
            .or_else(|| is_role_case(case_name).then(|| ROLE_ROOT_CASE_NAME.clone()));
        self.insert_node(ArcStr::from(case_name), parent, now);
    }

    fn insert_node(&mut self, case_name: ArcStr, parent_id: Option<ArcStr>, now: &str) {
        let node = CaseNode {
            case_name: case_name.clone(),
            parent_id: parent_id.clone(),
            children: Vec::new(),
            summary: ArcStr::new(),
            record_count: 0,
            start_time: ArcStr::from(now),
            end_time: ArcStr::from(now),
            created_at: ArcStr::from(now),
            updated_at: ArcStr::from(now),
        };
        if let Some(parent_node) = parent_id.and_then(|parent| self.nodes.get_mut(&parent)) {
            parent_node.children.push(case_name.clone());
        }
        self.nodes.insert(case_name, node);
    }

    /// 删除若干 case 及其整棵子树（并从父节点的 children 摘除）；隐藏的角色根永远不会被删除。
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

    /// 某 case（含自身）的整棵子树 id
    pub fn subtree_ids(&self, case_name: &str) -> BTreeSet<ArcStr> {
        let mut ids = BTreeSet::new();
        self.collect_subtree(case_name, &mut ids);
        ids
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
    fn create_role_case_links_parent_and_lists_levels() {
        let mut tree = CaseTree::new();
        let a = tree.create_role_case(None, "weather_chat", NOW.as_str());
        let b = tree.create_role_case(Some(&a), "rain", NOW.as_str());
        let c = tree.create_role_case(Some(&a), "snow", NOW.as_str());

        assert_eq!(a, "weather_chat");
        assert_eq!(b, "weather_chat/rain");
        assert_eq!(c, "weather_chat/snow");
        assert_eq!(tree.top_level_ids(), vec![ArcStr::from("weather_chat")]);
        assert_eq!(
            tree.children_ids(&a),
            vec![ArcStr::from("weather_chat/rain"), ArcStr::from("weather_chat/snow")]
        );
        assert!(tree.is_leaf(&b));
        assert!(!tree.is_leaf(&a));
        assert_eq!(tree.ancestors(&c), vec![ArcStr::from("weather_chat")]);
        assert!(tree.ancestors(&a).is_empty());
        assert_eq!(tree.get(&b).unwrap().parent_id.as_deref(), Some("weather_chat"));
    }

    #[test]
    fn same_name_under_different_parents_are_distinct() {
        let mut tree = CaseTree::new();
        let a = tree.create_role_case(None, "a", NOW.as_str());
        let b = tree.create_role_case(None, "b", NOW.as_str());
        let ax = tree.create_role_case(Some(&a), "x", NOW.as_str());
        let bx = tree.create_role_case(Some(&b), "x", NOW.as_str());

        assert_eq!(ax, "a/x");
        assert_eq!(bx, "b/x");
        assert_ne!(ax, bx);
        assert_eq!(tree.len(), 4);
        assert_eq!(tree.children_ids(&a), vec![ArcStr::from("a/x")]);
        assert_eq!(tree.children_ids(&b), vec![ArcStr::from("b/x")]);
    }

    #[test]
    fn same_name_under_same_parent_reuses_node() {
        let mut tree = CaseTree::new();
        let a = tree.create_role_case(None, "a", NOW.as_str());
        let first = tree.create_role_case(Some(&a), "x", NOW.as_str());
        let second = tree.create_role_case(Some(&a), "x", NOW.as_str());

        assert_eq!(first, second);
        assert_eq!(tree.len(), 2, "同名兄弟只有一个节点");
        assert_eq!(tree.children_ids(&a), vec![ArcStr::from("a/x")]);
    }

    #[test]
    fn event_root_and_child_paths() {
        let mut tree = CaseTree::new();
        let root = tree.create_event_root("e1", NOW.as_str());
        let child = tree.create_child(&root, "topic", NOW.as_str());

        assert_eq!(root, "[event]e1");
        assert_eq!(child, "[event]e1/topic");
        assert!(!tree.is_leaf(&root), "创建 child 后 root 不再是叶子");
        assert!(tree.is_leaf(&child));
        assert_eq!(tree.event_case("e1").map(|n| n.case_name.as_str()), Some("[event]e1"));
        assert!(tree.event_case("e2").is_none());
        assert_eq!(tree.children_ids(&root), vec![ArcStr::from("[event]e1/topic")]);
        assert_eq!(tree.get(&child).unwrap().parent_id.as_deref(), Some("[event]e1"));
    }

    #[test]
    fn role_root_is_hidden_and_role_cases_are_its_children() {
        let mut tree = CaseTree::new();
        tree.create_role_case(None, "a", NOW.as_str());
        tree.create_event_root("e1", NOW.as_str());

        assert_eq!(tree.len(), 2, "业务 case 数不含角色根");
        assert_eq!(tree.nodes().len(), 2, "对外节点不含角色根");
        assert_eq!(tree.all_nodes().len(), 3, "持久化包含角色根");
        assert!(tree.get(ROLE_ROOT_CASE_NAME_STR).is_some());
        assert_eq!(tree.get("a").unwrap().parent_id.as_deref(), Some(ROLE_ROOT_CASE_NAME_STR));
        assert_eq!(tree.get("[event]e1").unwrap().parent_id, None, "事件根是森林顶层");
        assert_eq!(
            tree.forest_top_level_ids(),
            vec![ArcStr::from("[event]e1"), ArcStr::from("a")],
            "森林顶层 = 事件根 + 角色根下的角色 case"
        );
        assert_eq!(tree.role_top_level_ids(), vec![ArcStr::from("a")]);
        assert!(tree.is_leaf("a"));
        assert!(!tree.is_leaf(ROLE_ROOT_CASE_NAME_STR), "角色根固定是复合 case");
    }

    #[test]
    fn from_nodes_restores_structure_from_paths() {
        let restored = CaseTree::from_nodes(vec![
            plain_node("a", None),
            plain_node("a/b", None),
            plain_node("[event]e1", None),
            plain_node("[event]e1/topic", None),
        ]);

        assert_eq!(restored.len(), 4);
        assert_eq!(restored.get("a").unwrap().parent_id.as_deref(), Some(ROLE_ROOT_CASE_NAME_STR));
        assert_eq!(restored.get("a/b").unwrap().parent_id.as_deref(), Some("a"));
        assert_eq!(restored.get("[event]e1").unwrap().parent_id, None);
        assert_eq!(restored.get("[event]e1/topic").unwrap().parent_id.as_deref(), Some("[event]e1"));
        assert_eq!(restored.children_ids("a"), vec![ArcStr::from("a/b")]);
        assert_eq!(
            restored.forest_top_level_ids(),
            vec![ArcStr::from("[event]e1"), ArcStr::from("a")]
        );
    }

    #[test]
    fn remove_nodes_removes_subtree_and_keeps_role_root() {
        let mut tree = CaseTree::new();
        tree.create_role_case(None, "a", NOW.as_str());
        tree.create_role_case(Some("a"), "b", NOW.as_str());
        tree.create_event_root("e1", NOW.as_str());

        tree.remove_nodes(&tree.role_case_ids());
        assert!(tree.get(ROLE_ROOT_CASE_NAME_STR).is_some(), "角色根不删除");
        assert!(tree.children_ids(ROLE_ROOT_CASE_NAME_STR).is_empty(), "角色 subtree 整体删除");
        assert_eq!(tree.len(), 1, "事件根保留");

        tree.remove_nodes(&tree.subtree_ids("[event]e1"));
        assert_eq!(tree.len(), 0);
        assert!(tree.get(ROLE_ROOT_CASE_NAME_STR).is_some(), "删除事件根也不影响角色根");
    }

    #[test]
    fn apply_records_tracks_count_and_time_bounds() {
        let mut tree = CaseTree::new();
        tree.create_role_case(None, "a", NOW.as_str());
        tree.apply_records("a", &[entry("2026-09-22 10:00:00", 1), entry("2026-09-22 10:05:00", 2)], NOW.as_str());
        let node = tree.get("a").unwrap();
        assert_eq!(node.record_count, 2);
        assert_eq!(node.start_time.as_str(), "2026-09-22 10:00:00");
        assert_eq!(node.end_time.as_str(), "2026-09-22 10:05:00");

        tree.apply_records("a", &[entry("2026-09-21 09:00:00", 3)], NOW.as_str());
        tree.apply_records("a", &[entry("2026-09-23 09:00:00", 4)], NOW.as_str());
        let node = tree.get("a").unwrap();
        assert_eq!(node.record_count, 4);
        assert_eq!(node.start_time.as_str(), "2026-09-21 09:00:00");
        assert_eq!(node.end_time.as_str(), "2026-09-23 09:00:00");
    }

    #[test]
    fn apply_records_rejects_non_leaf() {
        let mut tree = CaseTree::new();
        let a = tree.create_role_case(None, "a", NOW.as_str());
        tree.create_role_case(Some(&a), "b", NOW.as_str());
        assert!(tree.apply_records(&a, &[entry("2026-09-22 10:00:00", 1)], NOW.as_str()).is_none(), "复合 case 不收记录");
        assert!(tree.apply_records(ROLE_ROOT_CASE_NAME_STR, &[entry("2026-09-22 10:00:00", 1)], NOW.as_str()).is_none(), "角色根不收记录");
        assert!(tree.apply_records("a/b", &[entry("2026-09-22 10:00:00", 1)], NOW.as_str()).is_some(), "叶子可以收记录");
    }

    #[test]
    fn ensure_path_creates_ancestors_and_reuses_existing() {
        let mut tree = CaseTree::new();
        assert_eq!(tree.ensure_path("a/b/c", NOW.as_str()), "a/b/c");
        assert!(!tree.is_leaf("a"), "缺的祖先建成复合 case");
        assert!(!tree.is_leaf("a/b"));
        assert!(tree.is_leaf("a/b/c"));
        assert_eq!(tree.ensure_path("a/b/c", NOW.as_str()), "a/b/c");
        assert_eq!(tree.len(), 3, "重复确保不新增节点");

        assert_eq!(tree.ensure_path("[event]e1/topic", NOW.as_str()), "[event]e1/topic");
        assert!(tree.get("[event]e1").is_some());
        assert!(tree.is_leaf("[event]e1/topic"));
    }

    #[test]
    fn briefs_skip_missing_and_keep_order() {
        let mut tree = CaseTree::new();
        let a = tree.create_role_case(None, "a", NOW.as_str());
        tree.set_summary(&a, "摘要一", NOW.as_str());
        let b = tree.create_role_case(None, "b", NOW.as_str());
        tree.set_summary(&b, "摘要二", NOW.as_str());
        let briefs = tree.briefs(&[ArcStr::from("b"), ArcStr::from("missing"), ArcStr::from("a")]);
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].case_name.as_str(), "b");
        assert_eq!(briefs[0].summary.as_str(), "摘要二");
        assert_eq!(briefs[1].case_name.as_str(), "a");
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
}
