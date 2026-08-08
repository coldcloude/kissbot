# kissbot-agent 通道适配层重构设计

- 日期：2026-08-08
- 范围：kissbot-agent（coordinator / channel_manager / session_manager / command_router / main）、kissbot-channel-client（Terminal trait）、kissbot-channel-client-cli

## 一、背景与目标

当前 kissbot-agent 的通道相关实现（Terminal 回调、连接/重连、回显判定、发送、合批生产侧绑定）全部耦合在 AgentCoordinator 中。本次重构目标：

1. **ChannelManager 实现 Terminal，成为通道适配层**（连接/重连/回显过滤/发送封装），Coordinator 不再接触 ChannelClient；
2. **BatchProducer 从 Channel 删除**：合批入队直取 `session.batch_producer`，无 Channel 中转；
3. **AgentCoordinator 改为进程级单例**（`OnceLock<AgentCoordinator>` 存值）：Session / Channel 不再传参或保存 coordinator 引用，消除参数穿透与弱引用链；
4. **Terminal trait receiver 从 `self: Arc<Self>` 改为 `&self`**；
5. Coordinator 所有方法统一改 `&self`。

## 二、决策记录（brainstorming 结论）

| # | 决策点 | 结论 |
|---|--------|------|
| Q1 | join/leave/user_removed/download_chunk 归属 | 全部归 ChannelManager 自处理（no-op 占位）。服务端已把有业务意义的事件（群组变更、用户移除等）转化为 IncomingMessage 推送，Terminal 回调层不重复处理业务 |
| Q2 | ChannelManager 与 config 关系 | ChannelManager 构造时存入 `Arc<ConfigManager>`（重连循环实时读 bind_users，/bind 回写后重连即生效）；依赖方向 `ChannelManager → config_manager`，叶子依赖无环 |
| Q3 | coordinator 弱引用如何传递 | 不用后置 OnceLock、不做参数穿透：AgentCoordinator 改全局单例（`OnceLock<AgentCoordinator>` 存值），Session / Channel 不存 weak，需要时 `AgentCoordinator::instance()` |
| Q4 | 测试策略 | 转发路径不单独单测（升级真实 Coordinator 太重，与项目现有模式一致）；不引入 mock trait |
| Q5 | Terminal receiver | `&self`（对象安全成立；三个实现者方法体均不依赖 `Arc<Self>` 的 clone/降级） |
| Q6 | 所有使用 coordinator 的位置 | **一律不传 coordinator 参数，统一从单例获取**：command_router::execute 删 coordinator 参数、session_manager 删 Weak、各模块需要 coordinator 时 `AgentCoordinator::instance()` |

## 三、架构

### 3.1 AgentCoordinator 单例

```rust
static SINGLETON: OnceLock<AgentCoordinator> = OnceLock::new();

impl AgentCoordinator {
    /// 全局单例（进程内唯一；new() 完成后可用）
    pub fn instance() -> &'static AgentCoordinator {
        SINGLETON.get().expect("AgentCoordinator 未初始化")
    }

    /// 构造 + 初始化 + 注册单例；不再返回 Arc
    pub async fn new(config: Arc<ConfigManager>) -> Result<()> {
        // 构造值 → 用 &self 完成全部初始化（bind agent/session）→ SINGLETON.set(self) → Ok(())
    }
}
```

- `&'static` 引用可跨 await、跨任务持有（command_rx 任务、trigger 任务、ChannelManager 转发均直接 `instance()`）；
- `new()` 从返回 `Arc<Self>` 改为 `Result<()>`（值 move 进 OnceLock 后无法再返回 Arc），main 调用方式相应调整；
- 连带收益：Session 删除 coordinator 字段后，`coordinator → session_manager → session` 引用链无环，Weak 全部移除。

### 3.2 组件职责划分

**Channel（叶子，单 channel 运行态）**
- 保留：`pending_outgoing`（回显判定：add_pending/consume_pending/evict）、`agent_id`、`mode`、`client`
- 删除：`producer` 字段及 bind_producer/producer 方法

**ChannelManager（通道适配层）**
- 持有：`channels: DashMap<String, Arc<Channel>>`、`config: Arc<ConfigManager>`、`disconnect_notify: DashMap<String, Arc<Notify>>`（自 coordinator 移入）
- 实现 Terminal（`&self`）：
  - `incoming_message`：`consume_pending` 回显过滤（命中丢弃）→ `AgentCoordinator::instance().incoming_message(...)` 转发业务
  - `closed`：`disconnect_notify` 通知重连循环
  - `join_group` / `leave_group` / `user_removed` / `download_chunk`：no-op 占位
- `connect_all(self: &Arc<Self>)`：遍历 config 中 enabled channel → 创建 `ChannelClient`（`let terminal: Arc<dyn Terminal> = self.clone()`，全部 client 的 Weak 指向同一目标）→ `bind_client` → spawn 重连循环（连接成功后实时读 config 的 bind_users 逐个 bind；`notify.notified()` 等待断线）
- `send(&self, channel_id, msg: OutgoingMessage) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel_client::Error>`：取 client（缺失告警 + `Error::NotConnected`）→ `send_message` → `add_pending(msg_id)` → 返回 response
- 懒建保留且**无 coordinator 参数**：`get_or_create` / `set_agent_id` / `add_pending` / `set_mode`（channel 不存在则懒建）；get 类方法 `client` / `consume_pending` / `agent_id` / `mode` 不变
- 删除：`bind_producer` / `producer`

**Coordinator（业务核心）**
- 删除：Terminal impl、`connect_channels`、`disconnect_notify` 字段、`record_outgoing_msg_id`、`is_self_echo_by_msg_id`、`bind_batch`
- `incoming_message` 改为 `pub(crate) &self` 业务方法（不再见回显）：config 校验（channel 不存在则丢弃）→ 推记忆 is_self=0 → `handle_incoming`
- `handle_incoming`（`&self`）：系统事件过滤 → 管理命令路由（`handle_admin_command`）/ 普通消息（`ensure_session` + `enqueue_batch`）
- `enqueue_batch`（`&self`）：直取 `session.batch_producer` 发数据 + set_deadline + Trigger::At（删除 channel_id 参数与懒绑定路径）
- `send_admin_reply` / `send_outgoing`：改用 `channel_manager.send(...)`（msg_id 的 pending 记录由 send 内部完成），成功后推记忆 is_self=1
- 其余方法全部改 `&self`：`ensure_session` / `apply_channel_key` / `relocate_channel` / `set_session_model` 等（内部 `self.clone().xxx()` 改直接调用）
- command_rx 任务：任务内 `let coordinator = AgentCoordinator::instance();`（&'static）
- `new()`：构造值 → 校验 default_model / 建 station_runtimes → 每 channel `bind_channel_runtime` + `ensure_session` → `SINGLETON.set`（**不做连接**）
- `run()`：`channel_manager.connect_all()`（预建全部 channel + spawn 重连循环）→ 主循环（保持进程）；连接晚于单例注册，消息回调必然在 set 之后

**SessionManager**
- Session 删除 `coordinator: Weak<AgentCoordinator>` 字段
- `get_or_create` / `create_session` 删除 coordinator 参数
- `accept_batch`（trigger 任务 flush 入口）：`AgentCoordinator::instance()` 直接取，替代弱引用升级

**command_router**
- `execute` **删除 coordinator 参数**（原 `&Arc<AgentCoordinator>`），内部经 `AgentCoordinator::instance()` 取单例调用各 coordinator 方法（channel_session_key / resolve_agent_id_for_bind / change_channel_key / set_session_model / list_events 等）
- 辅助函数 `channel_current_key` 同样删除 coordinator 参数，内部取单例

**Terminal trait（kissbot-channel-client）**
- 全部方法 receiver `self: Arc<Self>` → `&self`；原解释 `Arc<Self>` receiver 的注释（trait 定义上方）**直接删除，不新增替代注释**
- 同步更新：`tests/mock.rs`（MockTerminal）、`kissbot-channel-client-cli/src/main.rs`（CliTerminal）的方法签名（方法体不变）
- `channel_client.rs` 调用点无需改动（upgrade 的 `Arc<dyn Terminal>` 自动解引用调 `&self`）

**main.rs**
- `coordinator::AgentCoordinator::new(config.clone()).await.expect("初始化 Coordinator 失败");`
- `coordinator::AgentCoordinator::instance().run().await;`

### 3.3 数据流

```
启动：AgentCoordinator::new
  → 构造值 → 校验 default_model → 建 station_runtimes
  → 每 channel：bind_channel_runtime（写 agent_id）+ ensure_session（建会话）
  → SINGLETON.set(self) → Ok(())

运行：AgentCoordinator::instance().run()
  → channel_manager.connect_all()（预建全部 Channel + spawn 重连循环）
  → 主循环（保持进程）

上行：ChannelClient ──incoming_message──▶ ChannelManager(Terminal)
  → consume_pending 回显过滤（命中丢弃）
  → AgentCoordinator::instance().incoming_message（业务）
      → config 校验 → 推记忆 is_self=0 → 命令路由 / ensure_session + enqueue_batch

下行：coordinator ──channel_manager.send(channel_id, msg)──▶ ChannelManager
  → client.send_message → add_pending(msg_id) → 返回 response
  → coordinator 推记忆 is_self=1（send_admin_reply / send_outgoing 共用）

合批：coordinator ──session.batch_producer 直取──▶（无 Channel 中转）
断线：closed ──▶ ChannelManager 自处理 notify ──▶ 重连循环
```

## 四、影响文件清单

| 文件 | 改动 |
|------|------|
| kissbot-channel-client/src/terminal.rs | trait receiver `&self` + 注释 |
| kissbot-channel-client/tests/mock.rs | MockTerminal 6 个方法签名 `&self` |
| kissbot-channel-client-cli/src/main.rs | CliTerminal 6 个方法签名 `&self` |
| kissbot-agent/src/channel_manager.rs | Channel 删 producer；ChannelManager 加 config/disconnect_notify/Terminal/connect_all/send |
| kissbot-agent/src/coordinator.rs | 删 Terminal/connect_channels/disconnect_notify/record_outgoing_msg_id/is_self_echo_by_msg_id/bind_batch；单例化；全部方法 `&self`；send 走 channel_manager |
| kissbot-agent/src/session_manager.rs | Session 删 coordinator；get_or_create/create_session 删参数；accept_batch 用 instance() |
| kissbot-agent/src/command_router.rs | execute / channel_current_key 删除 coordinator 参数，内部经 instance() 取单例 |
| kissbot-agent/src/main.rs | new() 返回 Result<()>；run 经 instance() |

## 五、测试

- kissbot-agent：`cargo test -p kissbot-agent`（现有 107 项 + 更新后）；channel_manager 3 项不变；session_manager 测试更新 get_or_create 调用（删 Weak::new() 参数）；coordinator 测试更新 tool_placeholder 的 get_or_create 调用
- kissbot-channel-client：`cargo test -p kissbot-channel-client`（MockTerminal 签名适配，测试逻辑不变）
- kissbot-channel-client-cli：`cargo build` 验证

## 六、错误处理与边界

- `send` 缺 client：ChannelManager 内部 `warn!` + 返回 `Error::NotConnected`（调用方已按 Err 处理，不 panic）
- 转发 `instance()`：expect panic（进程语义：main 先 new 再启动一切，必然已初始化；单测不碰单例）
- 未预建 channel 的懒建路径（set_agent_id/add_pending/set_mode）：保留懒建（异常路径兜底），不依赖 coordinator

## 七、风险与注意

- `new()` 返回值从 `Arc<Self>` 改为 `Result<()>`：main 调用方式必须同步调整，编译期可查
- `self: Arc<Self>` → `&self` 的批量转换：全部方法体内部 clone 点需逐一检查（`self.clone().xxx()` 全部改直接调用）；编译期可查
- command_rx 任务原持 `Arc` clone，改为持 `&'static` 引用（生命周期允许）
- kissbot-channel-client 为独立 crate：trait 改动需连同测试与 cli 一起编译通过
