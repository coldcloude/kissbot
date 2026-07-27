# Messenger/ChannelManager 与 Terminal/ChannelClient 关系重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 重构 Messenger↔ChannelManager 与 Terminal↔ChannelClient 的关系，从"Creator 注入多个 Weak handler trait"改为"直接持有对方引用、直接调函数"。

**Architecture:** 删除 4 个 handler trait（channel 侧）和 5 个 handler trait + TerminalCreator（channel-client 侧）；MessengerCreator 单参数 `Weak<ChannelManager>`；ChannelManager 暴露 pub inherent 方法；Terminal trait 所有函数带 `&str` id 参数；ChannelClient 构造时传入 `id: String + Weak<dyn Terminal>`，一个实例对应一个 ws 连接。

**设计文档:** `docs/superpowers/specs/2026-07-27-messenger-terminal-relation-refactor-design.md`

## Global Constraints

- 不要删除代码中的注释
- 所有文本文件 UTF-8 编码，`\n` 换行
- git 提交 comment 用中文，且包含本次提交的所有改动内容
- 项目无 workspace，每个组件是独立 crate，依赖用 path 引用
- 协议常量与结构体全部来自 `kissbot-api`，不要重复定义
- mocks 和 CLI 中的 `Result`/`Error` 类型从 `kissbot_channel_client::error` 导入

---

### Task 1: kissbot-channel — 删除 handler traits，简化 MessengerCreator，暴露 pub 方法

**Files:**
- Modify: `kissbot-channel/src/messenger.rs` — 简化 `MessengerCreator` 签名为单参数 `Weak<ChannelManager>`
- Modify: `kissbot-channel/src/data.rs` — 删除 4 个 handler trait（保留数据结构和 `group_change_to_incoming_message`）
- Modify: `kissbot-channel/src/channel_manager.rs` — 删除 4 个 trait impl 块，在 `impl ChannelManager` 中新增 5 个 pub inherent 方法（从 trait impl 迁移），修改 `register_messenger` 传 `Weak<ChannelManager>`
- Modify: `kissbot-channel/src/lib.rs` — 删除不再导出的类型
- Modify: `kissbot-channel-web/src/messenger.rs` — Task 2 处理；编译验证阶段会报错，先放一放

**Interfaces:**
- Consumes: 现有 `crate::data::*` 数据类型、`ChannelManager` 内部结构、`kissbot_channel::Error`
- Produces: Task 2 依赖新 `MessengerCreator` 签名和 `ChannelManager` 的 pub 方法

- [ ] **Step 1: 修改 `messenger.rs`**

将 `MessengerCreator` 的 `create` 从 4 个 Weak handler 参数改为单参数 `Weak<ChannelManager>`：

```rust
/// Messenger 创建器。M 为具体 Messenger 类型，create 返回 Arc<M> 供调用方直接使用。
#[async_trait]
pub trait MessengerCreator<M: Messenger> {
    async fn create(&self, manager: Weak<ChannelManager>) -> Result<Arc<M>>;
}
```

- [ ] **Step 2: 修改 `data.rs`**

删除以下 trait 的定义和实现助记（保留 `GroupChangeEvent`、`GroupChangeType`、`UserRemoveEvent` 数据结构，保留 `group_change_to_incoming_message` 函数）。

从文件开头（`use std::sync::Arc;` 块）找到并删除 4 个 trait 块：

删除 `IncomingMessageHandler`：
```rust
#[async_trait]
pub trait IncomingMessageHandler: Send + Sync {
    async fn handle_incoming_message(&self, message: Arc<IncomingMessage>);
}
```

删除 `AttachmentDownloadPayloadSender`：
```rust
#[async_trait]
pub trait AttachmentDownloadPayloadSender: Send + Sync {
    fn prepare_send(&self, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)>;
    async fn send(&self, sn: u32, transfer_id: u32, size: u32, pos: u64, buf: BytesMut) -> Result<AttachmentPayloadResponse>;
}
```

删除 `GroupChangeHandler`：
```rust
#[async_trait]
pub trait GroupChangeHandler: Send + Sync {
    async fn handle_group_change(&self, event: Arc<GroupChangeEvent>);
}
```

删除 `UserRemoveHandler`：
```rust
#[async_trait]
pub trait UserRemoveHandler: Send + Sync {
    async fn handle_user_remove(&self, event: Arc<UserRemoveEvent>);
}
```

同时删除 `data.rs` 中的 `pub use crate::messenger::*;` 不需要动（messenger.rs 有自己的导出），但 `lib.rs` 的 `pub use data::*;` 会将所有 pub 内容导出——删除 trait 后编译时 `IncomingMessageHandler` 等不再存在，lib.rs 的 glob 自动适配。

- [ ] **Step 3: 修改 `channel_manager.rs`**

3a. 删除末尾 4 个 `impl XxxHandler for ChannelManager` trait impl 块（约 773-850 行，各约 10-20 行）。这些块实现了 `GroupChangeHandler`、`IncomingMessageHandler`、`UserRemoveHandler`、`AttachmentDownloadPayloadSender`。

3b. 在现有的 `impl ChannelManager { ... }` 块中（`register_messenger` 所在块）追加 5 个 pub inherent 方法，方法体从被删除的 trait impl 中原样复制，修改方法名：

```rust
    // === 以下方法原为 handler trait，Messenger 通过 Weak<ChannelManager> 直接调用 ===

    pub async fn handle_group_change(&self, event: Arc<GroupChangeEvent>) {
        let span = span!(Level::INFO, "channel_manager handle group change");
        let _enter = span.enter();
        if let Err(e) = self.handle_group_change_internal(event).await {
            error!("Failed to handle group change: {:?}", e);
        }
    }

    pub async fn handle_incoming_message(&self, event: Arc<IncomingMessage>) {
        let span = span!(Level::INFO, "channel_manager handle incoming message");
        let _enter = span.enter();
        let results = tokio::join!(
            self.send_to_agent(event.clone()),
            self.send_to_memory_store(event.clone()),
        );
        for result in vec![results.0, results.1] {
            if let Err(e) = result {
                error!("Error processing incoming message: {:?}", e);
            }
        }
    }

    pub async fn handle_user_remove(&self, event: Arc<UserRemoveEvent>) {
        let span = span!(Level::INFO, "channel_manager handle user remove");
        let _enter = span.enter();
        if let Err(e) = self.process_user_remove(event).await {
            error!("handle_user_remove error: {:?}", e);
        }
    }

    pub fn prepare_download_payload(&self, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)> {
        let sender_info = self.attachment_sender_map.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?;
        let connect_context = sender_info.connect_context.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;
        drop(sender_info);

        let sn = connect_context.ws_context.next_request_sn();
        let capacity = OFFSET_ATT_DATA + size as usize;
        let mut buf = BytesMut::with_capacity(capacity);
        buf.put_u32(sn);
        buf.put_u32(TYPE_ATTACHMENT_PAYLOAD);
        buf.put_u32(CODE_SUCCESS);
        buf.put_u32(transfer_id);
        buf.put_u32(size);
        buf.put_u64(pos);
        Ok((sn, buf))
    }

    pub async fn send_download_payload(&self, sn: u32, transfer_id: u32, size: u32, pos: u64, buf: BytesMut) -> Result<AttachmentPayloadResponse> {
        let sender_info = self.attachment_sender_map.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?;
        let connect_context = sender_info.connect_context.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;
        let file_size = sender_info.info.size_bytes;
        drop(sender_info);

        let is_last = pos + size as u64 >= file_size;
        let result = self.send_download_attachment_payload(sn, buf, connect_context).await;
        let is_error = match result.as_ref() { Ok(res) => res.error_code != 0, Err(_) => true };
        if is_error || is_last {
            self.attachment_sender_map.remove(&transfer_id);
        }
        result
    }
```

3c. 修改 `register_messenger` 方法：将 4 个 handler 创建的代码替换为直接传 `Weak<ChannelManager>`。

原代码：
```rust
    pub async fn register_messenger<M,MC>(self: &Arc<Self>, messenger_id: &str, messenger_creator: MC) -> Result<Arc<M>>
    where
        M: Messenger,
        MC: MessengerCreator<M>
    {
        match self.messenger_map.entry(messenger_id.to_string()) {
            Entry::Vacant(entry) => {
                let group_change_handler = Arc::downgrade(self);
                let incoming_messages_handler = Arc::downgrade(self);
                let download_attachment_payload_handler = Arc::downgrade(self);
                let user_remove_handler = Arc::downgrade(self);
                let messenger = messenger_creator.create(
                    group_change_handler,
                    incoming_messages_handler,
                    download_attachment_payload_handler,
                    user_remove_handler,
                ).await?;
                let messenger_context = Arc::new(MessengerContext {
                    messenger: messenger.clone() as Arc<dyn Messenger>,
                    bound_map: DashMap::new(),
                });
                entry.insert(messenger_context);
                Ok(messenger)
            }
            Entry::Occupied(entry) => {
                Err(Error::MessengerAlreadyRegistered(entry.key().to_string()))
            }
        }
    }
```

改为：
```rust
    pub async fn register_messenger<M,MC>(self: &Arc<Self>, messenger_id: &str, messenger_creator: MC) -> Result<Arc<M>>
    where
        M: Messenger,
        MC: MessengerCreator<M>
    {
        match self.messenger_map.entry(messenger_id.to_string()) {
            Entry::Vacant(entry) => {
                let messenger = messenger_creator.create(Arc::downgrade(self)).await?;
                let messenger_context = Arc::new(MessengerContext {
                    messenger: messenger.clone() as Arc<dyn Messenger>,
                    bound_map: DashMap::new(),
                });
                entry.insert(messenger_context);
                Ok(messenger)
            }
            Entry::Occupied(entry) => {
                Err(Error::MessengerAlreadyRegistered(entry.key().to_string()))
            }
        }
    }
```

- [ ] **Step 4: 验证 kissbot-channel 编译**

Run: `cd kissbot-channel && cargo check 2>&1 | tail -5`
Expected: 编译成功（警告可忽略）

- [ ] **Step 5: 提交**

```bash
git add kissbot-channel/src/messenger.rs kissbot-channel/src/data.rs kissbot-channel/src/channel_manager.rs
git commit -m "kissbot-channel: 重构 Messenger/ChannelManager 关系 — 删除 4 个 handler trait，MessengerCreator 简化为单参数 Weak<ChannelManager>，ChannelManager 暴露 5 个 pub inherent 方法供 Messenger 直接调用"
```

---

### Task 2: kissbot-channel-web — 适配新 MessengerCreator 和 Messenger 回调方式

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs` — `WebMessengerCreator::create` 改为接收 `Weak<ChannelManager>`；`WebMessenger` 持有 `Weak<ChannelManager>`，事件处理改调 manager 的 pub 方法

- [ ] **Step 1: 分析当前 `WebMessenger` / `WebMessengerCreator` 的回调存储方式**

当前 `WebMessengerCreator::create` 接收 4 个 Weak handler，存在 `WebMessenger` 的 `RwLock` 字段中。`WebMessenger` 的方法 `get_on_group_change()` 等升级 Weak 并调 `handle_group_change` 等。改为直接存储 `Weak<ChannelManager>`，直接调方法。

- [ ] **Step 2: 修改 `WebMessenger`**

2a. 删除 4 个回调存储字段：
```rust
// 删除：
group_change_handler: RwLock<Option<Weak<dyn GroupChangeHandler>>>,
incoming_messages_handler: RwLock<Option<Weak<dyn IncomingMessageHandler>>>,
download_attachment_payload_handler: RwLock<Option<Weak<dyn AttachmentDownloadPayloadSender>>>,
user_remove_handler: RwLock<Option<Weak<dyn UserRemoveHandler>>>,
```

新增：
```rust
manager: Weak<ChannelManager>,
```

2b. 删除 `get_on_group_change()`、`get_on_incoming`、`get_on_download_sender`、`get_on_user_remove` 4 个辅助方法。

2c. 修改事件通知方法（`notify_group_change`、`notify_incoming`、`notify_user_remove`、`notify_download_attachment`），将原来 upgrade handler 再调 trait 方法的方式改为升级 `manager` 并直接调 pub 方法：

原 `notify_group_change` 类似：
```rust
async fn notify_group_change(&self, user_id: &str, group_id: &str, change_type: GroupChangeType, time: &str) {
    if let Some(handler) = self.get_on_group_change() {
        let event = Arc::new(GroupChangeEvent { ... });
        handler.handle_group_change(event).await;
    }
}
```

改为：
```rust
async fn notify_group_change(&self, user_id: &str, group_id: &str, change_type: GroupChangeType, time: &str) {
    if let Some(manager) = self.manager.upgrade() {
        let event = Arc::new(GroupChangeEvent { ... });
        manager.handle_group_change(event).await;
    }
}
```

同样修改 `notify_incoming`（改调 `manager.handle_incoming_message`）、`notify_user_remove`（改调 `manager.handle_user_remove`）、`notify_download_attachment`（改调 `manager.prepare_download_payload` / `manager.send_download_payload`）。

2d. `WebMessengerCreator::create` 签名从 4 个 handler 改为 `manager: Weak<ChannelManager>`：

```rust
#[async_trait]
impl MessengerCreator<WebMessenger> for WebMessengerCreator {
    async fn create(&self, manager: Weak<ChannelManager>) -> Result<Arc<WebMessenger>, kissbot_channel::Error> {
```

在函数体中将 manager 写入 `WebMessenger` 的 `self.manager` 字段。

- [ ] **Step 3: 编译验证**

Run: `cd kissbot-channel-web && cargo check 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 4: 提交**

```bash
git add kissbot-channel-web/src/messenger.rs
git commit -m "kissbot-channel-web: WebMessengerCreator/WebMessenger 适配新 API — 持有 Weak<ChannelManager> 直接调 pub 方法"
```

---

### Task 3: kissbot-channel-client — Terminal trait 改 id 参数，ChannelClient 改构造方式

**Files:**
- Modify: `kissbot-channel-client/src/terminal.rs` — Terminal trait 所有函数带 `&str id`；删除 5 个 handler trait + TerminalCreator
- Modify: `kissbot-channel-client/src/channel_client.rs` — ChannelClient 新增 `id: String` + `terminal: Weak<dyn Terminal>`（构造时传入）；所有 handler trait 方法移为 inherent pub 方法；处理器调 Terminal 时传 id；`connect` 不需要 creator 参数
- Modify: `kissbot-channel-client/src/lib.rs` — 删除不再导出的类型

- [ ] **Step 1: 修改 `terminal.rs`**

文件替换为以下内容（保留 Terminal trait，增加 id 参数；删除 handler traits 和 TerminalCreator）：

```rust
use std::sync::Arc;
use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::channel::*;
use kissbot_api::message::*;

use crate::error::Result;

/// 终端接口：ChannelClient 收到服务端推送后调用的回调函数。
/// id 是触发事件的 ChannelClient 的标识（由 ChannelClient::new 时传入）。
#[async_trait]
pub trait Terminal: Send + Sync + 'static {
    /// 收到上行消息
    async fn incoming_message(&self, id: &str, message: Arc<IncomingMessage>);
    /// 用户加入群组
    async fn join_group(&self, id: &str, notification: Arc<GroupChangeNotification>);
    /// 用户离开群组
    async fn leave_group(&self, id: &str, notification: Arc<GroupChangeNotification>);
    /// 用户被删除
    async fn user_removed(&self, id: &str, notification: Arc<UserRemoveNotification>);
    /// 下载分块到达（Ok/Err 即该块的确认结果）
    async fn download_chunk(&self, id: &str, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()>;
    /// 连接关闭（不做自动重连）
    async fn closed(&self, id: &str);
}
```

删除所有 import 中的 `Weak`（不再需要）、删除 `BindHandler`、`MessengerInfoHandler`、`OutgoingMessageHandler`、`AttachmentUploadHandler`、`AttachmentDownloadHandler`、`TerminalCreator` 的定义。

- [ ] **Step 2: 修改 `lib.rs`**

导出删除：删除 `pub use terminal::*;` 中不再存在的类型（glob 自动适应）；增加明确的 `pub use channel_client::ChannelClient;` 保持不变。

```rust
pub mod error;
pub mod terminal;
pub mod channel_client;

pub use error::{Error, Result};
pub use terminal::Terminal;
pub use channel_client::ChannelClient;
```

- [ ] **Step 3: 修改 `channel_client.rs`**

3a. 结构体定义与构造方法：

```rust
pub struct ChannelClient {
    id: String,
    terminal: Weak<dyn Terminal>,
    ws_context: RwLock<Option<Arc<WsContext>>>,
    download_transfer_map: DashMap<u32, Arc<AttachmentInfoResponse>>,
}

impl ChannelClient {
    pub fn new(id: String, terminal: Weak<dyn Terminal>) -> Arc<Self> {
        Arc::new(Self {
            id,
            terminal,
            ws_context: RwLock::new(None),
            download_transfer_map: DashMap::new(),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
```

3b. `connect` 方法：去掉 creator 参数和 Terminal 创建逻辑，`*self.terminal` 已在构造时设置不再需要 RwLock 写。

```rust
    pub async fn connect(self: &Arc<Self>, url: &str, api_key: &str) -> Result<()> {
        let headers = [(HEADER_API_KEY.to_string(), api_key.to_string())];
        kai_ws::ws_connect(url, &headers, QUEUE_SIZE, self.clone()).await?;
        Ok(())
    }
```

3c. 删除 `terminal` 字段的 `RwLock<Option<Arc<dyn Terminal>>>`（改为 `Weak<dyn Terminal>`），删除 `get_terminal` 方法，改为内联：

```rust
    fn get_terminal(&self) -> Option<Arc<dyn Terminal>> {
        self.terminal.upgrade()
    }
```

3d. 删除所有 `impl XxxHandler for ChannelClient` 块。将方法体移为 `impl ChannelClient` 中的 pub inherent 方法（方法名和签名不变，只是从 trait impl 变成 inherent）：

```rust
    pub async fn bind(&self, request: BindRequest) -> Result<()> { ... }
    pub async fn unbind(&self, request: BindRequest) -> Result<()> { ... }
    pub async fn get_info(&self, messenger_id: Arc<String>) -> Result<Arc<MessengerInfo>> { ... }
    pub async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>> { ... }
    pub async fn send_upload_chunk(&self, transfer_id: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse> { ... }
    pub async fn request_download(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>> { ... }
    pub async fn disconnect(&self) -> Result<()> { ... }
```

（方法体原样从 trait impl 复制，代码不变）

3e. 在所有调 Terminal 的地方传入 `self.id`：

- `TerminalJsonProcessor::process_json` — `client.get_terminal()` 后调各 Terminal 方法时传 `client.id()`：`terminal.incoming_message(client.id(), m).await`
- `TerminalCloseProcessor::process_close` — 同上：`terminal.closed(client.id()).await`
- `DownloadChunkProcessor::process_bin` — `terminal.download_chunk(client.id(), ...).await`

`get_terminal()` 改为返回 `Option<Arc<dyn Terminal>>`（不再返回 `Result`），处理器中 `let Some(terminal) = client.get_terminal() else { return }`。

3f. 删除 `terminal: RwLock<Option<Arc<dyn Terminal>>>` 的 RwLock 写操作（`*self.terminal.write().await = Some(...)` 不再需要）。

3g. 删除不再需要的 import：`use std::sync::{Arc, Weak};` 保留 Arc/Weak；删除 `use crate::terminal::*;` 改为 `use crate::terminal::Terminal;`（因为其他类型已删除）。

- [ ] **Step 4: 验证 kissbot-channel-client lib 编译**

Run: `cd kissbot-channel-client && cargo check --lib 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 5: 提交**

```bash
git add kissbot-channel-client/src/terminal.rs kissbot-channel-client/src/channel_client.rs kissbot-channel-client/src/lib.rs
git commit -m "kissbot-channel-client: Terminal trait 所有函数带 &str id 参数，删除 5 个 handler trait + TerminalCreator，ChannelClient 构造时传入 id + Weak<Terminal>，方法为 inherent pub"
```

---

### Task 4: 更新测试 mock 和 CLI

**Files:**
- Modify: `kissbot-channel-client/tests/mock.rs` — MockMessengerCreator 单参数；MockTerminal 各函数带 id；MockTerminalCreator 删除
- Modify: `kissbot-channel-client/tests/bind_message_test.rs` — 适配新 API（直接构造 MockTerminal，client.new 传 id/terminal_weak，不通过 creator）
- Modify: `kissbot-channel-client/tests/attachment_test.rs` — 同上
- Modify: `kissbot-channel-client-cli/src/main.rs` — CliTerminalCreator 删除；CliTerminal 持有 `RwLock<Option<Arc<ChannelClient>>>`；main 流程改为先建 terminal 再建 client

- [ ] **Step 1: 修改 `mock.rs`**

1a. `MockMessengerCreator` 的 `create` 改为单参数：

```rust
#[async_trait]
impl MessengerCreator<MockMessenger> for MockMessengerCreator {
    async fn create(&self, _manager: Weak<ChannelManager>) -> Result<Arc<MockMessenger>, ChannelError> {
        // Mock 不需要实际调 ChannelManager，仅返回 messenger
        Ok(self.messenger.clone())
    }
}
```

删除 import 中的 `GroupChangeHandler, IncomingMessageHandler, UserRemoveHandler, AttachmentDownloadPayloadSender`。

1b. `MockMessenger` 中删除 4 个 handler 存储字段和 `push_incoming`/`push_group_change`/`push_user_remove` 方法中升级 handler 的逻辑——mock 测试不走回调（测试中通过 side 方式触发事件）。  
注意：实际上 mock 的 `push_incoming` 等方法需要通过从 `register_messenger` 传入的 handler 来推送事件。但新 API 下 Messenger 只有 `Weak<ChannelManager>`，mock messenger 可以通过 store 这个 Weak 来推送事件。所以在 `MockMessenger` 中增加 `manager: RwLock<Option<Weak<ChannelManager>>>`，push 方法通过 `manager.upgrade()` 来调方法。

```rust
pub struct MockMessenger {
    pub info: Arc<MessengerInfo>,
    pub download_data: Bytes,
    manager: RwLock<Option<Weak<ChannelManager>>>,
    // 原有其他字段不变...
}

impl MockMessenger {
    // new 中初始化 manager: RwLock::new(None)
    
    pub fn push_incoming(&self, msg: IncomingMessage) {
        let handler = self.manager.read().unwrap().clone().and_then(|w| w.upgrade());
        if let Some(manager) = handler {
            tokio::spawn(async move {
                manager.handle_incoming_message(Arc::new(msg)).await;
            });
        }
    }

    pub fn push_group_change(&self, change_type: GroupChangeType, user_id: &str, group_id: &str) { ... }
    // 同上，manager.handle_group_change(...)

    pub fn push_user_remove(&self, user_id: &str) { ... }
    // 同上，manager.handle_user_remove(...)
}
```

1c. `MockMessengerCreator::create` 将传入的 `Weak<ChannelManager>` 存入 `MockMessenger.manager`。

1d. `MockTerminal` 的事件函数加 `&str` id：

```rust
#[async_trait]
impl Terminal for MockTerminal {
    async fn incoming_message(&self, _id: &str, message: Arc<IncomingMessage>) {
        let _ = self.incoming.send(message);
    }
    async fn join_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) {
        let _ = self.joins.send(notification);
    }
    async fn leave_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) {
        let _ = self.leaves.send(notification);
    }
    async fn user_removed(&self, _id: &str, notification: Arc<UserRemoveNotification>) {
        let _ = self.removals.send(notification);
    }
    async fn download_chunk(&self, _id: &str, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> ClientResult<()> {
        let _ = self.chunks.send((info, pos, data));
        Ok(())
    }
    async fn closed(&self, _id: &str) {
        let _ = self.closed_tx.send(());
    }
}
```

删除 `MockTerminalCreator` 整个结构体及其 `TerminalCreator` impl。删除 `MockTerminal` 中存储 5 个 handler Weak 的字段和对应的 handler() getter 方法（`bind_handler()`, `messenger_info_handler()`, `outgoing_message_handler()`, `attachment_upload_handler()`, `attachment_download_handler()`）。这些操作在测试中改为通过 `Arc<ChannelClient>` 直接调用 imherent 方法。

删除 import 中的 `TerminalCreator, BindHandler, MessengerInfoHandler, OutgoingMessageHandler, AttachmentUploadHandler, AttachmentDownloadHandler`。

- [ ] **Step 2: 修改 `bind_message_test.rs`**

改为直接构造 MockTerminal 和 ChannelClient，不使用 creator：

```rust
mod mock;

use std::sync::Arc;
use std::time::Duration;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel::GroupChangeType;
use kissbot_channel_client::{ChannelClient, Terminal};
use mock::*;

#[tokio::test]
async fn test_bind_send_and_notify() {
    test_config_setup();
    let messenger = MockMessenger::new(make_messenger_info("m1", "u1", "g1"), b"download-not-used");
    let _manager = start_test_server(19101, messenger.clone()).await;

    let terminal = Arc::new(MockTerminal::new());
    let client = ChannelClient::new("m1".to_string(), Arc::downgrade(&terminal) as Weak<dyn Terminal>);
    client.connect("ws://127.0.0.1:19101", "test-key").await.expect("connect failed");

    // 绑定（直接调 ChannelClient 的 pub 方法）
    client.bind(make_bind_request("m1", "u1")).await.expect("bind failed");

    // messenger info
    let info = client.get_info(Arc::new("m1".to_string())).await.expect("get_info failed");
    assert!(info.user_map.contains_key("u1"));

    // 发送文本消息
    client.send_message(OutgoingMessage {
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
        group_id: Arc::new("g1".to_string()),
        msg_type: Arc::new(MSG_TYPE_TEXT.to_string()),
        content: Content::Text(Arc::new("hello".to_string())),
    }).await.expect("send_message failed");
    let sent = tokio::time::timeout(Duration::from_secs(2), messenger.sent_messages_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(sent.content, Content::Text(Arc::new("hello".to_string())));

    // 上行消息
    messenger.push_incoming(make_text_incoming("m1", "u1", "g1", "hi"));
    let incoming = tokio::time::timeout(Duration::from_secs(2), terminal.incoming_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(incoming.content, Content::Text(Arc::new("hi".to_string())));

    // 群组变化
    messenger.push_group_change(GroupChangeType::Joined, "u1", "g1");
    let join = tokio::time::timeout(Duration::from_secs(2), terminal.joins_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(*join.group_id, "g1");

    messenger.push_group_change(GroupChangeType::Left, "u1", "g1");
    let leave = tokio::time::timeout(Duration::from_secs(2), terminal.leaves_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(*leave.group_id, "g1");

    // 用户删除
    messenger.push_user_remove("u1");
    let removed = tokio::time::timeout(Duration::from_secs(2), terminal.removals_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(*removed.user_id, "u1");

    // 重新绑定后解绑
    client.bind(make_bind_request("m1", "u1")).await.expect("re-bind failed");
    client.unbind(make_bind_request("m1", "u1")).await.expect("unbind failed");

    // 断开
    client.disconnect().await.expect("disconnect failed");
    tokio::time::timeout(Duration::from_secs(5), terminal.closed_rx().recv_async()).await.unwrap().unwrap();
}
```

注意：需要 import `Terminal` trait 以调用其方法（用于获取 rx channels 等）。同时需要 `Weak` import：

```rust
use std::sync::Weak;
```

`Arc::downgrade(&terminal) as Weak<dyn Terminal>` — 需要显式 cast，因为 `Arc::downgrade` 返回 `Weak<MockTerminal>`，需要自动（或强制）unsized coercion 到 `Weak<dyn Terminal>`。在赋值/传参位置，Rust 会自动进行 coercion。所以 `Arc::downgrade(&terminal) as Weak<dyn Terminal>` 或直接传 `Arc::downgrade(&terminal)` 到期望 `Weak<dyn Terminal>` 的参数位置即可。

- [ ] **Step 3: 修改 `attachment_test.rs`**

同样修改：删除 `MockTerminalCreator`，直接构造：

```rust
mod mock;

use std::sync::{Arc, Weak};
use std::time::Duration;
use bytes::Bytes;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel_client::{ChannelClient, Terminal};
use mock::*;

#[tokio::test]
async fn test_attachment_upload_download() {
    test_config_setup();
    let download_data = b"abcdefghij";
    let messenger = MockMessenger::new(make_messenger_info("m1", "u1", "g1"), download_data);
    let _manager = start_test_server(19102, messenger.clone()).await;

    let terminal = Arc::new(MockTerminal::new());
    let client = ChannelClient::new("m1".to_string(), Arc::downgrade(&terminal) as Weak<dyn Terminal>);
    client.connect("ws://127.0.0.1:19102", "test-key").await.expect("connect failed");
    client.bind(make_bind_request("m1", "u1")).await.expect("bind failed");

    // ===== 上传 =====
    let upload_data = b"0123456789";
    let response = client.send_message(OutgoingMessage { ... }).await.expect(...);

    let Content::AttachmentInfoResponse(att) = &response.content else { panic!(...) };

    let r1 = client.send_upload_chunk(att.transfer_id, 0, Bytes::copy_from_slice(&upload_data[..5])).await.expect(...);
    assert_eq!(r1.error_code, PAYLOAD_ERRCODE_OK);
    let r2 = client.send_upload_chunk(att.transfer_id, 5, Bytes::copy_from_slice(&upload_data[5..])).await.expect(...);
    assert_eq!(r2.error_code, PAYLOAD_ERRCODE_OK);

    // mock 验证收到两块
    ...

    // ===== 下载 =====
    let header = client.request_download(AttachmentDownloadRequest { ... }).await.expect(...);
    assert_eq!(header.info.size_bytes, download_data.len() as u64);

    // 收 3 块
    let mut received = Vec::new();
    for expect_pos in [0u64, 4, 8] {
        let (info, pos, data) = tokio::time::timeout(Duration::from_secs(2), terminal.chunks_rx().recv_async()).await.unwrap().unwrap();
        assert_eq!(pos, expect_pos);
        received.extend_from_slice(&data);
    }
    assert_eq!(received, download_data);

    client.disconnect().await.expect("disconnect failed");
    tokio::time::timeout(Duration::from_secs(5), terminal.closed_rx().recv_async()).await.unwrap().unwrap();
}
```

- [ ] **Step 4: 运行全部集成测试**

Run: `cd kissbot-channel-client && timeout 30 cargo test --test bind_message_test --test attachment_test 2>&1 | tail -15`
Expected: 2/2 passed

- [ ] **Step 5: 修改 CLI**

`kissbot-channel-client-cli/src/main.rs`：

删除 `CliTerminalCreator` 结构体和其 `TerminalCreator` impl。`CliTerminal` 不再持有 Weak handler 存储字段，改为持有 `RwLock<Option<Arc<ChannelClient>>>`。

```rust
struct CliTerminal {
    messenger_id: String,
    user_id: String,
    current_group: RwLock<String>,
    download_dir: String,
    client: RwLock<Option<Arc<ChannelClient>>>,  // 持有 ChannelClient
}
```

方法改为调 `client.method()`：

```rust
impl CliTerminal {
    async fn bind(&self) -> Result<()> {
        let client = self.client.read().await.as_ref().unwrap().clone();
        client.bind(BindRequest {
            agent_id: Arc::new("cli".to_string()),
            role_name: Arc::new("cli".to_string()),
            messenger_id: Arc::new(self.messenger_id.clone()),
            user_id: Arc::new(self.user_id.clone()),
        }).await
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        let client = self.client.read().await.as_ref().unwrap().clone();
        let response = client.send_message(OutgoingMessage { ... }).await?;
        println!(">> sent msg_id={}", response.msg_id);
        Ok(())
    }

    async fn download(&self, key: &str) -> Result<()> {
        let client = self.client.read().await.as_ref().unwrap().clone();
        let info = client.request_download(AttachmentDownloadRequest { ... }).await?;
        println!(">> downloading {} ({} bytes)", info.info.file_name, info.info.size_bytes);
        Ok(())
    }

    async fn upload(&self, path: &str) -> Result<()> {
        // 类似，用 client.send_message + client.send_upload_chunk
    }
}
```

Terminal impl 各函数带 `&str` id：

```rust
#[async_trait]
impl Terminal for CliTerminal {
    async fn incoming_message(&self, _id: &str, message: Arc<IncomingMessage>) {
        let json = serde_json::to_string(&message.content).unwrap();
        println!("<< [{}:{}] {}", message.user_id, message.group_id, json);
    }
    async fn join_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) { ... }
    async fn leave_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) { ... }
    async fn user_removed(&self, _id: &str, notification: Arc<UserRemoveNotification>) { ... }
    async fn download_chunk(&self, ...) { ... }
    async fn closed(&self, _id: &str) { println!("!! connection closed"); std::process::exit(0); }
}
```

main 流程改为：

```rust
#[tokio::main]
async fn main() {
    // 解析参数 ...

    let config: CliConfig = kissbot_config::Config::get().get_section("channel-client");
    let api_key = kissbot_security::SecurityConfig::get().api_key.clone();

    let cli_terminal = Arc::new(CliTerminal {
        messenger_id, user_id,
        current_group: RwLock::new(group_id),
        download_dir,
        client: RwLock::new(None),
    });

    let client = ChannelClient::new("cli".to_string(), Arc::downgrade(&cli_terminal) as Weak<dyn Terminal>);
    *cli_terminal.client.write().await = Some(client.clone());
    client.connect(&config.channel_ws_url, &api_key).await.expect("connect failed");
    client.bind(BindRequest { ... }).await.expect("bind failed");
    println!(">> bound. ...");

    // stdin 循环（同上，调 cli_terminal.send_text / client 等方法）
}
```

注意：`CliTerminal` 需要 `Weak<dyn Terminal>` 来构造 `ChannelClient`。需要使用 `Arc::downgrade(&cli_terminal) as Weak<dyn Terminal>`。

删除 import 中所有 handler trait 和 `TerminalCreator`（CLI 当前 import 了 `Terminal, TerminalCreator, BindHandler, OutgoingMessageHandler, AttachmentUploadHandler, AttachmentDownloadHandler`）。新的 import 只需 `Terminal, ChannelClient`。

- [ ] **Step 6: 验证 CLI 编译**

Run: `cd kissbot-channel-client-cli && cargo build 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 7: 提交**

```bash
git add kissbot-channel-client/tests/mock.rs kissbot-channel-client/tests/bind_message_test.rs kissbot-channel-client/tests/attachment_test.rs kissbot-channel-client-cli/src/main.rs
git commit -m "测试与 CLI: 适配 Terminal trait 新 API — 事件函数带 id 参数，删除 TerminalCreator 改为直接构造 ChannelClient(new + connect)"
```

---

## Self-Review 记录

- **Spec 覆盖**：Task 1（channel 侧 trait 删除、MessengerCreator 简化、pub 方法暴露）、Task 2（channel-web 适配）、Task 3（channel-client 侧 Terminal id 参数、ChannelClient 重构）、Task 4（mock、测试、CLI 适配）。覆盖全部 spec 要求。
- **类型一致性**：MockMessengerCreator::create 返回 Result<Arc<MockMessenger>, ChannelError> — 与 trait `MessengerCreator<M: Messenger>` 一致。`Weak<ChannelManager>` 在 mock 中存储和使用方式一致。`Terminal` trait 的 `&str id` 在 mock、测试、CLI 中类型一致。`ChannelClient::new(id: String, terminal: Weak<dyn Terminal>)` 各处使用一致。
- **已知取舍**：CLI 使用 `tokio::sync::RwLock` 持有 `Option<Arc<ChannelClient>>`（单 client），测试 mock 使用 `Weak<ChannelManager>` 推送事件。
