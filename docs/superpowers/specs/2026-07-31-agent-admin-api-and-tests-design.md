# agent 管理 API 与测试、WebMessengerRepo id 规范修正设计

## 概述

为 kissbot-agent 补齐三件事：

1. **agent 配置管理 API**（HTTP）：`HttpServer` 目前是骨架（accept 后 drop），本次按最小集实现 axum REST 端点（沿用 channel-web 模式），支持通过 HTTP 修改配置（providers / default / channels / admins），鉴权用 `security.admin_api_key`（`X-Api-Key` header）。
2. **两个自动化测试**（playwright，`test/tests/`）：
   - `agent-config-api.spec.ts`：通过 HTTP 请求修改 agent 配置（管理 API 测试）
   - `agent-commands.spec.ts`：用两个 channel-client-cli 绑定 user，连到 channel-web，由 cli 发送 agent 管理命令文本（`/admin`、`/model`）进行测试
3. **WebMessengerRepo id 规范修正**：测试用的 user/group id 从 `user-1`/`dev-team` 等改为符合代码规范的 `u{seq}` / `g{seq}`。

所有测试用配置同步写入 `script/config.json`，所有初始化的 repo 同步写入 `script/template/`；`test/workspace/` 在测试前清空并重新从 `test/workspace-template/` 加载（沿用现有 `resetWorkspace()`：rm -rf + cp -r，确保 agent-data 一并覆盖）。

## 现状与问题

- `kissbot-agent/src/http_server.rs` 只 accept TCP 连接后 drop，无任何路由；`ConfigManager` 只有读方法（`resolve_effective_config`、`default_*` getter、channels/admin CRUD），**没有 providers 增删与 default 写方法**。
- channel-web 已用 axum 0.7 实现 REST API（`X-Api-Key` 鉴权），agent 应沿用此模式。
- `test/workspace/channel-web-repo.json` 与模板中 user id 为 `user-1`/`user-2`、group id 为 `dev-team`/`project-x`，而 messenger.rs 的规范是 `add_user` 生成 `u{seq}`（`USER_ID_PREFIX="u"`）、`add_group` 生成 `g{seq}`（`GROUP_ID_PREFIX="g"`）、admin 固定 `"admin"`、admin-user 单聊组 `a_{user_id}`。
- `script/agent-reset.mjs` 还在读旧 `nexus.models[].api_key` 格式（Task 5 已把 nexus.json 迁移为 providers 嵌套，api_key 在 provider 级），已过期。
- 现有 playwright 测试框架（global-setup / helpers/server.ts）只构建并启动 channel-web 与 channel-client-cli，未涉及 agent。

## 目标结构

| 部分 | 实体 | 说明 |
|------|------|------|
| 管理 API | `HttpServer`（axum Router） | GET /config + POST 配置修改端点，X-Api-Key 鉴权 |
| ConfigManager 写方法 | `add_provider` / `remove_provider` / `set_default_model` | 落盘 save_nexus；channels/admins 增删已存在 |
| 测试 1 | `test/tests/agent-config-api.spec.ts` | HTTP 修改配置 + 验证 |
| 测试 2 | `test/tests/agent-commands.spec.ts` | cli 发 /admin、/model 管理命令 |
| 测试基建 | `helpers/server.ts` 加 startAgent/stopAgent；`global-setup.ts` 构建 agent | 环境管理 |
| id 规范 | 3 个 channel-web-repo.json | u1/u2/u3、g1/g2，next_seq 递增 |
| 配置同步 | script/config.json、script/template/*、test/workspace-template/ | 与测试环境一致 |

### 关键决策

- **user 布局（agent 测试）**：`u1` = agent 绑定（channel 的 `default_bind_user`），`u2` = 初始管理员（channel 的 `admins`），`u3` = admin/unadmin 测试对象。三个 user 同在 `g1`，cli 与 agent 消息互通。
- **管理命令测试范围**：只测 `/admin`、`/model`；覆盖"非管理员命令被忽略 → /admin 添加 → 生效 → /unadmin 移除 → 再被忽略"的完整权限链路。
- **管理 API 鉴权**：`X-Api-Key` header 与 `kissbot_security::SecurityConfig::get().admin_api_key` 比对，不匹配 401。
- **agent 测试环境**：agent 数据目录 `test/workspace/agent-data/`（初始 nexus.json/station.json 由 `resetWorkspace()` 从 `test/workspace-template/agent-data/` 复制）；`KISSBOT_CONFIG` 指向 `test/workspace/config.json`；agent 通过 channel 配置连接 channel-web（ws://127.0.0.1:8201）。
- **ConfigManager providers CRUD**：`add_provider`（检查重名）与 `remove_provider`（不存在报 ConfigNotFound），均 `save_nexus()` 落盘；`set_default_model` 落盘。
- **现有 spec 引用连锁**：4 个 spec 文件约 67 处 user/group id 引用同步更新（`user-1→u1`、`user-2→u2`、`dev-team→g1`、`project-x→g2`、`a_user-1→a_u1`）。

## Part 1：WebMessengerRepo id 规范修正

规范格式（与 messenger.rs 的 `add_user`/`add_group` 生成规则一致）：

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

修改文件：`test/workspace/channel-web-repo.json`、`test/workspace-template/channel-web-repo.json`、`script/template/channel-web-repo.json`（三份内容一致；test/workspace 是工作数据，由 resetWorkspace 从 template 复制）。

同步更新 spec 引用：
- `test/tests/channel-cli.spec.ts`、`channel-web-api.spec.ts`、`channel-web-client.spec.ts`、`channel-web-ui.spec.ts`
- 替换：`user-1`→`u1`、`user-2`→`u2`、`dev-team`→`g1`、`project-x`→`g2`、`a_user-1`→`a_u1`
- 注意正则中的 id（如 `/<< \[admin:dev-team\]/` → `/<< \[admin:g1\]/`）与 API body 中的 group_id/user_id

## Part 2：agent 管理 API

### Cargo 依赖

`kissbot-agent/Cargo.toml` 增加：
```toml
axum = { version = "0.7", features = ["json"] }
```

### ConfigManager 新增写方法（config_manager.rs）

```rust
// ---------- providers ----------
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

// ---------- default 读写 ----------
/// 设置默认模型（(provider, model) 打包），落盘
pub async fn set_default_model(&self, pm: ProviderModel) -> Result<()> {
    {
        let mut repo = self.nexus_repo.write().await;
        Arc::make_mut(&mut repo).default_model = Arc::new(pm);
    }
    self.save_nexus().await
}
```

（`add_channel` / `remove_channel` / `add_admin` / `remove_admin` 已存在，按 channel_id 定位。）

### HttpServer（http_server.rs）重写为 axum

```rust
pub struct HttpServer {
    config: Arc<ConfigManager>,
    host: String,
    port: u16,
}

impl HttpServer {
    pub fn new(config: Arc<ConfigManager>, host: String, port: u16) -> Self { ... }

    pub async fn start(&self) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        let app = self.build_router();
        let listener = TcpListener::bind(&addr).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        axum::serve(listener, app).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        Ok(())
    }
}
```

路由与 DTO：

| 端点 | 方法 | Body | 行为 |
|------|------|------|------|
| `/config` | GET | - | 返回 NexusRepo 快照 |
| `/config/providers` | POST | `ProviderConfig` JSON | `config.add_provider` |
| `/config/providers/remove` | POST | `{ "name": "..." }` | `config.remove_provider` |
| `/config/default` | POST | `ProviderModel` JSON | `config.set_default_model` |
| `/config/channels` | POST | `ChannelConfig` JSON | `config.add_channel` |
| `/config/channels/remove` | POST | `{ "channel_id": "..." }` | `config.remove_channel` |
| `/config/admins` | POST | `{ "channel_id", "messenger_id", "user_id" }` | `config.add_admin(channel_id, &ChannelUser{...})` |
| `/config/admins/remove` | POST | `{ "channel_id", "messenger_id", "user_id" }` | `config.remove_admin(channel_id, messenger_id, user_id)` |

鉴权：中间件校验 `X-Api-Key` == `admin_api_key`，不匹配返回 401。响应统一 `{ "success": bool, "data": ..., "error": ... }`（沿用 kissbot-api `ApiResponse` 风格，或在 agent 内部定义）。

## Part 3：agent 管理命令测试（agent-commands.spec.ts）

环境（beforeAll）：
- `resetWorkspace()`（清空 test/workspace 并从 template 复制，含 agent-data）
- 启动 channel-web（helpers/server.ts 的 startBackend，工作目录 test/workspace）
- 启动 agent（新 helper `startAgent(cwd)`：`KISSBOT_CONFIG=config.json` 环境变量，运行 kissbot-agent debug 二进制；等待管理端口 9090 就绪）
- cli1 = `spawnCli(['web', 'u2', 'g1', './downloads'], WORKSPACE)`（u2 初始管理员）
- cli2 = `spawnCli(['web', 'u3', 'g1', './downloads'], WORKSPACE)`（u3 非管理员）

agent 初始 nexus.json（test/workspace-template/agent-data/nexus.json）：
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
      "name": "deepseek", "provider_type": "openai",
      "base_url": "https://api.deepseek.com", "api_key": "",
      "default_context_length": 65536, "default_max_tokens": 4096,
      "default_temperature": 0.7, "default_timeout_secs": 60, "default_retry_count": 3,
      "models": { "deepseek-4-flash": { "model": "deepseek-4-flash" } }
    }
  },
  "memory_structs": {},
  "stations": {},
  "default_agent_id": "",
  "default_role": "",
  "default_model": { "provider": "deepseek", "model": "deepseek-4-flash" }
}
```

测试用例（串行，cli 消息走 g1 群组）：
1. **TC-01 非管理员命令被忽略**：cli2(u3) 发 `/model deepseek deepseek-4-flash`，断言 cli2 **不**收到 agent 回复（等待超时/断言无输出）
2. **TC-02 /admin 添加管理员**：cli1(u2) 发 `/admin web u3`，断言 cli1 收到 `✅ 已添加管理权限: web / u3`
3. **TC-03 /model 生效**：cli2(u3) 发 `/model deepseek deepseek-4-flash`，断言 cli2 收到 `✅ 已切换模型为: deepseek/deepseek-4-flash`
4. **TC-04 /unadmin 移除**：cli1(u2) 发 `/unadmin web u3`，断言 cli1 收到 `✅ 已移除管理权限`
5. **TC-05 移除后命令再被忽略**：cli2(u3) 发 `/model ...`，断言 cli2 不收到回复

验证要点：agent 回复经 send_reply 到 g1 群组，channel-web 分发给 u2/u3 的 cli 连接，cli stdout 出现回复文本。

## Part 4：agent 配置管理 API 测试（agent-config-api.spec.ts）

环境（beforeAll）：`resetWorkspace()` + 启动 agent（管理 API :9090，KISSBOT_CONFIG 指向 test/workspace/config.json）。

`BASE = 'http://127.0.0.1:9090'`，`ADMIN_KEY = 'admin-key-123'`（config.json security.admin_api_key）。

测试用例：
1. **TC-01 GET /config 读初始快照**：断言含 providers.deepseek 与 default_model
2. **TC-02 POST /config/providers 添加**：添加新 provider（如 anthropic 示例），GET 验证出现；读 test/workspace/agent-data/nexus.json 验证落盘
3. **TC-03 POST /config/default 修改**：改 default_model 指向新 provider/model，GET 验证 + 落盘验证
4. **TC-04 POST /config/channels + /config/admins**：添加 channel，再给该 channel 添加 admin，GET 验证
5. **TC-05 鉴权失败**：错误 X-Api-Key 请求返回 401
6. **TC-06 错误路径**：删除不存在的 provider 返回失败（success=false）

## Part 5：配置同步

| 文件 | 改动 |
|------|------|
| `script/config.json` | 确认 agent 段（data_dir=../workspace/agent-data、init_model 对象）与测试一致；channel-client 段 channel_ws_url |
| `script/template/nexus.json` | 配测试用 channel（web-main）+ admins(u2) + default_bind_user(u1) + deepseek provider + default_model（与 Part 3 一致） |
| `script/template/channel-web-repo.json` | 规范 id（u1/u2/u3、g1/g2） |
| `script/agent-reset.mjs` | 适配新 providers 格式：api_key 注入到 `nexus.providers[name].api_key`（key.local.json 的名称 = provider 名） |
| `test/workspace-template/agent-data/` | 新增 `nexus.json`（Part 3 内容）+ `station.json`（`{}`） |
| `test/workspace-template/config.json` | agent 段 data_dir=`agent-data`（已存在，确认） |
| `test/tests/helpers/server.ts` | 新增 `startAgent` / `stopAgent`（KISSBOT_CONFIG 环境变量、等待 mgmt 端口 9090） |
| `test/global-setup.ts` | 追加构建 kissbot-agent（cargo build --manifest-path ../kissbot-agent/Cargo.toml） |

`resetWorkspace()` 保持 rm -rf + cp -r 语义（清空 test/workspace 并从 workspace-template 重新加载，覆盖 agent-data、channel-web-repo.json、config.json 等全部数据）。

## 代码影响清单

| 文件 | 改动 |
|------|------|
| `kissbot-agent/Cargo.toml` | 加 axum |
| `kissbot-agent/src/config_manager.rs` | add_provider / remove_provider / set_default_model + 测试 |
| `kissbot-agent/src/http_server.rs` | axum Router 重写（8 端点 + 鉴权） |
| `test/tests/agent-config-api.spec.ts` | 新增（管理 API 测试） |
| `test/tests/agent-commands.spec.ts` | 新增（管理命令测试） |
| `test/tests/helpers/server.ts` | startAgent / stopAgent |
| `test/global-setup.ts` | 构建 kissbot-agent |
| `test/tests/channel-cli.spec.ts` 等 4 个 | id 引用更新（u1/u2/g1/g2/a_u1） |
| `test/workspace-template/channel-web-repo.json`、`test/workspace-template/agent-data/nexus.json` + `station.json` | 规范 id / agent 初始 repo |
| `script/template/channel-web-repo.json`、`script/template/nexus.json` | 规范 id / 测试用配置 |
| `script/agent-reset.mjs` | providers 嵌套格式适配 |
