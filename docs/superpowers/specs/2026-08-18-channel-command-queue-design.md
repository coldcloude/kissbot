# 2026-08-18 channel 配置写操作收拢：ChannelManager 方法 + 统一串行队列

## 背景与动机

CommandRouter 中 `/bind`、`/unbind`、`/bind-outgoing`、`/bind-outgoing off` 直接调 `ConfigManager::update_channel`
写 channel 配置，与 `change_channel_key` 的串行队列（`ConfigChange::ApplyKey`）并发执行。

所有 channel 字段（bind_users/outgoing/agent_id/role_name）都在同一条 `Arc<ChannelConfig>` 记录里，
任何写都是整条 clone-modify-store（read-modify-write）。两个独立写路径并发时存在丢更新竞态：
- 单次 `update_channel` 在 `write_nexus_config` 锁内原子，但「校验 → 执行」跨多次调用时不原子
  （如 `/bind-outgoing` 先读快照校验、再分多次 update_channel）
- 队列外写的命令与队列内写（apply_channel_key）互不串行

目标：把「涉及 channel 操作」的写全部收拢到 ChannelManager 方法，经 Nexus 新队列排队，
与 change_channel_key 队列在**单一消费者**内用 `select!` 合并等待，实现所有 channel 配置写全局串行。

## 范围

**纳入**（4 个命令路径）：
- `/bind`（bind_users 插入）
- `/unbind`（bind_users 移除 + outgoing 引用清空）
- `/bind-outgoing <p>`（校验 + 清同 agent/role 其他 channel + 设来源 outgoing）
- `/bind-outgoing off`（清来源 outgoing）

**不纳入**：
- `/admin`、`/unadmin`：纯改配置（admins 字段），单次 `write_nexus_config` 锁内原子，不进队列
- `/model ... true` 的 `set_default_model`：写全局 default_model，不在 channel 记录内，与 change_channel_key 无交集

## 1. ChannelManager 新增方法

内部仍调 `ConfigManager::update_channel`，写语义与现 CommandRouter 逻辑一致：

| 方法 | 内容 |
|---|---|
| `bind_user(channel_id: &str, user: &ChannelUser) -> Result<()>` | update_channel 闭包内 `Arc::make_mut(&mut c.bind_users).insert(user.clone())`（HashSet 天然去重幂等） |
| `unbind_user(channel_id: &str, user: &ChannelUser) -> Result<()>` | 一个 update_channel 闭包内：移除 bind_users + 若 outgoing 引用该身份（messenger_id+user_id 匹配）则清空 outgoing（原 CommandRouter 逻辑整体搬入，保持原子） |
| `bind_outgoing(channel_id: &str, params: &OutChannelParams) -> Result<()>` | 单任务内依次：校验 channel 存在（Err ConfigNotFound）→ 校验 params 身份已绑定（Err InvalidCommand）→ 遍历 channels 清同 (agent_id, role_name) 其他 channel 的 outgoing → update_channel 设来源 outgoing |
| `clear_outgoing(channel_id: &str) -> Result<()>` | update_channel 闭包清 outgoing |

## 2. ChannelCommand 枚举（types.rs，pub）

纯数据，CommandRouter 构造、Nexus 消费：

```rust
pub enum ChannelCommand {
    BindUser { channel_id: String, user: ChannelUser },
    UnbindUser { channel_id: String, user: ChannelUser },
    BindOutgoing { channel_id: String, params: OutChannelParams },
    ClearOutgoing { channel_id: String },
}
```

## 3. Nexus 新队列 + 合并消费者

- 内部消息（私有）：`ChannelTask { cmd: ChannelCommand, done: oneshot::Sender<Result<()>> }`
- 新字段：`channel_task_tx: UnboundedSender<ChannelTask>`
- **Nexus 只暴露 1 个对接方法**：
  ```rust
  pub async fn channel_command(&self, cmd: ChannelCommand) -> Result<()>
  ```
  内部：建 oneshot → `channel_task_tx.send(ChannelTask{cmd, done})` → `done_rx.await`。
  队列已关闭返回 Error::InternalError（与 change_channel_key 同模式）。CommandRouter 不碰 oneshot。

- 消费者合并（替换现有 `while let Some(change) = command_rx.recv().await`）：
  ```rust
  loop {
      tokio::select! {
          change = command_rx.recv() => match change {
              Some(ConfigChange::ApplyKey { channel_id, agent_id, role_name, mode, done }) => {
                  let rst = Nexus::get().apply_channel_key(&channel_id, agent_id, role_name, mode).await;
                  let _ = done.send(rst);
              }
              None => break,
          },
          task = channel_task_rx.recv() => match task {
              Some(ChannelTask { cmd, done }) => {
                  let rst = Nexus::get().apply_channel_command(cmd).await;
                  let _ = done.send(rst);
              }
              None => break,
          },
      }
  }
  ```
  任一 rx 关闭（None）则消费者退出——进程内 tx 存于 Nexus 单例不会发生，break 仅防御。

- 队列内分发（Nexus 私有）：`apply_channel_command(cmd: ChannelCommand) -> Result<()>` match 到
  `channel_manager.bind_user / unbind_user / bind_outgoing / clear_outgoing`。

两队列写（apply_channel_key 的 update_channel 与 channel_manager 各方法）在单一消费者内全局串行，
消除 read-modify-write 丢更新。

## 4. CommandRouter 改动

- `/bind` → `nexus.channel_command(ChannelCommand::BindUser { channel_id: channel_id.into(), user })`
- `/unbind` → `ChannelCommand::UnbindUser { .. }`
- `/bind-outgoing Some(p)` → `ChannelCommand::BindOutgoing { channel_id: channel_id.into(), params: p }`
  （校验已移入队列内，删除 CommandRouter 的 `channels()`/`channel()` 快照读取与 bound 检查）
- `/bind-outgoing off` → `ChannelCommand::ClearOutgoing { channel_id: channel_id.into() }`
- 移除 `OutChannelConfig` 构造与相关 import（CommandRouter 不再构造/读取 channel 配置写路径）

## 5. 错误处理 / 测试

- 校验失败（channel 不存在 / 未绑定）经 oneshot 回传原错误类型（`ConfigNotFound` / `InvalidCommand`），
  命令回复文案不变（Err → "❌ 命令执行失败: {}"）
- 现有测试全部保留（`update_channel` 语义不变；command_router parse 测试不变）；
  队列任务路径依赖全局单例，不新增单测

## 数据流

```
CommandRouter::execute
  → nexus.channel_command(cmd)
    → channel_task_tx.send(ChannelTask{cmd, done})
      → select! 消费者收到 → apply_channel_command(cmd) → channel_manager.xxx().await
        → done.send(rst)
          → CommandRouter 得到 Result
```

## 影响面

- `kissbot-agent/src/channel_manager.rs`：+4 方法（依赖已 import 的 ConfigManager / OutChannelConfig）
- `kissbot-agent/src/nexus.rs`：+ChannelTask 枚举、+channel_task_tx 字段、+channel_command 方法、
  +apply_channel_command 私有方法、消费者改 select!
- `kissbot-agent/src/types.rs`：+ChannelCommand 枚举（复用 ChannelUser / OutChannelParams）
- `kissbot-agent/src/command_router.rs`：4 个命令分支改走 nexus.channel_command，删快照读取与 OutChannelConfig 构造
