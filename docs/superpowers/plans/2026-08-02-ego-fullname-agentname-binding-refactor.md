# memory-ego full_name/代号/search + agent_name 绑定 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**前置：** 计划 1（消息模型 is_self/name + role 目录）已执行完成。

**Goal:** memory-ego 的 Role/RoleRelation 加 full_name（展示文本）；role_name/individual_name 收敛为代号（`^[A-Za-z0-9_]+$`）；search agent 侧 name_index 改 HashMap 全匹配、search_by_name 返回 Option；search role 侧 role_name_descr_index+RoleSearchMetadata 加 full_name；agent 绑定改用 agent_name（session_key 用 agent_name，memory-store/ego 用解析后的 agent_id，保留 agent_name="" -> agent_id="0"）。

**Architecture:** full_name 为可填展示文本，Role 加专用 update API、RoleRelation 经全量替换修改。代号在 memory-ego 写入入口校验、individual_name 不校验唯一性（HashMap 覆盖）。agent 侧 ChannelConfig/SessionKey 用 agent_name，coordinator 维护 agent_name->agent_id 缓存（/agent/search-name 全匹配解析，回退 "0"），memory-store/ego 用解析后的 agent_id（UUID），session_key_of 去 Option。

**Tech Stack:** Rust + cargo（kissbot-api / kissbot-memory-ego / kissbot-agent）；serde；tokio；dashmap；reqwest；regex。

## Global Constraints

- 所有文本文件 UTF-8、`\n` 换行；不删注释；读写用 Read/Write/Edit，禁 sed/python。
- Git commit 中文，含本次所有改动；TDD；每任务 `cargo test` 全过且 commit。
- 计划 1 已完成：IncomingMessage 无 is_self、含 name；ChannelRecord/ChannelRequest 含 name；memory-store role 目录为 `{year}-{role_name}`。

## File Structure

- `kissbot-api/src/ego.rs`：Role/RoleRelation 加 full_name；UpdateRoleFullNameRequest；SearchRequest 不变。
- `kissbot-memory-ego/src/error.rs`：新增 InvalidCode 错误。
- `kissbot-memory-ego/src/role_play.rs`：full_name 存储、update_role_full_name、代号校验。
- `kissbot-memory-ego/src/agent.rs`：代号校验。
- `kissbot-memory-ego/src/individual_recognition.rs`：代号校验。
- `kissbot-memory-ego/src/search.rs`：name_index HashMap、search_by_name Option、RoleSearchMetadata 加 full_name、force_sync 调整。
- `kissbot-memory-ego/src/api.rs`：/agent/search-name 返回 Option、新增 /role/update-full-name。
- `kissbot-agent/src/config_manager.rs`：ChannelConfig.agent_name、移除 init_agent_id/init_role/default_agent_id/default_role。
- `kissbot-agent/src/types.rs`：SessionKey.agent_name。
- `kissbot-agent/src/coordinator.rs`：RESERVED_*、resolve_agent_id、session_key_of 去 Option、load_ego_info /agent/get。
- `kissbot-agent/src/session_manager.rs`、`command_router.rs`、`main.rs`：agent_name 适配。
- 配置/模板：`config.json`、`script/config.json`、`test/workspace-template/config.json`、`script/template/nexus.json`、`test/workspace-template/agent-data/nexus.json`。
- 文档：`docs/spec/memory-ego.md`、`docs/spec/kissbot-agent-nexus.md` 等。

---

### Task 1: kissbot-api ego 结构加 full_name

**Files:**
- Modify: `kissbot-api/src/ego.rs`

**Interfaces:**
- Produces: Role/RoleRelation 新增 `full_name: Arc<String>`；新增 `UpdateRoleFullNameRequest`。

- [ ] **Step 1: 加字段并更新 serde 测试**

`kissbot-api/src/ego.rs`：
```rust
pub struct RoleRelation {
    pub relation: Arc<String>,
    pub full_name: Arc<String>,    // 新增
    pub description: Arc<String>,
}

pub struct Role {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub full_name: Arc<String>,    // 新增
    pub description: Arc<String>,
}
```
新增请求结构（紧挨 UpdateRoleDescriptionRequest 之后）：
```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateRoleFullNameRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub full_name: Arc<String>,
}
```
更新 ego.rs 内所有构造 Role/RoleRelation 的 serde 测试，加 `full_name: Arc::new("...".to_string())`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-api && cargo test`
Expected: 编译错误（kissbot-memory-ego 构造点缺 full_name）。

- [ ] **Step 3: 更新 kissbot-memory-ego 构造点（暂填空串）**

`kissbot-memory-ego/src/role_play.rs`：create_role / create_role_from / rename_role / update_role_description 内构造 Role 处加 `full_name`（create/create_from 填 `Arc::new(String::new())`；rename/update_description 保留旧值 `role.role.full_name.clone()`）；update_other_role_relation / replace_other_roles / 各 update_other_role_* 内构造 RoleRelation/OtherRole 处加 `full_name`（保留旧值 `other_role.role_relation.full_name.clone()` 或新 relation 的 full_name）。

`kissbot-memory-ego/src/agent.rs`：copy_agent 内 create_agent 调用不变（AgentMetadata 无 full_name）。

`kissbot-memory-ego/src/test_util.rs` 与各测试构造处补 full_name。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p kissbot-api -p kissbot-memory-ego`
Expected: PASS（full_name 为空串）。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(api,ego): Role/RoleRelation 增加 full_name 字段（构造点暂填空串）"
```

---

### Task 2: memory-ego full_name 存储 + 修改 API

**Files:**
- Modify: `kissbot-memory-ego/src/role_play.rs`、`kissbot-memory-ego/src/api.rs`

**Interfaces:**
- Produces: `update_role_full_name`（Role）；RoleRelation.full_name 经 `update_other_role_relation` 全量替换修改；路由 `/role/update-full-name`。

- [ ] **Step 1: 写失败测试 - update_role_full_name**

`kissbot-memory-ego/src/role_play.rs` 测试模块加测试：create_role 后 update_role_full_name(agent, role, "新展示名")，get_role 断言 role.full_name=="新展示名"、description 不变。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-memory-ego && cargo test update_role_full_name`
Expected: FAIL（方法未定义）。

- [ ] **Step 3: 实现 update_role_full_name**

`kissbot-memory-ego/src/role_play.rs` RolePlayManager 加（仿 update_role_description）：
```rust
pub async fn update_role_full_name(&self, agent_id: &str, role_name: &str, full_name: Arc<String>) -> Result<()> {
    self.write_role_play_ref(agent_id, role_name, |role_or_none| {
        match role_or_none {
            Some(role) => Ok(Arc::new(RolePlay {
                role: Arc::new(Role {
                    agent_id: role.role.agent_id.clone(),
                    role_name: role.role.role_name.clone(),
                    full_name,
                    description: role.role.description.clone(),
                }),
                other_roles: role.other_roles.clone(),
            })),
            None => Err(Error::AgentRoleNotFound(agent_id.to_string(), role_name.to_string())),
        }
    }).await?;
    SearchManager::get().await.mark_role_dirty(agent_id, role_name);
    Ok(())
}
```

确认 rename_role / update_role_description 构造 Role 时 full_name 保留旧值（Step 3 of Task 1 已处理；复查）。

`kissbot-memory-ego/src/api.rs` create_router 加路由 + handler：
```rust
.route("/role/update-full-name", put(update_role_full_name))
```
```rust
async fn update_role_full_name(Json(req): Json<ego::UpdateRoleFullNameRequest>) -> impl IntoResponse {
    let result = RolePlayManager::get().update_role_full_name(&req.agent_id, &req.role_name, req.full_name).await;
    match result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::success(()))),
        Err(Error::AgentRoleNotFound(_, _)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Role {} not found", req.role_name)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd kissbot-memory-ego && cargo test`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(ego): Role.full_name 存储 + update_role_full_name API（/role/update-full-name）"
```

---

### Task 3: memory-ego 代号校验

**Files:**
- Modify: `kissbot-memory-ego/src/error.rs`、`kissbot-memory-ego/src/agent.rs`、`kissbot-memory-ego/src/role_play.rs`、`kissbot-memory-ego/src/individual_recognition.rs`

**Interfaces:**
- Produces: `Error::InvalidCode`；代号 `^[A-Za-z0-9_]+$` 在写入入口强制（非空）；ChannelConfig.agent_name 不校验。

- [ ] **Step 1: 写失败测试 - 非法代号被拒**

`kissbot-memory-ego/src/agent.rs` 测试加：create_agent(individual_name="a b c", ...) 返回 Err(InvalidCode)；create_agent(individual_name="alice_01", ...) 成功。`role_play.rs` 测试加：create_role(role_name="a b", ...) 返回 Err；rename_individual 空串返回 Err。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-memory-ego && cargo test invalid_code`
Expected: FAIL（无校验）。

- [ ] **Step 3: 实现校验**

`kissbot-memory-ego/src/error.rs` 加：
```rust
#[error("Invalid code (only [A-Za-z0-9_] allowed, non-empty): {0}")]
InvalidCode(String),
```

加共享校验函数（`kissbot-memory-ego/src/lib.rs` 或新建 `code.rs`）：
```rust
use regex::Regex;
use std::sync::OnceLock;
use crate::error::{Error, Result};

static CODE_RE: OnceLock<Regex> = OnceLock::new();

pub fn validate_code(code: &str) -> Result<()> {
    let re = CODE_RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9_]+$").unwrap());
    if re.is_match(code) { Ok(()) } else { Err(Error::InvalidCode(code.to_string())) }
}
```
（Cargo.toml 加 `regex` 依赖若未引入。）

在 agent.rs `create_agent` / `update_agent_name` 起始调 `validate_code(&individual_name)?`。
在 role_play.rs `create_role` / `create_role_from`（new_name）/ `rename_role`（new_name）/ `replace_other_roles`（每个 insert entry.role_name）/ `update_other_role_individual_name`（new_individual_name）/ `replace_other_role_relations`（每个 insert entry.role_name）调 `validate_code(...)?`。
在 individual_recognition.rs `replace_individuals`（insert key）/ `rename_individual`（new_name）/ `replace_individual_identifiers` 等 individual_name 写入处调 `validate_code(...)?`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd kissbot-memory-ego && cargo test`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(ego): role_name/individual_name 代号校验（^[A-Za-z0-9_]+$，非空）"
```

---

### Task 4: memory-ego search agent 侧（name_index HashMap + search_by_name Option）

**Files:**
- Modify: `kissbot-memory-ego/src/search.rs`、`kissbot-memory-ego/src/api.rs`

**Interfaces:**
- Produces: `name_index: Arc<RwLock<HashMap<String, String>>>`（individual_name->agent_id 全匹配）；`search_by_name -> Option<String>`；force_sync_identity 用 SearchMetadata 旧 individual_name 同步 HashMap；/agent/search-name 返回 Option。

- [ ] **Step 1: 写失败测试 - search_by_name 全匹配返回 Option**

`kissbot-memory-ego/src/search.rs` 测试模块加：create_test_agent("fn-agt1","Alice","...")，force_sync_identity，search_by_name("Alice") 返回 Some("fn-agt1")，search_by_name("Al") 返回 None（全匹配），search_by_name("Bob") 返回 None。更新现有 test_search_by_name 断言（Vec -> Option）。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-memory-ego && cargo test search_by_name`
Expected: FAIL（返回 Vec 非 Option）。

- [ ] **Step 3: 实现 name_index HashMap 与 search_by_name**

`kissbot-memory-ego/src/search.rs` SearchManager 字段：将 `name_index: Arc<RwLock<SubstringIndex<String>>>` 改为 `name_index: Arc<RwLock<HashMap<String, String>>>`。new() 内 `name_index: Arc::new(RwLock::new(HashMap::new()))`。**保留** name_completion（SimplePrefixCompletion<String>）、name_descr_index（SubstringIndex）、search_metadata（SearchMetadata [individual_name, description] 不变）。

search_by_name：
```rust
pub async fn search_by_name(&self, query: &str) -> Option<String> {
    self.sync_all_identity().await;
    let guard = self.name_index.read().await;
    guard.get(query).cloned()
}
```

force_sync_identity：name 变更块改为操作 HashMap（其余 name_completion/name_descr_index 逻辑不变）：
```rust
// name_obsolute 为 true 时
if name_obsolute {
    let mut guard = self.name_index.write().await;
    if let Some(old_name) = old_name_or_none.as_ref() {
        guard.remove(old_name.as_str());
    }
    guard.insert(new_name.as_str().to_string(), agent_id.to_string());
    // name_completion（SubstringIndex 不变）
    let mut old_doc_name = old_name_or_none.clone();
    if let Some(old_name) = old_doc_name.take() {
        let old_doc = to_document(old_name);
        self.name_completion.remove(&agent_id.to_string(), &old_doc);
    }
    let new_doc = to_document(new_name.clone());
    self.name_completion.insert(&agent_id.to_string(), &new_doc);
}
```
else 分支（agent 不存在，移除）同理：HashMap.remove(old_name) + name_completion.remove(old_doc)。

注：`old_name_or_none` 仍取自 `search_metadata.value[0]`（SearchMetadata 不变，value[0]=individual_name）。new_name = metadata.individual_name.clone()。

- [ ] **Step 4: 更新 api.rs /agent/search-name 返回 Option**

```rust
async fn search_by_name(Json(req): Json<ego::SearchRequest>) -> impl IntoResponse {
    let ego_manager = SearchManager::get().await;
    let agent_id = ego_manager.search_by_name(&req.keyword).await;
    (StatusCode::OK, Json(ApiResponse::success(agent_id)))
}
```
（ApiResponse::success(Option<String>) 序列化为 data: null 或 agent_id 字符串。）

- [ ] **Step 5: 运行测试确认通过**

Run: `cd kissbot-memory-ego && cargo test`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(ego): search agent 侧 name_index 改 HashMap 全匹配，search_by_name 返回 Option"
```

---

### Task 5: memory-ego search role 侧（role_name_descr_index + RoleSearchMetadata 加 full_name）

**Files:**
- Modify: `kissbot-memory-ego/src/search.rs`

**Interfaces:**
- Produces: `RoleSearchMetadata.value = [role_name, full_name, description]`；role_name_descr_index 索引三者；role_name_index / role_name_completion / search_role_by_name 不变；force_sync_role descr 对比改 value[2]。

- [ ] **Step 1: 写失败测试 - role_name_descr_index 含 full_name 子串**

`kissbot-memory-ego/src/search.rs` 测试模块加：create_test_role 带 full_name="超级管理员"，force_sync_role，search_role_by_description("超级") 返回该 RoleKey。更新 create_test_role 支持 full_name。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-memory-ego && cargo test role_descr_full_name`
Expected: FAIL（full_name 未索引）。

- [ ] **Step 3: 实现 RoleSearchMetadata 加 full_name**

`kissbot-memory-ego/src/search.rs`：
```rust
impl RoleSearchMetadata {
    pub fn new(role: &Role) -> Self {
        Self { value: vec![role.role_name.clone(), role.full_name.clone(), role.description.clone()] }
    }
}
```
force_sync_role 内 descr 对比：`old_search_metadata.value[1]`（原 description）改为 `old_search_metadata.value[2]`（现 description）。`new_descr = role.description.clone()` 不变。name 对比 `value[0]`（role_name）不变。

role_name_index（SubstringIndex on role_name）、role_name_completion（PrefixCompletion on role_name）、search_role_by_name、search_role_by_description、role_name_completion 方法均不变（role_name_descr_index 经 RoleSearchMetadata 自动索引 [role_name, full_name, description]）。

更新 search.rs 内 create_test_role 辅助构造 Role 时加 full_name 字段。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd kissbot-memory-ego && cargo test`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(ego): search role 侧 RoleSearchMetadata/role_name_descr_index 加 full_name"
```

---

### Task 6: agent_name 绑定 - 配置

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`、`kissbot-agent/src/main.rs`

**Interfaces:**
- Produces: `ChannelConfig.agent_name`；移除 `init_agent_id/init_role/default_agent_id/default_role` 及 main.rs "Agent ID:" 日志；保留 init_model/default_model。

- [ ] **Step 1: 写失败测试 - agent_name 字段**

`kissbot-agent/src/config_manager.rs` 测试模块更新：构造 ChannelConfig 用 `agent_name`（非 agent_id）；构造 AgentConfig 去掉 init_agent_id/init_role；NexusRepo 去掉 default_agent_id/default_role。相关断言更新。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-agent && cargo test config`
Expected: 编译错误。

- [ ] **Step 3: 实现配置变更**

`kissbot-agent/src/config_manager.rs`：
- ChannelConfig：`pub agent_id: Arc<String>` -> `pub agent_name: Arc<String>`（注释更新为「绑定 agent_name（代号；空 = 保留 agent）」）。
- NexusRepo：删除 `default_agent_id` / `default_role` 字段；Default impl 删除对应两行。
- AgentConfig：删除 `init_agent_id` / `init_role` 字段。
- load_or_create_nexus：删除 `default_agent_id: cfg.init_agent_id.clone(), default_role: cfg.init_role.clone()` 两行（..NexusRepo::default() 不再含这两字段）。
- 删除 `default_agent_id()` / `default_role()` getter（确认无外部调用；main.rs 与 coordinator 调用点在 Task 7 处理，本任务先删 getter 并让编译报错指引）。
- 搜索全 crate `default_agent_id` / `default_role` / `.agent_id`（ChannelConfig 上的）调用点，本任务先处理 config_manager 内部；coordinator/session_manager/main 在 Task 7 处理（本任务后工作区可能暂不编译，Task 7 收尾）。

`kissbot-agent/src/main.rs`：删除 `info!("Agent ID: {}", config.default_agent_id().await);` 行。

- [ ] **Step 4: Commit（中间态，Task 7 完成后整体编译）**

```bash
git add -A
git commit -m "refactor(agent): 配置 agent_id->agent_name，移除 init_agent_id/init_role/default_agent_id/default_role（Task 7 收尾编译）"
```

---

### Task 7: agent_name 绑定 - SessionKey + resolve_agent_id + load_ego_info

**Files:**
- Modify: `kissbot-agent/src/types.rs`、`kissbot-agent/src/coordinator.rs`、`kissbot-agent/src/session_manager.rs`、`kissbot-agent/src/command_router.rs`

**Interfaces:**
- Produces: `SessionKey.agent_name`；`session_key_of -> SessionKey`（去 Option）；`resolve_agent_id(agent_name) -> Arc<String>`（缓存，回退 "0"）；memory-store/ego 调用点用解析后 agent_id；`RESERVED_AGENT_NAME=""`、`RESERVED_AGENT_ID="0"`、`RESERVED_ROLE_NAME=""`；load_ego_info 用 /agent/get。

- [ ] **Step 1: 写失败测试 - session_key_of 去 Option + resolve_agent_id**

`kissbot-agent/src/coordinator.rs` 测试模块更新 session_key_for_empty_detaches_but_zero_attaches：session_key_of 返回 SessionKey（非 Option），agent_name="" 返回 key（agent_name=""），agent_name="a1" 返回 key（agent_name="a1"）。新增 resolve_agent_id 单测（mock ego 不可用回退 "0"；agent_name="" 直接 "0"）。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-agent && cargo test session_key`
Expected: FAIL。

- [ ] **Step 3: 实现 SessionKey.agent_name 与常量**

`kissbot-agent/src/types.rs`：
```rust
pub struct SessionKey {
    pub agent_name: String,
    pub role_name: String,
    pub mode: Mode,
}
```
（agent_id -> agent_name；memory_role 用 key.role_name 不变。）

`kissbot-agent/src/coordinator.rs` 顶部常量：
```rust
pub const RESERVED_AGENT_NAME: &str = "";
pub const RESERVED_AGENT_ID: &str = "0";
pub const RESERVED_ROLE_NAME: &str = "";
```
（删除原 RESERVED_AGENT_ID="0"/RESERVED_ROLE_NAME="0" 中 role 的 "0"。）

- [ ] **Step 4: 实现 resolve_agent_id 与缓存**

AgentCoordinator 加字段：
```rust
agent_id_cache: Arc<DashMap<String, Arc<String>>>,
```
new() 初始化 `agent_id_cache: Arc::new(DashMap::new())`。

加方法：
```rust
async fn resolve_agent_id(&self, agent_name: &str) -> Arc<String> {
    if agent_name.is_empty() {
        return Arc::new(RESERVED_AGENT_ID.to_string());
    }
    if let Some(v) = self.agent_id_cache.get(agent_name) {
        return v.clone();
    }
    let ego_url = kissbot_api::ApiConfig::get().memory_ego_url.clone();
    let resolved = if ego_url.is_empty() {
        Arc::new(RESERVED_AGENT_ID.to_string())
    } else {
        let client = reqwest::Client::new();
        match client.post(format!("{}/agent/search-name", ego_url))
            .json(&serde_json::json!({ "keyword": agent_name }))
            .send().await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(data) => match data["data"].as_str() {
                    Some(id) if !id.is_empty() => Arc::new(id.to_string()),
                    _ => Arc::new(RESERVED_AGENT_ID.to_string()),
                },
                Err(_) => Arc::new(RESERVED_AGENT_ID.to_string()),
            },
            Err(_) => Arc::new(RESERVED_AGENT_ID.to_string()),
        }
    };
    self.agent_id_cache.insert(agent_name.to_string(), resolved.clone());
    resolved
}
```

- [ ] **Step 5: 迁移 session_key_for/of 与调用点**

`session_key_for(&ch)`：返回 `SessionKey { agent_name: ch.agent_name.to_string(), role_name: ch.role_name.to_string(), mode }`（去 Option，无空判断）。
`session_key_of(agent_name, role_name, mode)`：返回 `SessionKey { agent_name: agent_name.to_string(), role_name: role_name.to_string(), mode }`（去 Option）。更新测试。

调用点迁移：
- build_initial_context：`if session.key.agent_name == RESERVED_AGENT_NAME` -> default_system_prompt；否则 `let agent_id = self.resolve_agent_id(&session.key.agent_name).await;` 后 `load_ego_info(&agent_id, ...)`。read_history / read_memory_struct_index 用 `self.resolve_agent_id(&session.key.agent_name).await` 的结果作 agent_id 参数。
- resolve_send_channel（coordinator:249）：`other.agent_id.as_str() == key.agent_id` -> `other.agent_name.as_str() == key.agent_name`。
- session_manager.rs resolve_send_channel（:204）：`ch.agent_id` / `key.agent_id` -> `ch.agent_name` / `key.agent_name`。
- list_events：`&key.agent_id` -> `&self.resolve_agent_id(&key.agent_name).await`（注意 list_events 签名若取 &str 需先 resolve 存局部变量）。
- incoming_message / send_reply push_channel_record：`agent_id: Arc::new(key.agent_id.clone())` -> `agent_id: self.resolve_agent_id(&key.agent_name).await`（Arc<String>，直接用）。
- run_agentic_loop ThinkRequest：`let agent_id = session.key.agent_id.clone()` -> `let agent_id = self.resolve_agent_id(&session.key.agent_name).await;`（WriteTask::Think 的 agent_id 字段类型若为 String，用 `agent_id.as_str().to_string()` 或调整）。

- [ ] **Step 6: 改 load_ego_info 用 /agent/get**

```rust
async fn load_ego_info(&self, agent_id: &str, role_name: &str) -> Result<String> {
    let ego_url = kissbot_api::ApiConfig::get().memory_ego_url.clone();
    let client = reqwest::Client::new();
    let mut system_parts = vec![];
    if let Ok(resp) = client.post(format!("{}/agent/get", ego_url))
        .json(&serde_json::json!({ "agent_id": agent_id }))
        .send().await
    {
        if let Ok(data) = resp.json::<serde_json::Value>().await {
            if let Some(name) = data["data"]["individual_name"].as_str() {
                system_parts.push(format!("你的名字是: {}", name));
            }
            if let Some(desc) = data["data"]["description"].as_str() {
                system_parts.push(format!("你的描述: {}", desc));
            }
        }
    }
    // 角色设定 /role/get 部分保持不变
    ...
    if system_parts.is_empty() { system_parts.push("你是 kissbot 智能助手".to_string()); }
    Ok(system_parts.join("\n"))
}
```

- [ ] **Step 7: 改 command_router /agent /role 默认 ""**

`kissbot-agent/src/types.rs` AdminCommand::SetAgent 字段 `agent_id` -> `agent_name`。`command_router.rs`：parse `/agent` 产出 `SetAgent { agent_name, role }`；execute `let new_agent = agent_name.clone().unwrap_or_else(|| RESERVED_AGENT_NAME.to_string());` `c.agent_name = Arc::new(new_agent.clone());`。SetRole execute `unwrap_or_else(|| RESERVED_ROLE_NAME.to_string())`（RESERVED_ROLE_NAME=""）。更新 import（RESERVED_AGENT_NAME/RESERVED_ROLE_NAME）。

- [ ] **Step 8: 运行全工作区测试确认通过**

Run: `cargo test --workspace`
Expected: PASS。

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor(agent): SessionKey 用 agent_name + resolve_agent_id 解析 + session_key_of 去 Option + load_ego_info /agent/get"
```

---

### Task 8: 配置/模板/workspace 清理 + 文档

**Files:**
- Modify: `config.json`、`script/config.json`、`test/workspace-template/config.json`、`script/template/nexus.json`、`test/workspace-template/agent-data/nexus.json`
- 清理：`workspace/`、`test/workspace/`
- 文档：`docs/spec/memory-ego.md`、`docs/spec/kissbot-agent-nexus.md`、`docs/design/components-design/kissbot-memory-ego.md`、`docs/design/components-design/kissbot-agent-nexus.md`

**Interfaces:**
- Produces: 配置/模板与新结构一致；workspace 从模板重建；文档同步。

- [ ] **Step 1: 更新 config.json 三处 agent 段**

`config.json`、`script/config.json`、`test/workspace-template/config.json`：删除 `"init_agent_id": "",` 与 `"init_role": "",` 两行（保留 default_system_prompt / init_model 等）。

- [ ] **Step 2: 更新 nexus.json 模板**

`script/template/nexus.json`、`test/workspace-template/agent-data/nexus.json`：删除 `"default_agent_id": "",` 与 `"default_role": "",` 两行；channel 条目 `"agent_id": "0"` -> `"agent_name": ""`（保留 agent），`"role_name": "0"` -> `"role_name": ""`。

- [ ] **Step 3: 清理 workspace 并从模板重建**

```bash
rm -rf workspace test/workspace
# test/workspace 由测试 global-setup 从 test/workspace-template 复制生成
# workspace 手动测试时按需从 script/template 重建
```

- [ ] **Step 4: 运行 nexus-chat 集成测试验证**

Run: `cd test && npm test`（或项目既有测试命令，如 playwright nexus-chat）
Expected: PASS（模板 channel 初始即保留 agent agent_name=""，/agent 流程照常）。

- [ ] **Step 5: 更新文档**

- `docs/spec/memory-ego.md`：search 索引（name_index HashMap 全匹配、search_by_name Option、role_name_descr_index 加 full_name、name_completion 保留）、full_name 字段、代号限制。
- `docs/spec/kissbot-agent-nexus.md`：agent_name 绑定、resolve_agent_id、保留 agent 语义、session_key 去 Option。
- 对应 components-design 文档同步。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "chore: 配置/模板适配 agent_name + 清理 workspace + 文档同步（memory-ego search/full_name/代号、agent_name 绑定）"
```

---

## Self-Review

**1. Spec coverage（计划 2 覆盖 spec 第 3/4 节）：**
- 第 3 节 memory-ego full_name + 代号：Role/RoleRelation full_name（Task 1/2）；代号校验（Task 3）；search agent 侧（Task 4）；search role 侧（Task 5）；API 路由（Task 2/4）。✓
- 第 4 节 agent_name 绑定：配置（Task 6）；SessionKey/resolve_agent_id/session_key_of/load_ego_info（Task 7）；memory-store 用 agent_id（Task 7 调用点）；保留 agent 语义（Task 6/7）；role 目录已在计划 1 Task 6。✓
- 第 6 节受影响面中 ego/agent 部分：各 Task 覆盖；配置/模板/workspace（Task 8）。✓
- individual_name 不校验唯一性（HashMap 覆盖）：Task 4 不加唯一性校验。✓
- AgentMetadata/OtherRole/IndividualRecognition 不加 full_name：Task 1 仅 Role/RoleRelation 加。✓

**2. Placeholder scan：** 无 TBD/TODO；调用点迁移以「站点 + 改法」给出。✓

**3. Type consistency：** `resolve_agent_id -> Arc<String>`；SessionKey.agent_name（String）；RESERVED_AGENT_NAME="" / RESERVED_AGENT_ID="0" / RESERVED_ROLE_NAME=""；search_by_name -> Option<String>；name_index HashMap<String, String>；RoleSearchMetadata [role_name, full_name, description]（value[0/1/2] 对应）。✓

**4. 跨计划衔接：** 计划 1 已让 ChannelRecord/ChannelRequest 含 name、IncomingMessage 无 is_self、role 目录 `{year}-{role_name}`；计划 2 Task 7 的 push_channel_record 调用点沿用计划 1 Task 4 的 ChannelRecord 结构（含 name），仅 agent_id 字段值改为 resolve_agent_id 结果。✓

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-02-ego-fullname-agentname-binding-refactor.md`. 两计划按「计划 1 -> 计划 2」顺序执行。
