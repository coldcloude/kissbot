# Messenger/ChannelManager 与 Terminal/ChannelClient 关系重构设计

日期：2026-07-27

## 概述

重构两组组件关系的耦合方式：从"Creator 注入多个 Weak handler trait"改为"直接持有对方引用、直接调函数"。

- **Messenger ↔ ChannelManager**：Messenger 持有 `Weak<ChannelManager>`（Creator 传入），后续直接调 ChannelManager 的 pub 方法
- **Terminal ↔ ChannelClient**：Terminal trait 所有函数带 `id: &str` 参数；ChannelClient 构造时传入 `id: String` 与 `Weak<dyn Terminal>`，一个 ChannelClient 仅持有一个 ws 连接；Terminal 可启动多个 ChannelClient，持有方式由具体实现决定（不进 trait）

## 设计决策（brainstorming 结论）

1. ChannelClient 的持有方式由具体 Terminal 实现决定，Terminal trait 和 ChannelClient 不涉及该细节；channel-client-cli 单 client 用 `tokio::RwLock<Option<Arc<ChannelClient>>>`，不用 DashMap
2. id 在 `ChannelClient::new` 时传入（不是 connect 时）；所有由 ChannelClient 调用的 Terminal trait 函数都带 id 参数
3. id 类型为 `String`（存储），函数调用时传 `&str`（因为可能作为 Map 的 key）
4. 原 `AttachmentDownloadPayloadSender` 的 `prepare_send`/`send` 变为 ChannelManager pub 方法时改名为 `prepare_download_payload`/`send_download_payload`（与协议 payload 术语对齐）；事件方法名保持 `handle_group_change`/`handle_incoming_message`/`handle_user_remove`

## Messenger ↔ ChannelManager（kissbot-channel）

### 删除

4 个 handler trait：`GroupChangeHandler`、`IncomingMessageHandler`、`UserRemoveHandler`、`AttachmentDownloadPayloadSender`。

事件数据类型 `GroupChangeEvent`、`GroupChangeType`、`UserRemoveEvent` 保留（仍在方法签名中使用）；`group_change_to_incoming_message` 转换函数保留。

### MessengerCreator 简化

```rust
#[async_trait]
pub trait MessengerCreator<M: Messenger> {
    async fn create(&self, manager: Weak<ChannelManager>) -> Result<Arc<M>>;
}
```

`ChannelManager::register_messenger` 内部改为 `messenger_creator.create(Arc::downgrade(self))`。

### ChannelManager 暴露 pub inherent 方法

```rust
impl ChannelManager {
    pub async fn handle_group_change(&self, event: Arc<GroupChangeEvent>);
    pub async fn handle_incoming_message(&self, event: Arc<IncomingMessage>);
    pub async fn handle_user_remove(&self, event: Arc<UserRemoveEvent>);
    pub fn prepare_download_payload(&self, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)>;
    pub async fn send_download_payload(&self, sn: u32, transfer_id: u32, size: u32, pos: u64, buf: BytesMut) -> Result<AttachmentPayloadResponse>;
}
```

原 trait impl 的函数体直接迁移为 inherent 方法，逻辑不变。

### Messenger trait 不变

仍由 ChannelManager 调用（`get_info`、`send_message`、`send_attachment_payload`、`download_attachment_header`、`start_send_download_attachment_payload`）。

## Terminal ↔ ChannelClient（kissbot-channel-client）

### 删除

5 个 handler trait（`BindHandler`、`MessengerInfoHandler`、`OutgoingMessageHandler`、`AttachmentUploadHandler`、`AttachmentDownloadHandler`）+ `TerminalCreator`。

### Terminal trait

```rust
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

### ChannelClient

```rust
pub struct ChannelClient {
    id: String,
    terminal: Weak<dyn Terminal>,
    ws_context: RwLock<Option<Arc<WsContext>>>,  // tokio::sync::RwLock
    download_transfer_map: DashMap<u32, Arc<AttachmentInfoResponse>>,
}

impl ChannelClient {
    pub fn new(id: String, terminal: Weak<dyn Terminal>) -> Arc<Self>;
    pub fn id(&self) -> &str;
    pub async fn connect(self: &Arc<Self>, url: &str, api_key: &str) -> Result<()>;
    pub async fn disconnect(&self) -> Result<()>;
    pub async fn bind(&self, request: BindRequest) -> Result<()>;
    pub async fn unbind(&self, request: BindRequest) -> Result<()>;
    pub async fn get_info(&self, messenger_id: Arc<String>) -> Result<Arc<MessengerInfo>>;
    pub async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>>;
    pub async fn send_upload_chunk(&self, transfer_id: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse>;
    pub async fn request_download(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>>;
}
```

- 一个 ChannelClient 仅持有一个 ws 连接（`connect` 建立；重复 connect 报错或直接覆盖由实现时按最简处理——重复调用返回 `Error::InternalError`）
- ws 处理器（JSON 推送、下载分块、关闭）调 Terminal 时传 `client.id`；`terminal` Weak 升级失败时记录日志并丢弃该事件
- 原 5 个 handler trait 的方法体迁移为 inherent pub 方法，逻辑不变

### 持有方式（具体实现负责）

- **channel-client-cli**：单 client，`tokio::RwLock<Option<Arc<ChannelClient>>>`，id 固定 `"cli"`
- **agent（后续接入时）**：多 client，`DashMap<String, Arc<ChannelClient>>`（新建、删除不需要 mut）

## 受影响消费方

- **kissbot-channel-web**：`WebMessenger`/`WebMessengerCreator` 改为持有 `Weak<ChannelManager>`，四处事件回调改调 manager 的 pub 方法
- **kissbot-channel-client 测试 mock**（`tests/mock.rs`）：`MockMessenger`/`MockMessengerCreator` 适配单参数 create；`MockTerminal` 各事件函数带 id；`MockTerminalCreator` 删除，测试直接构造 MockTerminal 后 `ChannelClient::new(id, Arc::downgrade(&terminal)).connect(...)`
- **kissbot-channel-client-cli**：`CliTerminalCreator` 删除；`CliTerminal` 持有 `RwLock<Option<Arc<ChannelClient>>>`；main 流程改为：构造 CliTerminal → `ChannelClient::new("cli", downgrade)` → 存入 terminal → connect → bind → stdin 循环

## 测试

- kissbot-channel-client 两个集成测试（bind_message_test、attachment_test）适配新 API 后全部通过
- kissbot-channel / kissbot-channel-web 编译通过
- kai-ws 不受影响

## 后续（本次不做）

- kissbot-agent 接入：实现 Terminal trait，多 client 用 DashMap 持有，废弃 `nexus/ws_client.rs`
