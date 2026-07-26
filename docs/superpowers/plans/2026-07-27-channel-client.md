# channel-client 组件实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 kissbot-channel-client（ws 客户端库 + Terminal trait）与 kissbot-channel-client-cli（命令行测试工具），并在 kai-ws 中补充客户端连接函数 ws_connect。

**Architecture:** kai-ws 增加与服务端对称的 `ws_connect`；channel-client 定义 `Terminal` trait（事件函数）与一组 handler trait（ChannelClient 实现，经 `TerminalCreator` 以 Weak 注入 Terminal，镜像 channel 侧 Messenger/ChannelManager 模式），ChannelClient 只做连接管理、协议编解码与转发；CLI 实现 Terminal trait 做行式命令交互。

**Tech Stack:** Rust (edition 2024)、tokio、tokio-tungstenite、kai-ws、kissbot-api、kissbot-security。

**设计文档:** `docs/superpowers/specs/2026-07-27-channel-client-design.md`

## Global Constraints

- 不要删除代码中的注释
- 所有文本文件 UTF-8 编码，`\n` 换行
- git 提交 comment 用中文，且包含本次提交的所有改动内容
- 不使用 clap；配置从 `config.json` 经 `kissbot_config::Config::get().get_section(...)` 读取
- 项目无 workspace，每个组件是独立 crate，依赖用 path 引用（如 `kai-ws = { path = "../kai-rs/kai-ws" }`）
- ws 认证 header：`kissbot_security::HEADER_API_KEY`（值为 `"X-Api-Key"`）
- 协议常量与结构体全部来自 `kissbot-api`（`kissbot_api::channel::*`、`kissbot_api::message::*`），不要重复定义
- channel-client 实现要完整（错误处理、请求超时）；CLI 不考虑异常处理（出错打印继续）

---

### Task 1: kai-ws 补充 ws_connect 客户端连接函数

**Files:**
- Modify: `kai-rs/kai-ws/src/error.rs`
- Modify: `kai-rs/kai-ws/src/ws.rs`（重构提取 `spawn_ws_tasks`，新增 `ws_connect`，新增测试）

**Interfaces:**
- Consumes: 现有 `WsContext`、`WsProcessorInitializer`、`WsMessage`、processor 机制（`kai-rs/kai-ws/src/ws.rs`）
- Produces:
  - `pub async fn ws_connect<I, P>(url: &str, headers: &[(String, String)], queue_capacity: usize, processor_context: Arc<P>, initializer: &I) -> Result<Arc<WsContext>> where I: WsProcessorInitializer<P>` —— Task 2 的 `ChannelClient::connect` 依赖它
  - 新错误变体 `Error::InvalidHeader(String)`

- [ ] **Step 1: 写失败测试**

在 `kai-rs/kai-ws/src/ws.rs` 的 `#[cfg(test)] mod tests` 末尾（`test_filter_reject` 之后）追加：

```rust
    // === Group 7: ws_connect client tests ===

    use tokio::net::TcpListener;

    struct EchoServerCtx;

    struct EchoServerInitializer;

    #[async_trait]
    impl WsProcessorInitializer<EchoServerCtx> for EchoServerInitializer {
        async fn init(&self, ws_context: Arc<WsContext>, _ctx: Arc<EchoServerCtx>) -> Result<()> {
            struct EchoProcessor;

            #[async_trait]
            impl WsJsonProcessor for EchoProcessor {
                async fn process_json(&self, data: WsMessage, context: Arc<WsContext>) {
                    context.send_json(WsMessage {
                        sn: data.sn,
                        status_code: CODE_SUCCESS,
                        payload_type: TYPE_RESPONSE,
                        payload: data.payload,
                    }).await.unwrap();
                }
            }

            ws_context.set_json_processor(42, Arc::new(EchoProcessor));
            Ok(())
        }
    }

    struct EmptyClientCtx;

    struct EmptyClientInitializer;

    #[async_trait]
    impl WsProcessorInitializer<EmptyClientCtx> for EmptyClientInitializer {
        async fn init(&self, _ws_context: Arc<WsContext>, _ctx: Arc<EmptyClientCtx>) -> Result<()> {
            Ok(())
        }
    }

    struct OneShotResponseHandler {
        tx: Option<tokio::sync::oneshot::Sender<WsMessage>>,
    }

    #[async_trait]
    impl WsJsonProcessorMut for OneShotResponseHandler {
        async fn process_json(mut self: Box<Self>, data: WsMessage, _context: Arc<WsContext>) {
            if let Some(tx) = self.tx.take() {
                let _ = tx.send(data);
            }
        }
    }

    #[tokio::test]
    async fn test_ws_connect_json_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            ws_handle_connection(stream, 16, Arc::new(EchoServerCtx), &EchoServerInitializer).await.unwrap();
        });

        let ctx = ws_connect(&format!("ws://{}", addr), &[], 16, Arc::new(EmptyClientCtx), &EmptyClientInitializer)
            .await.unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let req = WsMessage {
            sn: ctx.next_request_sn(),
            payload_type: 42,
            status_code: CODE_SUCCESS,
            payload: Some(serde_json::json!({"hello": "world"})),
        };
        ctx.send_json_with_json_response(req, Box::new(OneShotResponseHandler { tx: Some(tx) })).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(2), rx).await.unwrap().unwrap();
        assert_eq!(resp.payload_type, TYPE_RESPONSE);
        assert_eq!(resp.payload, Some(serde_json::json!({"hello": "world"})));
    }

    #[tokio::test]
    async fn test_ws_connect_with_header() {
        struct RequireHeaderFilter;

        impl WsHeaderFilter for RequireHeaderFilter {
            fn filter(&self, request: &http::Request<()>) -> std::result::Result<(), http::Response<Option<String>>> {
                match request.headers().get("X-Test-Key").and_then(|v| v.to_str().ok()) {
                    Some("yes") => Ok(()),
                    _ => Err(http::Response::builder()
                        .status(http::StatusCode::UNAUTHORIZED)
                        .body(Some("missing header".to_string()))
                        .unwrap()),
                }
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let filter = RequireHeaderFilter;

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            ws_handle_connection_with_filter(stream, 16, Arc::new(EchoServerCtx), &EchoServerInitializer, &[&filter]).await.unwrap();
        });

        let headers = [("X-Test-Key".to_string(), "yes".to_string())];
        let ctx = ws_connect(&format!("ws://{}", addr), &headers, 16, Arc::new(EmptyClientCtx), &EmptyClientInitializer)
            .await.unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let req = WsMessage {
            sn: ctx.next_request_sn(),
            payload_type: 42,
            status_code: CODE_SUCCESS,
            payload: None,
        };
        ctx.send_json_with_json_response(req, Box::new(OneShotResponseHandler { tx: Some(tx) })).await.unwrap();
        let resp = tokio::time::timeout(Duration::from_secs(2), rx).await.unwrap().unwrap();
        assert_eq!(resp.payload_type, TYPE_RESPONSE);
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kai-rs/kai-ws && cargo test ws_connect 2>&1 | tail -5`
Expected: 编译失败，`cannot find function ws_connect in this scope`

- [ ] **Step 3: 实现**

3a. `kai-rs/kai-ws/src/error.rs` 在 `Error` 枚举末尾（`HeartbeatHandlerAlreadyStarted` 之后）追加变体：

```rust
    #[error("invalid header: {0}")]
    InvalidHeader(String),
```

3b. `kai-rs/kai-ws/src/ws.rs` 顶部 import 修改：

```rust
use tokio::{net::TcpStream, time::{Duration, Instant}, io::{AsyncRead, AsyncWrite}};
use tokio_tungstenite::{WebSocketStream, accept_async, accept_hdr_async, connect_async, tungstenite::{Message, Utf8Bytes, client::IntoClientRequest}};
```

3c. 在 `ws_handle_connection_with_filter` 之前插入共享的收发循环函数（从原函数体中提取，逻辑不变）：

```rust
/// 启动 ws 收/发两个后台任务，服务端与客户端连接共用。
fn spawn_ws_tasks<S>(ws_stream: WebSocketStream<S>, context: Arc<WsContext>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, mut receiver) = ws_stream.split();
    let recv_ctx = context.clone();
    let send_ctx = context.clone();
    let recv_running = Arc::new(AtomicBool::new(true));
    let send_running = recv_running.clone();

    //处理消息接收
    tokio::spawn(async move {
        let span = span!(Level::INFO, "ws receiving process");
        let _enter = span.enter();
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(msg) => {
                    match msg {
                        Message::Text(json) => {
                            if let Err(e) = ws_handle_json_message(json, recv_ctx.clone()).await {
                                error!("Error handling json message: {:?}", e);
                            }
                        }
                        Message::Binary(data) => {
                            if let Err(e) = ws_handle_bin_message(data, recv_ctx.clone()).await {
                                error!("Error handling bin message: {:?}", e);
                            }
                        }
                        Message::Close(_) => {
                            break;
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    error!("Error receiving message: {:?}", e);
                }
            };
        }
        //处理关闭回调
        if let Ok(true) = recv_running.compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed) {
            if let Err(e) = ws_handle_close(recv_ctx.clone()).await {
                error!("Error handling close message: {:?}", e);
            }
        }
    });

    //处理消息发送
    tokio::task::spawn(async move {
        let span = span!(Level::INFO, "ws sending process");
        let _enter = span.enter();
        while send_running.load(Ordering::Relaxed) {
            let Ok(msg) = send_ctx.sending_queue.1.recv_async().await else {
                break;
            };
            match msg {
                WsMessageUnion::Json(msg) => {
                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            if let Err(e) = sender.send(Message::text(json)).await {
                                error!("Error sending json message: {:?}", e);
                            }
                        },
                        Err(e) => {
                            error!("Error building json message: {:?}", e);
                        }
                    };
                }
                WsMessageUnion::Binary(msg) => {
                    if let Err(e) = sender.send(Message::binary(msg)).await {
                        error!("Error sending binary message: {:?}", e);
                    }
                }
                WsMessageUnion::Close => {
                    if let Err(e) = sender.send(Message::Close(None)).await {
                        error!("Error sending close message: {:?}", e);
                    }
                }
            }
        }
    });
}
```

3d. 将 `ws_handle_connection_with_filter` 函数体中 `let (mut sender, mut receiver) = ws_stream.split();` 起到函数结尾的两个 `tokio::spawn` 块全部替换为一行调用：

```rust
    let context = Arc::new(WsContext::new(queue_capacity));
    initializer.init(context.clone(), processor_context).await?;
    spawn_ws_tasks(ws_stream, context);
    Ok(())
}
```

（即 `ws_handle_connection_with_filter` 变为：握手 → 创建 context → init → `spawn_ws_tasks` → 返回。）

3e. 在 `ws_handle_connection` 之后追加客户端连接函数：

```rust
/// 作为客户端建立 WebSocket 连接。
/// headers 用于握手阶段携带自定义请求头（如 X-Api-Key 认证）。
/// 与服务端 ws_handle_connection 对称：创建 WsContext、执行 initializer、启动收发循环。
pub async fn ws_connect<I, P>(url: &str, headers: &[(String, String)], queue_capacity: usize, processor_context: Arc<P>, initializer: &I) -> Result<Arc<WsContext>>
where
    I: WsProcessorInitializer<P>,
{
    let mut request = url.into_client_request()?;
    for (name, value) in headers {
        let header_name = http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| Error::InvalidHeader(format!("invalid header name: {}", name)))?;
        let header_value = http::HeaderValue::from_str(value)
            .map_err(|_| Error::InvalidHeader(format!("invalid header value: {}", value)))?;
        request.headers_mut().insert(header_name, header_value);
    }
    let (ws_stream, _) = connect_async(request).await?;
    let context = Arc::new(WsContext::new(queue_capacity));
    initializer.init(context.clone(), processor_context).await?;
    spawn_ws_tasks(ws_stream, context);
    Ok(context)
}
```

- [ ] **Step 4: 运行全部测试确认通过**

Run: `cd kai-rs/kai-ws && cargo test 2>&1 | tail -5`
Expected: 全部 PASS（含原有测试与 2 个新测试）

- [ ] **Step 5: Commit**

```bash
git add kai-rs/kai-ws/src/error.rs kai-rs/kai-ws/src/ws.rs
git commit -m "kai-ws: 新增 ws_connect 客户端连接函数，提取服务端/客户端共用的收发循环 spawn_ws_tasks"
```

---

### Task 2: kissbot-channel-client 核心（连接、绑定、消息收发、通知、closed）

**Files:**
- Create: `kissbot-channel-client/Cargo.toml`
- Create: `kissbot-channel-client/src/lib.rs`
- Create: `kissbot-channel-client/src/error.rs`
- Create: `kissbot-channel-client/src/terminal.rs`
- Create: `kissbot-channel-client/src/channel_client.rs`
- Test: `kissbot-channel-client/tests/mock.rs`（mock Messenger/Terminal，Task 3 复用）
- Test: `kissbot-channel-client/tests/bind_message_test.rs`

**Interfaces:**
- Consumes: `kai_ws::ws_connect`（Task 1）、`kissbot_api::channel::*`、`kissbot_api::message::*`、`kissbot_security::HEADER_API_KEY`、测试侧消费 `kissbot_channel::{ChannelManager, Messenger, MessengerCreator}` 与 `kissbot_channel::data` 导出的 handler/event 类型
- Produces（Task 3、Task 4 依赖）:
  - `kissbot_channel_client::ChannelClient::new() -> Arc<ChannelClient>`
  - `ChannelClient::connect<T, TC>(self: &Arc<Self>, url: &str, api_key: &str, creator: TC) -> Result<Arc<T>> where T: Terminal, TC: TerminalCreator<T>`
  - `ChannelClient::disconnect(&self) -> Result<()>`
  - traits: `Terminal`、`TerminalCreator<T>`、`BindHandler`、`MessengerInfoHandler`、`OutgoingMessageHandler`、`AttachmentUploadHandler`、`AttachmentDownloadHandler`（后两个本任务只定义，Task 3 实现）
  - `kissbot_channel_client::error::{Error, Result}`

- [ ] **Step 1: 写 scaffold 与失败测试**

1a. `kissbot-channel-client/Cargo.toml`:

```toml
[package]
name = "kissbot-channel-client"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
bytes = "1.11"
dashmap = { version = "6.1", features = ["serde"] }
async-trait = "0.1"
thiserror = "2.0"
tracing = "0.1"
kai-ws = { path = "../kai-rs/kai-ws" }
kissbot-api = { path = "../kissbot-api" }
kissbot-security = { path = "../kissbot-security" }

[dev-dependencies]
flume = "0.12"
kissbot-channel = { path = "../kissbot-channel" }
```

1b. `kissbot-channel-client/src/lib.rs`（先放空壳，让 crate 可编译）:

```rust
pub mod error;
pub mod terminal;
pub mod channel_client;

pub use error::{Error, Result};
pub use terminal::*;
pub use channel_client::ChannelClient;
```

1c. `kissbot-channel-client/tests/mock.rs`——测试共享 mock（注意：integration test 中 `mod mock;` 引用，文件不放 `tests/mock/mod.rs` 而放 `tests/mock.rs`，每个 test 文件各自 `mod mock;`）:

```rust
#![allow(dead_code)]

use std::sync::{Arc, RwLock, Weak, atomic::{AtomicU32, Ordering}};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel::{Messenger, MessengerCreator, Error as ChannelError};
use kissbot_channel::{GroupChangeHandler, IncomingMessageHandler, UserRemoveHandler, AttachmentDownloadPayloadSender};
use kissbot_channel::{GroupChangeEvent, GroupChangeType, UserRemoveEvent};
use kissbot_channel_client::error::Result as ClientResult;
use kissbot_channel_client::{Terminal, TerminalCreator, BindHandler, MessengerInfoHandler, OutgoingMessageHandler, AttachmentUploadHandler, AttachmentDownloadHandler};

pub const TEST_TIME: &str = "2026-07-27 00:00:00";
pub const DOWNLOAD_CHUNK_SIZE: usize = 4;

/// 测试配置：ChannelManager 内部会读取 config.json（memory-store 推送、api key 校验），
/// 测试用临时配置文件，memory_store_url 指向不可达地址（错误由 NoopErrorHandler 吞掉）。
pub fn test_config_setup() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join("kissbot-channel-client-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{
            "security": { "api_key": "test-key", "admin_api_key": "admin-key" },
            "api": { "memory_store_url": "http://127.0.0.1:1", "memory_ego_url": "http://127.0.0.1:1" }
        }"#).unwrap();
        // edition 2024: set_var 是 unsafe
        unsafe { std::env::set_var("KISSBOT_CONFIG", &path); }
    });
}

pub fn make_messenger_info(messenger_id: &str, user_id: &str, group_id: &str) -> MessengerInfo {
    let group = Arc::new(GroupInfo {
        group_id: Arc::new(group_id.to_string()),
        group_name: Arc::new("test-group".to_string()),
    });
    let group_map = Arc::new(DashMap::new());
    group_map.insert(group_id.to_string(), group);
    let user = Arc::new(UserInfo {
        user_id: Arc::new(user_id.to_string()),
        user_name: Arc::new("test-user".to_string()),
        group_map,
    });
    let user_map = Arc::new(DashMap::new());
    user_map.insert(user_id.to_string(), user);
    MessengerInfo {
        messenger_id: Arc::new(messenger_id.to_string()),
        messenger_name: Arc::new("test-messenger".to_string()),
        user_map,
    }
}

// ========== Mock Messenger ==========

pub struct MockMessenger {
    pub info: Arc<MessengerInfo>,
    pub download_data: Bytes,
    next_transfer_id: AtomicU32,
    next_msg_id: AtomicU32,
    incoming_handler: RwLock<Option<Weak<dyn IncomingMessageHandler>>>,
    group_change_handler: RwLock<Option<Weak<dyn GroupChangeHandler>>>,
    user_remove_handler: RwLock<Option<Weak<dyn UserRemoveHandler>>>,
    download_sender: RwLock<Option<Weak<dyn AttachmentDownloadPayloadSender>>>,
    pub sent_messages: flume::Sender<OutgoingMessage>,
    sent_messages_rx: flume::Receiver<OutgoingMessage>,
    pub upload_chunks: flume::Sender<(u32, u64, Bytes)>,
    upload_chunks_rx: flume::Receiver<(u32, u64, Bytes)>,
}

impl MockMessenger {
    pub fn new(info: MessengerInfo, download_data: &[u8]) -> Arc<Self> {
        let (sent_messages, sent_messages_rx) = flume::unbounded();
        let (upload_chunks, upload_chunks_rx) = flume::unbounded();
        Arc::new(Self {
            info: Arc::new(info),
            download_data: Bytes::copy_from_slice(download_data),
            next_transfer_id: AtomicU32::new(1),
            next_msg_id: AtomicU32::new(1),
            incoming_handler: RwLock::new(None),
            group_change_handler: RwLock::new(None),
            user_remove_handler: RwLock::new(None),
            download_sender: RwLock::new(None),
            sent_messages,
            sent_messages_rx,
            upload_chunks,
            upload_chunks_rx,
        })
    }

    pub fn sent_messages_rx(&self) -> flume::Receiver<OutgoingMessage> {
        self.sent_messages_rx.clone()
    }

    pub fn upload_chunks_rx(&self) -> flume::Receiver<(u32, u64, Bytes)> {
        self.upload_chunks_rx.clone()
    }

    /// 模拟外部消息到达，经回调推给 ChannelManager
    pub fn push_incoming(&self, msg: IncomingMessage) {
        let handler = self.incoming_handler.read().unwrap().clone().unwrap().upgrade().unwrap();
        tokio::spawn(async move {
            handler.handle_incoming_message(Arc::new(msg)).await;
        });
    }

    /// 模拟群组变化
    pub fn push_group_change(&self, change_type: GroupChangeType, user_id: &str, group_id: &str) {
        let handler = self.group_change_handler.read().unwrap().clone().unwrap().upgrade().unwrap();
        let event = Arc::new(GroupChangeEvent {
            msg_id: Arc::new("gc-1".to_string()),
            notification: Arc::new(GroupChangeNotification {
                messenger_id: self.info.messenger_id.clone(),
                group_id: Arc::new(group_id.to_string()),
                user_id: Arc::new(user_id.to_string()),
            }),
            change_type,
            time: Arc::new(TEST_TIME.to_string()),
        });
        tokio::spawn(async move {
            handler.handle_group_change(event).await;
        });
    }

    /// 模拟用户被删除
    pub fn push_user_remove(&self, user_id: &str) {
        let handler = self.user_remove_handler.read().unwrap().clone().unwrap().upgrade().unwrap();
        let event = Arc::new(UserRemoveEvent {
            msg_id: Arc::new("ur-1".to_string()),
            notification: Arc::new(UserRemoveNotification {
                messenger_id: self.info.messenger_id.clone(),
                user_id: Arc::new(user_id.to_string()),
            }),
            time: Arc::new(TEST_TIME.to_string()),
        });
        tokio::spawn(async move {
            handler.handle_user_remove(event).await;
        });
    }
}

#[async_trait]
impl Messenger for MockMessenger {
    async fn get_info(&self) -> Result<Arc<MessengerInfo>, ChannelError> {
        Ok(self.info.clone())
    }

    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>, ChannelError> {
        // 附件消息转换为 AttachmentInfoResponse（嵌入 key 与 transfer_id），其他消息原样返回
        let content = match &message.content {
            Content::AttachmentInfo(info) => Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
                key: Arc::new(format!("key-{}", info.file_name)),
                info: info.clone(),
                transfer_id: self.next_transfer_id.fetch_add(1, Ordering::Relaxed),
            })),
            other => other.clone(),
        };
        let msg_type = message.msg_type.clone();
        let _ = self.sent_messages.send(message);
        Ok(Arc::new(OutgoingMessageResponse {
            msg_id: Arc::new(format!("msg-{}", self.next_msg_id.fetch_add(1, Ordering::Relaxed))),
            time: Arc::new(TEST_TIME.to_string()),
            msg_type,
            content,
        }))
    }

    async fn send_attachment_payload(&self, transfer_id: u32, size: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse, ChannelError> {
        let _ = self.upload_chunks.send((transfer_id, pos, data));
        Ok(AttachmentPayloadResponse {
            current_pos: pos + size as u64,
            error_code: PAYLOAD_ERRCODE_OK,
            error_msg: None,
        })
    }

    async fn download_attachment_header(&self, _request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>, ChannelError> {
        Ok(Arc::new(AttachmentInfoResponse {
            key: Arc::new("download-key".to_string()),
            info: Arc::new(AttachmentInfo {
                file_name: Arc::new("download.bin".to_string()),
                mime_type: Arc::new("application/octet-stream".to_string()),
                size_bytes: self.download_data.len() as u64,
            }),
            transfer_id: self.next_transfer_id.fetch_add(1, Ordering::Relaxed),
        }))
    }

    async fn start_send_download_attachment_payload(&self, transfer_id: u32) -> Result<(), ChannelError> {
        let sender = self.download_sender.read().unwrap().clone()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| ChannelError::InternalError("download sender is None".to_string()))?;
        let data = self.download_data.clone();
        tokio::spawn(async move {
            let mut pos = 0u64;
            while pos < data.len() as u64 {
                let end = (pos as usize + DOWNLOAD_CHUNK_SIZE).min(data.len());
                let chunk = data.slice(pos as usize..end);
                let size = chunk.len() as u32;
                // prepare_send 返回已写好帧头、预留数据位置的 buffer，直接填入数据
                let (sn, mut buf) = sender.prepare_send(transfer_id, size, pos).unwrap();
                buf.extend_from_slice(&chunk);
                let resp = sender.send(sn, transfer_id, size, pos, buf).await.unwrap();
                assert_eq!(resp.error_code, PAYLOAD_ERRCODE_OK);
                pos = end as u64;
            }
        });
        Ok(())
    }
}

pub struct MockMessengerCreator {
    pub messenger: Arc<MockMessenger>,
}

#[async_trait]
impl MessengerCreator<MockMessenger> for MockMessengerCreator {
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
    ) -> Result<Arc<MockMessenger>, ChannelError> {
        *self.messenger.group_change_handler.write().unwrap() = Some(on_group_change);
        *self.messenger.incoming_handler.write().unwrap() = Some(on_incoming_messages);
        *self.messenger.download_sender.write().unwrap() = Some(on_download_attachment_payload);
        *self.messenger.user_remove_handler.write().unwrap() = Some(on_user_remove);
        Ok(self.messenger.clone())
    }
}

// ========== Mock Terminal ==========

pub struct MockTerminal {
    bind_handler: RwLock<Option<Weak<dyn BindHandler>>>,
    messenger_info_handler: RwLock<Option<Weak<dyn MessengerInfoHandler>>>,
    outgoing_message_handler: RwLock<Option<Weak<dyn OutgoingMessageHandler>>>,
    attachment_upload_handler: RwLock<Option<Weak<dyn AttachmentUploadHandler>>>,
    attachment_download_handler: RwLock<Option<Weak<dyn AttachmentDownloadHandler>>>,
    pub incoming: flume::Sender<Arc<IncomingMessage>>,
    pub joins: flume::Sender<Arc<GroupChangeNotification>>,
    pub leaves: flume::Sender<Arc<GroupChangeNotification>>,
    pub removals: flume::Sender<Arc<UserRemoveNotification>>,
    pub chunks: flume::Sender<(Arc<AttachmentInfoResponse>, u64, Bytes)>,
    pub closed_tx: flume::Sender<()>,
    incoming_rx: flume::Receiver<Arc<IncomingMessage>>,
    joins_rx: flume::Receiver<Arc<GroupChangeNotification>>,
    leaves_rx: flume::Receiver<Arc<GroupChangeNotification>>,
    removals_rx: flume::Receiver<Arc<UserRemoveNotification>>,
    chunks_rx: flume::Receiver<(Arc<AttachmentInfoResponse>, u64, Bytes)>,
    closed_rx: flume::Receiver<()>,
}

impl MockTerminal {
    pub fn new() -> Arc<Self> {
        let (incoming, incoming_rx) = flume::unbounded();
        let (joins, joins_rx) = flume::unbounded();
        let (leaves, leaves_rx) = flume::unbounded();
        let (removals, removals_rx) = flume::unbounded();
        let (chunks, chunks_rx) = flume::unbounded();
        let (closed_tx, closed_rx) = flume::unbounded();
        Arc::new(Self {
            bind_handler: RwLock::new(None),
            messenger_info_handler: RwLock::new(None),
            outgoing_message_handler: RwLock::new(None),
            attachment_upload_handler: RwLock::new(None),
            attachment_download_handler: RwLock::new(None),
            incoming,
            joins,
            leaves,
            removals,
            chunks,
            closed_tx,
            incoming_rx,
            joins_rx,
            leaves_rx,
            removals_rx,
            chunks_rx,
            closed_rx,
        })
    }

    pub fn incoming_rx(&self) -> flume::Receiver<Arc<IncomingMessage>> { self.incoming_rx.clone() }
    pub fn joins_rx(&self) -> flume::Receiver<Arc<GroupChangeNotification>> { self.joins_rx.clone() }
    pub fn leaves_rx(&self) -> flume::Receiver<Arc<GroupChangeNotification>> { self.leaves_rx.clone() }
    pub fn removals_rx(&self) -> flume::Receiver<Arc<UserRemoveNotification>> { self.removals_rx.clone() }
    pub fn chunks_rx(&self) -> flume::Receiver<(Arc<AttachmentInfoResponse>, u64, Bytes)> { self.chunks_rx.clone() }
    pub fn closed_rx(&self) -> flume::Receiver<()> { self.closed_rx.clone() }

    pub fn bind_handler(&self) -> Arc<dyn BindHandler> {
        self.bind_handler.read().unwrap().as_ref().unwrap().upgrade().unwrap()
    }

    pub fn messenger_info_handler(&self) -> Arc<dyn MessengerInfoHandler> {
        self.messenger_info_handler.read().unwrap().as_ref().unwrap().upgrade().unwrap()
    }

    pub fn outgoing_message_handler(&self) -> Arc<dyn OutgoingMessageHandler> {
        self.outgoing_message_handler.read().unwrap().as_ref().unwrap().upgrade().unwrap()
    }

    pub fn attachment_upload_handler(&self) -> Arc<dyn AttachmentUploadHandler> {
        self.attachment_upload_handler.read().unwrap().as_ref().unwrap().upgrade().unwrap()
    }

    pub fn attachment_download_handler(&self) -> Arc<dyn AttachmentDownloadHandler> {
        self.attachment_download_handler.read().unwrap().as_ref().unwrap().upgrade().unwrap()
    }
}

#[async_trait]
impl Terminal for MockTerminal {
    async fn incoming_message(&self, message: Arc<IncomingMessage>) {
        let _ = self.incoming.send(message);
    }

    async fn join_group(&self, notification: Arc<GroupChangeNotification>) {
        let _ = self.joins.send(notification);
    }

    async fn leave_group(&self, notification: Arc<GroupChangeNotification>) {
        let _ = self.leaves.send(notification);
    }

    async fn user_removed(&self, notification: Arc<UserRemoveNotification>) {
        let _ = self.removals.send(notification);
    }

    async fn download_chunk(&self, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> ClientResult<()> {
        let _ = self.chunks.send((info, pos, data));
        Ok(())
    }

    async fn closed(&self) {
        let _ = self.closed_tx.send(());
    }
}

pub struct MockTerminalCreator {
    pub terminal: Arc<MockTerminal>,
}

#[async_trait]
impl TerminalCreator<MockTerminal> for MockTerminalCreator {
    async fn create(
        &self,
        bind_handler: Weak<dyn BindHandler>,
        messenger_info_handler: Weak<dyn MessengerInfoHandler>,
        outgoing_message_handler: Weak<dyn OutgoingMessageHandler>,
        attachment_upload_handler: Weak<dyn AttachmentUploadHandler>,
        attachment_download_handler: Weak<dyn AttachmentDownloadHandler>,
    ) -> ClientResult<Arc<MockTerminal>> {
        *self.terminal.bind_handler.write().unwrap() = Some(bind_handler);
        *self.terminal.messenger_info_handler.write().unwrap() = Some(messenger_info_handler);
        *self.terminal.outgoing_message_handler.write().unwrap() = Some(outgoing_message_handler);
        *self.terminal.attachment_upload_handler.write().unwrap() = Some(attachment_upload_handler);
        *self.terminal.attachment_download_handler.write().unwrap() = Some(attachment_download_handler);
        Ok(self.terminal.clone())
    }
}

// ========== 测试辅助 ==========

/// 启动带一个 mock messenger 的 ChannelManager，监听指定端口
pub async fn start_test_server(port: u16, messenger: Arc<MockMessenger>) -> Arc<kissbot_channel::ChannelManager> {
    let manager = Arc::new(kissbot_channel::ChannelManager::new());
    let messenger_id = messenger.info.messenger_id.to_string();
    manager.register_messenger(&messenger_id, MockMessengerCreator { messenger }).await.unwrap();
    let m = manager.clone();
    tokio::spawn(async move {
        m.start(&format!("127.0.0.1:{}", port)).await.unwrap();
    });
    // 等待 listener 就绪
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    manager
}

pub fn make_bind_request(messenger_id: &str, user_id: &str) -> BindRequest {
    BindRequest {
        agent_id: Arc::new("test-agent".to_string()),
        role_name: Arc::new("test-role".to_string()),
        messenger_id: Arc::new(messenger_id.to_string()),
        user_id: Arc::new(user_id.to_string()),
    }
}

pub fn make_text_incoming(messenger_id: &str, user_id: &str, group_id: &str, text: &str) -> IncomingMessage {
    IncomingMessage {
        msg_id: Arc::new("in-1".to_string()),
        messenger_id: Arc::new(messenger_id.to_string()),
        user_id: Arc::new(user_id.to_string()),
        group_id: Arc::new(group_id.to_string()),
        is_self: 0,
        msg_type: Arc::new(MSG_TYPE_TEXT.to_string()),
        content: Content::Text(Arc::new(text.to_string())),
        time: Arc::new(TEST_TIME.to_string()),
    }
}
```

1d. `kissbot-channel-client/tests/bind_message_test.rs`:

```rust
mod mock;

use std::sync::Arc;
use std::time::Duration;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel::GroupChangeType;
use kissbot_channel_client::ChannelClient;
use mock::*;

#[tokio::test]
async fn test_bind_send_and_notify() {
    test_config_setup();
    let messenger = MockMessenger::new(make_messenger_info("m1", "u1", "g1"), b"download-not-used");
    let _manager = start_test_server(19101, messenger.clone()).await;

    let terminal = MockTerminal::new();
    let client = ChannelClient::new();
    let terminal = client.connect("ws://127.0.0.1:19101", "test-key", MockTerminalCreator { terminal })
        .await.expect("connect failed");

    // 绑定
    terminal.bind_handler().bind(make_bind_request("m1", "u1")).await.expect("bind failed");

    // messenger info 查询
    let info = terminal.messenger_info_handler().get_info(Arc::new("m1".to_string())).await.expect("get_info failed");
    assert!(info.user_map.contains_key("u1"));

    // 发送文本消息 → mock messenger 收到
    let response = terminal.outgoing_message_handler().send_message(OutgoingMessage {
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
        group_id: Arc::new("g1".to_string()),
        msg_type: Arc::new(MSG_TYPE_TEXT.to_string()),
        content: Content::Text(Arc::new("hello".to_string())),
    }).await.expect("send_message failed");
    let sent = tokio::time::timeout(Duration::from_secs(2), messenger.sent_messages_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(sent.content, Content::Text(Arc::new("hello".to_string())));
    assert_eq!(response.content, Content::Text(Arc::new("hello".to_string())));

    // 上行消息 → terminal.incoming_message
    messenger.push_incoming(make_text_incoming("m1", "u1", "g1", "hi"));
    let incoming = tokio::time::timeout(Duration::from_secs(2), terminal.incoming_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(incoming.content, Content::Text(Arc::new("hi".to_string())));

    // 群组变化 join → terminal.join_group（同时会产生一条系统消息）
    messenger.push_group_change(GroupChangeType::Joined, "u1", "g1");
    let join = tokio::time::timeout(Duration::from_secs(2), terminal.joins_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(*join.group_id, "g1");

    // 群组变化 leave → terminal.leave_group
    messenger.push_group_change(GroupChangeType::Left, "u1", "g1");
    let leave = tokio::time::timeout(Duration::from_secs(2), terminal.leaves_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(*leave.group_id, "g1");

    // 用户删除 → terminal.user_removed
    messenger.push_user_remove("u1");
    let removed = tokio::time::timeout(Duration::from_secs(2), terminal.removals_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(*removed.user_id, "u1");

    // 重新绑定后解绑
    terminal.bind_handler().bind(make_bind_request("m1", "u1")).await.expect("re-bind failed");
    terminal.bind_handler().unbind(make_bind_request("m1", "u1")).await.expect("unbind failed");

    // 主动断开 → terminal.closed
    client.disconnect().await.expect("disconnect failed");
    tokio::time::timeout(Duration::from_secs(2), terminal.closed_rx().recv_async()).await.unwrap().unwrap();
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-channel-client && cargo test 2>&1 | tail -10`
Expected: 编译失败（`error.rs`/`terminal.rs`/`channel_client.rs` 不存在或缺少类型）

- [ ] **Step 3: 实现**

3a. `kissbot-channel-client/src/error.rs`:

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("WS error: {0}")]
    WsError(#[from] kai_ws::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Request error: {0}")]
    RequestError(String),

    #[error("Response error: status_code {0}")]
    ResponseError(u32),

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Not connected")]
    NotConnected,

    #[error("Internal error: {0}")]
    InternalError(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

3b. `kissbot-channel-client/src/terminal.rs`:

```rust
use std::sync::{Arc, Weak};
use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::channel::*;
use kissbot_api::message::*;

use crate::error::Result;

/// 终端接口：ChannelClient 直接调用的事件函数（ws 收到的服务端推送都转接到这里）。
#[async_trait]
pub trait Terminal: Send + Sync + 'static {
    /// 收到上行消息
    async fn incoming_message(&self, message: Arc<IncomingMessage>);
    /// 用户加入群组
    async fn join_group(&self, notification: Arc<GroupChangeNotification>);
    /// 用户离开群组
    async fn leave_group(&self, notification: Arc<GroupChangeNotification>);
    /// 用户被删除
    async fn user_removed(&self, notification: Arc<UserRemoveNotification>);
    /// 下载分块到达（请求下载后由服务端推送，Ok/Err 即该块的确认结果）
    async fn download_chunk(&self, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()>;
    /// 连接关闭（不做自动重连）
    async fn closed(&self);
}

/// 绑定/解绑（由 ChannelClient 实现，注入 Terminal）
#[async_trait]
pub trait BindHandler: Send + Sync {
    async fn bind(&self, request: BindRequest) -> Result<()>;
    async fn unbind(&self, request: BindRequest) -> Result<()>;
}

/// 查询 messenger 信息（由 ChannelClient 实现，注入 Terminal）
#[async_trait]
pub trait MessengerInfoHandler: Send + Sync {
    async fn get_info(&self, messenger_id: Arc<String>) -> Result<Arc<MessengerInfo>>;
}

/// 发送下行消息（由 ChannelClient 实现，注入 Terminal）
#[async_trait]
pub trait OutgoingMessageHandler: Send + Sync {
    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>>;
}

/// 上传附件分块（由 ChannelClient 实现，注入 Terminal）
#[async_trait]
pub trait AttachmentUploadHandler: Send + Sync {
    async fn send_upload_chunk(&self, transfer_id: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse>;
}

/// 请求附件下载（由 ChannelClient 实现，注入 Terminal）。
/// 返回下载头 AttachmentInfoResponse，之后分块经 Terminal::download_chunk 推送。
#[async_trait]
pub trait AttachmentDownloadHandler: Send + Sync {
    async fn request_download(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>>;
}

/// Terminal 创建器。T 为具体 Terminal 类型，create 返回 Arc<T> 供调用方直接使用。
/// ChannelClient 的各 handler 以 Weak 静态注入，避免循环引用。
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

3c. `kissbot-channel-client/src/channel_client.rs`（本任务包含 JSON 处理器与关闭处理器；下载分块处理器和附件 handler 在 Task 3 补上——本任务先实现完整核心，附件相关代码按 Task 3 的说明追加）:

```rust
use std::sync::{Arc, RwLock, Weak};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use kai_ws::*;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_security::HEADER_API_KEY;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tracing::error;

use crate::error::{Error, Result};
use crate::terminal::*;

const QUEUE_SIZE: usize = 100;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ChannelClient {
    ws_context: RwLock<Option<Arc<WsContext>>>,
    terminal: RwLock<Option<Arc<dyn Terminal>>>,
    // 下载方向：transfer_id → 下载头信息
    download_transfer_map: DashMap<u32, Arc<AttachmentInfoResponse>>,
}

/// 通用 JSON 请求-响应处理器：收到响应后经 oneshot 返回
struct JsonResponseHandler {
    tx: Option<oneshot::Sender<WsMessage>>,
}

#[async_trait]
impl WsJsonProcessorMut for JsonResponseHandler {
    async fn process_json(mut self: Box<Self>, data: WsMessage, _context: Arc<WsContext>) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(data);
        }
    }
}

impl ChannelClient {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ws_context: RwLock::new(None),
            terminal: RwLock::new(None),
            download_transfer_map: DashMap::new(),
        })
    }

    fn get_ws_context(&self) -> Result<Arc<WsContext>> {
        self.ws_context.read().unwrap().clone().ok_or(Error::NotConnected)
    }

    fn get_terminal(&self) -> Result<Arc<dyn Terminal>> {
        self.terminal.read().unwrap().clone()
            .ok_or_else(|| Error::InternalError("terminal is None".to_string()))
    }

    /// 连接 channel 的 ws 服务：创建 Terminal、注入 handler、建立连接。
    pub async fn connect<T, TC>(self: &Arc<Self>, url: &str, api_key: &str, creator: TC) -> Result<Arc<T>>
    where
        T: Terminal,
        TC: TerminalCreator<T>,
    {
        let bind_handler: Arc<dyn BindHandler> = self.clone();
        let messenger_info_handler: Arc<dyn MessengerInfoHandler> = self.clone();
        let outgoing_message_handler: Arc<dyn OutgoingMessageHandler> = self.clone();
        let attachment_upload_handler: Arc<dyn AttachmentUploadHandler> = self.clone();
        let attachment_download_handler: Arc<dyn AttachmentDownloadHandler> = self.clone();
        let terminal = creator.create(
            Arc::downgrade(&bind_handler),
            Arc::downgrade(&messenger_info_handler),
            Arc::downgrade(&outgoing_message_handler),
            Arc::downgrade(&attachment_upload_handler),
            Arc::downgrade(&attachment_download_handler),
        ).await?;
        *self.terminal.write().unwrap() = Some(terminal.clone());

        let headers = [(HEADER_API_KEY.to_string(), api_key.to_string())];
        kai_ws::ws_connect(url, &headers, QUEUE_SIZE, self.clone(), &ChannelClientInitializer).await?;
        Ok(terminal)
    }

    /// 主动断开连接
    pub async fn disconnect(&self) -> Result<()> {
        self.get_ws_context()?.send_close().await?;
        Ok(())
    }

    /// 发送 JSON 请求并等待响应（带超时）
    async fn request_json(&self, payload_type: u32, payload: serde_json::Value) -> Result<Option<serde_json::Value>> {
        let context = self.get_ws_context()?;
        let (tx, rx) = oneshot::channel();
        let msg = WsMessage {
            sn: context.next_request_sn(),
            payload_type,
            status_code: CODE_SUCCESS,
            payload: Some(payload),
        };
        context.send_json_with_json_response(msg, Box::new(JsonResponseHandler { tx: Some(tx) })).await?;
        let response = tokio::time::timeout(REQUEST_TIMEOUT, rx).await
            .map_err(|_| Error::Timeout(format!("request type {:#x} timeout", payload_type)))?
            .map_err(|_| Error::InternalError("response channel closed".to_string()))?;
        if response.status_code != CODE_SUCCESS {
            return Err(Error::ResponseError(response.status_code));
        }
        Ok(response.payload)
    }
}

#[async_trait]
impl BindHandler for ChannelClient {
    async fn bind(&self, request: BindRequest) -> Result<()> {
        self.request_json(TYPE_BIND_AGENT_USER, serde_json::to_value(request)?).await?;
        Ok(())
    }

    async fn unbind(&self, request: BindRequest) -> Result<()> {
        self.request_json(TYPE_UNBIND_AGENT_USER, serde_json::to_value(request)?).await?;
        Ok(())
    }
}

#[async_trait]
impl MessengerInfoHandler for ChannelClient {
    async fn get_info(&self, messenger_id: Arc<String>) -> Result<Arc<MessengerInfo>> {
        let payload = self.request_json(
            TYPE_MESSENGER_INFO_REQUEST,
            serde_json::to_value(MessengerInfoRequest { messenger_id })?,
        ).await?
        .ok_or_else(|| Error::InvalidMessage("messenger info response payload is None".to_string()))?;
        Ok(Arc::new(serde_json::from_value(payload)?))
    }
}

#[async_trait]
impl OutgoingMessageHandler for ChannelClient {
    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>> {
        let payload = self.request_json(TYPE_OUTGOING_MESSAGE, serde_json::to_value(message)?).await?
            .ok_or_else(|| Error::InvalidMessage("outgoing message response payload is None".to_string()))?;
        Ok(Arc::new(serde_json::from_value(payload)?))
    }
}

/// 服务端推送的 JSON 消息（上行消息、群组变化、用户删除）转接到 Terminal
struct TerminalJsonProcessor {
    client: Weak<ChannelClient>,
}

#[async_trait]
impl WsJsonProcessor for TerminalJsonProcessor {
    async fn process_json(&self, data: WsMessage, _context: Arc<WsContext>) {
        let Some(client) = self.client.upgrade() else { return };
        let Ok(terminal) = client.get_terminal() else { return };
        let Some(payload) = data.payload else {
            error!("TerminalJsonProcessor: payload is None, type {:#x}", data.payload_type);
            return;
        };
        match data.payload_type {
            TYPE_INCOMING_MESSAGE => match serde_json::from_value::<IncomingMessage>(payload) {
                Ok(m) => terminal.incoming_message(Arc::new(m)).await,
                Err(e) => error!("parse incoming message error: {:?}", e),
            },
            TYPE_JOIN_GROUP => match serde_json::from_value::<GroupChangeNotification>(payload) {
                Ok(n) => terminal.join_group(Arc::new(n)).await,
                Err(e) => error!("parse join group error: {:?}", e),
            },
            TYPE_LEAVE_GROUP => match serde_json::from_value::<GroupChangeNotification>(payload) {
                Ok(n) => terminal.leave_group(Arc::new(n)).await,
                Err(e) => error!("parse leave group error: {:?}", e),
            },
            TYPE_USER_REMOVED => match serde_json::from_value::<UserRemoveNotification>(payload) {
                Ok(n) => terminal.user_removed(Arc::new(n)).await,
                Err(e) => error!("parse user removed error: {:?}", e),
            },
            _ => {}
        }
    }
}

/// 连接关闭时通知 Terminal
struct TerminalCloseProcessor {
    client: Weak<ChannelClient>,
}

#[async_trait]
impl WsCloseProcessor for TerminalCloseProcessor {
    async fn process_close(&self, _context: Arc<WsContext>) {
        let Some(client) = self.client.upgrade() else { return };
        if let Ok(terminal) = client.get_terminal() {
            terminal.closed().await;
        }
    }
}

struct ChannelClientInitializer;

#[async_trait]
impl WsProcessorInitializer<ChannelClient> for ChannelClientInitializer {
    async fn init(&self, ws_context: Arc<WsContext>, client: Arc<ChannelClient>) -> std::result::Result<(), kai_ws::Error> {
        *client.ws_context.write().unwrap() = Some(ws_context.clone());
        // 心跳
        let heartbeat = Arc::new(WsHeartbeatHandler::new(HEARTBEAT_INTERVAL, ws_context.clone()));
        ws_context.set_bin_processor(TYPE_HEARTBEAT, heartbeat.clone());
        tokio::spawn(async move { let _ = heartbeat.start().await; });
        // 关闭通知
        ws_context.set_close_processor(Arc::new(TerminalCloseProcessor { client: Arc::downgrade(&client) }));
        // 服务端推送的 JSON 消息
        let json_processor = Arc::new(TerminalJsonProcessor { client: Arc::downgrade(&client) });
        ws_context.set_json_processor(TYPE_INCOMING_MESSAGE, json_processor.clone());
        ws_context.set_json_processor(TYPE_JOIN_GROUP, json_processor.clone());
        ws_context.set_json_processor(TYPE_LEAVE_GROUP, json_processor.clone());
        ws_context.set_json_processor(TYPE_USER_REMOVED, json_processor);
        Ok(())
    }
}
```

3d. 附件 handler 的 stub impl（`connect` 中的 trait 对象转换依赖这两个 impl 存在；Task 3 会替换为真正实现）。追加到 `channel_client.rs` 的 `OutgoingMessageHandler` impl 之后：

```rust
// Task 3 实现，此处为 stub
#[async_trait]
impl AttachmentUploadHandler for ChannelClient {
    async fn send_upload_chunk(&self, _transfer_id: u32, _pos: u64, _data: Bytes) -> Result<AttachmentPayloadResponse> {
        Err(Error::InternalError("send_upload_chunk not implemented".to_string()))
    }
}

// Task 3 实现，此处为 stub
#[async_trait]
impl AttachmentDownloadHandler for ChannelClient {
    async fn request_download(&self, _request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>> {
        Err(Error::InternalError("request_download not implemented".to_string()))
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd kissbot-channel-client && cargo test 2>&1 | tail -10`
Expected: `test_bind_send_and_notify` PASS

- [ ] **Step 5: Commit**

```bash
git add kissbot-channel-client/
git commit -m "新增 kissbot-channel-client 组件核心：Terminal/TerminalCreator/handler trait 定义，ChannelClient 连接管理、绑定、消息收发、群组变化与用户删除通知、closed 通知，含集成测试"
```

---

### Task 3: kissbot-channel-client 附件上传与下载

**Files:**
- Modify: `kissbot-channel-client/src/channel_client.rs`（追加附件 handler impl 与下载分块处理器）
- Test: `kissbot-channel-client/tests/attachment_test.rs`

**Interfaces:**
- Consumes: Task 2 的 `ChannelClient`、`request_json`、`download_transfer_map`、`JsonResponseHandler`、`ChannelClientInitializer`；`kissbot_api::channel::{parse_attachment_payload_header, OFFSET_ATT_DATA, TYPE_ATTACHMENT_PAYLOAD, PAYLOAD_ERRCODE_OK}`
- Produces: `AttachmentUploadHandler` / `AttachmentDownloadHandler` 的 `ChannelClient` 实现（Task 4 CLI 依赖）

- [ ] **Step 1: 写失败测试**

`kissbot-channel-client/tests/attachment_test.rs`:

```rust
mod mock;

use std::sync::Arc;
use std::time::Duration;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel_client::ChannelClient;
use mock::*;

#[tokio::test]
async fn test_attachment_upload_download() {
    test_config_setup();
    let download_data = b"abcdefghij"; // 10 字节，mock 按 4 字节分块 → 3 块
    let messenger = MockMessenger::new(make_messenger_info("m1", "u1", "g1"), download_data);
    let _manager = start_test_server(19102, messenger.clone()).await;

    let terminal = MockTerminal::new();
    let client = ChannelClient::new();
    let terminal = client.connect("ws://127.0.0.1:19102", "test-key", MockTerminalCreator { terminal })
        .await.expect("connect failed");
    terminal.bind_handler().bind(make_bind_request("m1", "u1")).await.expect("bind failed");

    // ===== 上传 =====
    let upload_data = b"0123456789";
    let response = terminal.outgoing_message_handler().send_message(OutgoingMessage {
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
        group_id: Arc::new("g1".to_string()),
        msg_type: Arc::new(MSG_TYPE_ATTACHMENT.to_string()),
        content: Content::AttachmentInfo(Arc::new(AttachmentInfo {
            file_name: Arc::new("upload.bin".to_string()),
            mime_type: Arc::new("application/octet-stream".to_string()),
            size_bytes: upload_data.len() as u64,
        })),
    }).await.expect("send attachment message failed");

    // 响应 content 中取出 transfer_id
    let Content::AttachmentInfoResponse(att) = &response.content else {
        panic!("expected AttachmentInfoResponse, got {:?}", response.content);
    };
    assert_eq!(*att.key, "key-upload.bin");

    // 分两块上传
    let r1 = terminal.attachment_upload_handler()
        .send_upload_chunk(att.transfer_id, 0, Bytes::copy_from_slice(&upload_data[..5]))
        .await.expect("upload chunk 1 failed");
    assert_eq!(r1.error_code, PAYLOAD_ERRCODE_OK);
    let r2 = terminal.attachment_upload_handler()
        .send_upload_chunk(att.transfer_id, 5, Bytes::copy_from_slice(&upload_data[5..]))
        .await.expect("upload chunk 2 failed");
    assert_eq!(r2.error_code, PAYLOAD_ERRCODE_OK);

    // mock messenger 收到两块且数据正确
    let (tid1, pos1, data1) = tokio::time::timeout(Duration::from_secs(2), messenger.upload_chunks_rx().recv_async()).await.unwrap().unwrap();
    let (tid2, pos2, data2) = tokio::time::timeout(Duration::from_secs(2), messenger.upload_chunks_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!((tid1, pos1, data1.as_ref()), (att.transfer_id, 0, &upload_data[..5]));
    assert_eq!((tid2, pos2, data2.as_ref()), (att.transfer_id, 5, &upload_data[5..]));

    // ===== 下载 =====
    let header = terminal.attachment_download_handler().request_download(AttachmentDownloadRequest {
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
        group_id: Arc::new("g1".to_string()),
        key: Arc::new("download-key".to_string()),
    }).await.expect("request_download failed");
    assert_eq!(header.info.size_bytes, download_data.len() as u64);
    assert_eq!(*header.info.file_name, "download.bin");

    // 收 3 块并重组
    let mut received = Vec::new();
    for expect_pos in [0u64, 4, 8] {
        let (info, pos, data) = tokio::time::timeout(Duration::from_secs(2), terminal.chunks_rx().recv_async()).await.unwrap().unwrap();
        assert_eq!(pos, expect_pos);
        assert_eq!(info.transfer_id, header.transfer_id);
        received.extend_from_slice(&data);
    }
    assert_eq!(received, download_data);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-channel-client && cargo test --test attachment_test 2>&1 | tail -10`
Expected: 测试失败——`send_upload_chunk` / `request_download` 还是 stub，返回 `InternalError("... not implemented")`

- [ ] **Step 3: 实现**

在 `kissbot-channel-client/src/channel_client.rs` 中：

3a. 将 import 行 `use bytes::Bytes;` 改为：

```rust
use bytes::{BufMut, Bytes, BytesMut};
```

3b. 用真正实现**替换** Task 2 的两个附件 handler stub impl：

```rust
#[async_trait]
impl AttachmentUploadHandler for ChannelClient {
    async fn send_upload_chunk(&self, transfer_id: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse> {
        let context = self.get_ws_context()?;
        let sn = context.next_request_sn();
        // 二进制帧：sn + payload_type + status_code + transfer_id + size + pos + data
        let mut buf = BytesMut::with_capacity(OFFSET_ATT_DATA + data.len());
        buf.put_u32(sn);
        buf.put_u32(TYPE_ATTACHMENT_PAYLOAD);
        buf.put_u32(CODE_SUCCESS);
        buf.put_u32(transfer_id);
        buf.put_u32(data.len() as u32);
        buf.put_u64(pos);
        buf.extend_from_slice(&data);
        let (tx, rx) = oneshot::channel();
        context.send_bin_with_json_response(sn, buf.freeze(), Box::new(JsonResponseHandler { tx: Some(tx) })).await?;
        let response = tokio::time::timeout(REQUEST_TIMEOUT, rx).await
            .map_err(|_| Error::Timeout("upload chunk timeout".to_string()))?
            .map_err(|_| Error::InternalError("response channel closed".to_string()))?;
        if response.status_code != CODE_SUCCESS {
            return Err(Error::ResponseError(response.status_code));
        }
        let payload = response.payload
            .ok_or_else(|| Error::InvalidMessage("upload chunk response payload is None".to_string()))?;
        Ok(serde_json::from_value(payload)?)
    }
}

#[async_trait]
impl AttachmentDownloadHandler for ChannelClient {
    async fn request_download(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>> {
        let payload = self.request_json(TYPE_ATTACHMENT_DOWNLOAD_REQUEST, serde_json::to_value(request)?).await?
            .ok_or_else(|| Error::InvalidMessage("download response payload is None".to_string()))?;
        let info = Arc::new(serde_json::from_value::<AttachmentInfoResponse>(payload)?);
        // 注册 transfer 映射，之后服务端推送的分块按 transfer_id 找到下载头
        self.download_transfer_map.insert(info.transfer_id, info.clone());
        Ok(info)
    }
}
```

3c. 下载分块处理器（追加到 `TerminalCloseProcessor` 之后）：

```rust
const TRANSFER_WAIT_INTERVAL: Duration = Duration::from_millis(50);
const TRANSFER_WAIT_MAX: Duration = Duration::from_secs(2);

/// 下载分块处理器：服务端推送的附件分块转接到 Terminal::download_chunk，并按结果确认
struct DownloadChunkProcessor {
    client: Weak<ChannelClient>,
}

impl DownloadChunkProcessor {
    // 下载头响应与首个分块的派发存在并发竞争（kai-ws 按消息 spawn 处理任务），
    // transfer 映射可能尚未注册，此处做有限等待
    async fn wait_transfer_info(client: &ChannelClient, transfer_id: u32) -> Option<Arc<AttachmentInfoResponse>> {
        let deadline = tokio::time::Instant::now() + TRANSFER_WAIT_MAX;
        loop {
            if let Some(info) = client.download_transfer_map.get(&transfer_id) {
                return Some(info.clone());
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(TRANSFER_WAIT_INTERVAL).await;
        }
    }
}

#[async_trait]
impl WsBinaryProcessor for DownloadChunkProcessor {
    async fn process_bin(&self, data: Bytes, context: Arc<WsContext>) {
        let sn = match parse_bin_sn(data.as_ref()) {
            Ok(sn) => sn,
            Err(e) => {
                error!("download chunk parse sn error: {:?}", e);
                return;
            }
        };
        let Some(client) = self.client.upgrade() else { return };
        let header = match parse_attachment_payload_header(data.as_ref()) {
            Ok(h) => h,
            Err(e) => {
                error!("download chunk parse header error: {:?}", e);
                return;
            }
        };
        let info = Self::wait_transfer_info(&client, header.id).await;
        let result = match &info {
            Some(info) => match client.get_terminal() {
                Ok(terminal) => terminal.download_chunk(info.clone(), header.pos, data.slice(OFFSET_ATT_DATA..)).await,
                Err(e) => Err(e),
            },
            None => Err(Error::InvalidMessage(format!("unknown transfer_id {}", header.id))),
        };
        let response = match &result {
            Ok(()) => AttachmentPayloadResponse {
                current_pos: header.pos + header.size as u64,
                error_code: PAYLOAD_ERRCODE_OK,
                error_msg: None,
            },
            Err(e) => AttachmentPayloadResponse {
                current_pos: header.pos,
                error_code: 1,
                error_msg: Some(Arc::new(e.to_string())),
            },
        };
        // 最后一块或出错时清理映射
        if let Some(info) = &info {
            if result.is_err() || header.pos + header.size as u64 >= info.info.size_bytes {
                client.download_transfer_map.remove(&header.id);
            }
        }
        let payload = match serde_json::to_value(&response) {
            Ok(p) => Some(p),
            Err(e) => {
                error!("serialize download chunk response error: {:?}", e);
                None
            }
        };
        if let Err(e) = context.send_json(WsMessage {
            sn,
            status_code: CODE_SUCCESS,
            payload_type: TYPE_RESPONSE,
            payload,
        }).await {
            error!("send download chunk response error: {:?}", e);
        }
    }
}
```

3d. 在 `ChannelClientInitializer::init` 中（`TYPE_USER_REMOVED` 注册之后、`Ok(())` 之前）注册下载分块处理器：

```rust
        // 下载分块
        ws_context.set_bin_processor(TYPE_ATTACHMENT_PAYLOAD, Arc::new(DownloadChunkProcessor { client: Arc::downgrade(&client) }));
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd kissbot-channel-client && cargo test 2>&1 | tail -10`
Expected: `test_bind_send_and_notify` 与 `test_attachment_upload_download` 全部 PASS

- [ ] **Step 5: Commit**

```bash
git add kissbot-channel-client/
git commit -m "kissbot-channel-client: 实现附件上传分块与下载（请求下载头 + 分块转接 Terminal::download_chunk 并逐块确认），含集成测试"
```

---

### Task 4: kissbot-channel-client-cli 命令行工具

**Files:**
- Create: `kissbot-channel-client-cli/Cargo.toml`
- Create: `kissbot-channel-client-cli/src/main.rs`
- Modify: `config.json`（新增 `channel-client` 段）

**Interfaces:**
- Consumes: Task 2/3 的 `ChannelClient`、`Terminal`、`TerminalCreator`、各 handler trait；`kissbot_config::Config::get().get_section("channel-client")`；`kissbot_security::SecurityConfig::get().api_key`
- Produces: 可执行文件 `kissbot-channel-client-cli`，用法 `kissbot-channel-client-cli <messenger_id> <user_id> <group_id> [download_dir]`

- [ ] **Step 1: 创建 crate**

`kissbot-channel-client-cli/Cargo.toml`:

```toml
[package]
name = "kissbot-channel-client-cli"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
bytes = "1.11"
async-trait = "0.1"
kissbot-api = { path = "../kissbot-api" }
kissbot-channel-client = { path = "../kissbot-channel-client" }
kissbot-config = { path = "../kissbot-config" }
kissbot-security = { path = "../kissbot-security" }
```

- [ ] **Step 2: 实现 main.rs**

`kissbot-channel-client-cli/src/main.rs`:

```rust
use std::sync::{Arc, RwLock, Weak};
use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel_client::{ChannelClient, Error, Result};
use kissbot_channel_client::{Terminal, TerminalCreator, BindHandler, OutgoingMessageHandler, AttachmentUploadHandler, AttachmentDownloadHandler};
use serde::Deserialize;

#[derive(Deserialize)]
struct CliConfig {
    channel_ws_url: String,
}

const UPLOAD_CHUNK_SIZE: usize = 64 * 1024;

struct CliTerminal {
    messenger_id: String,
    user_id: String,
    current_group: RwLock<String>,
    download_dir: String,
    bind_handler: RwLock<Option<Weak<dyn BindHandler>>>,
    outgoing_handler: RwLock<Option<Weak<dyn OutgoingMessageHandler>>>,
    upload_handler: RwLock<Option<Weak<dyn AttachmentUploadHandler>>>,
    download_handler: RwLock<Option<Weak<dyn AttachmentDownloadHandler>>>,
}

impl CliTerminal {
    fn current_group(&self) -> String {
        self.current_group.read().unwrap().clone()
    }

    async fn bind(&self) -> Result<()> {
        let handler = self.bind_handler.read().unwrap().as_ref().unwrap().upgrade()
            .ok_or_else(|| Error::InternalError("bind handler is None".to_string()))?;
        handler.bind(BindRequest {
            agent_id: Arc::new("cli".to_string()),
            role_name: Arc::new("cli".to_string()),
            messenger_id: Arc::new(self.messenger_id.clone()),
            user_id: Arc::new(self.user_id.clone()),
        }).await
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        let handler = self.outgoing_handler.read().unwrap().as_ref().unwrap().upgrade()
            .ok_or_else(|| Error::InternalError("outgoing handler is None".to_string()))?;
        let response = handler.send_message(OutgoingMessage {
            messenger_id: Arc::new(self.messenger_id.clone()),
            user_id: Arc::new(self.user_id.clone()),
            group_id: Arc::new(self.current_group()),
            msg_type: Arc::new(MSG_TYPE_TEXT.to_string()),
            content: Content::Text(Arc::new(text.to_string())),
        }).await?;
        println!(">> sent msg_id={}", response.msg_id);
        Ok(())
    }

    async fn download(&self, key: &str) -> Result<()> {
        let handler = self.download_handler.read().unwrap().as_ref().unwrap().upgrade()
            .ok_or_else(|| Error::InternalError("download handler is None".to_string()))?;
        let info = handler.request_download(AttachmentDownloadRequest {
            messenger_id: Arc::new(self.messenger_id.clone()),
            user_id: Arc::new(self.user_id.clone()),
            group_id: Arc::new(self.current_group()),
            key: Arc::new(key.to_string()),
        }).await?;
        // 重新下载时先删除旧文件，避免 append 叠加
        let path = format!("{}/{}", self.download_dir, info.info.file_name);
        let _ = std::fs::remove_file(&path);
        println!(">> downloading {} ({} bytes)", info.info.file_name, info.info.size_bytes);
        Ok(())
    }

    async fn upload(&self, path: &str) -> Result<()> {
        let data = std::fs::read(path)?;
        let file_name = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "upload.bin".to_string());
        let outgoing = self.outgoing_handler.read().unwrap().as_ref().unwrap().upgrade()
            .ok_or_else(|| Error::InternalError("outgoing handler is None".to_string()))?;
        let response = outgoing.send_message(OutgoingMessage {
            messenger_id: Arc::new(self.messenger_id.clone()),
            user_id: Arc::new(self.user_id.clone()),
            group_id: Arc::new(self.current_group()),
            msg_type: Arc::new(MSG_TYPE_ATTACHMENT.to_string()),
            content: Content::AttachmentInfo(Arc::new(AttachmentInfo {
                file_name: Arc::new(file_name.clone()),
                mime_type: Arc::new("application/octet-stream".to_string()),
                size_bytes: data.len() as u64,
            })),
        }).await?;
        // 响应 content 中取 transfer_id
        let Content::AttachmentInfoResponse(att) = &response.content else {
            println!("!! unexpected response content, upload aborted");
            return Ok(());
        };
        let upload = self.upload_handler.read().unwrap().as_ref().unwrap().upgrade()
            .ok_or_else(|| Error::InternalError("upload handler is None".to_string()))?;
        let mut pos = 0u64;
        while pos < data.len() as u64 {
            let end = (pos as usize + UPLOAD_CHUNK_SIZE).min(data.len());
            let chunk = Bytes::copy_from_slice(&data[pos as usize..end]);
            let resp = upload.send_upload_chunk(att.transfer_id, pos, chunk).await?;
            if resp.error_code != PAYLOAD_ERRCODE_OK {
                println!("!! upload chunk error: {:?}", resp.error_msg);
                return Ok(());
            }
            pos = end as u64;
        }
        println!(">> uploaded {} key={}", file_name, att.key);
        Ok(())
    }
}

#[async_trait]
impl Terminal for CliTerminal {
    async fn incoming_message(&self, message: Arc<IncomingMessage>) {
        // 展示 content 原始 JSON 串
        let json = serde_json::to_string(&message.content).unwrap();
        println!("<< [{}:{}] {}", message.user_id, message.group_id, json);
    }

    async fn join_group(&self, notification: Arc<GroupChangeNotification>) {
        println!("<< join group: {} @ {}", notification.group_id, notification.messenger_id);
    }

    async fn leave_group(&self, notification: Arc<GroupChangeNotification>) {
        println!("<< leave group: {} @ {}", notification.group_id, notification.messenger_id);
    }

    async fn user_removed(&self, notification: Arc<UserRemoveNotification>) {
        println!("<< user removed: {} @ {}", notification.user_id, notification.messenger_id);
    }

    async fn download_chunk(&self, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.download_dir)?;
        let path = format!("{}/{}", self.download_dir, info.info.file_name);
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(&data)?;
        if pos + data.len() as u64 >= info.info.size_bytes {
            println!(">> downloaded to {}", path);
        }
        Ok(())
    }

    async fn closed(&self) {
        println!("!! connection closed");
        std::process::exit(0);
    }
}

struct CliTerminalCreator {
    terminal: Arc<CliTerminal>,
}

#[async_trait]
impl TerminalCreator<CliTerminal> for CliTerminalCreator {
    async fn create(
        &self,
        bind_handler: Weak<dyn BindHandler>,
        _messenger_info_handler: Weak<dyn kissbot_channel_client::MessengerInfoHandler>,
        outgoing_message_handler: Weak<dyn OutgoingMessageHandler>,
        attachment_upload_handler: Weak<dyn AttachmentUploadHandler>,
        attachment_download_handler: Weak<dyn AttachmentDownloadHandler>,
    ) -> Result<Arc<CliTerminal>> {
        *self.terminal.bind_handler.write().unwrap() = Some(bind_handler);
        *self.terminal.outgoing_handler.write().unwrap() = Some(outgoing_message_handler);
        *self.terminal.upload_handler.write().unwrap() = Some(attachment_upload_handler);
        *self.terminal.download_handler.write().unwrap() = Some(attachment_download_handler);
        Ok(self.terminal.clone())
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: {} <messenger_id> <user_id> <group_id> [download_dir]", args[0]);
        std::process::exit(1);
    }
    let messenger_id = args[1].clone();
    let user_id = args[2].clone();
    let group_id = args[3].clone();
    let download_dir = args.get(4).cloned().unwrap_or_else(|| "./downloads".to_string());

    let config: CliConfig = kissbot_config::Config::get().get_section("channel-client");
    let api_key = kissbot_security::SecurityConfig::get().api_key.clone();

    let terminal = Arc::new(CliTerminal {
        messenger_id,
        user_id,
        current_group: RwLock::new(group_id),
        download_dir,
        bind_handler: RwLock::new(None),
        outgoing_handler: RwLock::new(None),
        upload_handler: RwLock::new(None),
        download_handler: RwLock::new(None),
    });

    let client = ChannelClient::new();
    let terminal = client.connect(&config.channel_ws_url, &api_key, CliTerminalCreator { terminal })
        .await.expect("connect failed");
    terminal.bind().await.expect("bind failed");
    println!(">> bound. 输入行发送文本；/group <id> 切换群组；/download <key>；/upload <path>");

    // stdin 按行读取（独立线程，避免阻塞 tokio runtime）
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => if tx.send(line).is_err() { break; },
                Err(_) => break,
            }
        }
    });

    while let Ok(line) = rx.recv() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("/group ") {
            *terminal.current_group.write().unwrap() = rest.trim().to_string();
            println!(">> current group: {}", rest.trim());
        } else if let Some(rest) = line.strip_prefix("/download ") {
            if let Err(e) = terminal.download(rest.trim()).await {
                println!("!! download error: {}", e);
            }
        } else if let Some(rest) = line.strip_prefix("/upload ") {
            if let Err(e) = terminal.upload(rest.trim()).await {
                println!("!! upload error: {}", e);
            }
        } else if line.starts_with('/') {
            println!("!! unknown command: {}", line);
        } else {
            if let Err(e) = terminal.send_text(line).await {
                println!("!! send error: {}", e);
            }
        }
    }
}
```

- [ ] **Step 3: config.json 增加 channel-client 段**

在项目根 `config.json` 的 `"channel-web"` 段之后追加（注意前一段末尾补逗号）：

```json
  "channel-client": {
    "channel_ws_url": "ws://127.0.0.1:8201"
  }
```

- [ ] **Step 4: 编译验证**

Run: `cd kissbot-channel-client-cli && cargo build 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 5: 手动冒烟验证（可选项，若 channel-web 可运行）**

```bash
# 终端1：启动 channel-web（其 ChannelManager ws 监听 8201）
cd kissbot-channel-web && cargo run

# 终端2：启动 CLI（u1/g1 需为 channel-web 中已配置的用户/群组）
cd kissbot-channel-client-cli && cargo run -- <messenger_id> <user_id> <group_id>
# 输入文本 → channel-web 侧收到；从 web 侧发消息 → CLI 打印 content JSON
```

- [ ] **Step 6: Commit**

```bash
git add kissbot-channel-client-cli/ config.json
git commit -m "新增 kissbot-channel-client-cli 命令行测试工具：实现 Terminal trait，按行发送文本、/group 切换群组、/download 下载附件、/upload 上传附件，config.json 新增 channel-client 配置段"
```

---

## Self-Review 记录

- **Spec 覆盖**：kai-ws ws_connect（Task 1）；Terminal/TerminalCreator/handler traits、connect、绑定、消息收发、通知、closed（Task 2）；附件上传/下载、download_chunk 流式回调、下载头直接返回（Task 3）；CLI 全部命令与行为、config.json 段（Task 4）。断线不重连 + closed 通知在 Task 2。测试要求覆盖在 Task 1/2/3。
- **类型一致性**：`send_upload_chunk(transfer_id, pos, data)` 签名在 trait 定义、ChannelClient impl、mock、CLI 中一致；`request_download` 返回 `Arc<AttachmentInfoResponse>` 一致；mock 中 `GroupChangeEvent`/`UserRemoveEvent` 字段与 `kissbot-channel/src/data.rs` 一致。
- **已知取舍**：下载头响应与首个分块的派发竞争通过 `wait_transfer_info` 有限等待解决（kai-ws 按消息 spawn 任务，无法保证同连接消息的处理顺序）。Task 2 中附件 handler 先以 stub impl 存在（`connect` 的 trait 对象转换需要），Task 3 替换为真正实现。
