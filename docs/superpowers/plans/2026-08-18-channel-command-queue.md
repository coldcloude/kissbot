# channel 配置写操作收拢（ChannelManager + 统一串行队列）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 `/bind`、`/unbind`、`/bind-outgoing`、`/bind-outgoing off` 的 channel 配置写操作收拢为 ChannelManager 方法，经 Nexus 新队列排队，与 change_channel_key 队列在单一消费者内用 select! 合并串行执行。

**Architecture:** 新增 `ChannelCommand` 纯数据枚举（types.rs）供 CommandRouter 构造；Nexus 提供单一 `channel_command` 对接方法，内部包装 `ChannelTask { cmd, done }` 发入新队列；现有消费者循环改为 `tokio::select!` 同时监听 ConfigChange 队列与新队列，串行执行 ChannelManager 方法（bind_user/unbind_user/bind_outgoing/clear_outgoing）。

**Tech Stack:** Rust, tokio（mpsc UnboundedSender/Receiver + select! + oneshot），arc_swap（Arc::make_mut 写时复制）

**Spec:** `docs/superpowers/specs/2026-08-18-channel-command-queue-design.md`

## Global Constraints

- 不删除代码中的注释（项目 CLAUDE.md 约定）；被搬移的逻辑连同注释一起迁移
- 文本文件 UTF-8，`\n` 换行
- 所有改动在 `kissbot-agent/` crate 内，验证命令在 `/home/admin/project/kissbot/kissbot-agent` 下执行
- 写操作一律走 `Arc::make_mut`（ArcSwap 写时复制）模式，与现有 `update_channel` 闭包一致
- 不新增依赖全局单例（ConfigManager::get）的单元测试；行为保持由现有 131 个测试 + cargo check 保证
- 提交 comment 用中文，覆盖该任务全部改动

---

### Task 1: types.rs 添加 ChannelCommand 枚举

**Files:**
- Modify: `kissbot-agent/src/types.rs`（顶部 import 区 + OutChannelParams 之后）

**Interfaces:**
- Consumes: `kissbot_api::channel::ChannelUser`、`crate::types::OutChannelParams`（均已存在）
- Produces: `pub enum ChannelCommand { BindUser { channel_id: String, user: ChannelUser }, UnbindUser { channel_id: String, user: ChannelUser }, BindOutgoing { channel_id: String, params: OutChannelParams }, ClearOutgoing { channel_id: String } }` —— Task 3（Nexus 消费）、Task 4（CommandRouter 构造）使用

- [ ] **Step 1: 添加 ChannelUser import**

`kissbot-agent/src/types.rs` 顶部当前为：

```rust
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config_manager::ProviderModel;
```

改为：

```rust
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use kissbot_api::channel::ChannelUser;

use crate::config_manager::ProviderModel;
```

- [ ] **Step 2: 添加 ChannelCommand 枚举**

在 `OutChannelParams` 结构体（`pub struct OutChannelParams { pub messenger_id: String, pub user_id: String, pub group_id: String }`，约 types.rs:136）之后追加：

```rust
/// channel 配置变更任务（纯数据；CommandRouter 构造，Nexus 排队调 ChannelManager 执行）
/// /bind、/unbind、/bind-outgoing、/bind-outgoing off 统一走此枚举
pub enum ChannelCommand {
    /// 绑定 channel 用户（bind_users 追加，HashSet 天然去重幂等）
    BindUser { channel_id: String, user: ChannelUser },
    /// 解绑 channel 用户（移除 bind_users；若 outgoing 引用该身份则清空）
    UnbindUser { channel_id: String, user: ChannelUser },
    /// 设 out_channel（校验已绑 + 清同 agent/role 其他 channel outgoing + 设来源，单任务原子）
    BindOutgoing { channel_id: String, params: OutChannelParams },
    /// 清空 out_channel（回到只存不回复模式）
    ClearOutgoing { channel_id: String },
}
```

- [ ] **Step 3: 编译验证**

Run: `cargo check`
Expected: `Finished`，无错误、无新警告

- [ ] **Step 4: 提交**

```bash
git add kissbot-agent/src/types.rs
git commit -m "feat(types): ChannelCommand 枚举——bind/unbind/bind-outgoing/clear-outgoing 纯数据任务定义"
```

---

### Task 2: ChannelManager 新增 4 个 config 写方法

**Files:**
- Modify: `kissbot-agent/src/channel_manager.rs`（import 区 + `mode()`/`session_key()` 之后的 impl 块内）

**Interfaces:**
- Consumes: `ConfigManager::update_channel/channel/channels`、`kissbot_api::channel::ChannelUser`、`crate::types::{Error, OutChannelParams, Result}`、`crate::config_manager::OutChannelConfig`（均需加 import，ConfigManager 已有）
- Produces: `pub async fn bind_user(&self, channel_id: &str, user: &ChannelUser) -> Result<()>`、`pub async fn unbind_user(&self, channel_id: &str, user: &ChannelUser) -> Result<()>`、`pub async fn bind_outgoing(&self, channel_id: &str, params: &OutChannelParams) -> Result<()>`、`pub async fn clear_outgoing(&self, channel_id: &str) -> Result<()>` —— Task 3 的 `apply_channel_command` 调用

- [ ] **Step 1: 调整 imports**

`kissbot-agent/src/channel_manager.rs` 当前相关 import：

```rust
use kissbot_api::channel::{BindRequest, IncomingMessageEvent, OutgoingMessage, OutgoingMessageResponse};
...
use crate::config_manager::ConfigManager;
use crate::nexus::Nexus;
use crate::types::{Mode, SessionKey};
```

改为：

```rust
use kissbot_api::channel::{BindRequest, ChannelUser, IncomingMessageEvent, OutgoingMessage, OutgoingMessageResponse};
...
use crate::config_manager::{ConfigManager, OutChannelConfig};
use crate::nexus::Nexus;
use crate::types::{Error, Mode, OutChannelParams, Result, SessionKey};
```

- [ ] **Step 2: 添加 4 个方法**

在 `session_key` 方法（`pub async fn session_key(&self, channel_id: &str) -> Option<SessionKey>`，约 channel_manager.rs:158）之后、`connect_all` 之前插入：

```rust
    /// 绑定 channel 用户（/bind：bind_users 追加，HashSet 天然去重幂等）
    pub async fn bind_user(&self, channel_id: &str, user: &ChannelUser) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| {
            Arc::make_mut(&mut c.bind_users).insert(user.clone());
        }).await
    }

    /// 解绑 channel 用户（/unbind：移除 bind_users；若 outgoing 引用该身份则清空，避免悬空引用）
    pub async fn unbind_user(&self, channel_id: &str, user: &ChannelUser) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| {
            Arc::make_mut(&mut c.bind_users).remove(user);
            // 移除的是 outgoing 引用身份则清空 outgoing（避免悬空引用）
            if let Some(out) = &c.outgoing {
                if out.messenger_id.as_str() == user.messenger_id && out.user_id.as_str() == user.user_id {
                    c.outgoing = None;
                }
            }
        }).await
    }

    /// 设 out_channel（/bind-outgoing）：校验 channel 存在 + ChannelUser 已绑（未绑拒绝），
    /// 清空同 (agent_id, role_name) 其他 channel 的 outgoing（保证至多 1 个），再设来源 channel 的 outgoing；
    /// 队列内单任务原子执行（校验与写入不交错）
    pub async fn bind_outgoing(&self, channel_id: &str, params: &OutChannelParams) -> Result<()> {
        let cm = ConfigManager::get();
        // 1. 校验 channel 存在 + ChannelUser 已绑定（未绑拒绝）
        let src = cm.channel(channel_id).await
            .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
        let cu = ChannelUser { messenger_id: params.messenger_id.clone(), user_id: params.user_id.clone() };
        if !src.bind_users.contains(&cu) {
            return Err(Error::InvalidCommand(format!(
                "ChannelUser 未绑定: {} / {}", params.messenger_id, params.user_id)));
        }
        // 2. 清空同 (agent_id, role_name) 其他 channel 的 outgoing（保证至多 1 个）
        for (cid, c) in cm.channels().await {
            if cid != channel_id && c.agent_id == src.agent_id && c.role_name == src.role_name {
                if c.outgoing.is_some() {
                    cm.update_channel(&cid, |cc| cc.outgoing = None).await?;
                }
            }
        }
        // 3. 设来源 channel 的 outgoing
        cm.update_channel(channel_id, |c| {
            c.outgoing = Some(Arc::new(OutChannelConfig {
                messenger_id: Arc::new(params.messenger_id.clone()),
                user_id: Arc::new(params.user_id.clone()),
                group_id: Arc::new(params.group_id.clone()),
            }));
        }).await
    }

    /// 清空 out_channel（/bind-outgoing off：回到只存不回复模式）
    pub async fn clear_outgoing(&self, channel_id: &str) -> Result<()> {
        ConfigManager::get().update_channel(channel_id, |c| c.outgoing = None).await
    }
```

- [ ] **Step 3: 编译验证**

Run: `cargo check`
Expected: `Finished`，无错误、无新警告（pub 方法未使用不告警）

- [ ] **Step 4: 提交**

```bash
git add kissbot-agent/src/channel_manager.rs
git commit -m "feat(channel_manager): bind_user/unbind_user/bind_outgoing/clear_outgoing——channel 配置写操作收拢为 ChannelManager 方法"
```

---

### Task 3: Nexus 新队列 + 合并消费者（select!）

**Files:**
- Modify: `kissbot-agent/src/nexus.rs`（types import、ConfigChange 定义附近、Nexus 字段、new()、消费者 spawn 块、change_channel_key 附近）

**Interfaces:**
- Consumes: Task 1 的 `ChannelCommand`、Task 2 的 `ChannelManager` 4 方法、现有 `ConfigChange::ApplyKey`
- Produces: `pub async fn channel_command(&self, cmd: ChannelCommand) -> Result<()>`（Task 4 调用）、私有 `async fn apply_channel_command(&self, cmd: ChannelCommand) -> Result<()>`、私有 `struct ChannelTask { cmd: ChannelCommand, done: tokio::sync::oneshot::Sender<Result<()>> }`、新字段 `channel_task_tx: tokio::sync::mpsc::UnboundedSender<ChannelTask>`

- [ ] **Step 1: 调整 types import，加 OutChannelParams/ChannelCommand**

`kissbot-agent/src/nexus.rs` 当前：

```rust
use crate::types::{
    Error, Message, Mode, ModelResponse, RESERVED_AGENT_ID, Result, SessionKey, ToolCall, memory_role,
};
```

改为：

```rust
use crate::types::{
    ChannelCommand, Error, Message, Mode, ModelResponse, OutChannelParams, RESERVED_AGENT_ID, Result,
    SessionKey, ToolCall, memory_role,
};
```

- [ ] **Step 2: 添加 ChannelTask 结构体**

在 `ConfigChange` 枚举定义（约 nexus.rs:31）之后追加：

```rust
/// channel 配置变更任务（排队调 ChannelManager 方法执行；与 ConfigChange 同消费者串行，写-写无竞态）
struct ChannelTask {
    cmd: ChannelCommand,
    done: tokio::sync::oneshot::Sender<Result<()>>,
}
```

- [ ] **Step 3: 添加字段 + new() 初始化**

`Nexus` 结构体（约 nexus.rs:50）在 `command_tx` 字段后追加：

```rust
    /// channel 配置变更串行队列（bind/unbind/bind-outgoing/clear-outgoing；与 ConfigChange 同一消费者 select! 等待）
    channel_task_tx: tokio::sync::mpsc::UnboundedSender<ChannelTask>,
```

`new()`（约 nexus.rs:60）在现有 `(command_tx, mut command_rx)` 之后追加：

```rust
        let (channel_task_tx, mut channel_task_rx) = tokio::sync::mpsc::unbounded_channel::<ChannelTask>();
```

`Self { ... }` 构造（约 nexus.rs:74）在 `command_tx,` 后追加：

```rust
            channel_task_tx,
```

- [ ] **Step 4: 消费者改为 select! 合并等待**

现有消费者块（`tokio::spawn(async move { while let Some(change) = command_rx.recv().await { ... } });`，约 nexus.rs:97-111）整体替换为：

```rust
        // 启动变更消费者：agent/role/event 变更 + channel 配置变更串行处理（避免写-写竞态；读不受影响）
        // 两队列经 select! 合并到同一消费者，所有 channel 配置写全局串行
        // spawn 晚于 SINGLETON.set，任务内 get() 必然就绪
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    change = command_rx.recv() => {
                        match change {
                            Some(ConfigChange::ApplyKey { channel_id, agent_id, role_name, mode, done }) => {
                                let coordinator = Nexus::get();
                                let rst = coordinator.apply_channel_key(&channel_id, agent_id, role_name, mode).await;
                                let _ = done.send(rst);
                            }
                            // 任一队列关闭则消费者退出（进程内 tx 存于单例不会发生，break 仅防御）
                            None => break,
                        }
                    }
                    task = channel_task_rx.recv() => {
                        match task {
                            Some(ChannelTask { cmd, done }) => {
                                let coordinator = Nexus::get();
                                let rst = coordinator.apply_channel_command(cmd).await;
                                let _ = done.send(rst);
                            }
                            None => break,
                        }
                    }
                }
            }
        });
```

- [ ] **Step 5: 添加 channel_command 与 apply_channel_command**

在 `change_channel_key` 方法（约 nexus.rs:214）之后追加：

```rust
    /// channel 配置变更统一入口（/bind、/unbind、/bind-outgoing、/bind-outgoing off）：
    /// 排队调 ChannelManager 方法执行，与 change_channel_key 同一消费者串行；返回时已生效
    pub async fn channel_command(&self, cmd: ChannelCommand) -> Result<()> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        self.channel_task_tx.send(ChannelTask { cmd, done: done_tx })
            .map_err(|_| Error::InternalError("变更队列已关闭".to_string()))?;
        done_rx.await.map_err(|_| Error::InternalError("变更处理中断".to_string()))?
    }
```

在 `apply_channel_key` 方法（约 nexus.rs:245）之后追加：

```rust
    /// channel 配置变更执行（队列内串行，不对外）：分发到 ChannelManager 方法
    async fn apply_channel_command(&self, cmd: ChannelCommand) -> Result<()> {
        match cmd {
            ChannelCommand::BindUser { channel_id, user } => self.channel_manager.bind_user(&channel_id, &user).await,
            ChannelCommand::UnbindUser { channel_id, user } => self.channel_manager.unbind_user(&channel_id, &user).await,
            ChannelCommand::BindOutgoing { channel_id, params } => self.channel_manager.bind_outgoing(&channel_id, &params).await,
            ChannelCommand::ClearOutgoing { channel_id } => self.channel_manager.clear_outgoing(&channel_id).await,
        }
    }
```

- [ ] **Step 6: 编译 + 测试验证**

Run: `cargo check && cargo test`
Expected: `Finished`；`test result: ok. 131 passed; 0 failed`

- [ ] **Step 7: 提交**

```bash
git add kissbot-agent/src/nexus.rs
git commit -m "feat(nexus): ChannelTask 队列 + channel_command 入口——与 change_channel_key 队列经 select! 合并串行消费"
```

---

### Task 4: CommandRouter 4 个命令分支改走 nexus.channel_command

**Files:**
- Modify: `kissbot-agent/src/command_router.rs`（import 区 + execute 的 Bind/Unbind/BindOutgoing 分支）

**Interfaces:**
- Consumes: Task 3 的 `Nexus::channel_command(cmd: ChannelCommand) -> Result<()>`、Task 1 的 `ChannelCommand`
- Produces: 无（最终任务）

- [ ] **Step 1: 调整 imports**

`kissbot-agent/src/command_router.rs` 当前：

```rust
use std::sync::Arc;

use crate::types::{AdminCommand, Error, Mode, OutChannelParams, RESERVED_AGENT_ID, Result};
use crate::config_manager::{ConfigManager, OutChannelConfig, ProviderModel};
```

改为（删 Arc 与 OutChannelConfig，加 ChannelCommand；ConfigManager 仍用于 check_admin）：

```rust
use crate::types::{AdminCommand, ChannelCommand, Error, Mode, OutChannelParams, RESERVED_AGENT_ID, Result};
use crate::config_manager::{ConfigManager, ProviderModel};
```

- [ ] **Step 2: 改写 /bind 分支**

现有（约 command_router.rs:169-176）：

```rust
            AdminCommand::Bind { messenger_id, user_id } => {
                ConfigManager::get().update_channel(channel_id, |c| {
                    // HashSet 天然去重：已存在则幂等忽略
                    let cu = ChannelUser { messenger_id: messenger_id.clone(), user_id: user_id.clone() };
                    Arc::make_mut(&mut c.bind_users).insert(cu);
                }).await?;
                Ok(format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id))
            }
```

改为：

```rust
            AdminCommand::Bind { messenger_id, user_id } => {
                let cu = ChannelUser { messenger_id: messenger_id.clone(), user_id: user_id.clone() };
                // 统一走串行队列应用（防写-写竞态；bind_users 追加，HashSet 天然去重幂等）
                nexus.channel_command(ChannelCommand::BindUser { channel_id: channel_id.to_string(), user: cu }).await?;
                Ok(format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id))
            }
```

- [ ] **Step 3: 改写 /unbind 分支**

现有（约 command_router.rs:178-188）：

```rust
            AdminCommand::Unbind { messenger_id, user_id } => {
                ConfigManager::get().update_channel(channel_id, |c| {
                    // 移除指定 ChannelUser
                    let cu = ChannelUser { messenger_id: messenger_id.clone(), user_id: user_id.clone() };
                    Arc::make_mut(&mut c.bind_users).remove(&cu);
                    // 移除的是 outgoing 引用身份则清空 outgoing（避免悬空引用）
                    if let Some(out) = &c.outgoing {
                        if out.messenger_id.as_str() == messenger_id && out.user_id.as_str() == user_id {
                            c.outgoing = None;
                        }
                    }
                }).await?;
                Ok(format!("✅ 已移除 channel 用户: {} / {}", messenger_id, user_id))
            }
```

改为：

```rust
            AdminCommand::Unbind { messenger_id, user_id } => {
                let cu = ChannelUser { messenger_id: messenger_id.clone(), user_id: user_id.clone() };
                // 统一走串行队列应用（防写-写竞态；移除 bind_users，outgoing 引用该身份则一并清空）
                nexus.channel_command(ChannelCommand::UnbindUser { channel_id: channel_id.to_string(), user: cu }).await?;
                Ok(format!("✅ 已移除 channel 用户: {} / {}", messenger_id, user_id))
            }
```

- [ ] **Step 4: 改写 /bind-outgoing 分支**

现有（约 command_router.rs:235-267，`AdminCommand::BindOutgoing(params)` 整个 match）：

```rust
            AdminCommand::BindOutgoing(params) => {
                match params {
                    Some(p) => {
                        // 1. 校验 ChannelUser 已绑定（src 按 channel_id 直接 map get，O(1) 不遍历）
                        let channels = ConfigManager::get().channels().await;
                        let src = ConfigManager::get().channel(channel_id).await
                            .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
                        let cu = ChannelUser { messenger_id: p.messenger_id.clone(), user_id: p.user_id.clone() };
                        let bound = src.bind_users.contains(&cu);
                        if !bound {
                            return Err(Error::InvalidCommand(format!(
                                "ChannelUser 未绑定: {} / {}", p.messenger_id, p.user_id)));
                        }
                        // 2. 清空同 (agent_id, role_name) 其他 channel 的 outgoing（保证至多 1 个）
                        for (cid, c) in channels.iter() {
                            if cid != channel_id && c.agent_id == src.agent_id && c.role_name == src.role_name {
                                if c.outgoing.is_some() {
                                    ConfigManager::get().update_channel(cid, |cc| cc.outgoing = None).await?;
                                }
                            }
                        }
                        // 3. 设来源 channel 的 outgoing
                        ConfigManager::get().update_channel(channel_id, |c| {
                            c.outgoing = Some(Arc::new(OutChannelConfig {
                                messenger_id: Arc::new(p.messenger_id.clone()),
                                user_id: Arc::new(p.user_id.clone()),
                                group_id: Arc::new(p.group_id.clone()),
                            }));
                        }).await?;
                        Ok(format!("✅ 已设发送通道: {} / {} -> {}", p.messenger_id, p.user_id, p.group_id))
                    }
                    None => {
                        ConfigManager::get().update_channel(channel_id, |c| c.outgoing = None).await?;
                        Ok("✅ 已取消发送通道（只存不回复）".to_string())
                    }
                }
            }
```

改为：

```rust
            AdminCommand::BindOutgoing(params) => {
                match params {
                    Some(p) => {
                        // 校验 + 清同 agent/role 其他 channel + 设来源全部移入队列内 ChannelManager.bind_outgoing 原子执行
                        let reply = format!("✅ 已设发送通道: {} / {} -> {}", p.messenger_id, p.user_id, p.group_id);
                        nexus.channel_command(ChannelCommand::BindOutgoing { channel_id: channel_id.to_string(), params: p }).await?;
                        Ok(reply)
                    }
                    None => {
                        nexus.channel_command(ChannelCommand::ClearOutgoing { channel_id: channel_id.to_string() }).await?;
                        Ok("✅ 已取消发送通道（只存不回复）".to_string())
                    }
                }
            }
```

- [ ] **Step 5: 编译 + 测试验证**

Run: `cargo check && cargo test`
Expected: `Finished`；`test result: ok. 131 passed; 0 failed`（parse 相关测试不受影响；若出现 dead_code 警告（如 ConfigManager 部分方法），确认后保留——update_channel 仍有 apply_channel_key 使用，不应告警）

- [ ] **Step 6: 提交**

```bash
git add kissbot-agent/src/command_router.rs
git commit -m "refactor(command_router): bind/unbind/bind-outgoing 改走 nexus.channel_command——删除直接 update_channel 与快照读取"
```

---

## 验证收尾

- [ ] **Step 1: 全量验证**

Run: `cargo check && cargo test`
Expected: 编译通过，`test result: ok. 131 passed; 0 failed`

- [ ] **Step 2: 确认无残留直接写路径**

Run: `rg -n "update_channel" kissbot-agent/src/command_router.rs`
Expected: 无输出（CommandRouter 不再直接调 update_channel；ConfigManager::get() 仅剩 check_admin 读取）

Run: `rg -n "OutChannelConfig" kissbot-agent/src/command_router.rs`
Expected: 无输出

## 参考

- Spec: `docs/superpowers/specs/2026-08-18-channel-command-queue-design.md`
- 现有队列模式：`kissbot-agent/src/nexus.rs` 的 `ConfigChange::ApplyKey` / `change_channel_key` / `apply_channel_key`
