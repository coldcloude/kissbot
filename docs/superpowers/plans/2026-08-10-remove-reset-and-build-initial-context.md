# 移除 /reset 命令与 build_initial_context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 移除 /reset 命令整条路径与 build_initial_context，role 上下文构建抽为 build_role_context（新建/溢出重置共用），SessionContext 删除 reset()。

**Architecture:** 会话上下文由 SessionContext 一体化管理（内存+缓存+历史，归档与清空永远配对）。本次删除管理命令中的 reset 及其 CommandEffect 机制；将 role 模式上下文构建从 build_initial_context 中抽出为 build_role_context（查询记忆 → archive_and_clear_cache → rebuild），event 新建直接 recover_from_cache；run_agentic_loop 溢出重建时 Role/Event 共用尾部 Forced flush。

**Tech Stack:** Rust / tokio / kissbot-agent crate

## Global Constraints

- 项目规则：**不要删除代码中的注释**——被删方法/分支的注释如有价值，移到新位置或改写语义，不得无声丢失
- 全部文本 UTF-8、\n 换行
- 提交 comment 用中文，覆盖该提交全部改动
- 每任务结束 `cargo test` 全绿 + `cargo clippy --all-targets` 警告数不增加（基线 55）+ 提交

---

### Task 1: 命令基础设施删除（types / command_router / coordinator / nexus 文档）

**Files:**
- Modify: `kissbot-agent/src/types.rs`（AdminCommand 枚举、CommandEffect 枚举）
- Modify: `kissbot-agent/src/command_router.rs`（import、parse、execute 签名与全部分支）
- Modify: `kissbot-agent/src/coordinator.rs`（handle_admin_command、reset_session_for）
- Modify: `docs/spec/kissbot-agent-nexus.md`（/reset 命令表行）

**Interfaces:**
- Consumes: 无
- Produces: `CommandRouter::execute(&AdminCommand, &ConfigManager, &str) -> Result<String>`（不再返回 CommandEffect）

- [ ] **Step 1: types.rs 删除 Reset 变体与 CommandEffect 枚举**

在 `kissbot-agent/src/types.rs` 中：

a) `AdminCommand` 枚举中删除 `    Reset,` 行（位于 `Events,` 与 `Model(...)` 之间）。

b) 整个删除以下块（位于 OutChannelParams 之后、"模型相关" 之前）：

```rust
// ========== 管理命令执行效果 ==========

/// 命令执行后协调器需做的后续动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEffect {
    None,
    /// 重置来源 channel 所属会话的上下文
    ResetSession,
}
```

- [ ] **Step 2: command_router.rs 删除 reset 解析与执行分支、改 execute 签名**

在 `kissbot-agent/src/command_router.rs` 中：

a) import 行去掉 CommandEffect：
```rust
use crate::types::{AdminCommand, CommandEffect, Error, Mode, OutChannelParams, Result, SessionKey};
```
→
```rust
use crate::types::{AdminCommand, Error, Mode, OutChannelParams, Result, SessionKey};
```

b) 解析分支删除：
```rust
            "events" => Ok(AdminCommand::Events),
            "reset" => Ok(AdminCommand::Reset),
            "model" => {
```
→
```rust
            "events" => Ok(AdminCommand::Events),
            "model" => {
```

c) execute 签名：
```rust
    ) -> Result<(String, CommandEffect)> {
```
→
```rust
    ) -> Result<String> {
```

d) 删除 Reset 执行分支：
```rust
            AdminCommand::Reset => {
                Ok(("🔄 正在重置上下文...".to_string(), CommandEffect::ResetSession))
            }
```

e) 全部分支返回值去掉 `, CommandEffect::None)` 后缀：把每个
`Ok((X, CommandEffect::None))` 改为 `Ok(X)`，`Ok(("文字".to_string(), CommandEffect::None))` 改为 `Ok("文字".to_string())`。
共 13 处（execute 内 12 处 + Events 分支 1 处），逐条改为：
- `Ok((format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id), CommandEffect::None))` → `Ok(format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id))`
- `Ok((format!("✅ 已移除 channel 用户: {} / {}", messenger_id, user_id), CommandEffect::None))` → 同上模式
- `Ok((format!("✅ 已添加管理权限: {} / {}", messenger_id, user_id), CommandEffect::None))` → 同上模式
- `Ok((format!("✅ 已移除管理权限: {} / {}", messenger_id, user_id), CommandEffect::None))` → 同上模式
- `Ok((format!("✅ 已设置 agent: {} / role: {}", new_agent, new_role), CommandEffect::None))` → 同上模式
- `Ok((format!("✅ 已设置 role: {}", new_role), CommandEffect::None))` → 同上模式
- `Ok((format!("✅ 新事件 ID: {}", id), CommandEffect::None))` → 同上模式
- `Ok(("✅ 已切换为角色模式".to_string(), CommandEffect::None))` → `Ok("✅ 已切换为角色模式".to_string())`
- `Ok((format!("✅ 将重进事件: {}", event_id), CommandEffect::None))` → 同上 format 模式
- `Ok((format!("✅ 已设发送通道: {} / {} -> {}", p.messenger_id, p.user_id, p.group_id), CommandEffect::None))` → 同上 format 模式
- `Ok(("✅ 已取消发送通道（只存不回复）".to_string(), CommandEffect::None))` → `Ok("✅ 已取消发送通道（只存不回复）".to_string())`
- `Ok((reply, CommandEffect::None))`（Events 分支）→ `Ok(reply)`
- `Ok((reply, CommandEffect::None))`（Model 分支）→ `Ok(reply)`

- [ ] **Step 3: coordinator.rs handle_admin_command 去 effect 匹配、删除 reset_session_for**

在 `kissbot-agent/src/coordinator.rs` 中：

a) handle_admin_command 内：
```rust
                match CommandRouter::execute(&cmd, &self.config, channel_id).await {
                    Ok((reply, effect)) => {
                        // 回复：系统命令始终发回来源 channel（不走 out_channel）
                        self.send_admin_reply(channel_id, event, reply).await;

                        // 应用命令执行效果
                        match effect {
                            crate::types::CommandEffect::ResetSession => {
                                self.reset_session_for(channel_id).await;
                            }
                            crate::types::CommandEffect::None => {}
                        }
                    }
```
→
```rust
                match CommandRouter::execute(&cmd, &self.config, channel_id).await {
                    Ok(reply) => {
                        // 回复：系统命令始终发回来源 channel（不走 out_channel）
                        self.send_admin_reply(channel_id, event, reply).await;
                    }
```

b) 整个删除 reset_session_for 方法（含 doc 注释）：
```rust
    /// 重置来源 channel 所属会话的上下文
    async fn reset_session_for(&self, channel_id: &str) {
        if let Some(ch) = self.config.channel(channel_id).await {
            let key = self.session_key_for(&ch);
            if let Some(session) = self.session_manager.get(&key) {
                self.reset_context(&session).await;
                return;
            }
        }
        warn!("reset: channel {} 无会话可重置", channel_id);
    }
```

- [ ] **Step 4: nexus.md 删除 /reset 命令表行**

在 `docs/spec/kissbot-agent-nexus.md` 中删除包含 `reset` 的命令表行（形如 `| reset | ... | 上下文重置 |`）。

- [ ] **Step 5: 编译验证 + 提交**

```bash
cd /home/admin/project/kissbot/kissbot-agent && cargo test 2>&1 | grep -E "^test result"
```
预期：`test result: ok. N passed`（数量不变或按现有测试数），无编译错误。

```bash
git add -A && git commit -m "refactor(agent): 删除 /reset 命令整条路径与 CommandEffect 机制（execute 只返回回复文本）"
```

---

### Task 2: coordinator 上下文构建重构（build_role_context + ensure_session 内联 + 溢出路径）

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`

**Interfaces:**
- Consumes: `SessionContext::{recover_from_cache, archive_and_clear_cache, rebuild, set_system_message}`（现行 API，Task 3 之前均存在）、`MemoryReader::read_recent_for_context`、`pack_memory_messages`、`MemoryReader::read_memory_struct_index`
- Produces: `fn build_role_context(&self, session: &Arc<Session>)`（私有方法，新建/溢出共用）；`ensure_session` 的 `if created` 分支内联构建；溢出分支共用 Forced flush；**删除 `build_initial_context` 与 `reset_context`（后者是 `SessionContext::reset()` 的唯一调用方，Task 3 删 reset() 的前提）**

- [ ] **Step 1: 新增 build_role_context 方法**

在 `kissbot-agent/src/coordinator.rs` 中 build_initial_context 所在位置（ensure_session 之后）新增：

```rust
    /// role 模式上下文构建（新建/溢出重置共用）：查询记忆打包 → 归档旧上下文+清空缓存（内部幂等）→ 重建
    /// 取记忆用会话状态保存的 agent_id（session_key 仅去重，不从 key 提取 agent_name）
    async fn build_role_context(&self, session: &Arc<Session>) {
        // 记忆打包：组合查询 + 每组合全史查询 + 并集算法（最后 N 条 ∪ [M, T_N] 同时间组，窗口内早于 T_N 的记录不含），打包为一条 user 消息作为首条内容
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        let packed = if let Ok(msgs) = self.memory_reader
            .read_recent_for_context(session.agent_id.as_str(), session.role_name.as_str(), &cfg)
            .await
        {
            pack_memory_messages(&msgs)
        } else {
            None
        };
        // 归档旧上下文（新建时无内容幂等跳过）+ 清空缓存 → 重建（清空内存 + 从内存写回缓存；无消息不落盘）
        let _ = session.context.lock().await.archive_and_clear_cache().await;
        let msgs = packed.map(|m| vec![m]).unwrap_or_default();
        let _ = session.context.lock().await.rebuild(msgs).await;
    }
```

- [ ] **Step 2: ensure_session 的 if created 分支内联**

将 `kissbot-agent/src/coordinator.rs` 中：

```rust
        let (session, created) = self.session_manager.get_or_create(key, model, agent_id);
        if created {
            self.build_initial_context(&session).await;
        }
        (session, created)
```
→
```rust
        let (session, created) = self.session_manager.get_or_create(key, model, agent_id);
        if created {
            // 新建会话上下文：event 从缓存恢复（不清理）；role 查询记忆重建（归档+清空在 build_role_context 内部）
            match &*session.mode {
                Mode::Event(_) => {
                    let _ = session.context.lock().await.recover_from_cache().await;
                }
                Mode::Role => {
                    self.build_role_context(&session).await;
                }
            }
            // 系统消息：保留 agent（agent_id="0"）用 NexusRepo 默认系统提示词；其余走 load_ego_info。
            // 生成结果执行一次 set（待定，下次发送前对比应用；与缓存恢复的系统不一致时旧上下文先归档）
            if session.agent_id.as_str() == RESERVED_AGENT_ID {
                let prompt = self.config.default_system_prompt().await;
                session.context.lock().await.set_system_message(prompt);
            } else if let Ok(ego_info) = self.load_ego_info(session.agent_id.as_str(), &session.role_name).await {
                session.context.lock().await.set_system_message(ego_info);
            }
            // 顶层记忆索引（memory-struct 未实现时静默跳过）
            let _ = self.memory_reader
                .read_memory_struct_index(&self.config, session.agent_id.as_str(), &session.role_name, &session.mode)
                .await;
        }
        (session, created)
```

- [ ] **Step 3: 删除 build_initial_context 整个方法**

删除 `kissbot-agent/src/coordinator.rs` 中的整个 `build_initial_context` 方法（doc 注释 + 方法体，其 event 分支、role 分支、系统消息 set、read_memory_struct_index 四块已分别移入 ensure_session / build_role_context）。

同时更新文件顶部 RESERVED_AGENT_NAME 注释：

```rust
/// 保留 agent/role：agent_name 为空 = 保留 agent（建会话但初始上下文用默认系统提示词，见 build_initial_context）；
```
→
```rust
/// 保留 agent/role：agent_name 为空 = 保留 agent（建会话但初始上下文用默认系统提示词，见 ensure_session）；
```

- [ ] **Step 4: 溢出重建路径（run_agentic_loop）**

将 `kissbot-agent/src/coordinator.rs` 中 run_agentic_loop 尾部的：

```rust
        if overflow {
            warn!("会话上下文超长，触发重置: role={} mode={:?}", session.role_name, session.mode);
            // 按模式处理：event 超长压缩（LLM 总结归档），role 归档后从记忆重建
            match &*session.mode {
                Mode::Event(_) => self.compress_context(session).await,
                Mode::Role => self.reset_context(session).await,
            }
        }
```
→
```rust
        if overflow {
            warn!("会话上下文超长，触发重建: role={} mode={:?}", session.role_name, session.mode);
            // 按模式重建：event 超长压缩（LLM 总结归档）；role 从记忆重建（新建/重置共用 build_role_context）
            match &*session.mode {
                Mode::Event(_) => self.compress_context(session).await,
                Mode::Role => {
                    self.build_role_context(session).await;
                    info!("会话上下文已重置: role={} mode={:?}", session.role_name, session.mode);
                }
            }
            // 重建完成：强制 flush（不检查 deadline），重建期间到达的消息即刻并入新上下文（Role/Event 共通）
            // （重建可能由 trigger 任务的 flush → run_agentic_loop 溢出路径调用，Forced 入队后由任务串行处理）
            session.batch_producer.trigger_tx.send(crate::session_manager::Trigger::Forced).ok();
        }
```

- [ ] **Step 5: 删除 reset_context 整个方法**

删除 `kissbot-agent/src/coordinator.rs` 中的整个 `reset_context` 方法（doc 注释 + 方法体；其 Forced flush 已移入溢出分支尾部、info 日志已移入 Role 分支、archive+clear+rebuild 由 build_role_context 承担）。

- [ ] **Step 6: 测试通过 + 提交**

```bash
cd /home/admin/project/kissbot/kissbot-agent && cargo test 2>&1 | grep -E "^test result"
```
预期：全部通过（109 个）。

```bash
git add -A && git commit -m "refactor(agent): 删除 build_initial_context 与 reset_context，role 上下文构建抽为 build_role_context（查询记忆→归档清空→rebuild），新建/溢出共用；event 新建直接 recover_from_cache；溢出尾部 Forced flush Role/Event 共通"
```

---

### Task 3: SessionContext 删除 reset() + 测试调整

**Files:**
- Modify: `kissbot-agent/src/session_manager.rs`

**Interfaces:**
- Consumes: Task 2 已删除 `reset_context`（`SessionContext::reset()` 的唯一调用方）
- Produces: `SessionContext` 公共方法收敛为：`new` / `set_system_message` / `apply_pending_system` / `system_message` / `append` / `push` / `recover_from_cache` / `archive_and_clear_cache` / `rebuild` / `build` / `len` / `is_overflow`

- [ ] **Step 1: 删除 reset() 方法**

在 `kissbot-agent/src/session_manager.rs` 中删除：

```rust
    /// 重置上下文：归档当前内存（含当前系统消息）→ 清空缓存 → 清空内存（system 保留，待下次发送前对比替换）
    pub async fn reset(&mut self) -> Result<()> {
        self.archive_and_clear_cache().await?;
        self.messages.clear();
        Ok(())
    }
```

（reset() 职责已被 archive_and_clear_cache + rebuild 覆盖，role 构建/压缩路径统一走这两者。）

- [ ] **Step 2: 改写测试 append_twice_accumulates_and_reset_clears**

将 `kissbot-agent/src/session_manager.rs` 中 `append_twice_accumulates_and_reset_clears` 整个改为（用 rebuild 替代 reset 语义）：

```rust
    #[tokio::test]
    async fn append_twice_accumulates_and_rebuild_clears() {
        let dir = tempfile::tempdir().unwrap();
        let k = cache_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        ctx.append(&sample_msgs()).await.unwrap();
        ctx.append(&[Message::User { content: Arc::new("再问".into()) }]).await.unwrap();
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), &k);
        recovered.recover_from_cache().await.unwrap();
        assert_eq!(recovered.len(), 3, "追加不截断");
        // 重建（role 构建路径的一部分）：rebuild 空消息 → 清空内存，缓存不残留
        ctx.rebuild(vec![]).await.unwrap();
        assert!(ctx.build().is_empty(), "rebuild 清空内存");
        let mut after = SessionContext::new(dir.path().to_str().unwrap(), &k);
        after.recover_from_cache().await.unwrap();
        assert!(after.build().is_empty(), "rebuild 后缓存为空");
        // 无内容时 rebuild 幂等
        ctx.rebuild(vec![]).await.unwrap();
    }
```

- [ ] **Step 3: 测试通过 + 提交**

```bash
cd /home/admin/project/kissbot/kissbot-agent && cargo test 2>&1 | grep -E "^test result"
```
预期：全部通过。

```bash
git add -A && git commit -m "refactor(agent): SessionContext 删除 reset()（职责由 archive_and_clear_cache + rebuild 覆盖），测试改用 rebuild 验证清空"
```

---

### Task 4: 文档同步 + 残留验证

**Files:**
- Modify: `docs/spec/kissbot-agent-modules.md`

**Interfaces:**
- Consumes: 无（纯文档）

- [ ] **Step 1: agent-modules.md 更新**

在 `docs/spec/kissbot-agent-modules.md` 中：

a) 组件表 command_router 行：描述中的命令列表若含 `/reset` 则去掉（检查实际文本，形如 "（/bind、/mode、/model、/reset 等）" → "（/bind、/mode、/model 等）"）。

b) 组件表 session_manager 行：管理命令效果描述若提到 reset 则去掉。

c) 第五节（Agentic Loop）溢出 note：
```
            Note over CO: 溢出检查（effective.max_context_messages）<br/>event → compress_context；role → reset_context
```
→
```
            Note over CO: 溢出检查（effective.max_context_messages）<br/>event → compress_context；role → build_role_context；重建后共通 Forced flush
```

d) 第六节（会话构建与上下文管理）时序图与 Note：将 `build_initial_context` / `reset_context` 相关表述改为新结构：
- `ensure_session` 新建：event → `recover_from_cache()`；role → `build_role_context()`（查询记忆 → `archive_and_clear_cache()` → `rebuild`）
- Note 中 `reset_context：SessionContext.reset()（archive → clear cache → clear 内存）→ build_initial_context → Trigger::Forced 强制 flush` 改为：
  `role 新建/溢出共用 build_role_context（查询记忆 → archive_and_clear_cache → rebuild）；溢出尾部共通 Forced flush`

- [ ] **Step 2: 残留验证**

```bash
cd /home/admin/project/kissbot && rg -n "CommandEffect|ResetSession|reset_session_for|reset_context|build_initial_context" kissbot-agent/src/ ; rg -n "/reset|CommandEffect|ResetSession" docs/spec/
```
预期：全部无输出。

```bash
cd /home/admin/project/kissbot/kissbot-agent && rg -n "\.reset\(\)" src/
```
预期：无输出。

- [ ] **Step 3: 全量验证 + 提交**

```bash
cd /home/admin/project/kissbot/kissbot-agent && cargo test 2>&1 | grep -E "^test result" && cargo clippy --all-targets 2>&1 | grep -cE "^warning: "
```
预期：测试全绿；警告数 55（不增加）。

```bash
git add -A && git commit -m "docs: 同步移除 /reset 后的命令描述与上下文构建/溢出时序（build_role_context 共用、共通 Forced flush）"
```

---

## Self-Review

**Spec 覆盖：**
- 命令基础设施删除（AdminCommand::Reset、CommandEffect、router、handle_admin_command、reset_session_for、nexus.md）→ Task 1 ✓
- ensure_session 内联（event recover / role build_role_context + 系统消息 set + read_memory_struct_index）→ Task 2 Step 2 ✓
- build_role_context（查询记忆 → archive_and_clear_cache → rebuild，新建/溢出共用）→ Task 2 Step 1 ✓
- 溢出重建（Role/Event 共通 Forced flush 尾部；reset_context 删除）→ Task 2 Step 4-5 ✓
- SessionContext::reset() 删除 + 测试 → Task 3 ✓
- 文档同步 → Task 4 ✓

**类型一致性：** `build_role_context(&self, session: &Arc<Session>)` 在 Task 2 Step 1 定义，Step 2/4 调用，签名一致；`CommandRouter::execute` 返回 `Result<String>` 在 Task 1 定义，coordinator 调用点在 Task 1 Step 3 同步；Task 2 删除 reset_context（SessionContext::reset() 唯一调用方）后，Task 3 才删 reset()，全程可编译。
