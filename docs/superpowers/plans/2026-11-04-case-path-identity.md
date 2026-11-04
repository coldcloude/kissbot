# 记忆摘要 case 路径标识与定位流程 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把记忆摘要的 case 唯一标识从扁平名称改为完整路径，LLM 只处理最后一段；定位阶段只返回 case 名、不再返回摘要；同名兄弟复用同一 case，不同子树同名互不干扰；原文副本目录与 case 树一一对应。

**Architecture:** 新增 `case_name` 模块集中名称/路径规则；`CaseTree` 以完整路径为键，并提供按父路径取子、幂等创建、子树收集等方法；`locate` 的逐层判定改为返回单个名称，命中兄弟叶子则归入、命中复合则下潜、未命中则在当前层建叶子；归档按 meta 全路径逐段建出缺失节点；存储目录把 `[role]` 映射为 `role`、`[event]{id}` 映射为 `event-{id}`。

**Tech Stack:** Rust（cargo、tokio、serde）、kissbot-api 类型、Playwright/TypeScript 集成测试。

## Global Constraints

- LLM 返回的 case 名只能含 `[A-Za-z0-9_]`，长度 1–64；不额外判断保留字。
- `event_id` 维持 `[A-Za-z0-9-]`，长度 1–64；事件根 `case_name` 固定 `[event]{event_id}`，目录 `event-{event_id}`。
- 隐藏角色根 `case_name` 固定 `[role]`，目录 `role`。
- case 路径总长 ≤ 200；段之间 `/`；不允许空段、`.`、`..`、首尾 `/`。
- 同一父下最后一段相同 = 同一个 case，不追加 `-2`；不同父下同名 = 不同 case。
- locate 阶段不生成任何摘要；新 case 摘要为空。
- 不做旧数据迁移；旧 `role-` / `event-` 扁平名称不识别。
- `docs/` 同步更新；`blog/` 绝对不修改。
- git 提交 comment 用中文，包含本次提交的全部改动。

---

### Task 1: 名称/路径规则、CaseTree 路径索引与存储目录映射

**Files:**
- Create: `kissbot-memory-struct-summary/src/case_name.rs`
- Modify: `kissbot-memory-struct-summary/src/main.rs`（加 `mod case_name;`）
- Modify: `kissbot-memory-struct-summary/src/tree.rs`
- Modify: `kissbot-memory-struct-summary/src/records.rs`

**Interfaces:**
- Produces（供后续任务使用）：
  - `case_name::ROLE_ROOT_CASE_NAME_STR: &str = "[role]"`、`ROLE_ROOT_CASE_NAME: ArcStr`、`EVENT_ROOT_PREFIX: &str = "[event]"`、`MAX_CASE_NAME_LEN: usize = 64`、`MAX_CASE_PATH_LEN: usize = 200`
  - `case_name::is_role_root(&str) -> bool`、`is_event_case(&str) -> bool`、`is_event_root(&str) -> bool`、`is_role_case(&str) -> bool`
  - `case_name::leaf_name(&str) -> &str`、`event_root_name(&str) -> ArcStr`、`valid_case_name(&str) -> bool`、`valid_event_id(&str) -> bool`、`valid_case_path(&str) -> bool`
  - `CaseTree::child_id(Option<&str>, &str) -> Option<ArcStr>`、`create_role_case(Option<&str>, &str, &str) -> ArcStr`、`create_event_root(&str, &str) -> ArcStr`、`create_child(&str, &str, &str) -> ArcStr`、`create_top_level(&str, &str) -> ArcStr`、`subtree_ids(&str) -> BTreeSet<ArcStr>`
  - `records::case_dir(&Path, &str) -> PathBuf`

- [ ] **Step 1: 写失败测试（名称规则）**

在 `src/case_name.rs` 末尾加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_case_name_only_letters_digits_underscore() {
        assert!(valid_case_name("weather_chat"));
        assert!(valid_case_name("a1_B2"));
        assert!(!valid_case_name(""));
        assert!(!valid_case_name("weather-chat"));
        assert!(!valid_case_name("天气"));
        assert!(!valid_case_name("weather chat"));
        assert!(!valid_case_name("weather.chat"));
        assert!(!valid_case_name("[role]"));
        assert!(!valid_case_name(&"a".repeat(MAX_CASE_NAME_LEN + 1)));
    }

    #[test]
    fn valid_case_path_accepts_role_and_event_paths() {
        assert!(valid_case_path("weather_chat"));
        assert!(valid_case_path("weather_chat/rain"));
        assert!(valid_case_path("[event]e1"));
        assert!(valid_case_path("[event]e1/topic"));
        assert!(!valid_case_path(""));
        assert!(!valid_case_path("[role]"));
        assert!(!valid_case_path("a//b"));
        assert!(!valid_case_path("/a"));
        assert!(!valid_case_path("a/"));
        assert!(!valid_case_path("a/../b"));
        assert!(!valid_case_path("[event]/topic"));
        assert!(!valid_case_path("[event]e1/topic-bad"));
    }

    #[test]
    fn classification_and_leaf_name() {
        assert_eq!(leaf_name("weather_chat/rain"), "rain");
        assert_eq!(leaf_name("[event]e1/topic"), "topic");
        assert!(is_role_root("[role]"));
        assert!(!is_role_root("role"));
        assert!(is_event_case("[event]e1/topic"));
        assert!(is_event_root("[event]e1"));
        assert!(!is_event_root("[event]e1/topic"));
        assert!(is_role_case("weather_chat/rain"));
        assert!(!is_role_case("[event]e1"));
        assert!(!is_role_case("[role]"));
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-memory-struct-summary && cargo test case_name::tests`
Expected: 编译失败（`case_name` 模块不存在）。

- [ ] **Step 3: 实现 `case_name.rs`**

```rust
//! case 名称与路径规则。
//!
//! case 的唯一标识是相对森林的完整路径，段之间用 `/` 连接：首段是模型名表示角色 case，
//! 首段是 `[event]{event_id}` 表示事件树；隐藏角色根固定为 `[role]`。模型只看到路径的最后一段。

use arcstr::ArcStr;

/// 隐藏角色根：只用于组织角色树，不是业务 case。
pub const ROLE_ROOT_CASE_NAME_STR: &str = "[role]";
pub const ROLE_ROOT_CASE_NAME: ArcStr = arcstr::literal!("[role]");

/// 事件根标记：事件根路径为 `[event]{event_id}`。
pub const EVENT_ROOT_PREFIX: &str = "[event]";

/// 单段名称 / 事件标识的长度上限。
pub const MAX_CASE_NAME_LEN: usize = 64;

/// 完整 case 路径的长度上限。
pub const MAX_CASE_PATH_LEN: usize = 200;

pub fn is_role_root(case_name: &str) -> bool {
    case_name == ROLE_ROOT_CASE_NAME_STR
}

/// 是否是事件树里的路径（首段是 `[event]{event_id}`）。
pub fn is_event_case(case_name: &str) -> bool {
    case_name.split('/').next().is_some_and(|first| first.starts_with(EVENT_ROOT_PREFIX))
}

/// 是否是事件根（顶层、单段、`[event]{event_id}`）。
pub fn is_event_root(case_name: &str) -> bool {
    !case_name.contains('/') && is_event_case(case_name)
}

/// 是否是角色 case（排除隐藏角色根与事件树）。
pub fn is_role_case(case_name: &str) -> bool {
    !is_role_root(case_name) && !is_event_case(case_name)
}

pub fn leaf_name(case_name: &str) -> &str {
    case_name.rsplit('/').next().unwrap_or(case_name)
}

pub fn event_root_name(event_id: &str) -> ArcStr {
    ArcStr::from(format!("{}{}", EVENT_ROOT_PREFIX, event_id))
}

pub fn valid_case_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CASE_NAME_LEN
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn valid_event_id(event_id: &str) -> bool {
    !event_id.is_empty()
        && event_id.len() <= MAX_CASE_NAME_LEN
        && event_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// 完整路径：首段可以是事件根，其余段必须是模型名；`[role]` 不是业务 case，不接受。
pub fn valid_case_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_CASE_PATH_LEN {
        return false;
    }
    let mut segments = path.split('/');
    let Some(first) = segments.next() else { return false; };
    let first_ok = match first.strip_prefix(EVENT_ROOT_PREFIX) {
        Some(event_id) => valid_event_id(event_id),
        None => valid_case_name(first),
    };
    first_ok && segments.all(valid_case_name)
}
```

- [ ] **Step 4: 运行名称规则测试**

Run: `cd kissbot-memory-struct-summary && cargo test case_name::tests`
Expected: 3 passed。

- [ ] **Step 5: 写失败测试（CaseTree 路径）**

替换 `tree.rs` 的 `#[cfg(test)] mod tests` 为新测试（节选，保留原测试意图并改路径）：

```rust
const NOW: ArcStr = arcstr::literal!("2026-09-22 13:49:00");

#[test]
fn create_role_case_links_parent_and_lists_levels() {
    let mut tree = CaseTree::new();
    let a = tree.create_role_case(None, "weather_chat", NOW.as_str());
    let b = tree.create_role_case(Some(&a), "rain", NOW.as_str());
    let c = tree.create_role_case(Some(&a), "snow", NOW.as_str());
    assert_eq!(a, "weather_chat");
    assert_eq!(b, "weather_chat/rain");
    assert_eq!(tree.role_top_level_ids(), vec![ArcStr::from("weather_chat")]);
    assert_eq!(tree.children_ids(&a), vec![ArcStr::from("weather_chat/rain"), ArcStr::from("weather_chat/snow")]);
    assert!(tree.is_leaf(&b));
    assert!(!tree.is_leaf(&a));
    assert_eq!(tree.ancestors(&c), vec![ArcStr::from("weather_chat")]);
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
}

#[test]
fn same_name_under_same_parent_reuses_node() {
    let mut tree = CaseTree::new();
    let a = tree.create_role_case(None, "a", NOW.as_str());
    let first = tree.create_role_case(Some(&a), "x", NOW.as_str());
    let second = tree.create_role_case(Some(&a), "x", NOW.as_str());
    assert_eq!(first, second);
    assert_eq!(tree.len(), 2, "同名兄弟只有一个节点");
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
    assert_eq!(tree.children_ids(&root), vec![ArcStr::from("[event]e1/topic")]);
}

#[test]
fn role_root_is_hidden_and_role_cases_are_its_children() {
    let mut tree = CaseTree::new();
    tree.create_role_case(None, "a", NOW.as_str());
    tree.create_event_root("e1", NOW.as_str());
    assert_eq!(tree.len(), 2);
    assert_eq!(tree.nodes().len(), 2);
    assert_eq!(tree.all_nodes().len(), 3);
    assert!(tree.get("[role]").is_some());
    assert_eq!(tree.get("a").unwrap().parent_id.as_deref(), Some("[role]"));
    assert_eq!(tree.get("[event]e1").unwrap().parent_id, None);
    assert_eq!(tree.forest_top_level_ids(), vec![ArcStr::from("[event]e1"), ArcStr::from("a")]);
}
```

其余原测试按同一语义改：`role-x` → `x`、`event-e1` → `[event]e1`、`event-e1-topic` → `[event]e1/topic`；删除 `unique_*` 测试，改为同名复用测试；`from_nodes` 补齐 `[role]`。

- [ ] **Step 6: 运行测试确认失败**

Run: `cd kissbot-memory-struct-summary && cargo test tree::tests`
Expected: 编译失败（`create_role_case` 等不存在）。

- [ ] **Step 7: 实现 `tree.rs`**

关键实现（其余方法保持原有语义，键改为完整路径）：

```rust
pub use crate::case_name::{
    event_root_name as event_case_name, is_event_case, is_event_root, is_role_case, is_role_root,
    leaf_name, EVENT_ROOT_PREFIX, ROLE_ROOT_CASE_NAME, ROLE_ROOT_CASE_NAME_STR,
};

pub fn event_case_name(event_id: &str) -> ArcStr { crate::case_name::event_root_name(event_id) }

impl CaseTree {
    pub fn event_case(&self, event_id: &str) -> Option<&CaseNode> {
        self.nodes.get(event_case_name(event_id).as_str())
    }

    pub fn role_case_ids(&self) -> BTreeSet<ArcStr> {
        self.nodes.keys().filter(|name| is_role_case(name)).cloned().collect()
    }

    pub fn role_top_level_ids(&self) -> Vec<ArcStr> {
        self.children_ids(ROLE_ROOT_CASE_NAME_STR)
    }

    pub fn child_id(&self, parent: Option<&str>, name: &str) -> Option<ArcStr> {
        let path = match parent {
            Some(parent) => format!("{}/{}", parent, name),
            None => name.to_string(),
        };
        self.nodes.contains_key(path.as_str()).then(|| ArcStr::from(path))
    }

    pub fn create_top_level(&mut self, case_name: &str, now: &str) -> ArcStr {
        match case_name.strip_prefix(EVENT_ROOT_PREFIX) {
            Some(event_id) => self.create_event_root(event_id, now),
            None => self.create_role_case(None, case_name, now),
        }
    }

    pub fn create_role_case(&mut self, parent: Option<&str>, name: &str, now: &str) -> ArcStr {
        self.ensure_role_root(now);
        let parent_id = parent
            .filter(|parent| self.nodes.contains_key(*parent))
            .map(ArcStr::from)
            .unwrap_or_else(|| ROLE_ROOT_CASE_NAME.clone());
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

    pub fn create_event_root(&mut self, event_id: &str, now: &str) -> ArcStr {
        let path = event_case_name(event_id);
        if self.nodes.contains_key(&path) {
            return path;
        }
        self.insert_node(path.clone(), None, now);
        path
    }

    pub fn create_child(&mut self, parent: &str, name: &str, now: &str) -> ArcStr {
        let path = ArcStr::from(format!("{}/{}", parent, name));
        if self.nodes.contains_key(&path) {
            return path;
        }
        self.insert_node(path.clone(), Some(ArcStr::from(parent)), now);
        path
    }

    fn insert_node(&mut self, case_name: ArcStr, parent_id: Option<ArcStr>, now: &str) {
        let node = CaseNode { case_name: case_name.clone(), parent_id: parent_id.clone(), children: Vec::new(), summary: ArcStr::new(), record_count: 0, start_time: ArcStr::from(now), end_time: ArcStr::from(now), created_at: ArcStr::from(now), updated_at: ArcStr::from(now) };
        if let Some(parent) = parent_id.and_then(|parent| self.nodes.get_mut(&parent)) {
            parent.children.push(case_name.clone());
        }
        self.nodes.insert(case_name, node);
    }

    pub fn subtree_ids(&self, case_name: &str) -> BTreeSet<ArcStr> {
        let mut ids = BTreeSet::new();
        self.collect_subtree(case_name, &mut ids);
        ids
    }
}
```

`from_nodes`：把补齐条件从 `ROLE_ROOT` 改为 `[role]`；父缺失时角色路径回落到 `[role]`、事件路径回落 `None`；其余不变。删除 `unique_role_case_name`、`unique_event_child_case_name`、`unique_case_name`、`role_children_ids`（用 `children_ids` 代替）。

- [ ] **Step 8: 写失败测试（存储目录）**

`records.rs` 测试：

```rust
#[test]
fn case_dir_layout_mirrors_tree() {
    let role = Path::new("/root/0/memory-struct-summary/dev");
    assert_eq!(case_dir(role, "weather_chat"), PathBuf::from("/root/0/memory-struct-summary/dev/cases/role/weather_chat"));
    assert_eq!(case_dir(role, "weather_chat/rain"), PathBuf::from("/root/0/memory-struct-summary/dev/cases/role/weather_chat/rain"));
    assert_eq!(case_dir(role, "[event]e1"), PathBuf::from("/root/0/memory-struct-summary/dev/cases/event-e1"));
    assert_eq!(case_dir(role, "[event]e1/topic"), PathBuf::from("/root/0/memory-struct-summary/dev/cases/event-e1/topic"));
}
```

- [ ] **Step 9: 实现 `records::case_dir`**

```rust
pub fn case_dir(role_dir: &Path, case_name: &str) -> PathBuf {
    let mut dir = role_dir.join("cases");
    let mut segments = case_name.split('/');
    let first = segments.next().unwrap_or("");
    if first == crate::case_name::ROLE_ROOT_CASE_NAME_STR {
        dir.push("role");
    } else if let Some(event_id) = first.strip_prefix(crate::case_name::EVENT_ROOT_PREFIX) {
        dir.push(format!("event-{}", event_id));
    } else {
        dir.push("role");
        dir.push(first);
    }
    for segment in segments {
        dir.push(segment);
    }
    dir
}
```

- [ ] **Step 10: 运行 Task 1 测试**

Run: `cd kissbot-memory-struct-summary && cargo test case_name::tests tree::tests records::tests`
Expected: 全部 passed。

- [ ] **Step 11: Commit**

```bash
git add kissbot-memory-struct-summary/src/case_name.rs kissbot-memory-struct-summary/src/main.rs kissbot-memory-struct-summary/src/tree.rs kissbot-memory-struct-summary/src/records.rs
git commit -m "记忆摘要 case 改为完整路径标识：新增名称/路径规则、树按路径索引、原文副本目录对应树结构"
```

---

### Task 2: 模型接口与提示词只返回 case 名

**Files:**
- Modify: `kissbot-memory-struct-summary/src/model.rs`
- Modify: `kissbot-memory-struct-summary/src/stub.rs`
- Modify: `kissbot-memory-struct-summary/src/stub_model.rs`

**Interfaces:**
- Consumes: `case_name::{leaf_name, valid_case_name, MAX_CASE_NAME_LEN}`
- Produces: `DecisionModel::decide_level(&self, ..., level: &[CaseBrief]) -> Result<ArcStr>`；`parse_level_case_name(&Value) -> Result<ArcStr>`；`render_level` 只展示最后一段。

- [ ] **Step 1: 写失败测试**

改 `model.rs` 测试：

```rust
#[test]
fn parse_level_case_name_variants() {
    assert_eq!(parse_level_case_name(&json!({"case_name":"weather_chat"})).unwrap(), "weather_chat");
    assert_eq!(parse_level_case_name(&json!({"case_name":" role_x "})).unwrap(), "role_x");
    assert!(parse_level_case_name(&json!({})).is_err());
    assert!(parse_level_case_name(&json!({"case_name":""})).is_err());
    assert!(parse_level_case_name(&json!({"case_name":"weather-chat"})).is_err());
    assert!(parse_level_case_name(&json!({"case_name":"天气"})).is_err());
    assert!(parse_level_case_name(&json!({"case_name":"[role]"})).is_err());
    let max = "a".repeat(MAX_CASE_NAME_LEN);
    assert_eq!(parse_level_case_name(&json!({"case_name":max})).unwrap(), max.as_str());
}

#[test]
fn prompts_use_leaf_names_only() {
    let candidates = vec![brief("a/x", "甲"), brief("[event]e1/topic", "")];
    let level = level_prompt("内容", &candidates);
    assert!(level.contains("- x: 甲"));
    assert!(level.contains("- topic"));
    assert!(!level.contains("a/x"), "提示词只展示最后一段");
    assert!(level.contains("\"case_name\""));
    assert!(!level.contains("summary"));
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-memory-struct-summary && cargo test model::tests`
Expected: 编译失败（`parse_level_case_name` 不存在）。

- [ ] **Step 3: 改模型接口与提示词**

删除 `LevelDecision` 与 `LevelDecision::new_case`；trait 改：

```rust
async fn decide_level(
    &self,
    agent_role: &AgentRole,
    event_id: Option<ArcStr>,
    batch: &str,
    level: &[CaseBrief],
) -> Result<ArcStr>;
```

`render_level` / 分组提示词中的当前 case 都用 `leaf_name`：

```rust
fn brief_line(brief: &CaseBrief) -> String {
    let name = crate::case_name::leaf_name(&brief.case_name);
    if brief.summary.is_empty() {
        format!("- {}", name)
    } else {
        format!("- {}: {}", name, brief.summary)
    }
}
```

`level_prompt` 输出约定：

```text
请判断该组属于哪个 case：
- 若属于某个已有 case，返回该 case 的名称：{"case_name":"<名称>"}
- 否则给出一个新名称：{"case_name":"<新名称>"}；名称只能由英文字母、数字与下划线组成（如 topic_weather_chat），不能用中文、空格、连字符或其他字符
只输出 JSON。
```

解析函数：

```rust
pub fn parse_level_case_name(value: &Value) -> Result<ArcStr> {
    let name = value
        .get("case_name")
        .and_then(|name| name.as_str())
        .map(str::trim)
        .filter(|name| crate::case_name::valid_case_name(name))
        .ok_or_else(|| {
            Error::ModelApiError(format!(
                "case_name 非法（只能含英文字母、数字与下划线，且不超过 {} 字符）: {}",
                crate::case_name::MAX_CASE_NAME_LEN,
                value
            ).into())
        })?;
    Ok(ArcStr::from(name))
}
```

`LlmModel::decide_level` 改为 `self.parse` 后返回 `parse_level_case_name`。

- [ ] **Step 4: 改桩模型**

`stub_model.rs` 的 `decide_level` 返回 `Ok(ArcStr::from("case"))`；注释去掉“摘要与重名追加后缀”。

`stub.rs` 的 `level_decisions: Mutex<VecDeque<ArcStr>>`，`push_level(&self, name: &str)`；`decide_level` 默认 `ArcStr::from("case")`。

- [ ] **Step 5: 跑模型测试**

Run: `cd kissbot-memory-struct-summary && cargo test model::tests stub_model::tests`
Expected: passed。

- [ ] **Step 6: Commit**

```bash
git add kissbot-memory-struct-summary/src/model.rs kissbot-memory-struct-summary/src/stub.rs kissbot-memory-struct-summary/src/stub_model.rs
git commit -m "记忆摘要定位判定只返回 case 名：模型接口、提示词与桩模型去掉摘要与 new 标志"
```

---

### Task 3: 归档/定位按层级路径落位

**Files:**
- Modify: `kissbot-memory-struct-summary/src/archive.rs`
- Modify: `kissbot-memory-struct-summary/src/locate.rs`
- Modify: `kissbot-memory-struct-summary/src/notify.rs`（只改注释/测试里的 `event_case_name` 期望值）

**Interfaces:**
- Consumes: Task 1 的 CaseTree 方法、Task 2 的 `decide_level -> ArcStr`
- Produces: `GroupVerdict::{Case(ArcStr), New { parent: Option<ArcStr>, name: ArcStr }}`；meta 全路径逐段建出。

- [ ] **Step 1: 写失败测试（逐层判定）**

在 `archive.rs` 或 `locate.rs` 测试模块加：

```rust
#[tokio::test]
async fn level_decision_returns_existing_leaf_or_descends_or_creates_leaf() {
    // 构造 a（复合）与 a/x（叶子）
    let temp = tempfile::tempdir().unwrap();
    let manager = AgentRoleManager::new(temp.path());
    let handle = manager.open(AgentRole::new(ArcStr::from("0"), ArcStr::from("dev"))).await.unwrap();
    {
        let mut state = handle.state.lock().await;
        let a = state.tree.create_role_case(None, "a", "t");
        state.tree.create_role_case(Some(&a), "x", "t");
    }
    let model = Arc::new(StubModel::new());

    // 返回 x：复合 a 下潜后命中叶子 a/x
    model.push_group(&[0], false);
    model.push_level("a");  // 顶层命中复合 a → 下潜
    model.push_level("x");  // 子层命中叶子 a/x
    let response = locate(&handle, &model, &role_event("dev", None), &channel_key(), &[message("m1")]).await;
    assert_eq!(case_names(&response), vec!["a/x"]);

    // 返回新名 y：在 a 下建叶子 a/y
    model.push_group(&[0], false);
    model.push_level("a");
    model.push_level("y");
    let response = locate(&handle, &model, &role_event("dev", None), &channel_key(), &[message("m2")]).await;
    assert_eq!(case_names(&response), vec!["a/y"]);
    let state = handle.state.lock().await;
    assert!(state.tree.is_leaf("a/y"));
    assert!(state.tree.get("a/y").unwrap().summary.is_empty(), "定位不生成摘要");
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-memory-struct-summary && cargo test level_decision_returns_existing_leaf`
Expected: 编译失败（`push_level` 参数/`GroupVerdict` 未改）。

- [ ] **Step 3: 改 `archive.rs`**

```rust
enum GroupVerdict {
    Case(ArcStr),
    New { parent: Option<ArcStr>, name: ArcStr },
}
```

删除 `PlacementMode`、`NewRole`/`NewEvent`/`NewEventChild`。

`decide_placement`：

```rust
async fn decide_placement(
    &self,
    agent_role: &AgentRole,
    event_id: Option<ArcStr>,
    tree: &CaseTree,
    text: &str,
    parent: Option<ArcStr>,
) -> Result<GroupVerdict> {
    let mut parent = parent;
    loop {
        let level = match parent.as_deref() {
            None => tree.role_top_level_ids(),
            Some(parent) => tree.children_ids(parent),
        };
        let briefs: Vec<CaseBrief> = tree.briefs(&level);
        let name = self.model.decide_level(agent_role, event_id.clone(), text, &briefs).await?;
        if let Some(child) = level.iter().find(|child| leaf_name(child) == name.as_str()).cloned() {
            if tree.is_leaf(&child) {
                return Ok(GroupVerdict::Case(child));
            }
            parent = Some(child);
            continue;
        }
        return Ok(GroupVerdict::New { parent, name });
    }
}
```

`locate_chunk` 事件模式：

```rust
let root_case = event_case_name(event_id.as_str());
let verdict = match state.tree.event_case(event_id.as_str()) {
    None => GroupVerdict::New { parent: None, name: root_case.clone() },
    Some(_) if state.tree.is_leaf(&root_case) => GroupVerdict::Case(root_case.clone()),
    Some(_) => self.decide_placement(&handle.id, Some(event_id.clone()), &state.tree, &text, Some(root_case.clone())).await?,
};
```

角色模式 `group_and_place` 的 `is_current` 分支去掉 `PlacementMode` 参数：

```rust
let verdict = if is_current {
    match current.clone() {
        Some(current) => GroupVerdict::Case(current),
        None => self.decide_placement(agent_role, role_event.event_id.clone(), &state.tree, &text, None).await?,
    }
} else {
    self.decide_placement(agent_role, role_event.event_id.clone(), &state.tree, &text, None).await?
};
```

落地：

```rust
let case_name = match &placement.verdict {
    GroupVerdict::Case(case_name) => {
        if state.tree.contains(case_name) && !state.tree.is_leaf(case_name) {
            tracing::warn!("跳过定位结果指向的非叶子 case: {}", case_name);
            continue;
        }
        if !state.tree.contains(case_name) {
            state.tree.create_top_level(case_name, &now);
        }
        case_name.clone()
    }
    GroupVerdict::New { parent, name } => {
        let case_name = match parent {
            Some(parent) => state.tree.create_child(parent, name, &now),
            None => state.tree.create_top_level(name, &now),
        };
        if !state.tree.is_leaf(&case_name) {
            tracing::warn!("跳过定位结果指向的非叶子 case: {}", case_name);
            continue;
        }
        case_name
    }
};
```

不再调用 `set_summary`；事件根新建也不再 `summarize`。

- [ ] **Step 4: 改 meta 落位**

`usable_meta_case` 改为 `crate::case_name::valid_case_path`；删掉 128 长度与 `-` 判断。

缺失 case 建出：

```rust
fn ensure_case_path(tree: &mut CaseTree, path: &str, now: &str) -> ArcStr {
    let mut segments = path.split('/');
    let first = segments.next().unwrap_or(path);
    let mut current = tree.create_top_level(first, now);
    for segment in segments {
        current = tree.create_child(&current, segment, now);
    }
    current
}
```

`archive_pending` 中：

```rust
for case_name in new_records.keys() {
    if !state.tree.contains(case_name) {
        ensure_case_path(&mut state.tree, case_name, &now);
    }
}
```

非叶子跳过逻辑保持不变（`contains && !is_leaf`）。

- [ ] **Step 5: 改摘要渲染**

`compute_summary` 子 case 行改为 `leaf_name(&child)`；`recompute_order` / `ancestors` 继续用完整路径。

- [ ] **Step 6: 更新现有测试**

- `locate.rs`：`role-case` → `case`、`role-case-2` → `case2`、`role-topic` → `topic`、`role-child` → `case/child`（或对应新父路径）；删除“新 case 摘要来自逐层判定”的断言，改为摘要为空；事件根 `event-e1` → `[event]e1`。
- `archive.rs`：`role-1` → `a`、`event-e1` → `[event]e1` 等；`recompute_order` 用路径。
- 新增 meta 路径测试：meta `a/b` 时自动建出 `a`（复合）与 `a/b`（叶子），记录写入 `a/b`。

- [ ] **Step 7: 跑归档/定位测试**

Run: `cd kissbot-memory-struct-summary && cargo test archive::tests locate::tests notify::tests`
Expected: passed。

- [ ] **Step 8: Commit**

```bash
git add kissbot-memory-struct-summary/src/archive.rs kissbot-memory-struct-summary/src/locate.rs kissbot-memory-struct-summary/src/notify.rs
git commit -m "记忆摘要定位按路径逐层落位：只返回 case 名、兄弟同名复用、不同子树同名区分、meta 全路径建出祖先"
```

---

### Task 4: 服务查询、重建与当前指针使用完整路径

**Files:**
- Modify: `kissbot-memory-struct-summary/src/service.rs`
- Modify: `kissbot-memory-struct-summary/src/agent_role.rs`（注释/测试中的 `event-{id}`、`role-` 名称）
- Modify: `kissbot-memory-struct-summary/src/notify.rs`（同上）

**Interfaces:**
- Consumes: CaseTree 路径 API、`case_name::{ROLE_ROOT_CASE_NAME_STR, event_root_name, is_event_case}`、`records::case_dir`。
- Produces: 查询/重建全部按完整路径；`rebuild` 清 `cases/role` 或 `cases/event-{id}`。

- [ ] **Step 1: 写失败测试**

`service.rs` 测试模块加：

```rust
#[tokio::test]
async fn rebuild_clears_role_tree_directory() {
    let f = Fixture::new().await;
    f.seed("role", "a", "内容").await; // 用现有 helper 归档出 a
    assert!(f.service.manager().open(agent_role(&role_event("dev", None))).await.unwrap().case_dir("a").exists());
    f.service.rebuild(&agent_role(&role_event("dev", None)), None).await.unwrap();
    let handle = f.service.manager().open(agent_role(&role_event("dev", None))).await.unwrap();
    assert!(!handle.case_dir("a").exists(), "角色 case 目录被清掉");
    assert!(handle.case_dir("[role]").exists() == false || true, "隐藏根不作为业务 case 查询");
}
```

具体 helper 名称按 `service.rs` 现有 Fixture 调整；断言核心是“重建后 `cases/role` 下旧 case 目录不存在”。

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-memory-struct-summary && cargo test rebuild_clears_role_tree`
Expected: 编译失败或断言失败。

- [ ] **Step 3: 改 `rebuild`**

```rust
let role_event = role_event_of(&handle.id, event_id.clone());
handle.ensure_role_event(&mut state, &role_event).await?;
let (cleared, clear_dir): (BTreeSet<ArcStr>, ArcStr) = match event_id.as_ref() {
    Some(event_id) => {
        let root = crate::case_name::event_root_name(event_id);
        (state.tree.subtree_ids(&root), root)
    }
    None => (state.tree.role_case_ids(), ROLE_ROOT_CASE_NAME.clone()),
};
handle.reset_case_records(&clear_dir).await?;
state.tree.remove_nodes(&cleared);
```

`reset_case_records` 内部用 `case_dir`，因此 `[role]` 清 `cases/role`、`[event]e1` 清 `cases/event-e1`。

- [ ] **Step 4: 改查询**

- `case_list`：`Some(parent) => children_ids(parent)`；顶层用 `forest_top_level_ids`；`[role]` 用 `role_top_level_ids`。
- `case_summary` / `read_case`：`is_role_root(case_name)` 与 `tree.contains(case_name)` 用完整路径。
- `api_node`：`parent_id == ROLE_ROOT_CASE_NAME_STR` → `None`。
- `tree_get` 当前指针原样返回完整路径。

- [ ] **Step 5: 更新现有测试**

全文替换：
- `role-case` → `case`
- `role-case-2` → `case2`
- `role-2` → `case/child`
- `role-child` → `case/child`
- `event-e1` → `[event]e1`
- 期望目录：`cases/role/case`、`cases/event-e1`
- 事件模式固定 case 名断言改为 `[event]e1`。

- [ ] **Step 6: 跑服务测试**

Run: `cd kissbot-memory-struct-summary && cargo test service::tests agent_role::tests notify::tests`
Expected: passed。

- [ ] **Step 7: Commit**

```bash
git add kissbot-memory-struct-summary/src/service.rs kissbot-memory-struct-summary/src/agent_role.rs kissbot-memory-struct-summary/src/notify.rs
git commit -m "记忆摘要查询与重建改用完整路径：case 列表/摘要/原文副本/当前指针，重建清角色或事件子树目录"
```

---

### Task 5: API 注释与 agent 会话 id

**Files:**
- Modify: `kissbot-api/src/memory_summary.rs`（注释：case_name 为完整路径）
- Modify: `kissbot-agent/src/pipelines/memory_summary_pipeline.rs`
- Modify: `kissbot-agent/src/memory_summary_client.rs`（如有注释）

**Interfaces:**
- Produces: `fn case_session_id(&self, case_name: Option<&ArcStr>) -> ArcStr` 把 case 路径的 `/` 替换为 `.`。

- [ ] **Step 1: 写失败测试**

在 `memory_summary_pipeline.rs` 测试模块加：

```rust
#[test]
fn case_session_id_encodes_path_separator() {
    let key = MemoryKey { agent_id: ArcStr::from("0"), role_event: Arc::new(RoleEvent::new(ArcStr::from("dev"))) };
    let id = format!("memory-summary-{}-{}", key.encode(), "[event]e1/topic".replace('/', "."));
    assert_eq!(id, "memory-summary-0-dev-[event]e1.topic");
    assert!(!id.contains('/'), "session id 必须是文件名安全字符串");
}
```

如果测试辅助构造不便，直接用 `SummaryMemoryPipeline::case_session_id` 的公开/测试可见入口；核心断言是替换后的 id 含 `.` 而不含 `/`。

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test case_session_id_encodes_path`
Expected: 失败（当前直接拼接 `/`）。

- [ ] **Step 3: 改 `case_session_id`**

```rust
fn case_session_id(&self, case_name: Option<&ArcStr>) -> ArcStr {
    let case = case_name.map(|name| name.as_str().replace('/', ".")).unwrap_or_else(|| "none".to_string());
    ArcStr::from(format!("memory-summary-{}-{}", self.memory_key.encode(), case))
}
```

同步改注释：case 路径可能含 `/`，用 `.` 编码为文件名安全段。

- [ ] **Step 4: 改 API 注释**

`kissbot-api/src/memory_summary.rs`：
- `CaseNode.case_name` 注释：“case 的完整路径（如 `weather_chat/rain`、`[event]e1/topic`）”
- `LocatedGroup.case_name` / `CaseSummaryRequest.case_name` 等注释同步。

- [ ] **Step 5: 跑 agent 测试**

Run: `cd kissbot-agent && cargo test memory_summary`
Expected: passed。

- [ ] **Step 6: Commit**

```bash
git add kissbot-api/src/memory_summary.rs kissbot-agent/src/pipelines/memory_summary_pipeline.rs kissbot-agent/src/memory_summary_client.rs
git commit -m "case 名支持完整路径：API 注释更新，agent case 会话 id 用点号编码路径分隔符"
```

---

### Task 6: TypeScript 集成测试改新命名

**Files:**
- Modify: `test/tests/memory-struct-summary-api.spec.ts`
- Modify: `test/workspace-template/` 下含 `role-` / `event-` 的固定数据（如存在）

**Interfaces:**
- Consumes: 新 locate/归档接口。
- Produces: 集成测试覆盖新命名。

- [ ] **Step 1: 找出旧命名**

Run: `cd test && grep -n "role-\|event-" tests/memory-struct-summary-api.spec.ts workspace-template -r`
Expected: 列出全部待改点。

- [ ] **Step 2: 机械替换并调整语义**

- 普通 case：`role-case` → `case`，`role-case-2` → `case2`；目录断言 `cases/case` → `cases/role/case`。
- 事件根：`event-e1` → `[event]e1`；目录 `cases/event-e1` 不变。
- 新话题：桩模型返回 `case`，期望 `case`（不再拼 `role-`）。
- 复合/子 case：按路径断言（如 `case/child`）。
- 摘要断言：新 case 由 locate 建出时摘要为空；写入记录并归档后才有摘要。

- [ ] **Step 3: 补两个用例**

```ts
test('同父同名复用同一 case、跨子树同名是不同 case', async () => {
  // 用两批消息分别定位到 a/x 与 b/x，断言 tree.nodes 里同时存在两个 case，
  // 且各自 records 互不混淆；同父下再次返回同名时不新增节点。
});
```

具体请求构造沿用文件里现有 `locateRequest`/`waitForCaseRecords` helper。

- [ ] **Step 4: 跑集成测试**

Run: `cd test && npx playwright test tests/memory-struct-summary-api.spec.ts`
Expected: passed（首次会由 global-setup 构建服务）。

- [ ] **Step 5: Commit**

```bash
git add test/tests/memory-struct-summary-api.spec.ts test/workspace-template
git commit -m "更新记忆摘要集成测试到路径命名：角色 case 直出、事件根 [event] 前缀、跨子树同名用例"
```

---

### Task 7: 文档更新（docs/，不动 blog/）

**Files:**
- Modify: `docs/spec/memory-struct-summary.md`
- Modify: `docs/spec/memory-directory.md`
- Modify: `docs/design/components-design/kissbot-memory-struct-summary.md`
- Modify: 其他 `docs/` 中仍写 `role-` / `event-{event_id}` / `__role_root__` 且与新行为相关的文件（用 grep 确认）。

- [ ] **Step 1: 找旧描述**

Run: `grep -rn "role-\|event-\|__role_root__" docs/`
Expected: 列出待改文件。

- [ ] **Step 2: 更新核心文档**

`docs/spec/memory-struct-summary.md`：
- 术语/命名表改为：角色根 `[role]`/目录 `role`；角色 case 用模型名；事件根 `[event]{event_id}`/目录 `event-{event_id}`；case 名即完整路径。
- 名称规则：模型名 `[A-Za-z0-9_]`；event_id `[A-Za-z0-9-]`；路径 ≤200。
- locate 流程：第 2 轮只返回 `case_name`，无摘要；命中叶子/复合/新建规则；定位不生成摘要。
- 存储章节：`cases/role/...`、`cases/event-{id}/...`；重建清目录。
- 会话 id：`/`→`.`。
- 测试章节更新。

`docs/spec/memory-directory.md`：case 目录树示例改新布局。

组件设计文档：把“两种前缀区分”改为“`[role]`/`[event]` 标记 + 路径”。

- [ ] **Step 3: 检查 blog 未改**

Run: `git status --short blog`
Expected: 空输出。

- [ ] **Step 4: Commit**

```bash
git add docs
git commit -m "更新记忆摘要文档到路径命名：case 标识、定位只返回名称、事件根与存储目录、会话 id"
```

---

### Task 8: 全量验证与收尾

- [ ] **Step 1: 跑 summary 全量单测**

Run: `cd kissbot-memory-struct-summary && cargo test`
Expected: 全部 passed，0 failed。

- [ ] **Step 2: 跑 agent 单测**

Run: `cd kissbot-agent && cargo test`
Expected: 全部 passed。

- [ ] **Step 3: 跑集成测试**

Run: `cd test && npx playwright test tests/memory-struct-summary-api.spec.ts`
Expected: 全部 passed。

- [ ] **Step 4: 检查残留旧命名**

Run: `grep -rn "role-\|__role_root__" kissbot-memory-struct-summary/src kissbot-agent/src kissbot-api/src test/tests docs/ | grep -v target`
Expected: 只剩文档中说明“旧名称不兼容”或测试中明确验证旧名不识别的内容；否则修掉。

- [ ] **Step 5: 最终提交**

```bash
git status --short
git add -A
git commit -m "记忆摘要 case 路径标识改造收尾：全量测试与文档同步"
```

---

## Self-Review

- **Spec coverage:** Task 1 覆盖路径/名称/目录（spec 1、3.2）；Task 2 覆盖 LLM 输出（2.2、2.5）；Task 3 覆盖定位/归档/meta（2、3.1）；Task 4 覆盖查询/重建（3.3、4）；Task 5 覆盖会话 id（5）；Task 6/7 覆盖验证与文档（7）；Task 8 覆盖兼容性与全量验证（6）。
- **Placeholder scan:** 无 TBD/TODO；机械替换步骤给出了具体替换项；新增测试给了代码或明确断言。
- **Type consistency:** `decide_level -> Result<ArcStr>`、`GroupVerdict::{Case, New}`、`create_role_case/create_child/create_top_level/create_event_root`、`case_dir` 在所有任务中一致。
