# channel-client 组件设计

日期：2026-07-27

## 概述

新建两个组件：

- **kissbot-channel-client**（库）：ws 客户端 `ChannelClient`，对接 kissbot-channel 中 `ChannelManager` 的 ws 服务。定义 `Terminal` trait 承载实际业务逻辑（用户绑定、消息收发、附件上传下载、群组变更响应），ChannelClient 只负责连接管理、协议编解码与转发，ws 中收发的请求都转接到 Terminal。后续由 agent 或其他模块实现 Terminal trait。
- **kissbot-channel-client-cli**（bin）：实现 Terminal trait 的命令行工具，用于 agent 与 channel 通信的测试。按行发送文本消息，收到消息打印 content 原始 JSON 串，支持 `/download`、`/upload`、`/group` 命令。尽量简单，不考虑异常情况处理。

同时需要在 kai-ws 中补充客户端连接函数（目前 kai-ws 只有服务端 accept 循环）。

kissbot-agent 中现有的 `nexus/ws_client.rs` 后续废弃、改为实现 Terminal trait，**本次不做改动**。

## 背景与协议要点

- `kai-ws` 定义了 `WsContext`、JSON/二进制 processor 注册、`send_json_with_json_response` 等请求-响应机制，但仅有服务端连接循环 `ws_handle_connection`。
- 消息类型常量与结构体全部在 `kissbot-api` 中定义：
  - 客户端请求：`TYPE_MESSENGER_INFO_REQUEST`、`TYPE_BIND_AGENT_USER` / `TYPE_UNBIND_AGENT_USER`（`BindRequest`）、`TYPE_OUTGOING_MESSAGE`（`OutgoingMessage` → `OutgoingMessageResponse`）、`TYPE_ATTACHMENT_DOWNLOAD_REQUEST`（`AttachmentDownloadRequest` → `AttachmentInfoResponse`）
  - 服务端推送：`TYPE_INCOMING_MESSAGE`（`IncomingMessage`）、`TYPE_JOIN_GROUP` / `TYPE_LEAVE_GROUP`（`GroupChangeNotification`）、`TYPE_USER_REMOVED`（`UserRemoveNotification`）
  - 附件二进制帧 `TYPE_ATTACHMENT_PAYLOAD`：`sn + payload_type + status_code + transfer_id + size + pos + data`，解析用 `kissbot-api` 的 `parse_attachment_payload_header` / `OFFSET_ATT_DATA`
- 认证：ws 握手携带 `X-Api-Key` header（`kissbot-security::HEADER_API_KEY`）。
- 附件协议：
  - **上传**：发送方 `send_message` 携带 `AttachmentInfo`，响应 content 中的 `AttachmentInfoResponse` 含服务端分配的 key 和 transfer_id，之后按 transfer_id 分块上传，每块服务端回 JSON `AttachmentPayloadResponse` 确认
  - **下载**：请求方带 key 请求下载头，响应返回 `AttachmentInfoResponse`（含本次下载的 transfer_id），之后服务端主动推块，每块需请求方回 `AttachmentPayloadResponse` 确认
- 心跳：连接双方均运行 `WsHeartbeatHandler`（10 秒间隔，3 倍超时关闭）。

## 设计决策（brainstorming 结论）

1. CLI `/upload` 命令只带文件路径（`/upload {path}`），附件 key 由服务端分配，上传后打印返回的 key
2. CLI 发送目标群：启动参数指定初始群组 + `/group {group_id}` 命令切换
3. 下载数据流式传递给 Terminal（`download_chunk` 每块回调），下载请求的 ws 响应（下载头）在请求异步函数中直接返回
4. 回调方向：ChannelClient 经 TerminalCreator 向 Terminal 静态注册回调函数（Weak handler，镜像 channel 侧 Messenger/ChannelManager 模式）；ChannelClient 调 Terminal 直接调 trait 函数，无单独的 ops trait
5. 客户端 ws 连接循环补充在 kai-ws 中（`ws_connect`），与服务端对称
6. 断线不重连，但 Terminal 有 `closed()` 函数接收关闭通知；channel-client 实现要完整（错误处理、超时），"不考虑异常"仅针对 CLI 工具
7. CLI 下载默认保存到 `./downloads/{file_name}`，可在启动参数中指定目录

## kai-ws 补充：ws_connect

```rust
pub async fn ws_connect<I, P>(
    url: &str,
    headers: &[(String, String)],        // 用于携带 X-Api-Key
    queue_capacity: usize,
    processor_context: Arc<P>,
    initializer: &I,
) -> Result<Arc<WsContext>>
where
    I: WsProcessorInitializer<P>,
```

- 与服务端 `ws_handle_connection` 对称：`connect_async` 建立连接（握手请求携带 headers）→ 创建 `WsContext` → `initializer.init` → spawn 接收/发送循环 → 返回 `Arc<WsContext>`
- 完全复用现有 `WsContext`、processor 注册、请求-响应等待机制
- 接收循环中连接关闭时触发 `close_processor`（供 channel-client 通知 Terminal）

## kissbot-channel-client（库）

### Terminal trait

ChannelClient 直接调用，函数无 `on_` 前缀：

```rust
#[async_trait]
pub trait Terminal: Send + Sync + 'static {
    async fn incoming_message(&self, message: Arc<IncomingMessage>);
    async fn join_group(&self, notification: Arc<GroupChangeNotification>);
    async fn leave_group(&self, notification: Arc<GroupChangeNotification>);
    async fn user_removed(&self, notification: Arc<UserRemoveNotification>);
    async fn download_chunk(&self, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()>;
    async fn closed(&self);
}
```

### Handler traits 与 TerminalCreator

ChannelClient 实现以下 handler，经 creator 以 Weak 静态注入 Terminal（镜像 `MessengerCreator` 模式）：

```rust
#[async_trait]
pub trait BindHandler: Send + Sync {
    async fn bind(&self, request: BindRequest) -> Result<()>;
    async fn unbind(&self, request: BindRequest) -> Result<()>;
}

#[async_trait]
pub trait MessengerInfoHandler: Send + Sync {
    async fn get_info(&self, messenger_id: Arc<String>) -> Result<Arc<MessengerInfo>>;
}

#[async_trait]
pub trait OutgoingMessageHandler: Send + Sync {
    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>>;
}

#[async_trait]
pub trait AttachmentUploadHandler: Send + Sync {
    async fn send_upload_chunk(&self, transfer_id: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse>;
}

#[async_trait]
pub trait AttachmentDownloadHandler: Send + Sync {
    async fn request_download(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>>;
}

#[async_trait]
pub trait TerminalCreator<T: Terminal> {
    async fn create(
        &self,
        bind_handler: Weak<dyn BindHandler>,
        messenger_info_handler: Weak<dyn MessengerInfoHandler>,
        outgoing_message_handler: Weak<dyn OutgoingMessageHandler>,
        attachment_upload_handler: Weak<dyn AttachmentUploadHandler>,
        attachment_download_handler: Weak<dyn AttachmentDownloadHandler>,
    ) -> Result<Arc<T>>;
}
```

上传分块由 Terminal 全权驱动：`send_message` 后自行遍历响应 content 中的 `AttachmentInfoResponse`（含 multi 嵌套），逐块调 `send_upload_chunk`；块大小、顺序由 Terminal 决定，ChannelClient 只透传。

### ChannelClient

- 内部状态：`ws_context: Arc<WsContext>`、`terminal: Arc<dyn Terminal>`、下载方向 `transfer_id → Arc<AttachmentInfoResponse>` 映射（DashMap）
- `connect<T, TC>(self: &Arc<Self>, url: &str, api_key: &str, creator: TC) -> Result<Arc<T>>`：
  1. 从 self 生成各 Weak handler
  2. `creator.create(...)` 得到 `Arc<Terminal>` 并持有
  3. `ws_connect`（headers 携带 `X-Api-Key`），initializer 中注册各类处理器
- ws 处理器：
  - JSON：`TYPE_INCOMING_MESSAGE` / `TYPE_JOIN_GROUP` / `TYPE_LEAVE_GROUP` / `TYPE_USER_REMOVED` → 反序列化 payload 后调 Terminal 对应函数
  - 二进制 `TYPE_ATTACHMENT_PAYLOAD`（下载块）：`parse_attachment_payload_header` 解析帧头 → 按 transfer_id 查映射取 `AttachmentInfoResponse` → `terminal.download_chunk(info, pos, data)`；按结果回 JSON `AttachmentPayloadResponse`（Ok → error_code 0，Err → error_code 1 通用错误）；收到最后一块（pos + size >= size_bytes）或出错时清理映射
  - 心跳：`WsHeartbeatHandler`（10 秒）
  - close processor → `terminal.closed()`，不做自动重连
- Handler 实现：统一请求-响应模式——`send_json_with_json_response` + oneshot 等待响应（带超时）；上传块为二进制帧，用 `send_bin_with_json_response`；响应 `status_code != CODE_SUCCESS` 转为 Err

### 数据流

- **绑定**：Terminal → `bind_handler.bind(BindRequest)` → ws `TYPE_BIND_AGENT_USER` → 等响应返回
- **发消息**：Terminal → `send_message(OutgoingMessage)` → ws `TYPE_OUTGOING_MESSAGE` → 响应 `OutgoingMessageResponse`（附件含 transfer_id）
- **上传**：Terminal 从响应取 transfer_id → 循环 `send_upload_chunk(transfer_id, pos, data)` → 每块等 `AttachmentPayloadResponse` 确认
- **下载**：Terminal → `request_download(AttachmentDownloadRequest)` → 异步函数直接返回下载头 `AttachmentInfoResponse` → 注册 transfer 映射 → 服务端推块 → `terminal.download_chunk(info, pos, data)` 逐块回调并确认
- **收消息/通知**：服务端推送 → ChannelClient 处理器 → `terminal.incoming_message` / `join_group` / `leave_group` / `user_removed`

## kissbot-channel-client-cli（bin）

尽量简单，不做异常处理（出错打印错误继续）。

- 配置：`config.json` 新增 `channel-client` 段（如 `channel_ws_url`），api_key 复用 `security` 段
- 启动参数（手动解析，不用 clap）：

  ```
  channel-client-cli <messenger_id> <user_id> <group_id> [download_dir]
  ```

  `download_dir` 缺省 `./downloads`
- 启动流程：connect → 自动 bind（agent_id / role_name 固定 `"cli"`）
- stdin 按行处理：
  - `/group {group_id}` — 切换当前目标群
  - `/download {key}` — 以当前 messenger/user/group + key 请求下载，块经 `download_chunk` 顺序写入 `{download_dir}/{file_name}`，完成打印保存路径
  - `/upload {path}` — 发送附件消息（msg_type=attachment，content 为 AttachmentInfo），拿到 transfer_id 后按 64KB 块顺序上传，完成打印服务端返回的 key
  - 其他行 — 作为文本消息发到当前群
- Terminal 实现：
  - `incoming_message` → 打印 content 原始 JSON 串（`serde_json::to_string`）
  - `join_group` / `leave_group` / `user_removed` → 打印一行提示
  - `download_chunk` → 写文件
  - `closed` → 打印提示后退出

## 测试

- **kai-ws**：`ws_connect` 与服务端 `ws_handle_connection` 互通单测（连接、JSON 请求-响应、二进制、关闭）
- **kissbot-channel-client**：集成测试——真实 `ChannelManager` + mock Messenger + mock Terminal，覆盖 bind/unbind、messenger info、收发消息、上传、下载、群组变更、用户删除、closed
- **CLI**：手动测试

## 后续（本次不做）

- kissbot-agent 的 `nexus/ws_client.rs` 废弃，改为基于 kissbot-channel-client 实现 Terminal trait
