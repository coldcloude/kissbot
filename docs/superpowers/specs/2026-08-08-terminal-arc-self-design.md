# Terminal 全局唯一 + Arc<Self> Receiver 重构设计

日期：2026-08-08
状态：已批准（brainstorming 流程）

## 背景与动机

### 现状问题

`connect_channels`（coordinator.rs:650）在循环里**每个 channel 新建一个** `TerminalHandle(coordinator.clone())` 包装成 `Arc<dyn Terminal>`，`ChannelClient` 只持 `Weak<dyn Terminal>`（channel_client.rs:23）。这带来两个问题：

1. **Terminal 语义上应全局唯一（就是 Coordinator）**，但实现上每 channel 新建一个包装对象，靠 spawn 任务内 `let _keepalive = terminal;` 保活（e5f2b42 修复的 Critical 问题）。保活依赖任务体，脆弱且概念混乱。
2. **ChannelClient 分散在 coordinator 的 `channel_clients: DashMap<String, Arc<ChannelClient>>`**，而每 channel 的运行时上下文已归入 `channel_contexts: DashMap<String, Arc<ChannelContext>>`——client 应随上下文内聚。

### 根因

`Terminal trait` 方法 receiver 是 `&self`，而 coordinator 的 Arc 链方法（构造会话需 `Arc::downgrade(self)`）需要 `&Arc<Self>` 形态——trait 里写 `&Arc<Self>` 报 E0038（不可对象化）。因此之前用 `TerminalHandle(Arc<AgentCoordinator>)` 薄适配器桥接。

## 设计决策：`self: Arc<Self>` receiver

**结论：`self: Arc<Self>` 比 `&Arc<Self>` 更符合常规，采用前者。**

依据（brainstorming 评估）：
- `&Arc<Self>` 不在 Rust receiver 允许列表（E0038 不可对象化，trait 里写不了）；生态罕见，语义模糊（既非借用也非拥有）
- `self: Arc<Self>` 自 Rust 1.33 稳定，官方支持；async-trait 0.1.89 官方测试明确覆盖（test.rs:307/1367/1481）；actor/异步生态常见
- 本项目 Terminal trait 回调路径（`Weak.upgrade()` → `Arc<dyn Terminal>`）与 by-value Arc receiver 天然匹配，**ChannelClient 零改动**
- coordinator 内部 Arc 链方法与 trait 方法同形态，类型统一；move 语义有编译器兜底（忘 clone 编译错，非静默 bug）；`self.clone()` 是廉价原子加一

## 具体改动

### 1. Terminal trait 签名（kissbot-channel-client/src/terminal.rs）

6 个方法 receiver `&self` → `self: Arc<Self>`：
`incoming_message` / `join_group` / `leave_group` / `user_removed` / `download_chunk` / `closed`

文档注释更新：说明调用方经 `Weak<dyn Terminal>.upgrade()` 得 `Arc<dyn Terminal>`，与 by-value Arc receiver 匹配；无需 downcast（vtable 分发自动还原具体类型）。

**ChannelClient（channel_client.rs）零改动**——所有调用点（169/176/180/184/188/206/258）都是 `get_terminal()` 每次新 upgrade 出的 `Arc<dyn Terminal>`，调 `self: Arc<Self>` 方法天然匹配（单次 move，无连续调用）。

### 2. 删 TerminalHandle，AgentCoordinator 直接 impl Terminal（coordinator.rs）

- 删 `TerminalHandle` 结构体与 `impl Terminal for TerminalHandle`（当前 727 行起）
- 新增 `impl Terminal for AgentCoordinator`，6 个方法签名 `self: Arc<Self>`：
  - `incoming_message`：原固有方法（758）逻辑搬入（trait 方法 receiver 为 Arc<Self> 后无需固有版本），末尾 `self.handle_incoming(...)` 改 `self.clone().handle_incoming(...)`（若其后无逻辑可 move）
  - `join_group`/`leave_group`/`user_removed`/`download_chunk`：原固有方法体直接搬入，签名改 `self: Arc<Self>`
  - `closed`：原固有方法体搬入（`self.disconnect_notify` 照常）

### 3. coordinator 固有方法 `&Arc<Self>` → `self: Arc<Self>`

8 个方法：`ensure_session`(314) / `relocate_channel`(379) / `apply_channel_key`(589) / `set_session_model`(610) / `connect_channels`(650) / `handle_incoming`(821) / `handle_admin_command`(881)（原 incoming_message 758 成为 trait 方法，不在其列）。

规则：
- 方法体内互相调用 `self.xxx()` → `self.clone().xxx()`（Arc clone 廉价）
- 调用点：coordinator.rs:165/201 `coordinator.clone().xxx()`（循环内保留下轮用）；:205 `coordinator.connect_channels()` 最后调用直接 move
- `CommandRouter::execute`（command_router.rs:172）保持 `coordinator: &Arc<AgentCoordinator>` 不变；handle_admin_command 内调用点传 `&self`（&Arc<Self> 借用，不 move）；execute 内部 `coordinator.set_session_model(...)`（296）改 `coordinator.clone().set_session_model(...)`

### 4. connect_channels 重构（核心目标）

```rust
async fn connect_channels(self: Arc<Self>) {
    // Terminal 即 Coordinator 自身：循环外建一次全局唯一 Terminal 视图，所有 client 弱引用共享同一目标
    let terminal: Arc<dyn Terminal> = coordinator.clone();   // unsize coercion
    for (_, ch) in self.config.channels().await {
        if !ch.enabled { continue; }
        // client 弱引用直接指向 coordinator（不再新建 TerminalHandle）
        let client = ChannelClient::new(channel_id.clone(), Arc::downgrade(&terminal));
        // ChannelClient 归入该 channel 的 ChannelContext（懒建后 bind_client）
        let ctx = coordinator.channel_contexts
            .entry(channel_id.clone()).or_insert_with(|| Arc::new(ChannelContext::new())).clone();
        ctx.bind_client(client.clone());
        tokio::spawn(async move { loop { client.connect(...)... } });   // 删 _keepalive
    }
}
```

- 强引用保活：coordinator 由 command_rx 循环、session_manager 等其余 Arc 持有，connect_channels 结束后 client 的 Weak 仍可 upgrade——**不再需要 `_keepalive`**
- `disconnect_notify` 保持 coordinator 字段（本轮不并入 ChannelContext，最小改动）

### 5. ChannelContext 加 client 字段（coordinator.rs）

```rust
/// 本 channel 的 ChannelClient（connect_channels 时绑定；消息/回复路径从本字段取 client，
/// ArcSwapOption 无锁读写——连接与回调并发访问安全）
client: ArcSwapOption<ChannelClient>,
```

- `new()` 初始化 `None`；方法 `fn bind_client(&self, client: Arc<ChannelClient>)`（store）、`fn client(&self) -> Option<Arc<ChannelClient>>`（load_full）
- 删 coordinator 的 `channel_clients` 字段与 new 初始化
- 使用点改从 channel_contexts 取：
  - `send_admin_reply`(917)：`self.channel_contexts.get(channel_id).and_then(|ctx| ctx.client())`
  - `send_outgoing`(1228)：同上（out_channel.channel_id）

### 6. CLI 同步（kissbot-channel-client-cli/src/main.rs）

`impl Terminal for CliTerminal`（110）6 个方法签名改 `self: Arc<Self>`，方法体不变（Arc deref 访问字段照常；closed 里 exit 照常）。

## 影响面与边界

- **改动文件**：terminal.rs（trait）、cli main.rs（impl 签名）、coordinator.rs（主体）、command_router.rs（1 处调用点）
- **零改动**：channel_client.rs、session_manager.rs（run_agentic_loop / resolve_out_channel_for_session 是 `&self`，accept_batch 调用不变）
- `impl Terminal` 全项目仅 2 处（agent + cli），无其他实现者

## 验证

- `cd kissbot-channel-client && cargo check`（trait 改动 + ChannelClient 调用点兼容）
- `cd kissbot-channel-client-cli && cargo build`
- `cd kissbot-agent && cargo test`（106 测试基线）+ `cargo build` 0 warnings
- 手动确认：connect_channels 每 channel 一个 client、共享同一 Terminal 弱引用目标；send_admin_reply/send_outgoing 从 ChannelContext 取 client

## 不做的事（YAGNI）

- 不把 `disconnect_notify` 并入 ChannelContext（用户仅要求 client）
- 不改 ChannelClient 的 `Weak<dyn Terminal>` 存储形态（upgrade 每次得新 Arc，与 Arc<Self> receiver 匹配）
- 不引入 coordinator 自引用 weak（c0461a5 已删除的模式，方向相反）
