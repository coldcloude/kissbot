# out_channel 移入 (agent, role) 的 ContextConfig Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 out_channel（回复通道）配置从 `ChannelConfig.outgoing` 移入 `(agent, role)` 的 `ContextConfig`，命令改修 agent context，删除动态 resolve 函数，send 时校验绑定、未绑定清理配置。

**Architecture:** `ContextConfig` 加 `out_channel: Option<Arc<OutChannel>>`（`OutChannel` 补 serde，删 `OutChannelConfig`）；`/bind-outgoing`、`/unbind-outgoing` 改经 `ConfigManager::set_out_channel` 纯配置写（`write_nexus_config` 原子，无需队列）；删除 `resolve_out_channel*`，读取统一经 `context_config().out_channel`；`send_outgoing` 签名带 `(agent, role)`，发送前校验目标 channel 绑定，未绑定清理该 `(agent, role)` out 配置。

**Tech Stack:** Rust, arc_swap（ArcSwapHashMap/HashMap entry 写时复制）, serde, tokio

**Spec:** `docs/superpowers/specs/2026-08-18-out-channel-agent-context-design.md`

## Global Constraints

- 不删除代码中的任何注释（项目 CLAUDE.md 铁律）；被移除功能的注释随功能迁移/改写
- 文本文件 UTF-8，`\n` 换行；代码按任务文本 verbatim
- 验证命令在 `/home/admin/project/kissbot/kissbot-agent` 下执行（cargo check 无错误无新警告 + cargo test 全绿）
- 每任务保持 crate 可编译（删除引用前先改写调用方）
- 提交 comment 用中文，覆盖该任务全部改动

---

### Task 1: config_manager 结构加法（ContextConfig.out_channel + OutChannel serde + set_out_channel）

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`（AgentContextConfig/ContextConfig/EffectiveContextConfig/merge_context_config + set_out_channel 方法 + 测试）

**Interfaces:**
- Consumes: 现有 `OutChannel`（config_manager.rs:262，当前仅 Debug/Clone）、`ContextConfig`、`merge_context_config`、`write_nexus_config`
- Produces: `ContextConfig.out_channel: Option<Arc<OutChannel>>`、`EffectiveContextConfig.out_channel: Option<Arc<OutChannel>>`、`pub async fn set_out_channel(&self, agent_id: &str, role_name: &str, out: Option<Arc<OutChannel>>) -> Result<()>` —— Task 2（命令）与 Task 3（send 清理）使用

- [ ] **Step 1: OutChannel 补 serde**

`kissbot-agent/src/config_manager.rs` 中 `OutChannel` 定义（约 line 262）当前：

```rust
/// out_channel 运行态（Nexus 实时从配置构造）
#[derive(Debug, Clone)]
pub struct OutChannel {
    pub channel_id: Arc<String>,
    pub user: ChannelUser,
    pub group_id: Arc<String>,
}
```

改为（持久化为 (agent, role) 级回复通道配置，读取即用）：

```rust
/// out_channel 配置（(agent, role) 级回复通道，持久化到 nexus.json；channel_id 为发送目标）
/// 由 /bind-outgoing 在来源 channel 构造（channel_id = 来源 channel），Agentic Loop 回复经此发送
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutChannel {
    pub channel_id: Arc<String>,
    pub user: ChannelUser,
    pub group_id: Arc<String>,
}
```

- [ ] **Step 2: ContextConfig 加 out_channel 字段**

`ContextConfig` 定义（约 line 56）末尾 `toolkits` 字段后加：

```rust
    /// out_channel（agent+role 级回复通道；/bind-outgoing、/unbind-outgoing 修改；role 覆盖 or agent 默认回落）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_channel: Option<Arc<OutChannel>>,
```

- [ ] **Step 3: AgentContextConfig 加 Default**

`AgentContextConfig` derive 行（`#[derive(Debug, Clone, Serialize, Deserialize)]`，约 line 40）改为：

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentContextConfig {
```

- [ ] **Step 4: EffectiveContextConfig + merge 加 out_channel**

`EffectiveContextConfig` 结构体（约 line 66）加字段：

```rust
    pub toolkits: HashSet<String>,
    /// (agent, role) 有效 out_channel（role 覆盖 or agent 默认回落；None = 无回复通道）
    pub out_channel: Option<Arc<OutChannel>>,
```

`merge_context_config` 两处（agent None 分支 + 正常分支）：
- `let Some(a) = agent else { return ... }` 分支的 `EffectiveContextConfig { ... }` 加 `out_channel: None,`（toolkits 行后）
- 正常分支 `toolkits: ...` 后加：

```rust
        out_channel: role.and_then(|r| r.out_channel.clone())
            .or_else(|| d.out_channel.clone()),
```

- [ ] **Step 5: 添加 set_out_channel 方法**

在 `context_config` 方法（约 line 548）之后添加：

```rust
    /// 设置 (agent, role) 的 out_channel（/bind-outgoing、/unbind-outgoing：role 空写 agent 默认，
    /// 非空写 role 覆盖；None 清除；write_nexus_config 单次原子，无需串行队列；agent 条目懒建）
    pub async fn set_out_channel(&self, agent_id: &str, role_name: &str, out: Option<Arc<OutChannel>>) -> Result<()> {
        self.write_nexus_config(|repo| {
            let map = Arc::make_mut(&mut repo.agent_contexts);
            // agent 条目不存在则懒建（缺省 AgentContextConfig）
            let entry = map.entry(agent_id.to_string()).or_insert_with(|| ArcSwap::new(Arc::new(AgentContextConfig::default())));
            let mut agent = (*entry.load_full()).clone();
            let agent_mut = Arc::make_mut(&mut agent);
            if role_name.is_empty() {
                agent_mut.default_context_config.out_channel = out;
            } else {
                let role_map = Arc::make_mut(&mut agent_mut.roles);
                let role_entry = role_map.entry(role_name.to_string()).or_insert_with(|| ArcSwap::new(Arc::new(ContextConfig::default())));
                let mut role = (*role_entry.load_full()).clone();
                let role_mut = Arc::make_mut(&mut role);
                role_mut.out_channel = out;
                role_entry.store(role);
            }
            entry.store(agent);
            Ok(())
        }).await
    }
```

- [ ] **Step 6: 添加 roundtrip 测试**

在 `config_manager.rs` tests mod 添加（放 context 配置相关测试附近）：

```rust
    #[test]
    fn context_config_out_channel_serde_roundtrip() {
        // out_channel 作为 ContextConfig 字段序列化/反序列化（agent+role 级回复通道）
        let ctx = ContextConfig {
            channel_batch_interval_secs: None,
            memory_time_secs: None,
            memory_count: None,
            compress_prompt: None,
            toolkits: None,
            out_channel: Some(Arc::new(OutChannel {
                channel_id: Arc::new("web-main".into()),
                user: ChannelUser { messenger_id: "web".into(), user_id: "u1".into() },
                group_id: Arc::new("g1".into()),
            })),
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: ContextConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.out_channel.as_ref().unwrap().channel_id.as_str(), "web-main");
        assert_eq!(back.out_channel.as_ref().unwrap().user.user_id, "u1");
    }
```

- [ ] **Step 7: 编译 + 测试验证**

Run: `cargo check && cargo test`
Expected: `Finished`；`test result: ok.` 全绿（ContextConfig 构造处若因新字段报 E0063，补 `out_channel: None`——测试与生产代码均需核对）

- [ ] **Step 8: 提交**

```bash
git add kissbot-agent/src/config_manager.rs
git commit -m "feat(config_manager): ContextConfig 加 out_channel 字段（OutChannel serde 持久化）+ set_out_channel 方法——(agent, role) 级回复通道配置"
```

---

### Task 2: 命令改写 + ChannelCommand 缩变体 + channel_manager 删减

**Files:**
- Modify: `kissbot-agent/src/types.rs`、`kissbot-agent/src/command_router.rs`、`kissbot-agent/src/channel_manager.rs`、`kissbot-agent/src/nexus.rs`

**Interfaces:**
- Consumes: Task 1 的 `set_out_channel` / `OutChannel`
- Produces: 无（衔接 Task 3）

- [ ] **Step 1: types.rs 缩 ChannelCommand、删 OutChannelParams**

`ChannelCommand` 枚举（约 types.rs:120）改为（删 BindOutgoing/ClearOutgoing 变体与相关注释）：

```rust
/// channel 配置变更任务（纯数据；CommandRouter 构造，Nexus 排队调 ChannelManager 执行）
/// /bind、/unbind 统一走此枚举（out_channel 属 (agent, role) context，由 /bind-outgoing 纯配置写，不走此队列）
pub enum ChannelCommand {
    /// 绑定 channel 用户（bind_users 追加，HashSet 天然去重幂等）
    BindUser { channel_id: String, user: ChannelUser },
    /// 解绑 channel 用户（移除 bind_users）
    UnbindUser { channel_id: String, user: ChannelUser },
}
```

删除 `OutChannelParams` 结构体及其 `/// /bind-outgoing 命令参数（转 OutChannelConfig 持久化）` 注释（约 line 116-121）。

- [ ] **Step 2: channel_manager.rs 删 bind_outgoing/clear_outgoing + unbind_user 清空联动**

删除 `bind_outgoing` 与 `clear_outgoing` 两个方法（约 line 164-196）。

`unbind_user`（约 line 158）改为（删 outgoing 引用清空逻辑——out_channel 移 agent context 后与 channel 绑定解耦）：

```rust
    /// 解绑 channel 用户（/unbind：移除 bind_users）
    pub async fn unbind_user(&self, channel_id: &str, user: &ChannelUser) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| {
            Arc::make_mut(&mut c.bind_users).remove(user);
        }).await
    }
```

import 调整：`use crate::config_manager::{ConfigManager, OutChannelConfig};` 改回 `use crate::config_manager::ConfigManager;`；`use crate::types::{Error, Mode, OutChannelParams, Result};` 改 `use crate::types::{Error, Mode, Result};`

- [ ] **Step 3: nexus.rs apply_channel_command 缩为两分支**

`apply_channel_command`（约 line 330）改为：

```rust
    /// channel 配置变更执行（队列内串行，不对外）：分发到 ChannelManager 方法
    async fn apply_channel_command(&self, cmd: ChannelCommand) -> Result<String> {
        match cmd {
            ChannelCommand::BindUser { channel_id, user } => {
                self.channel_manager.bind_user(&channel_id, &user).await?;
                Ok(format!("✅ 已绑定 channel 用户: {} / {}", user.messenger_id, user.user_id))
            },
            ChannelCommand::UnbindUser { channel_id, user } => {
                self.channel_manager.unbind_user(&channel_id, &user).await?;
                Ok(format!("✅ 已移除 channel 用户: {} / {}", user.messenger_id, user.user_id))
            },
        }
    }
```

`nexus.rs` 的 types import 删 `OutChannelParams`（若引入）。

- [ ] **Step 4: command_router.rs 改写 bind-outgoing/unbind-outgoing**

`"bind-outgoing"` 与 `"unbind-outgoing"` 分支（约 line 133-150）整体替换为（纯配置写 set_out_channel；不再构造 ChannelCommand）：

```rust
            "bind-outgoing" => {
                // /bind-outgoing <messenger_id> <user_id> <group_id>：设 (agent, role) 的 out_channel
                // （纯配置写 set_out_channel；先校验身份已绑定来源 channel）
                if parts.len() < 4 {
                    return Err(Error::InvalidCommand(
                        "格式: /bind-outgoing <messenger_id> <user_id> <group_id>".to_string()
                    ));
                }
                let messenger_id = parts[1].to_string();
                let user_id = parts[2].to_string();
                let group_id = parts[3].to_string();
                let cm = ConfigManager::get();
                let Some(ch) = cm.channel(channel_id).await else {
                    return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
                };
                // 校验 ChannelUser 已绑定（未绑拒绝）
                let cu = ChannelUser { messenger_id: messenger_id.clone(), user_id: user_id.clone() };
                if !ch.bind_users.contains(&cu) {
                    return Err(Error::InvalidCommand(format!(
                        "ChannelUser 未绑定: {} / {}", messenger_id, user_id)));
                }
                // 设 (agent, role) 的 out_channel（channel_id = 来源 channel）
                cm.set_out_channel(ch.agent_id.as_str(), ch.role_name.as_str(),
                    Some(Arc::new(OutChannel {
                        channel_id: Arc::new(channel_id.to_string()),
                        user: cu,
                        group_id: Arc::new(group_id),
                    }))).await?;
                Ok(format!("✅ 已设发送通道: {} / {} -> {}", messenger_id, user_id, group_id))
            }
            "unbind-outgoing" => {
                // /unbind-outgoing：清空 (agent, role) 的 out_channel（回到只存不回复模式）
                if parts.len() > 1 {
                    return Err(Error::InvalidCommand(
                        "格式: /unbind-outgoing（无参数）".to_string()
                    ));
                }
                let cm = ConfigManager::get();
                let Some(ch) = cm.channel(channel_id).await else {
                    return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
                };
                cm.set_out_channel(ch.agent_id.as_str(), ch.role_name.as_str(), None).await?;
                Ok("✅ 已取消发送通道（只存不回复）".to_string())
            }
```

import 调整：`use crate::config_manager::{ConfigManager, ProviderModel};` 改 `use crate::config_manager::{ConfigManager, OutChannel, ProviderModel};`；`use crate::types::{ChannelCommand, Error, Mode, OutChannelParams, RESERVED_AGENT_ID, Result};` 改 `use crate::types::{ChannelCommand, Error, Mode, RESERVED_AGENT_ID, Result};`

- [ ] **Step 5: 编译 + 测试验证**

Run: `cargo check && cargo test`
Expected: `Finished`；`test result: ok.` 全绿（ChannelConfig.outgoing 字段仍在，由 Task 3 删除——本任务只删其 channel_manager/command_router 引用）

- [ ] **Step 6: 提交**

```bash
git add kissbot-agent/src/types.rs kissbot-agent/src/command_router.rs kissbot-agent/src/channel_manager.rs kissbot-agent/src/nexus.rs
git commit -m "refactor: bind-outgoing/unbind-outgoing 改纯配置写 set_out_channel——删除 ChannelCommand::BindOutgoing/ClearOutgoing 与 ChannelManager::bind_outgoing/clear_outgoing"
```

---

### Task 3: nexus + session 读取改造 + 删 ChannelConfig.outgoing/OutChannelConfig

**Files:**
- Modify: `kissbot-agent/src/nexus.rs`、`kissbot-agent/src/session_manager.rs`、`kissbot-agent/src/config_manager.rs`

**Interfaces:**
- Consumes: Task 1 的 `EffectiveContextConfig.out_channel` / `set_out_channel`；Task 2 的 ChannelCommand 缩变体
- Produces: 无（最终链路）

- [ ] **Step 1: nexus.rs 删除 resolve_out_channel / resolve_out_channel_for_session**

删除 `resolve_out_channel`（约 line 507）与 `resolve_out_channel_for_session`（约 line 528）两个方法及注释。

- [ ] **Step 2: incoming_message 步骤 5 改读 context_config**

`incoming_message` 步骤 5（约 line 475）当前：

```rust
        // 5. 普通消息：无 out_channel 不进 Agentic Loop（ChannelRecord 已存，结束）
        let Some(_out_channel) = self.resolve_out_channel(channel_id).await else {
            return;
        };
        let session = self.ensure_session(key).await;
```

改为：

```rust
        // 5. 普通消息：无 out_channel 不进 Agentic Loop（ChannelRecord 已存，结束）
        let cfg = ConfigManager::get().context_config(key.agent_id.as_str(), key.role_name.as_str()).await;
        if cfg.out_channel.is_none() {
            return;
        }
        let session = self.ensure_session(key).await;
```

- [ ] **Step 3: send_outgoing 签名 + 校验 + 清理**

`send_outgoing`（约 line 614）整体替换为：

```rust
    /// Agentic Loop 产出回复：发到 out_channel（agent/role 为产出会话的 (agent, role) 定位）
    /// 发送前校验 out_channel 身份在目标 channel 仍绑定；未绑定 → 清理该 (agent, role) 的 out 配置并跳过
    pub async fn send_outgoing(&self, agent_id: &str, role_name: &str, out_channel: &OutChannel, content: Arc<String>) {
        // 1. 校验 out_channel 身份在目标 channel 仍绑定（未绑定 = 配置悬空，清理并跳过发送）
        let bound = ConfigManager::get().channel(out_channel.channel_id.as_str()).await
            .map(|c| c.bind_users.contains(&out_channel.user))
            .unwrap_or(false);
        if !bound {
            warn!("send_outgoing: out_channel 身份未绑定，清理 {}/{} 的回复通道", agent_id, role_name);
            let _ = ConfigManager::get().set_out_channel(agent_id, role_name, None).await;
            return;
        }
        // 2. 分别取 config 绑定身份（agent_id/role_name）与 channel_manager 运行态 mode，不合成 session_key
        let mode = self.channel_manager.mode(out_channel.channel_id.as_str());
        let role_key = memory_role(role_name, mode.as_ref());

        let msg = OutgoingMessage {
            messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
            user_id: Arc::new(out_channel.user.user_id.clone()),
            group_id: out_channel.group_id.clone(),
            content: Content::Text(content),
        };

        // 发送经 ChannelManager（内部取 client + 记录 pending msg_id 供回显判定）
        match self.channel_manager.send(out_channel.channel_id.as_str(), msg).await {
            Ok(response) => {
                // 下行成功后：推记忆（is_self=1）
                self.memory_store_client.push_channel_record(ChannelRequest {
                    agent_id: Arc::new(agent_id.to_string()),
                    role_name: Arc::new(role_key),
                    messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
                    user_id: Arc::new(out_channel.user.user_id.clone()),
                    self_user_id: Arc::new(out_channel.user.user_id.clone()),
                    group_id: out_channel.group_id.clone(),
                    is_self: 1,
                    messenger_name: response.messenger_name.clone(),
                    user_name: response.user_name.clone(),
                    group_name: response.group_name.clone(),
                    content: response.content.clone(),
                    time: response.time.clone(),
                }).await;
            }
            Err(e) => {
                warn!("send_outgoing 失败: {:?}", e);
            }
        }
    }
```

- [ ] **Step 4: session_manager.rs run_agentic_loop 改读 context_config.out_channel**

`run_agentic_loop`（约 line 281-287）当前：

```rust
        let coordinator = Nexus::get();

        let Some(out_channel) = coordinator.resolve_out_channel_for_session(self.clone()).await else {
            warn!("accept_batch: 会话无 out_channel，跳过");
            return;
        };
```

改为（实时从 (agent, role) context 配置读取，None = 只存不回复）：

```rust
        let coordinator = Nexus::get();

        // out_channel 从 (agent, role) context 配置实时读（None = 只存不回复）
        let cfg = ConfigManager::get().context_config(self.agent_id.as_str(), self.role_name.as_str()).await;
        let Some(out_channel) = cfg.out_channel else {
            warn!("accept_batch: 会话无 out_channel，跳过");
            return;
        };
```

`send_outgoing` 调用点（约 line 449）改为（传 (agent, role) 定位；`&out_channel` 为 `&Arc<OutChannel>` 自动 deref）：

```rust
                    coordinator.send_outgoing(self.agent_id.as_str(), self.role_name.as_str(), &out_channel, model_resp.content).await;
```

- [ ] **Step 5: config_manager.rs 删 ChannelConfig.outgoing / OutChannelConfig + 测试清理**

- `ChannelConfig`（约 line 242）删字段 `pub outgoing: Option<Arc<OutChannelConfig>>,` 及其 `/// out_channel 配置（Option，至多 1 个；存于被绑定的 channel 下）` 注释
- 删除 `OutChannelConfig` 结构体（约 line 252-258）及注释
- 测试清理：`sample_channel`（约 line 742 `outgoing: None,` 删）；`channel_config_bind_users_and_outgoing_roundtrip` 测试（约 line 750）删除或改写——删除该测试（outgoing 已移出 ChannelConfig，roundtrip 由 Task 1 的 context_config_out_channel_serde_roundtrip 覆盖）；其余构造 ChannelConfig 处若带 outgoing 字段一并删除
- `update_channel` doc 注释（约 line 514「绑定/agent/role/outgoing 等运行时回写统一入口」）改「绑定/agent/role 等运行时回写统一入口」

- [ ] **Step 6: 编译 + 测试验证**

Run: `cargo check && cargo test`
Expected: `Finished`；`test result: ok.` 全绿；`rg -n "outgoing" kissbot-agent/src` 仅剩 ContextConfig.out_channel / set_out_channel / EffectiveContextConfig 等新语义引用（无 ChannelConfig.outgoing / OutChannelConfig 残留）

- [ ] **Step 7: 提交**

```bash
git add kissbot-agent/src/nexus.rs kissbot-agent/src/session_manager.rs kissbot-agent/src/config_manager.rs
git commit -m "refactor: 删除 resolve_out_channel*/ChannelConfig.outgoing——读取统一经 context_config().out_channel，send_outgoing 校验绑定并清理失效配置"
```

---

### Task 4: E2E 测试 + 测试模板迁移

**Files:**
- Modify: `test/tests/nexus-ego-chat-store.spec.ts`、`test/workspace-template/agent-data/nexus.json`

**Interfaces:**
- Consumes: Task 1-3 的配置结构（out_channel 在 agent_contexts，channel 无 outgoing）

- [ ] **Step 1: 模板 nexus.json 迁移**

`test/workspace-template/agent-data/nexus.json`：channel `web-main` 删 `"outgoing"` 行；新增 `agent_contexts` 段（a1 默认 out_channel web/u1/g1，供场景 5/6 的 /agent a1 回落）：

```json
{
  "channels": {
    "web-main": {
      "channel_id": "web-main",
      "ws_url": "ws://127.0.0.1:8201",
      "admins": [{ "messenger_id": "web", "user_id": "u2" }],
      "bind_users": [{ "messenger_id": "web", "user_id": "u1" }],
      "agent_name": "",
      "role_name": "",
      "enabled": true
    }
  },
  "agent_contexts": {
    "a1": {
      "default_context_config": {
        "out_channel": {
          "channel_id": "web-main",
          "user": { "messenger_id": "web", "user_id": "u1" },
          "group_id": "g1"
        }
      },
      "roles": {}
    }
  },
  "providers": { ... 保持 ... },
  "memory_structs": {},
  "default_model": { "provider": "deepseek", "model": "deepseek-v4-flash" },
  "default_system_prompt": "你是 kissbot 智能助手",
  "context": {}
}
```

- [ ] **Step 2: E2E helper 改读 agent context**

`nexus-ego-chat-store.spec.ts` 的 `readChannelConfig`（约 line 100）替换为：

```ts
// 读取 nexus.json 的 (agent, role) 有效 out_channel 配置（role 覆盖 or agent 默认回落；命令回写先于回复，断言时已持久化）
function readOutChannel(agentId: string, roleName: string): any {
  const nexus = JSON.parse(readFileSync(join(WORKSPACE, 'agent-data', 'nexus.json'), 'utf8'));
  const agent = nexus.agent_contexts?.[agentId];
  const role = agent?.roles?.[roleName];
  return role?.out_channel ?? agent?.default_context_config?.out_channel ?? null;
}
```

- [ ] **Step 3: 场景 5 断言迁移**

场景 5（约 line 296-312）两处断言：
- `expect(readChannelConfig().outgoing).toBeNull();` → `expect(readOutChannel('a1', 'out1')).toBeNull();`（注释同步「(a1, out1) 有效 out_channel 已清空」）
- `expect(readChannelConfig().outgoing).toEqual({ messenger_id: 'web', user_id: 'u1', group_id: 'g1' });` → `expect(readOutChannel('a1', 'out1')).toEqual({ channel_id: 'web-main', user: { messenger_id: 'web', user_id: 'u1' }, group_id: 'g1' });`

- [ ] **Step 4: 场景 6 语义迁移**

场景 6 当前「unbind 移除后 outgoing 自动清空」——新语义：`/unbind` 不动 out_channel 配置；`send_outgoing` 发送前校验未绑定才清理。调整：
- 测试名/注释改为「unbind 不动 out_channel；send 时校验未绑定才清理」
- unbind 后断言 `readOutChannel('a1', 'out2')` 仍存在（未清理）
- （若场景 6 原本依赖 unbind 触发清理的后续断言，改为：发消息触发 send 校验失败 → 清理后断言 `readOutChannel('a1', 'out2')` 为 null）
- 具体按场景 6 原文（约 line 318-345）结构改写，保留「bind 追加 u3 + bind-outgoing 指向 u3」主流程

- [ ] **Step 5: 语法验证**

Run: `cd /home/admin/project/kissbot/test && npx playwright test --list tests/nexus-ego-chat-store.spec.ts`
Expected: 测试可解析列出（场景 5/6 新名字）

- [ ] **Step 6: 提交**

```bash
git add test/tests/nexus-ego-chat-store.spec.ts test/workspace-template/agent-data/nexus.json
git commit -m "test: out_channel 移 agent context——E2E 场景 5/6 断言与模板 nexus.json 迁移"
```

---

## 验证收尾

- [ ] **Step 1: 全量验证**

Run: `cargo check && cargo test`（kissbot-agent）
Expected: 编译通过，`test result: ok.` 全绿

- [ ] **Step 2: 残留检查**

Run: `rg -n "OutChannelConfig|resolve_out_channel|ChannelConfig\.outgoing|outgoing" kissbot-agent/src -g '*.rs'`
Expected: 仅 `ContextConfig.out_channel` / `set_out_channel` / `EffectiveContextConfig.out_channel` / `send_outgoing` 等新语义；无 `OutChannelConfig`、`resolve_out_channel`、`ChannelConfig.outgoing` 残留

## 参考

- Spec: `docs/superpowers/specs/2026-08-18-out-channel-agent-context-design.md`
- 现有队列模式：`kissbot-agent/src/nexus.rs` 的 `ChannelCommand::BindUser/UnbindUser` + `channel_command`/`apply_channel_command`
