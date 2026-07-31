# agent 管理 API 与测试实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 kissbot-agent 实现管理 API（HTTP 修改配置）+ 两个 playwright 自动化测试（管理 API 测试、/admin 与 /model 管理命令测试）+ 修正 WebMessengerRepo 的 user/group id 规范 + 配置同步到 script 与模板。

**Architecture:** agent 管理 API 用 axum 0.7（沿用 channel-web 模式），`HttpServer` 重写为 8 端点 Router，`X-Api-Key` 鉴权（`security.admin_api_key`）；`ConfigManager` 补 `add_provider`/`remove_provider`/`set_default_model`/`nexus_snapshot` 写方法并落盘。WebMessengerRepo id 改为规范 `u{seq}`/`g{seq}`（u1/u2/u3、g1/g2）。测试沿用 playwright（`test/tests/`），helpers 加 agent 启动，global-setup 构建 kissbot-agent；`resetWorkspace()` 清空 test/workspace 并从 workspace-template 重载（含 agent-data）。

**Tech Stack:** Rust 2024 / axum 0.7 / tokio / reqwest / serde；TypeScript / Playwright。

**Spec:** `docs/superpowers/specs/2026-07-31-agent-admin-api-and-tests-design.md`

## Global Constraints

- 工作目录：`/home/admin/project/kissbot`；Rust crate 各自独立（无 workspace）：`cd kissbot-agent && cargo test`、`cd kissbot-channel-web && cargo test`
- 不删除代码中的注释；文本 UTF-8、`\n`
- 提交 comment 用中文，包含该提交所有改动内容
- 读写文件必须用 Read/Write/Edit 工具，禁止 sed/python 改文件
- 配置结构体不加 `#[serde(default)]`；Option 字段 `#[serde(skip_serializing_if = "Option::is_none")]`
- playwright 测试命令：`cd test && npx playwright test tests/<spec>.ts`（global-setup 自动构建 channel-web/cli/agent）
- id 规范：user_id = `u{seq}`、group_id = `g{seq}`、admin 固定 `"admin"`、admin-user 单聊组 `a_{user_id}`
- 现有基线：`channel-cli.spec.ts` 9/9 通过（已验证）；改 id 后 4 个 channel spec 必须仍通过

---

### Task 1: WebMessengerRepo id 规范修正 + 现有 spec 同步

**Files:**
- Modify: `test/workspace-template/channel-web-repo.json`、`test/workspace/channel-web-repo.json`、`script/template/channel-web-repo.json`（三份内容一致）
- Modify: `test/tests/channel-cli.spec.ts`、`test/tests/channel-web-api.spec.ts`、`test/tests/channel-web-client.spec.ts`、`test/tests/channel-web-ui.spec.ts`
- Test: `cd test && npx playwright test tests/channel-cli.spec.ts tests/channel-web-api.spec.ts tests/channel-web-client.spec.ts`

**Interfaces:**
- Consumes: 现有测试与 repo 文件
- Produces: 规范 id 的 channel-web-repo（u1/u2/u3、g1/g2、next_user_seq=4、next_group_seq=3）；u3 静态存在，动态创建从 u4 开始（channel-web-api.spec.ts TC-12/13/16 断言改为 u4）。Task 4/5 的 agent 测试依赖此布局（u1=agent 绑定、u2=初始管理员、u3=admin 测试对象）。

**说明：** 动态创建用户从 `u4` 开始（`next_user_seq=4`），所以 `channel-web-api.spec.ts` 中 TC-11 创建的动态用户 id 变为 `u4`，TC-12/13/16 的 `u3` 断言同步改为 `u4`。

- [ ] **Step 1: 改写 3 个 channel-web-repo.json**

内容（三份文件完全一致）：

```json
{
  "messenger_id": "web",
  "admin_name": "管理员",
  "users": {
    "u1": { "user_id": "u1", "user_name": "助手小A" },
    "u2": { "user_id": "u2", "user_name": "助手小B" },
    "u3": { "user_id": "u3", "user_name": "助手小C" }
  },
  "groups": {
    "g1": { "group_id": "g1", "group_name": "开发组", "members": ["admin", "u1", "u2", "u3"] },
    "g2": { "group_id": "g2", "group_name": "项目X", "members": ["u1", "u2"] }
  },
  "next_user_seq": 4,
  "next_group_seq": 3
}
```

- [ ] **Step 2: 更新 test/tests/channel-cli.spec.ts**

替换映射（Edit 工具逐处）：
- `'user-1'` → `'u1'`、`'user-2'` → `'u2'`（spawnCli 参数与注释）
- `'dev-team'` → `'g1'`、`'project-x'` → `'g2'`
- 正则：`/<< \[admin:dev-team\]/` → `/<< \[admin:g1\]/`、`/<< leave group: project-x @ web/` → `/<< leave group: g2 @ web/`、`/<< join group: project-x @ web/` → `/<< join group: g2 @ web/`
- 断言：`msg.user_id` `'user-1'` → `'u1'`、`msg.group_id` `'dev-team'` → `'g1'`
- TC-09 单聊组：`group_id: 'a_user-1'` → `'a_u1'`，正则 `/<< \[admin:a_user-1\]/` → `/<< \[admin:a_u1\]/`

- [ ] **Step 3: 更新 test/tests/channel-web-api.spec.ts**

- TC-01：`users['user-1']` → `users['u1']`、`user_id` `'user-1'` → `'u1'`、`users['user-2']` → `users['u2']`、`groups['dev-team']` → `groups['g1']`、`groups['project-x']` → `groups['g2']`
- TC-03/TC-05：`group_id: 'dev-team'` → `'g1'`（含 `/api/messages/recent?group_id=dev-team` → `g1`）
- TC-06/TC-07：`member_ids: ['user-1']` → `['u1']`；`g.members` `toContain('user-1')` → `'u1'`
- TC-09：`add_ids: ['user-2']` → `['u2']`；断言 `['user-1', 'user-2']` → `['u1', 'u2']`
- TC-10：`remove_ids: ['user-2']` → `['u2']`；断言 `['user-1']` → `['u1']`
- TC-12：`toHaveProperty('u3')` → `'u4'`（三处：`'u4'`、`u4.user_id`、`user_name` 保持 `'助手小C'`）
- TC-13/TC-16：`user_id: 'u3'` → `'u4'`、`users.u3` → `users.u4`、`not.toHaveProperty('u3')` → `'u4'`

- [ ] **Step 4: 更新 test/tests/channel-web-client.spec.ts**

映射同 channel-cli：`'user-1'`→`'u1'`、`'user-2'`→`'u2'`、`'dev-team'`→`'g1'`、`'project-x'`→`'g2'`（spawnCli 参数、正则 `/<< \[admin:dev-team\]/` 等、注释）。注意 UI 交互选择器按 user_name（"助手小A"/"助手小B"）不变，只改 id 字符串。

- [ ] **Step 5: 更新 test/tests/channel-web-ui.spec.ts**

- 消息发送 `group_id: 'dev-team'` → `'g1'`
- 群组列表断言文本 `'dev-team 和 project-x'` → `'g1 和 g2'`（若断言的是 id 字符串；若是 group_name 则不动——按实际断言内容判断）
- members 断言 `'admin, user-1, user-2'` → `'admin, u1, u2'`

- [ ] **Step 6: 运行受影响测试确认通过**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/channel-cli.spec.ts tests/channel-web-api.spec.ts tests/channel-web-client.spec.ts`
Expected: 全部 PASS（channel-cli 9 + channel-web-api 18 + channel-web-client 若干）。若 `channel-web-api` TC-12 失败，检查 `next_user_seq` 是否 4 且断言为 u4。

（channel-web-ui.spec.ts 需 UI dev server，若环境不便可单独跑：`npx playwright test tests/channel-web-ui.spec.ts`，也要求 PASS。）

- [ ] **Step 7: 提交**

```bash
cd /home/admin/project/kissbot && git add test/workspace-template/channel-web-repo.json test/workspace/channel-web-repo.json script/template/channel-web-repo.json test/tests/channel-cli.spec.ts test/tests/channel-web-api.spec.ts test/tests/channel-web-client.spec.ts test/tests/channel-web-ui.spec.ts && git commit -m "test(channel-web): WebMessengerRepo id 规范修正（user-1/user-2 → u1/u2/u3，dev-team/project-x → g1/g2），同步 4 个 spec 引用；动态创建从 u4 起"
```

---

### Task 2: ConfigManager 写方法（providers CRUD + set_default_model + nexus_snapshot）

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`
- Test: `kissbot-agent/src/config_manager.rs`（`#[cfg(test)]` 段）

**Interfaces:**
- Consumes: 现有 `ProviderConfig`/`ProviderModel`/`NexusRepo`/`save_nexus`/`ArcSwapHashMap`
- Produces: `add_provider(&self, cfg: ProviderConfig) -> Result<()>`、`remove_provider(&self, name: &str) -> Result<()>`、`set_default_model(&self, pm: ProviderModel) -> Result<()>`、`nexus_snapshot(&self) -> NexusRepo`。Task 3 的 http_server.rs 依赖这四个方法。

- [ ] **Step 1: 写失败测试**

在 `config_manager.rs` 测试段末尾（`resolve_effective_config_inherits_provider_defaults` 之后）追加：

```rust
    #[tokio::test]
    async fn provider_crud_and_default_set() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        // add_provider → resolve 可见
        manager.add_provider(sample_provider("deepseek")).await.unwrap();
        let eff = manager.resolve_effective_config(&ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() }).await;
        assert!(eff.is_none(), "provider 无 models 时 model 不存在返回 None");
        // 带 models 的 provider
        let mut provider = sample_provider("openai");
        {
            let map = Arc::make_mut(&mut provider.models);
            map.insert("gpt-4o".into(), ArcSwap::new(Arc::new(ModelConfig {
                model: "gpt-4o".into(), max_tokens: None, temperature: None,
                timeout_secs: None, retry_count: None, context_length: None,
            })));
        }
        manager.add_provider(provider).await.unwrap();
        // 重名报错
        let err = manager.add_provider(sample_provider("openai")).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        // set_default_model → getter 可见 + 落盘
        let pm = ProviderModel { provider: "openai".into(), model: "gpt-4o".into() };
        manager.set_default_model(pm.clone()).await.unwrap();
        assert_eq!(manager.default_model().await, pm);
        // remove_provider → resolve 返回 None；不存在报错
        manager.remove_provider("openai").await.unwrap();
        assert!(manager.resolve_effective_config(&pm).await.is_none());
        let err = manager.remove_provider("nope").await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
        // nexus_snapshot 反映变更
        let snap = manager.nexus_snapshot().await;
        assert_eq!(*snap.default_model, pm);
        assert!(!snap.providers.contains_key("openai"));
    }
```

（注：`sample_provider` 已在 Task 1 轮次中定义于测试段；`ProviderModel` 已 derive `PartialEq`。）

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test provider_crud_and_default_set 2>&1 | tail -10`
Expected: 编译失败，报 `add_provider` / `remove_provider` / `set_default_model` / `nexus_snapshot` 不存在。

- [ ] **Step 3: 实现**

在 `config_manager.rs` 中：

(a) providers 段（`resolve_effective_config` 之后）追加：

```rust
    // ---------- providers CRUD（管理 API 使用，落盘） ----------
    /// 添加 provider（重名报 ConfigNotFound），落盘
    pub async fn add_provider(&self, cfg: ProviderConfig) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.providers);
            if map.contains_key(&cfg.name.to_string()) {
                return Err(Error::ConfigNotFound(format!("provider 已存在: {}", cfg.name)));
            }
            map.insert(cfg.name.to_string(), ArcSwap::new(Arc::new(cfg)));
        }
        self.save_nexus().await
    }
    /// 删除 provider（不存在报 ConfigNotFound），落盘
    pub async fn remove_provider(&self, name: &str) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            let map = Arc::make_mut(&mut repo.providers);
            if !map.contains_key(name) {
                return Err(Error::ConfigNotFound(format!("provider 不存在: {}", name)));
            }
            map.remove(name);
        }
        self.save_nexus().await
    }
    /// 返回 NexusRepo 快照（管理 API GET /config 使用）
    pub async fn nexus_snapshot(&self) -> NexusRepo {
        self.nexus_repo.read().await.clone()
    }
```

(b) default 读写段（`default_model` getter 之后）追加：

```rust
    /// 设置默认模型（(provider, model) 打包），落盘
    pub async fn set_default_model(&self, pm: ProviderModel) -> Result<()> {
        {
            let mut repo = self.nexus_repo.write().await;
            Arc::make_mut(&mut repo).default_model = Arc::new(pm);
        }
        self.save_nexus().await
    }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test 2>&1 | tail -5`
Expected: 全部 PASS（新增 1 个 + 原有 15 个）。

- [ ] **Step 5: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-agent/src/config_manager.rs && git commit -m "feat(agent): ConfigManager 增加 providers CRUD（add_provider/remove_provider）、set_default_model、nexus_snapshot，均落盘（管理 API 用）"
```

---

### Task 3: HttpServer 重写为 axum 管理 API（8 端点 + 鉴权）

**Files:**
- Modify: `kissbot-agent/Cargo.toml`（加 axum）
- Modify: `kissbot-agent/src/http_server.rs`（全量重写）
- Test: `kissbot-agent/src/http_server.rs`（`#[cfg(test)]` 段）

**Interfaces:**
- Consumes: Task 2 的 `add_provider`/`remove_provider`/`set_default_model`/`nexus_snapshot`；现有 `add_channel`/`remove_channel`/`add_admin`/`remove_admin`；`kissbot_security` 的 `admin_api_key`
- Produces: `HttpServer::new(config, host, port)`（生产，从 SecurityConfig 读 admin_api_key）+ `HttpServer::with_admin_key(config, admin_api_key, host, port)`（可测注入）；`start()` 用 axum::serve。main.rs 调用点不变。

**说明：** `kissbot_config::Config::get()` 是 OnceLock 全局单例（测试环境无 KISSBOT_CONFIG 会 panic），所以鉴权 key 在 `HttpServer::new` 中读取并缓存为字段，测试用 `with_admin_key` 注入，避免触发单例。

- [ ] **Step 1: Cargo.toml 加 axum**

`kissbot-agent/Cargo.toml` dependencies 追加：

```toml
axum = { version = "0.7", features = ["json"] }
```

- [ ] **Step 2: 写失败测试（http_server.rs `#[cfg(test)]`）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use kissbot_api::ArcSwapHashMap;
    use std::collections::HashSet;
    use tempfile::tempdir;

    fn test_manager(dir: &tempfile::TempDir) -> Arc<ConfigManager> {
        Arc::new(ConfigManager {
            agent_config: crate::config_manager::tests::agent_config(dir.path().to_str().unwrap()),
            nexus_repo: Arc::new(tokio::sync::RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(tokio::sync::RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        })
    }

    fn test_provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: Arc::new(name.into()),
            provider_type: "openai".into(),
            base_url: "https://api.example.com".into(),
            api_key: "sk-test".into(),
            default_context_length: 65536,
            default_max_tokens: 4096,
            default_temperature: 0.7,
            default_timeout_secs: 60,
            default_retry_count: 3,
            models: Arc::new(ArcSwapHashMap::new()),
        }
    }

    async fn send(app: axum::Router, method: &str, uri: &str, key: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if !key.is_empty() {
            builder = builder.header("x-api-key", key);
        }
        let req = if let Some(b) = body {
            builder.header("content-type", "application/json")
                .body(Body::from(b.to_string())).unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({})))
    }

    #[tokio::test]
    async fn config_endpoints_auth_and_crud() {
        let dir = tempdir().unwrap();
        let manager = test_manager(&dir);
        let server = HttpServer::with_admin_key(manager.clone(), "admin-key-123".into(), "127.0.0.1".into(), 0);
        let app = server.build_router();

        // 无 key → 401
        let (status, _) = send(app.clone(), "GET", "/config", "", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // 错误 key → 401
        let (status, _) = send(app.clone(), "GET", "/config", "wrong", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // GET /config 初始快照
        let (status, body) = send(app.clone(), "GET", "/config", "admin-key-123", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"]["providers"].is_object());
        assert!(body["data"]["default_model"].is_object());

        // POST /config/providers 添加
        let (status, body) = send(app.clone(), "POST", "/config/providers", "admin-key-123",
            Some(serde_json::to_value(test_provider("deepseek")).unwrap())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        // 重名 → 失败
        let (status, body) = send(app.clone(), "POST", "/config/providers", "admin-key-123",
            Some(serde_json::to_value(test_provider("deepseek")).unwrap())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false);

        // POST /config/default
        let (status, body) = send(app.clone(), "POST", "/config/default", "admin-key-123",
            Some(serde_json::json!({ "provider": "deepseek", "model": "deepseek-4-flash" }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(manager.default_model().await.model, "deepseek-4-flash");

        // POST /config/providers/remove
        let (status, body) = send(app.clone(), "POST", "/config/providers/remove", "admin-key-123",
            Some(serde_json::json!({ "name": "deepseek" }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);

        // POST /config/channels 添加 + admins
        let (status, body) = send(app.clone(), "POST", "/config/channels", "admin-key-123",
            Some(serde_json::json!({
                "channel_id": "web-main", "ws_url": "ws://127.0.0.1:8201",
                "admins": [], "default_bind_user": null, "enabled_by_default": true
            }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        let (status, body) = send(app.clone(), "POST", "/config/admins", "admin-key-123",
            Some(serde_json::json!({ "channel_id": "web-main", "messenger_id": "web", "user_id": "u2" }))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        // 落盘验证
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("nexus.json")).unwrap()).unwrap();
        assert!(saved["providers"].is_object());
        assert_eq!(saved["default_model"]["model"], "deepseek-4-flash");
    }
}
```

（注：`agent_config` 是 config_manager 测试模块的私有 helper——本测试直接引用 `crate::config_manager::tests::agent_config`，需确认该模块可见（同 crate 内 `pub(crate)` 或直接路径可访问）。若不可访问，在 `test_manager` 内联构造 AgentConfig。）

- [ ] **Step 3: 运行测试确认失败**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test config_endpoints_auth_and_crud 2>&1 | tail -15`
Expected: 编译失败（`build_router` / `with_admin_key` 不存在）或测试失败。

- [ ] **Step 4: 实现（http_server.rs 全量重写）**

```rust
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;
use tracing::info;

use crate::config_manager::{ChannelConfig, ChannelUser, ConfigManager, ProviderConfig, ProviderModel};
use crate::types::Result;

/// 管理 REST API 服务器（axum，X-Api-Key 鉴权，security.admin_api_key）
pub struct HttpServer {
    #[allow(dead_code)]
    config: Arc<ConfigManager>,
    admin_api_key: String,
    host: String,
    port: u16,
}

// ========== 请求 DTO ==========

#[derive(Deserialize)]
struct NameRequest {
    name: String,
}

#[derive(Deserialize)]
struct ChannelIdRequest {
    channel_id: String,
}

#[derive(Deserialize)]
struct AddAdminRequest {
    channel_id: String,
    messenger_id: String,
    user_id: String,
}

#[derive(Deserialize)]
struct RemoveAdminRequest {
    channel_id: String,
    messenger_id: String,
    user_id: String,
}

impl HttpServer {
    /// 生产构造：admin_api_key 从 kissbot_security 全局配置读取
    pub fn new(config: Arc<ConfigManager>, host: String, port: u16) -> Self {
        let admin_api_key = kissbot_security::SecurityConfig::get().admin_api_key.clone();
        Self::with_admin_key(config, admin_api_key, host, port)
    }

    /// 测试/注入构造：显式传入 admin_api_key，避免触发 kissbot-config 全局单例
    pub fn with_admin_key(config: Arc<ConfigManager>, admin_api_key: String, host: String, port: u16) -> Self {
        Self { config, admin_api_key, host, port }
    }

    fn build_router(&self) -> Router {
        let config = self.config.clone();
        let key = self.admin_api_key.clone();
        Router::new()
            .route("/config", get(get_config))
            .route("/config/providers", post(add_provider))
            .route("/config/providers/remove", post(remove_provider))
            .route("/config/default", post(set_default))
            .route("/config/channels", post(add_channel))
            .route("/config/channels/remove", post(remove_channel))
            .route("/config/admins", post(add_admin))
            .route("/config/admins/remove", post(remove_admin))
            .with_state(AppState { config, key })
    }

    /// 启动 HTTP 服务器（阻塞，在协程中运行）
    pub async fn start(&self) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        let listener = TcpListener::bind(&addr).await
            .map_err(|e| crate::types::Error::IoError(e.to_string()))?;
        info!("管理 API 服务器启动: {}", addr);
        axum::serve(listener, self.build_router()).await
            .map_err(|e| crate::types::Error::IoError(e.to_string()))?;
        Ok(())
    }
}

#[derive(Clone)]
struct AppState {
    config: Arc<ConfigManager>,
    key: String,
}

/// 鉴权：X-Api-Key 与 admin_api_key 比对
fn check_api_key(headers: &HeaderMap, expected: &str) -> bool {
    headers.get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|k| k == expected)
        .unwrap_or(false)
}

fn unauthorized() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::UNAUTHORIZED, Json(json!({ "success": false, "error": "unauthorized" })))
}

fn ok<T: serde::Serialize>(data: T) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(json!({ "success": true, "data": data })))
}

fn fail(e: crate::types::Error) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(json!({ "success": false, "error": e.to_string() })))
}

// ========== Handlers ==========

async fn get_config(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    let snap = state.config.nexus_snapshot().await;
    ok(snap)
}

async fn add_provider(State(state): State<AppState>, headers: HeaderMap, Json(cfg): Json<ProviderConfig>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.add_provider(cfg).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn remove_provider(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<NameRequest>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.remove_provider(&req.name).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn set_default(State(state): State<AppState>, headers: HeaderMap, Json(pm): Json<ProviderModel>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.set_default_model(pm).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn add_channel(State(state): State<AppState>, headers: HeaderMap, Json(ch): Json<ChannelConfig>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.add_channel(ch).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn remove_channel(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<ChannelIdRequest>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.remove_channel(&req.channel_id).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn add_admin(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<AddAdminRequest>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    let admin = ChannelUser {
        messenger_id: Arc::new(req.messenger_id),
        user_id: Arc::new(req.user_id),
    };
    match state.config.add_admin(&req.channel_id, &admin).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

async fn remove_admin(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<RemoveAdminRequest>) -> impl IntoResponse {
    if !check_api_key(&headers, &state.key) {
        return unauthorized();
    }
    match state.config.remove_admin(&req.channel_id, &req.messenger_id, &req.user_id).await {
        Ok(()) => ok(json!({})),
        Err(e) => fail(e),
    }
}

#[cfg(test)]
mod tests {
    // （Step 2 的测试代码，含 test_manager / test_provider / send helper 与用例）
}
```

（注：`http_body_util` 需要加入依赖（`BodyExt` 用于测试读取响应体）。在 `kissbot-agent/Cargo.toml` 的 `[dev-dependencies]` 加 `http-body-util = "0.1"`。若 axum 测试编译报 `oneshot` 不可用，确认 axum feature（默认含 http）。）

- [ ] **Step 5: 运行测试确认通过**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo test 2>&1 | tail -10`
Expected: 全部 PASS（新增 1 个 + 原有 16 个）。

- [ ] **Step 6: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-agent/Cargo.toml kissbot-agent/Cargo.lock kissbot-agent/src/http_server.rs && git commit -m "feat(agent): HttpServer 重写为 axum 管理 API（GET /config + providers/default/channels/admins 增删 8 端点，X-Api-Key 鉴权 security.admin_api_key）"
```

---

### Task 4: 测试基建与配置同步（agent 启动 helper、workspace-template、script 模板、agent-reset 适配）

**Files:**
- Modify: `test/tests/helpers/server.ts`、`test/global-setup.ts`
- Create: `test/workspace-template/agent-data/nexus.json`、`test/workspace-template/agent-data/station.json`
- Modify: `script/template/nexus.json`、`script/agent-reset.mjs`
- Test: 构建验证 + agent 启动冒烟

**Interfaces:**
- Consumes: Task 1 的规范 id（u1/u2/u3、g1/g2）
- Produces: `startAgent(cwd): ChildProcess`、`stopAgent(proc?)`（helpers/server.ts）；`test/workspace-template/agent-data/nexus.json`（Part 3 内容，供 resetWorkspace 复制）；`script/template/nexus.json` 配 channel+admin；`agent-reset.mjs` 适配 providers 格式。Task 5 的 playwright 测试依赖这些。

- [ ] **Step 1: helpers/server.ts 加 startAgent/stopAgent**

在 `test/tests/helpers/server.ts` 追加：

```ts
const AGENT_BINARY = join(REPO_ROOT, 'kissbot-agent', 'target', 'debug', 'kissbot-agent');
const AGENT_MGMT_PORT = 9090;

export function startAgent(cwd: string): ChildProcess {
  const proc = spawn(AGENT_BINARY, [], {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'info', KISSBOT_CONFIG: join(cwd, 'config.json') },
  });
  proc.stdout?.on('data', (d) => process.stdout.write(`[agent] ${d}`));
  proc.stderr?.on('data', (d) => process.stderr.write(`[agent:err] ${d}`));
  return proc;
}

export function stopAgent(proc?: ChildProcess): void {
  if (proc && !proc.killed) {
    proc.kill('SIGTERM');
  }
}

export const AGENT_MGMT_PORT = AGENT_MGMT_PORT_VALUE;
```

（`AGENT_MGMT_PORT_VALUE` 为数字 9090；或直接导出 `export const AGENT_MGMT_PORT = 9090;`）

- [ ] **Step 2: global-setup.ts 构建 kissbot-agent**

在 `test/global-setup.ts` 的构建段追加：

```ts
  console.log('[global-setup] Building kissbot-agent...');
  execSync('cargo build --manifest-path ../kissbot-agent/Cargo.toml', {
    stdio: 'inherit',
    cwd: __dirname,
  });
```

- [ ] **Step 3: 创建 test/workspace-template/agent-data/nexus.json 与 station.json**

`test/workspace-template/agent-data/nexus.json`：

```json
{
  "channels": {
    "web-main": {
      "channel_id": "web-main",
      "ws_url": "ws://127.0.0.1:8201",
      "admins": [{ "messenger_id": "web", "user_id": "u2" }],
      "default_bind_user": { "messenger_id": "web", "user_id": "u1" },
      "enabled_by_default": true
    }
  },
  "providers": {
    "deepseek": {
      "name": "deepseek",
      "provider_type": "openai",
      "base_url": "https://api.deepseek.com",
      "api_key": "",
      "default_context_length": 65536,
      "default_max_tokens": 4096,
      "default_temperature": 0.7,
      "default_timeout_secs": 60,
      "default_retry_count": 3,
      "models": {
        "deepseek-4-flash": { "model": "deepseek-4-flash" }
      }
    }
  },
  "memory_structs": {},
  "stations": {},
  "default_agent_id": "",
  "default_role": "",
  "default_model": { "provider": "deepseek", "model": "deepseek-4-flash" }
}
```

`test/workspace-template/agent-data/station.json`：

```json
{}
```

- [ ] **Step 4: script/template/nexus.json 配测试用 channel + admin**

改为与 Step 3 相同的 nexus.json 内容（`api_key` 保持 `""`，由 `agent-reset.mjs` 注入）。

- [ ] **Step 5: script/agent-reset.mjs 适配 providers 嵌套格式**

`agent-reset.mjs` 中注入 api_key 的逻辑改为按 provider 名注入：

```js
  // 2. 注入 api key（名称 = provider 配置名）
  if (existsSync(keyFile)) {
    const keys = JSON.parse(await readFile(keyFile, 'utf8'));
    for (const [name, key] of Object.entries(keys)) {
      if (nexus.providers[name]) {
        nexus.providers[name].api_key = key;
        console.log(`  ✓ ${name}: api_key 已注入`);
      } else {
        console.warn(`  ⚠ ${name}: nexus.json 中没有名为 ${name} 的 provider，跳过（可先在 template/nexus.json 中添加）`);
      }
    }
    // 提示没有注入 key 的 provider
    for (const [name, provider] of Object.entries(nexus.providers)) {
      if (!provider.api_key) {
        console.warn(`  ⚠ provider ${name} 未配置 api_key（key.local.json 中无对应条目）`);
      }
    }
  } else {
    console.warn(`  ⚠ ${keyFile} 不存在，api_key 将保持为空`);
  }
```

（`nexus.models[name]` → `nexus.providers[name]`；文件顶部注释同步更新为"按 provider 名注入"。）

- [ ] **Step 6: 验证构建与 agent 启动冒烟**

Run: `cd /home/admin/project/kissbot/kissbot-agent && cargo build 2>&1 | tail -3`
Expected: 构建成功。

手动冒烟（可选，需 channel-web 运行）：
```bash
cd /home/admin/project/kissbot/test && npx playwright test tests/channel-cli.spec.ts 2>&1 | tail -3
```
Expected: 9/9 PASS（确认 Task 1 改动未破坏基线）。

- [ ] **Step 7: 提交**

```bash
cd /home/admin/project/kissbot && git add test/tests/helpers/server.ts test/global-setup.ts test/workspace-template/agent-data/nexus.json test/workspace-template/agent-data/station.json script/template/nexus.json script/agent-reset.mjs && git commit -m "test(agent): 测试基建与配置同步——startAgent helper、global-setup 构建 agent、workspace-template/agent-data 初始 repo、script/template nexus 配 channel+admin、agent-reset 适配 providers 嵌套格式"
```

---

### Task 5: playwright agent 集成测试（agent-config-api + agent-commands）

**Files:**
- Create: `test/tests/agent-config-api.spec.ts`、`test/tests/agent-commands.spec.ts`
- Test: `cd test && npx playwright test tests/agent-config-api.spec.ts tests/agent-commands.spec.ts`

**Interfaces:**
- Consumes: Task 2 的管理 API、Task 4 的 startAgent/stopAgent 与模板 repo（u1/u2/u3、g1/g2）

**说明：** 两个 spec 各自 beforeAll 起环境（playwright workers=1 串行）。agent 管理端口 9090。

- [ ] **Step 1: 写 test/tests/agent-config-api.spec.ts**

```typescript
import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startAgent, stopAgent, waitForPort } from './helpers/server';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { readFileSync } from 'fs';

const BASE = 'http://127.0.0.1:9090';
const ADMIN_KEY = 'admin-key-123';
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');
const NEXUS_FILE = join(WORKSPACE, 'agent-data', 'nexus.json');

let agent: ChildProcess;

async function apiGet(request: APIRequestContext, path: string, key = ADMIN_KEY) {
  return (await request.get(`${BASE}${path}`, {
    headers: { 'X-Api-Key': key },
  })).json();
}

async function apiPost(request: APIRequestContext, path: string, body: unknown, key = ADMIN_KEY) {
  return (await request.post(`${BASE}${path}`, {
    headers: { 'X-Api-Key': key, 'Content-Type': 'application/json' },
    data: body,
  })).json();
}

test.describe.serial('agent 配置管理 API 测试（HTTP 修改配置）', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    agent = startAgent(WORKSPACE);
    await waitForPort(9090, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopAgent(agent);
  });

  test('TC-01: GET /config 读初始快照', async ({ request }) => {
    const resp = await apiGet(request, '/config');
    expect(resp.success).toBe(true);
    expect(resp.data.providers.deepseek).toBeTruthy();
    expect(resp.data.default_model).toEqual({ provider: 'deepseek', model: 'deepseek-4-flash' });
  });

  test('TC-02: POST /config/providers 添加并落盘', async ({ request }) => {
    const resp = await apiPost(request, '/config/providers', {
      name: 'anthropic', provider_type: 'anthropic',
      base_url: 'https://api.anthropic.com', api_key: '',
      default_context_length: 200000, default_max_tokens: 8192,
      default_temperature: 0.7, default_timeout_secs: 60, default_retry_count: 3,
      models: { 'claude-sonnet-4': { model: 'claude-sonnet-4' } },
    });
    expect(resp.success).toBe(true);
    // GET 验证
    const info = await apiGet(request, '/config');
    expect(info.data.providers.anthropic).toBeTruthy();
    // nexus.json 落盘验证
    const saved = JSON.parse(readFileSync(NEXUS_FILE, 'utf8'));
    expect(saved.providers.anthropic).toBeTruthy();
  });

  test('TC-03: POST /config/default 修改默认模型', async ({ request }) => {
    const resp = await apiPost(request, '/config/default', {
      provider: 'anthropic', model: 'claude-sonnet-4',
    });
    expect(resp.success).toBe(true);
    const info = await apiGet(request, '/config');
    expect(info.data.default_model).toEqual({ provider: 'anthropic', model: 'claude-sonnet-4' });
    const saved = JSON.parse(readFileSync(NEXUS_FILE, 'utf8'));
    expect(saved.default_model.model).toBe('claude-sonnet-4');
  });

  test('TC-04: POST /config/channels 与 /config/admins', async ({ request }) => {
    const ch = await apiPost(request, '/config/channels', {
      channel_id: 'web-2', ws_url: 'ws://127.0.0.1:8201',
      admins: [], default_bind_user: null, enabled_by_default: false,
    });
    expect(ch.success).toBe(true);
    const adm = await apiPost(request, '/config/admins', {
      channel_id: 'web-2', messenger_id: 'web', user_id: 'u3',
    });
    expect(adm.success).toBe(true);
    const info = await apiGet(request, '/config');
    expect(info.data.channels['web-2']).toBeTruthy();
  });

  test('TC-05: 错误 API Key → 401', async ({ request }) => {
    const resp = await apiGet(request, '/config', 'wrong-key');
    expect(resp.success).toBe(false);
    const resp401 = await request.get(`${BASE}/config`, {
      headers: { 'X-Api-Key': 'wrong-key' },
    });
    expect(resp401.status()).toBe(401);
  });

  test('TC-06: 删除不存在的 provider → 失败', async ({ request }) => {
    const resp = await apiPost(request, '/config/providers/remove', { name: 'nope' });
    expect(resp.success).toBe(false);
    expect(resp.error).toBeTruthy();
  });
});
```

- [ ] **Step 2: 写 test/tests/agent-commands.spec.ts**

```typescript
import { test, expect } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, startAgent, stopAgent, waitForPort } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

let backend: ChildProcess;
let agent: ChildProcess;
let cliAdmin: SpawnedCli;   // u2：初始管理员
let cliUser: SpawnedCli;    // u3：admin/unadmin 测试对象

// 等待 cli 输出（返回 Promise，用于"不应出现"的断言配合超时）
function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

test.describe.serial('agent 管理命令测试（/admin 与 /model，cli 经 channel-web 发送）', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
    agent = startAgent(WORKSPACE);
    await waitForPort(9090, '127.0.0.1', 30000);
    // 等待 agent 完成 channel 连接与绑定
    await sleep(2000);
    cliAdmin = spawnCli(['web', 'u2', 'g1', './downloads'], WORKSPACE);
    await cliAdmin.waitForOutput(/bound\./);
    cliUser = spawnCli(['web', 'u3', 'g1', './downloads'], WORKSPACE);
    await cliUser.waitForOutput(/bound\./);
  });

  test.afterAll(() => {
    if (cliAdmin) cliAdmin.proc.kill();
    if (cliUser) cliUser.proc.kill();
    stopAgent(agent);
    stopBackend(backend);
  });

  test('TC-01: 非管理员（u3）发送 /model 被忽略', async ({ request }) => {
    cliUser.stdin('/model deepseek deepseek-4-flash');
    // 等待一段时间确认没有 agent 回复
    await sleep(3000);
    expect(cliUser.hasOutput(/切换模型|模型调用失败|不存在/)).toBe(false);
  });

  test('TC-02: 管理员（u2）发送 /admin web u3 添加管理权限', async () => {
    cliAdmin.stdin('/admin web u3');
    await cliAdmin.waitForOutput(/✅ 已添加管理权限: web \/ u3/, 10000);
  });

  test('TC-03: u3 成为管理员后发送 /model 生效', async () => {
    cliUser.stdin('/model deepseek deepseek-4-flash');
    await cliUser.waitForOutput(/✅ 已切换模型为: deepseek\/deepseek-4-flash/, 15000);
  });

  test('TC-04: 管理员（u2）发送 /unadmin web u3 移除权限', async () => {
    cliAdmin.stdin('/unadmin web u3');
    await cliAdmin.waitForOutput(/✅ 已移除管理权限: web \/ u3/, 10000);
  });

  test('TC-05: 移除权限后 u3 发送 /model 再被忽略', async () => {
    cliUser.stdin('/model deepseek deepseek-4-flash');
    await sleep(3000);
    expect(cliUser.hasOutput(/切换模型/)).toBe(false);
  });
});
```

- [ ] **Step 3: 运行两个新 spec**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/agent-config-api.spec.ts tests/agent-commands.spec.ts 2>&1 | tail -20`
Expected: 全部 PASS（agent-config-api 6 + agent-commands 5）。

若 TC 失败，按以下排查：
- agent 未连上 channel-web：看 agent 日志（[agent] 前缀输出），确认 nexus.json channel ws_url 与 channel-web 端口一致
- 管理命令未执行：确认 u2 在 nexus.json `admins` 中（`{ messenger_id: "web", user_id: "u2" }`）、agent 的 bound_channels 含该 channel（default_bind_user=u1 只影响绑定过滤，admins 匹配走消息身份）
- cli 收不到回复：确认 u2/u3 在 g1 群组成员中（channel-web-repo.json）

- [ ] **Step 4: 全量回归**

Run: `cd /home/admin/project/kissbot/test && npx playwright test tests/channel-cli.spec.ts tests/channel-web-api.spec.ts tests/channel-web-client.spec.ts tests/agent-config-api.spec.ts tests/agent-commands.spec.ts 2>&1 | tail -10`
Expected: 全部 PASS（基线 + 新增）。

- [ ] **Step 5: 提交**

```bash
cd /home/admin/project/kissbot && git add test/tests/agent-config-api.spec.ts test/tests/agent-commands.spec.ts && git commit -m "test(agent): 新增管理 API 测试（HTTP 修改配置 6 TC）与管理命令测试（/admin、/model 权限链路 5 TC，cli 经 channel-web 发送）"
```

---

## Self-Review 记录

**1. Spec 覆盖：**
- WebMessengerRepo id 规范修正 + 4 spec 同步 → Task 1
- ConfigManager 写方法（add_provider/remove_provider/set_default_model/nexus_snapshot）→ Task 2
- HttpServer axum 8 端点 + X-Api-Key 鉴权 → Task 3
- 测试基建（startAgent/global-setup/workspace-template agent-data）→ Task 4
- script 配置同步（template/nexus.json、agent-reset.mjs providers 格式）→ Task 4
- agent-config-api.spec.ts（HTTP 修改配置 + 落盘验证 + 401）→ Task 5
- agent-commands.spec.ts（/admin、/model 权限链路 + 非管理员忽略）→ Task 5
- resetWorkspace 清空重载（含 agent-data）→ Task 4（复用现有 resetWorkspace，workspace-template 加 agent-data 即覆盖）

**2. 占位符扫描：** 无 TBD/TODO；所有代码步骤含完整代码。

**3. 类型一致性：**
- `add_provider(ProviderConfig)` / `remove_provider(&str)` / `set_default_model(ProviderModel)` / `nexus_snapshot() -> NexusRepo`：Task 2/3 一致
- `HttpServer::with_admin_key(config, admin_api_key, host, port)` + `build_router()`：Task 3 一致
- `remove_admin(channel_id, messenger_id, user_id)`：Task 3 的 /config/admins/remove 端点一致（spec 已修正）
- `startAgent(cwd)` / `stopAgent(proc)` / `waitForPort(9090)`：Task 4/5 一致
- id 布局 u1（agent）/u2（管理员）/u3（admin 测试）：Task 1 的 repo、Task 4 的 nexus.json admins、Task 5 的 spec 一致
- 动态用户断言 u4：Task 1 的 channel-web-api TC-12/13/16 一致（next_user_seq=4）
