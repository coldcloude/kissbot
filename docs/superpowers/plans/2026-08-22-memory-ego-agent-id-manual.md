# memory-ego agent_id 手工化 + 去除 individual_name + 精简搜索索引 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** agent_id 改为调用方在 create 时手工指定的易读标识符，删除 AgentMetadata.individual_name 字段，精简搜索索引（name_index / role_name_index）并同步调整 API、消费方与测试。

**Architecture:** 数据模型（kissbot-api）先行，随后按依赖顺序修改 memory-ego 各管理器（agent / role_play / search / api），再适配 kissbot-agent 消费方，最后更新 playwright 测试与文档。每次改动都保持 crate 可编译、测试通过。

**Tech Stack:** Rust (tokio/axum/serde)、kai-index 子串索引、Playwright (TypeScript)

## Global Constraints

- 不删除代码中的注释（项目 CLAUDE.md 原则），修改处保留/更新注释
- 所有文本文件 UTF-8、`\n` 换行
- 使用中文写 git commit comment，comment 应包含该提交所有改动内容
- agent_id / role_name 代号规则：`^[A-Za-z0-9_]+$`、非空（`validate_code`）
- agent_id 创建后不可变；role_name 创建后不可变更（role 内容 description/full_name/其他角色关系仍可改）
- 读写文件必须使用 Read/Write/Edit 工具，禁止 sed/python 修改文件

---

### Task 1: kissbot-api 数据模型（ego.rs）

**Files:**
- Modify: `kissbot-api/src/ego.rs`（AgentMetadata / CreateAgentRequest / CopyAgentRequest 结构、删除 UpdateAgentNameRequest / RemoveRoleRequest / RenameRoleRequest、serde 测试）

**Interfaces:**
- Consumes: 无（起点）
- Produces: `AgentMetadata { agent_id: Arc<String>, description: Arc<String>, created_at: Arc<String> }`；`CreateAgentRequest { agent_id, description }`；`CopyAgentRequest { agent_id, new_agent_id }`；删除 `UpdateAgentNameRequest`、`RemoveRoleRequest`、`RenameRoleRequest`

- [ ] **Step 1: 修改 AgentMetadata / CreateAgentRequest / CopyAgentRequest 结构体**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub agent_id: Arc<String>,
    pub description: Arc<String>,
    pub created_at: Arc<String>,
}
```

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    pub agent_id: Arc<String>,
    pub description: Arc<String>,
}
```

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct CopyAgentRequest {
    pub agent_id: Arc<String>,
    pub new_agent_id: Arc<String>,
}
```

- [ ] **Step 2: 删除三个不再使用的请求结构体**（整块删除）

`UpdateAgentNameRequest`、`RemoveRoleRequest`、`RenameRoleRequest` 的定义块。

- [ ] **Step 3: 更新 serde 测试**

- `test_serde_agent_metadata`：删除 `individual_name` 构造与断言
- `test_serde_create_agent_request`：`individual_name: Arc::new("Alice".to_string())` 改为 `agent_id: Arc::new("a1".to_string())`，断言 `deserialized.individual_name` 改为 `deserialized.agent_id == "a1"`
- `test_serde_copy_agent_request`：增加 `new_agent_id: Arc::new("a2".to_string())`，断言 `deserialized.new_agent_id == "a2"`
- 删除 `test_serde_update_agent_name_request`、`test_serde_remove_role_request`、`test_serde_rename_role_request` 整个测试函数

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-api`
Expected: 全部 PASS，无编译错误。
注：kissbot-memory-ego / kissbot-agent 此时编译失败为计划内瞬态（individual_name 引用待 Task 2/4/5/6 清理），不影响本任务验收。

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-api/src/ego.rs
git commit -m "kissbot-api: AgentMetadata 删除 individual_name，CreateAgentRequest/CopyAgentRequest 改为手工指定 agent_id，删除 UpdateAgentNameRequest/RemoveRoleRequest/RenameRoleRequest"
```

---

### Task 2: memory-ego 错误类型与 Agent 管理（error.rs / code.rs / agent.rs）

**Files:**
- Modify: `kissbot-memory-ego/src/error.rs`（新增 AgentAlreadyExists）
- Modify: `kissbot-memory-ego/src/code.rs`（注释更新）
- Modify: `kissbot-memory-ego/src/agent.rs`（create_agent / copy_agent / 删除 update_agent_name / 测试）

**Interfaces:**
- Consumes: Task 1 的 `AgentMetadata` / `CreateAgentRequest` / `CopyAgentRequest`
- Produces: `AgentManager::create_agent(agent_id: Arc<String>, description: Arc<String>) -> Result<Arc<String>>`；`AgentManager::copy_agent(agent_id: &str, new_agent_id: Arc<String>) -> Result<Arc<String>>`；删除 `update_agent_name`；`Error::AgentAlreadyExists(String)`

- [ ] **Step 1: error.rs 新增 AgentAlreadyExists**

```rust
    #[error("Agent already exists: {0}")]
    AgentAlreadyExists(String),
```
（插入到 `AgentNotFound` 之后）

- [ ] **Step 2: code.rs 注释更新**

```rust
/// 校验代号：仅字母/数字/下划线，且非空（等价于 `^[A-Za-z0-9_]+$`）。
/// 用于 agent_id / role_name 等代号字段的写入入口。
```

- [ ] **Step 3: 写失败测试（agent.rs tests 模块）**

替换 `test_create_agent`，新增重复创建与 copy 目标已存在的测试：

```rust
    #[tokio::test]
    async fn test_create_agent() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("alice".to_string()),
            Arc::new("Test agent".to_string()),
        ).await.unwrap();
        assert_eq!(*agent_id, "alice");
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.agent_id, "alice");
        assert_eq!(*agent.description, "Test agent");
    }

    #[tokio::test]
    async fn test_create_agent_duplicate() {
        setup().await;
        let manager = AgentManager::get();
        manager.create_agent(
            Arc::new("dup-alice".to_string()),
            Arc::new("Test agent".to_string()),
        ).await.unwrap();
        let result = manager.create_agent(
            Arc::new("dup-alice".to_string()),
            Arc::new("Another agent".to_string()),
        ).await;
        assert!(matches!(result, Err(Error::AgentAlreadyExists(_))));
    }
```

- [ ] **Step 4: 实现 create_agent / copy_agent，删除 update_agent_name**

```rust
    pub async fn create_agent(&self, agent_id: Arc<String>, description: Arc<String>) -> Result<Arc<String>> {
        validate_code(agent_id.as_str())?;
        // 查重：agent 目录下 metadata.json 已存在则报错，不覆盖已有数据
        let metadata_path = agent_metadata_path(agent_id.as_str()).await?;
        if metadata_path.exists() {
            return Err(Error::AgentAlreadyExists(agent_id.to_string()));
        }
        let created_at = Arc::new(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());

        let metadata = AgentMetadata {
            agent_id: agent_id.clone(),
            description,
            created_at,
        };

        self.write_agent_metadata_ref(metadata.agent_id.clone().as_str(), |_| {
            Ok(Arc::new(metadata))
        }).await?;
        // 新 agent 需入搜索索引（name_completion/name_descr_index 依赖 agent_id；
        // 与 update_agent_description 的 mark_identity_dirty 对齐）
        SearchManager::get().await.mark_identity_dirty(agent_id.as_str());
        Ok(agent_id)
    }
```

```rust
    pub async fn copy_agent(&self, agent_id: &str, new_agent_id: Arc<String>) -> Result<Arc<String>> {
        let metadata = self.get_agent(agent_id).await?;
        self.create_agent(new_agent_id, metadata.description.clone()).await
    }
```

删除整个 `update_agent_name` 方法（含其内 SearchManager 调用）。删除文件顶部 `use uuid::Uuid;`。

- [ ] **Step 5: 更新其余 agent.rs 测试**

- `test_update_agent_name`：整块删除
- `test_update_agent_description`：删除 `assert_eq!(*agent.individual_name, "Alice");` 一行
- `test_copy_agent`：

```rust
    #[tokio::test]
    async fn test_copy_agent() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("alice".to_string()),
            Arc::new("Test".to_string()),
        ).await.unwrap();
        let new_id = manager.copy_agent(&agent_id, Arc::new("alice-copy".to_string())).await.unwrap();
        assert_eq!(*new_id, "alice-copy");
        let original = manager.get_agent(&agent_id).await.unwrap();
        let copy = manager.get_agent(&new_id).await.unwrap();
        assert_eq!(*original.description, *copy.description);
    }
```

- `test_crud_chain`：删除 `manager.update_agent_name(...)` 一行及对应断言

```rust
    #[tokio::test]
    async fn test_crud_chain() {
        setup().await;
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("alice".to_string()),
            Arc::new("Original".to_string()),
        ).await.unwrap();
        manager.update_agent_description(&agent_id, Arc::new("Updated".to_string())).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.description, "Updated");
    }
```

- `test_create_agent_rejects_invalid_code`：保持不变（`create_agent(Arc::new("a b c"), ...)` 应返回 InvalidCode）
- `test_create_agent_valid_code`：断言 `*agent.agent_id, "alice_01"`，删除 individual_name 断言
- `setup()` 中 metadata json 里的 `"individual_name": "Setup",` 一行删除

- [ ] **Step 6: 运行检查确认本任务改动无残留错误**

Run: `cd /home/admin/project/kissbot && cargo check -p kissbot-memory-ego 2>&1 | grep -E "error|warning" | head -40`
Expected: 错误仅出现在 `search.rs` 与 `api.rs`（引用 individual_name / 旧 create_agent 签名，Task 4/5 清理）；`agent.rs` 无错误。crate 整体编译通过要等到 Task 5。

- [ ] **Step 7: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-memory-ego/src/error.rs kissbot-memory-ego/src/code.rs kissbot-memory-ego/src/agent.rs
git commit -m "memory-ego: agent_id 改为 create/copy 时手工指定（校验+查重 AgentAlreadyExists），删除 update_agent_name 与 individual_name 相关逻辑"
```

---

### Task 3: memory-ego 角色设定精简（role_play.rs）

**Files:**
- Modify: `kissbot-memory-ego/src/role_play.rs`

**Interfaces:**
- Consumes: Task 1（无需再引用被删请求结构）
- Produces: 删除 `RolePlayManager::remove_role`、`rename_role`、私有 `remove_role_play_ref`

- [ ] **Step 1: 删除 remove_role / rename_role / remove_role_play_ref 方法**

删除 `remove_role_play_ref` 私有方法（约 line 101，仅被 remove_role 使用）、`remove_role` 方法、`rename_role` 方法。保留 `create_role`、`create_role_from`、`update_role_description`、`update_role_full_name` 及所有 other_roles 相关方法。

- [ ] **Step 2: 删除对应测试**

删除 `test_remove_role`、`test_rename_role`、`test_rename_role_rejects_invalid_code` 三个测试函数。

- [ ] **Step 3: 运行检查确认本任务改动无残留错误**

Run: `cd /home/admin/project/kissbot && cargo check -p kissbot-memory-ego 2>&1 | grep -E "error|warning" | head -40`
Expected: 错误仍仅出现在 `search.rs` 与 `api.rs`（Task 4/5 清理）；`role_play.rs` 无错误。crate 整体编译通过要等到 Task 5。

- [ ] **Step 4: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-memory-ego/src/role_play.rs
git commit -m "memory-ego: role_name 创建后不可变更，删除 remove_role/rename_role/remove_role_play_ref 及其测试"
```

---

### Task 4: memory-ego 搜索精简（search.rs）

**Files:**
- Modify: `kissbot-memory-ego/src/search.rs`

**Interfaces:**
- Consumes: Task 1 的 `AgentMetadata`（无 individual_name）、Task 2 的 `create_agent`（agent_id 不可变）
- Produces: `SearchManager` 字段删除 `name_index` / `role_name_index`；删除 `search_by_name` / `search_role_by_name`；`SearchMetadata.value = [agent_id, description]`；`name_completion` 索引 agent_id

- [ ] **Step 1: 修改 SearchMetadata 与删除字段/方法**

```rust
impl SearchMetadata {
    pub fn new(metadata: &AgentMetadata) -> Self {
        Self {
            value: vec![metadata.agent_id.clone(), metadata.description.clone()],
        }
    }
}
```

`SearchManager` 结构体删除字段：`name_index: Arc<RwLock<HashMap<String, String>>>` 和 `role_name_index: Arc<RwLock<SubstringIndex<RoleKey>>>`；`new()` 中删除对应初始化。删除文件顶部 `use std::collections::HashMap;`。

删除方法：`search_by_name`、`search_role_by_name`。

- [ ] **Step 2: 重写 force_sync_identity**

```rust
    pub async fn force_sync_identity(&self, agent_id: &str) {
        let old_metadata = self.search_metadata.remove(agent_id).map(|(_, m)| m);
        let was_indexed = old_metadata.is_some();
        if let Ok(metadata) = AgentManager::get().get_agent(agent_id).await {
            //存在agent，更新索引
            let new_search_metadata = SearchMetadata::new(&metadata);
            let mut fulltext_obsolute = true;
            //没旧值，或新旧值不同，则需要变更全文索引
            if let Some(old) = old_metadata.as_ref() {
                if old.value == new_search_metadata.value {
                    fulltext_obsolute = false;
                }
            }
            if fulltext_obsolute {
                let mut guard = self.name_descr_index.write().await;
                //有旧值，先移除
                if let Some(old) = old_metadata {
                    guard.remove(&agent_id.to_string(), &old);
                }
                //插入新值
                guard.insert(&agent_id.to_string(), &new_search_metadata);
            }
            //name_completion（索引 agent_id，不可变，仅首次索引时插入）
            if !was_indexed {
                let new_id_document = to_document(metadata.agent_id.clone());
                self.name_completion.insert(&agent_id.to_string(), &new_id_document);
            }
            //保存search_metadata
            self.search_metadata.insert(agent_id.to_string(), new_search_metadata);
        }
        else {
            //移除旧全文索引与补全索引
            if let Some(old) = old_metadata {
                let mut guard = self.name_descr_index.write().await;
                guard.remove(&agent_id.to_string(), &old);
                let old_id_document = to_document(old.value[0].clone());
                self.name_completion.remove(&agent_id.to_string(), &old_id_document);
            }
        }
    }
```

- [ ] **Step 3: 更新 search.rs 测试**

- `create_test_agent` 签名改为 `(agent_id: &str, description: &str)`（individual_name 字段已不存在，删除 name 参数）：

```rust
    async fn create_test_agent(agent_id: &str, description: &str) {
        let dm = DirectoryManager::get();
        let agent_dir = dm.ensure_agent_dir(agent_id).await.unwrap();
        let metadata = serde_json::json!({
            "agent_id": agent_id,
            "description": description,
            "created_at": "2026-06-25 10:00:00"
        });
        tokio::fs::write(
            agent_dir.join("metadata.json"),
            serde_json::to_string_pretty(&metadata).unwrap(),
        ).await.unwrap();
    }
```

- 更新各调用点（删除中间的 name 参数）：
  - `test_search_by_description`：`create_test_agent("desc-agt", "Alice", "Some searchable text here")` → `create_test_agent("desc-agt", "Some searchable text here")`
  - `test_search_role_by_name`：`create_test_agent("role-name-agt", "Alice", "")` → `create_test_agent("role-name-agt", "")`
  - `test_search_role_by_description`：`create_test_agent("role-desc-agt", "Alice", "")` → `create_test_agent("role-desc-agt", "")`
  - `test_search_role_by_description_matches_full_name`：`create_test_agent("role-fn-agt", "Alice", "")` → `create_test_agent("role-fn-agt", "")`
  - `test_role_description_only_change_reindexes`：`create_test_agent("role-desc-chg-agt", "Alice", "")` → `create_test_agent("role-desc-chg-agt", "")`
  - `test_role_full_name_only_change_reindexes`：`create_test_agent("role-fn-chg-agt", "Alice", "")` → `create_test_agent("role-fn-chg-agt", "")`
  - `test_retrieve_agents`：`create_test_agent("ret-agt1", "Alice", "Desc1")` → `create_test_agent("ret-agt1", "Desc1")`；`create_test_agent("ret-agt2", "Bob", "Desc2")` → `create_test_agent("ret-agt2", "Desc2")`
- 删除 `test_search_by_name`、`test_search_by_name_no_match`
- `test_agent_name_completion`：改为按 agent_id 补全：

```rust
    #[tokio::test]
    async fn test_agent_name_completion() {
        crate::test_util::init_test_config();
        create_test_agent("alice", "", "").await;
        create_test_agent("albert", "", "").await;
        create_test_agent("bob", "", "").await;
        let manager = SearchManager::new();
        manager.force_sync_identity("alice").await;
        manager.force_sync_identity("albert").await;
        manager.force_sync_identity("bob").await;
        let results = manager.name_completion("al").await;
        assert_eq!(results.len(), 2, "expected 2, got {:?}", results);
        let ids: Vec<&str> = results.iter().map(|r| r.key.as_str()).collect();
        assert!(ids.contains(&"alice"));
        assert!(ids.contains(&"albert"));
    }
```

- 删除 `test_search_role_by_name`
- `test_retrieve_agents`：`let names: Vec<&str> = results.iter().map(|a| a.individual_name.as_str()).collect();` 改为 `let names: Vec<&str> = results.iter().map(|a| a.agent_id.as_str()).collect();`，`assert!(names.contains(&"Alice"))` 改为 `assert!(names.contains(&"ret-agt1"))`，`"Bob"` → `"ret-agt2"`

- [ ] **Step 4: 运行检查确认本任务改动无残留错误**

Run: `cd /home/admin/project/kissbot && cargo check -p kissbot-memory-ego 2>&1 | grep -E "error|warning" | head -40`
Expected: 错误仅剩 `api.rs`（旧 create_agent 签名调用、被删方法引用，Task 5 清理）；`search.rs` 无错误。crate 整体编译通过要等到 Task 5。

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-memory-ego/src/search.rs
git commit -m "memory-ego: 搜索去掉 name_index/role_name_index 与 search_by_name/search_role_by_name，SearchMetadata 改为 [agent_id, description]，name_completion 改索引 agent_id"
```

---

### Task 5: memory-ego API 精简（api.rs）

**Files:**
- Modify: `kissbot-memory-ego/src/api.rs`

**Interfaces:**
- Consumes: Task 1 请求结构、Task 2/4 管理器新接口
- Produces: 路由删除 `/agent/update-name`、`/agent/search-name`、`/role/search-name`、`/role/remove`、`/role/rename`；create/copy handler 适配新请求结构；删除 `search_name_http_success` 测试，新增重复创建 409 测试

- [ ] **Step 1: 删除路由**

```rust
        .route("/agent/update-name", put(update_agent_name))
        .route("/agent/search-name", post(search_by_name))
        .route("/role/search-name", post(search_role_by_name))
        .route("/role/remove", delete(remove_role))
        .route("/role/rename", put(rename_role))
```
整块删除上述 5 行。同时删除 import 中的 `delete`（`/role/remove` 是唯一 delete 路由）：

```rust
use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
```

（`put` 仍用于 update-description / update-full-name / other 系列，保留；`get` 用于 /agent/list。）

- [ ] **Step 2: 更新 create_agent / copy_agent handler，删除被删 API 的 handler**

```rust
async fn create_agent(Json(req): Json<ego::CreateAgentRequest>) -> impl IntoResponse {
    let agent_id = req.agent_id.clone();
    let result = AgentManager::get().create_agent(req.agent_id, req.description).await;

    match result {
        Ok(agent_id) => (StatusCode::OK, Json(ApiResponse::success(agent_id))),
        Err(Error::AgentAlreadyExists(_)) => (StatusCode::CONFLICT, Json(ApiResponse::error(format!("Agent {} already exists", agent_id)))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
```

```rust
async fn copy_agent(Json(req): Json<ego::CopyAgentRequest>) -> impl IntoResponse {
    let result = AgentManager::get().copy_agent(&req.agent_id, req.new_agent_id).await;

    match result {
        Ok(agent_id) => (StatusCode::OK, Json(ApiResponse::success(agent_id))),
        Err(Error::AgentNotFound(_)) => (StatusCode::NOT_FOUND, Json(ApiResponse::error(format!("Agent {} not found", req.agent_id)))),
        Err(Error::AgentAlreadyExists(_)) => (StatusCode::CONFLICT, Json(ApiResponse::error("New agent already exists".to_string()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
```

删除 handler：`update_agent_name`、`search_by_name`、`search_role_by_name`、`remove_role`、`rename_role`。

- [ ] **Step 3: 更新 api.rs 测试**

删除 `search_name_http_success` 测试，替换为重复创建冲突测试：

```rust
    // /agent/create 重复 agent_id mock 测试：第二次创建同一 agent_id 返回 409 CONFLICT
    #[tokio::test]
    async fn create_agent_http_duplicate() {
        crate::test_util::init_test_config();

        let app = create_router();
        let body = r#"{"agent_id":"dup-http","description":"First"}"#.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/agent/create")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let req = Request::builder()
            .method("POST")
            .uri("/agent/create")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"agent_id":"dup-http","description":"Second"}"#.to_string()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["success"], false);
    }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-memory-ego`
Expected: 全部 PASS

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-memory-ego/src/api.rs
git commit -m "memory-ego: 删除 /agent/update-name、/agent/search-name、/role/search-name、/role/remove、/role/rename 路由，create/copy 适配手工 agent_id，新增重复创建 409 测试"
```

---

### Task 6: kissbot-agent 消费方适配（ego_md.rs）

**Files:**
- Modify: `kissbot-agent/src/ego_md.rs`

**Interfaces:**
- Consumes: Task 1 的 `AgentMetadata`（无 individual_name）
- Produces: `build_ego_identity_md` 使用 `metadata.agent_id`

- [ ] **Step 1: 修改 build_ego_identity_md**

```rust
/// 由 AgentMetadata 生成系统提示词 markdown（身份）
pub fn build_ego_identity_md(metadata: &AgentMetadata) -> String {
    format!(
        "# Agent Identity\n\n- **Name**\n {}\n- **Created At**\n {}\n- **Description**\n {}\n",
        metadata.agent_id, metadata.created_at, metadata.description
    )
}
```

- [ ] **Step 2: 编译确认**

Run: `cd /home/admin/project/kissbot && cargo build -p kissbot-agent`
Expected: 编译成功，无警告

- [ ] **Step 3: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/ego_md.rs
git commit -m "kissbot-agent: 身份 markdown 改用 agent_id 替代 individual_name"
```

---

### Task 7: Playwright 测试更新

**Files:**
- Modify: `test/tests/memory-ego-api.spec.ts`
- Modify: `test/tests/agent-commands.spec.ts`
- Modify: `test/tests/nexus-ego-chat-store.spec.ts`

**Interfaces:**
- Consumes: 本计划全部 Rust 侧改动

- [ ] **Step 1: memory-ego-api.spec.ts — Agent 部分**

- TC-01：`individual_name: 'alice'` 改为 `agent_id: 'alice'`；`expect(resp.data).toBeTruthy()` 改为 `expect(resp.data).toBe('alice')`；`agentId = resp.data;`
- TC-03：删除 `expect(resp.data.individual_name).toBe('alice');`
- TC-04（更新 agent 名称）：整个 test 块删除
- TC-06：请求体改为 `{ agent_id: agentId, new_agent_id: 'alice_copy' }`；`expect(resp.data).toBe('alice_copy')`；`copiedAgentId = resp.data;`
- TC-07（按名称搜索）：整个 test 块删除
- TC-08：保持不变（keyword '新描述' 命中 agentId 的描述）
- TC-10：注释/断言保持不变（agentId='alice'，prefix 'ali' 命中 agent_id 补全）

- [ ] **Step 2: memory-ego-api.spec.ts — 角色部分**

- TC-23（重命名角色）：整个 test 块删除
- TC-24（按名称搜索角色）：整个 test 块删除
- TC-25：断言 `k.role_name === 'mod'` 改为 `k.role_name === 'admin2'`（TC-22 创建 admin2，未被改名；admin 与 admin2 描述均为 '新描述'）
- TC-26：`role_name: 'mod'` 改为 `role_name: 'admin2'`，断言 `resp.data[0].role_name` 为 `'admin2'`
- TC-27：`prefix: 'mo'` 改为 `prefix: 'ad'`，断言 `c.key.role_name === 'mod'` 改为 `c.key.role_name === 'admin'`
- TC-35（删除角色）：整个 test 块删除

- [ ] **Step 3: agent-commands.spec.ts**

TC-02 中 create 请求体 `data: { individual_name: 'a1', description: '测试 agent' }` 改为 `data: { agent_id: 'a1', description: '测试 agent' }`；注释中的「search-name 全匹配」改为「按 agent_id 解析」。

- [ ] **Step 4: nexus-ego-chat-store.spec.ts**

- create 请求体 `data: { individual_name: 'a1', description: 'ego 测试 agent' }` 改为 `data: { agent_id: 'a1', description: 'ego 测试 agent' }`
- 删除 search-name 解析确认块（`/agent/search-name` 已删），改为：

```ts
    // 解析确认：agent 存在
    const getResp = await (await request.post(`${EGO_BASE}/agent/get`, {
      headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
      data: { agent_id: agentId },
    })).json();
    expect(getResp.success).toBe(true);
    expect(getResp.data.agent_id).toBe(agentId);
```

- [ ] **Step 5: 运行 playwright 相关测试**

Run:
```bash
cd /home/admin/project/kissbot/test
npx playwright test tests/memory-ego-api.spec.ts tests/nexus-ego-chat-store.spec.ts --reporter=line
```
（agent-commands.spec.ts 依赖完整后端，如环境可运行则一并执行：`npx playwright test tests/agent-commands.spec.ts --reporter=line`）

Expected: 全部 PASS

- [ ] **Step 6: Commit**

```bash
cd /home/admin/project/kissbot
git add test/tests/memory-ego-api.spec.ts test/tests/agent-commands.spec.ts test/tests/nexus-ego-chat-store.spec.ts
git commit -m "test: ego agent 创建改传手工 agent_id，删除 search-name/update-name/role rename/remove 用例，角色搜索断言适配"
```

---

### Task 8: 文档更新

**Files:**
- Modify: `docs/spec/memory-ego.md`
- Modify: `docs/design/components-design/kissbot-memory-ego.md`

**Interfaces:**
- Consumes: 本计划全部实现

- [ ] **Step 1: docs/spec/memory-ego.md**

- 删除第 18 行 name_index 条目，第 19 行改为：

```markdown
- **name_completion**：agent_id 前缀补全（保留）
- **name_descr_index**：`[agent_id, description]` 子串搜索
```

- 删除第 24 行 `role_name_index` 条目，`role_name_descr_index` 条目注明 role_name 搜索统一走该全文索引
- 第 33 行改为：

```markdown
- **代号限制**：`agent_id`、`role_name` 仅允许字母、数字、下划线（`^[A-Za-z0-9_]+$`），在写入入口强制校验（非空）；agent_id 在 create 时手工指定且创建后不可变，重复创建返回 AgentAlreadyExists
```

- [ ] **Step 2: docs/design/components-design/kissbot-memory-ego.md**

- Epic 1 用户故事「以分配唯一的 agent ID」改为「以指定易读的 agent ID」
- 第 83-84 行搜索描述改为：

```markdown
- **Agent 搜索**：`name_completion`（agent_id 前缀补全）与 `name_descr_index`（[agent_id, description] 子串）保留；name_index（individual_name 全匹配）已删除
- **Role 搜索**：`role_name_completion`（role_name 前缀）保留；`role_name_descr_index` 索引 `[role_name, full_name, description]`（full_name 为展示文本），按 role_name 搜索统一走该全文索引；role_name_index 已删除
```

- 第 87 行 API 清单改为：创建、查询、更新描述、复制（指定 new_agent_id）、按描述搜索、名称补全、批量获取；删除 update-name / search-name
- 第 90 行代号校验改为 `agent_id`/`role_name`
- 「客观设定-身份标识」描述中的「个体名称」更新说明（个体名称职责由 agent_id 承接）

- [ ] **Step 3: 确认 plan 文档无需修改**

Run: `grep -n "individual_name\|name_index\|role_name_index\|search-name\|update-name" docs/plan/components-plan/kissbot-memory-ego.md`
Expected: 无输出（此前已确认无相关条目）

- [ ] **Step 4: Commit**

```bash
cd /home/admin/project/kissbot
git add docs/spec/memory-ego.md docs/design/components-design/kissbot-memory-ego.md
git commit -m "docs: memory-ego 搜索索引与代号说明更新（agent_id 手工指定、删除 name_index/role_name_index、SearchMetadata 含 agent_id）"
```

---

### Task 9: 全量验证

- [ ] **Step 1: cargo 全量测试**

Run: `cd /home/admin/project/kissbot && cargo test --workspace`
Expected: 全部 PASS（kissbot-api、kissbot-memory-ego 及其依赖 crate）

- [ ] **Step 2: playwright 全量 ego 相关测试**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/memory-ego-api.spec.ts tests/nexus-ego-chat-store.spec.ts tests/agent-commands.spec.ts --reporter=line`
Expected: 全部 PASS

- [ ] **Step 3: 验收清单核对**

- create 传 agent_id 成功；重复 agent_id 返回 409 / AgentAlreadyExists
- `/agent/search-name`、`/agent/update-name`、`/role/search-name`、`/role/remove`、`/role/rename` 路由已删除（`grep -n` 确认 api.rs 无残留）
- `/agent/search-description` 可命中 agent_id 或 description；`/agent/name-completion` 按 agent_id 前缀补全
- `cargo build --workspace` 无警告（无未使用 import / 死代码）
