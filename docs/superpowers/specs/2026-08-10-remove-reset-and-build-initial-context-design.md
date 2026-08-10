# 移除 /reset 命令与 build_initial_context，role 上下文构建抽为独立方法

日期：2026-08-10
组件：kissbot-agent

## 背景

`/reset` 管理命令的语义在整条路径上都不对：会话上下文的新建、重置、压缩三条路径各自管理
持久化状态，`/reset` 额外插入一条"重置"路径，与 SessionContext 一体化（内存+缓存+历史一体，
归档与清空永远配对）的设计冲突。本次：

1. 移除 `/reset` 命令整条路径（含 `CommandEffect` 枚举）
2. 移除 `build_initial_context`，role 模式上下文构建抽为独立方法 `build_role_context`
   （新建 / 溢出重置共用同一路径）
3. event 模式新建直接 `recover_from_cache`（不清理；重置已随 /reset 移除，event 溢出走压缩）
4. `SessionContext::reset()` 删除

## 改动

### 1. 命令基础设施删除

- `types.rs`：删除 `AdminCommand::Reset` 变体；**整个删除** `CommandEffect` 枚举
- `command_router.rs`：删除 `"reset"` 解析分支与 `AdminCommand::Reset` 执行分支；
  `execute` 返回 `Result<String>`（各分支去掉 `, CommandEffect::None` 后缀）
- `coordinator.rs`：`handle_admin_command` 删除 effect 匹配，execute 结果直接作为回复；
  删除 `reset_session_for`
- `docs/spec/kissbot-agent-nexus.md`：删除 /reset 命令表行

### 2. ensure_session 新建分支内联（原 build_initial_context 拆除）

`ensure_session` 的 `if created` 分支按模式构建上下文：

- **event**：直接 `recover_from_cache()`（不清理）
- **role**：`self.build_role_context(session)`

之后内联两块公共逻辑（原 build_initial_context 尾部）：

- 系统消息 `set_system_message`（保留 agent 用默认提示词；其余 `load_ego_info` → 待定，
  下次发送前 `apply_pending_system` 对比应用）
- `read_memory_struct_index`（memory-struct 未实现时静默跳过）

### 3. 新方法 build_role_context（coordinator，新建/溢出重置共用）

1. **查询记忆**：`read_recent_for_context` + `pack_memory_messages`
   （组合查询 + 每组合全史查询 + 并集算法，打包为一条 user 消息）
2. **`archive_and_clear_cache()`**：归档旧上下文（含系统消息，System 首行）并清空缓存；
   新建时无内容幂等跳过
3. **`rebuild(packed 或空)`**：清空内存 → 装入打包消息 → 从内存写回缓存（无消息则不落盘）

### 4. run_agentic_loop 溢出重建（Role/Event 共通尾部）

```
溢出时：
  warn(...)
  match mode {
    Mode::Event(_) => compress_context()          // 内部已有"已压缩"日志
    Mode::Role     => build_role_context() + info("会话上下文已重置")
  }
  尾部共通：Trigger::Forced 强制 flush
  （不检查 deadline；重建期间到达的消息即刻并入新上下文；原 reset_context 尾部动作，Role/Event 共用）
```

原 `reset_context`（reset + build_initial_context + flush）整个删除。

### 5. SessionContext 删除 reset()

`reset()`（归档 + 清缓存 + 清内存）职责已被 `archive_and_clear_cache` + `rebuild` 覆盖，
唯一调用方 `reset_context` 已删除。测试同步：

- `append_twice_accumulates_and_reset_clears` 改为 `append_twice_accumulates_and_rebuild_clears`：
  追加不截断验证保留；清空验证改用 `rebuild(vec![])`（role 构建路径的一部分），并验证幂等

### 6. 文档同步

- `docs/spec/kissbot-agent-modules.md`：溢出重建 note（event → compress；role → build_role_context；
  尾部 Forced flush 共通）、管理命令描述去 reset、组件表、`RESERVED_AGENT_NAME` 注释
  （"见 build_initial_context" → "见 ensure_session"）
- `docs/spec/kissbot-agent-nexus.md`：命令表删 /reset 行

## 关键语义

- role 新建与溢出重置**完全同一路径**（build_role_context），不再有独立 reset_context
- event 无重置路径：新建恢复缓存，溢出压缩
- 归档与清空始终配对（archive_and_clear_cache）；rewrite_cache 只写回不清理
- SessionContext API 收敛：append / recover_from_cache / archive_and_clear_cache /
  rebuild / apply_pending_system / build / is_overflow / set_system_message

## 测试

- 单元测试：追加不截断 + rebuild 清空（替代原 reset 测试）
- 现有 109 个测试应全部继续通过（仅 reset 相关测试改名调整）

## 验证

- `cargo test` 全部通过
- `cargo clippy --all-targets` 警告数不增加（基线 55）
- grep 确认无残留：`/reset`、`CommandEffect`、`AdminCommand::Reset`、`reset_context`、
  `reset_session_for`、`build_initial_context`、`SessionContext::reset`
