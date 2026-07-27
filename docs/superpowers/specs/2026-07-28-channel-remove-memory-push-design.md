# 从 channel-manager 移除记忆推送 — 设计文档

日期：2026-07-28

## 背景与目标

设计文档已调整：channel 不再负责向 memory 推送通道记忆消息，改由 agent（nexus）推送。本次代码改动使 channel 实现与该设计对齐：

- 从 channel-manager 中去掉推送记忆的部分
- channel 完全不需要 agent_id、role_name 等 agent 相关的信息，绑定时仅记录 connect_id
- memory_store_client 暂时保留（文件留盘、从编译中排除），后续移植到 agent 中

## 改动内容

### 1. kissbot-channel（核心改动）

**src/channel_manager.rs**
- 删除 `use crate::memory_store_client::MemoryStoreClient`
- `BoundInfo` 只保留 `connect_id`（删除 `agent_id`、`role_name`）
- `BindAgentUserProcessor`：不再从 `BindRequest` 读取 agent_id/role_name，绑定时仅记录 `connect_id`
- `ChannelManager` 删除 `memory_store_client` 字段及构造函数中的初始化
- 删除 `send_to_memory_store` 方法
- `handle_incoming_message`：去掉 `tokio::join!` 双路推送，只调用 `send_to_agent`

**src/lib.rs**
- 注释掉 `pub mod memory_store_client;` 和 `pub use memory_store_client::MemoryStoreClient;`，加注释说明"暂时保留，后续移植到 agent"；文件保留在磁盘但不参与编译

**src/error.rs**
- 删除 `use crate::memory_store_client::MessageRecord`
- 删除 `flume::SendError<MessageRecord>` 和 `flume::RecvError` 两个变体（flume 在本 crate 仅此两处使用）

**Cargo.toml**
- 移除随编译排除后不再使用的依赖：`reqwest`、`kai-file`、`flume`

### 2. kissbot-api（协议改动）

- `src/channel.rs`：`BindRequest` 删除 `agent_id`、`role_name` 字段，只保留 `messenger_id`、`user_id`
- 同步修改同文件中的单元测试（去掉构造和断言中的这两个字段）

### 3. 下游适配

- **kissbot-channel-client-cli**：`bind()` 中构造 `BindRequest` 去掉 agent_id/role_name 的 dummy 值
- **kissbot-channel-client/tests/mock.rs**：`make_bind_request` 去掉这两个字段
- **kissbot-agent**：本次不动（其 ws_client 用 `json!` 构造绑定请求，serde 反序列化时忽略多余字段，运行时仍兼容；后续统一改）

## 验证

- `cargo build` / `cargo test` 通过的范围：kissbot-api、kissbot-channel、kissbot-channel-client（含 mock 测试）、kissbot-channel-client-cli、kissbot-channel-web（依赖 channel 库，确认无受影响引用）
- kissbot-agent 不在本次编译验证范围
