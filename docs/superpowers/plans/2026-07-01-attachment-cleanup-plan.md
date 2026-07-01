# 附件清理实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除冗余的 att_id/upload_id/global_attachment_sn，下载方向改为每个 chunk 等待 agent response 后再发下一个

**Architecture:** 从底向上分 5 个任务：(1) API 类型定义，(2) Channel trait 变更，(3) ChannelManager 下载 reply 实现，(4) WebMessenger 适应新 trait，(5) HTTP handler 删除 upload_id

**Tech Stack:** Rust, kissbot-api, kissbot-channel, kissbot-channel-web, kai-ws

## Global Constraints

- 不要删除代码中的注释
- `prepare_send` 返回 `(BytesMut, u32)` 而非 `Result<BytesMut>`
- `send` 返回 `Result<DownloadAttachmentPayloadResponse>` 而非 `Result<()>`
- `DownloadAttachmentPayloadResponse` 字段：`key: Arc<String>`, `error_code: u32`, `error_msg: Option<Arc<String>>`
- `DownloadResponseHandler` 用 `tokio::sync::Mutex<Option<oneshot::Sender<DownloadAttachmentPayloadResponse>>>`
- `AttachmentDownloadRequestProcessor` 直接实现 `WsJsonProcessor`（不用 `JsonProcessorWrapper`）
- `start_send_download_attachment_payload` 在 Messenger 内部 `tokio::spawn`，同步返回
- `AttachmentInfo` 和 `AttachmentMeta` 删除 `att_id` 字段

---

### Task 1: API 类型变更

**Files:**
- Modify: `kissbot-api/src/message.rs` — 删除 `AttachmentInfo.att_id`
- Modify: `kissbot-api/src/channel.rs` — 删除 `WsOutgoingMessageResponse.attachment_upload_id_map`；新增 `DownloadAttachmentPayloadResponse`

**Interfaces:**
- Consumes: 无
- Produces: `DownloadAttachmentPayloadResponse`（Task 2/3 使用）；无 att_id 的 `AttachmentInfo`

- [ ] **Step 1: 删除 AttachmentInfo.att_id**

在 `kissbot-api/src/message.rs` 中，`AttachmentInfo` 结构体删除 `att_id` 字段：

```rust
/// 附件信息。含 key 时表示已由 channel 处理后嵌入 key。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
}
```

- [ ] **Step 2: 更新 message.rs 中的测试**

找到所有构造 `AttachmentInfo` 的位置，删除 `att_id` 字段：

```rust
// make_content 函数中
MSG_TYPE_ATTACHMENT => Content::AttachmentInfo(Arc::new(AttachmentInfo {
    file_name: Arc::new("photo.png".to_string()),
    mime_type: Arc::new("image/png".to_string()),
    size_bytes: 1024,
})),

// test_serde_content_attachment_info 中
AttachmentInfo {
    file_name: Arc::new("photo.png".to_string()),
    mime_type: Arc::new("image/png".to_string()),
    size_bytes: 1024,
}

// test_serde_content_attachment_info_response 中
AttachmentInfo {
    file_name: Arc::new("photo.png".to_string()),
    mime_type: Arc::new("image/png".to_string()),
    size_bytes: 1024,
}
```

- [ ] **Step 3: 删除 WsOutgoingMessageResponse.attachment_upload_id_map**

在 `kissbot-api/src/channel.rs` 中：

```rust
/// ChannelManager 返回给 agent 的 response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsOutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub content: Content,
}
```

（删除 `attachment_upload_id_map` 字段和对应的 `use dashmap::DashMap` import）

- [ ] **Step 4: 新增 DownloadAttachmentPayloadResponse**

在 `kissbot-api/src/channel.rs` 中 `WsAttachmentDownloadResponseHeader` 之后添加：

```rust
/// Agent 对 attachment payload chunk 的确认 response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadAttachmentPayloadResponse {
    pub key: Arc<String>,
    pub error_code: u32,
    pub error_msg: Option<Arc<String>>,
}
```

- [ ] **Step 5: 更新 channel.rs 中的测试**

`tests::test_serde_outgoing_message_response` 中删除 `attachment_upload_id_map`：
```rust
#[test]
fn test_serde_outgoing_message_response() {
    let content = Content::Text(Arc::new("response content".to_string()));
    let obj = OutgoingMessageResponse {
        msg_id: Arc::new("msg1".to_string()),
        time: Arc::new("2026-01-01 00:00:00".to_string()),
        content,
    };
    let json = serde_json::to_value(&obj).unwrap();
    let deserialized: OutgoingMessageResponse = serde_json::from_value(json).unwrap();
    assert_eq!(*deserialized.msg_id, "msg1");
    assert_eq!(deserialized.content, Content::Text(Arc::new("response content".to_string())));
}
```

`test_serde_attachment_download_response_header` 中 `AttachmentInfo` 删除 `att_id`：
```rust
let metadata = Arc::new(AttachmentInfo {
    file_name: Arc::new("doc.pdf".to_string()),
    mime_type: Arc::new("application/pdf".to_string()),
    size_bytes: 99999,
});
```
（同时更新后面的 `assert_eq!(*deserialized.info.att_id, "att1");` 断言——改为 `file_name`）

- [ ] **Step 6: 编译验证 + 运行测试**

```bash
cd kissbot-api && cargo test
```

Expected: 编译通过，全部测试通过。

- [ ] **Step 7: Commit**

```bash
git add kissbot-api/src/message.rs kissbot-api/src/channel.rs
git commit -m "refactor: API 类型变更 — 删除 att_id 和 attachment_upload_id_map，新增 DownloadAttachmentPayloadResponse

- AttachmentInfo 删除 att_id 字段
- WsOutgoingMessageResponse 删除 attachment_upload_id_map
- 新增 DownloadAttachmentPayloadResponse { key, error_code, error_msg }

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: Channel trait 变更

**Files:**
- Modify: `kissbot-channel/src/data.rs` — `prepare_send` 返回 `(BytesMut, u32)`；`send` 返回 `DownloadAttachmentPayloadResponse`
- Modify: `kissbot-channel/src/messenger.rs` — `Messenger` trait 新增 `start_send_download_attachment_payload`
- Modify: `kissbot-channel/src/attachment.rs` — 删除 `attachment_key_map` 中 att_id 的使用

**Interfaces:**
- Consumes: Task 1 的 `DownloadAttachmentPayloadResponse`，无 att_id 的 `AttachmentInfo`
- Produces: 新签名的 `AttachmentDownloadPayloadSender`；新增 `start_send_download_attachment_payload`

- [ ] **Step 1: 修改 AttachmentDownloadPayloadSender**

在 `kissbot-channel/src/data.rs` 中：

```rust
#[async_trait]
pub trait AttachmentDownloadPayloadSender: Send + Sync {
    fn prepare_send(&self, key: &str, size: u32, pos: u64) -> Result<(BytesMut, u32)>;
    async fn send(&self, key: &str, size: u32, pos: u64, buf: BytesMut) -> Result<DownloadAttachmentPayloadResponse>;
}
```

同时添加必要的 use：
```rust
use kissbot_api::channel::DownloadAttachmentPayloadResponse;
```

- [ ] **Step 2: Messenger trait 新增 start_send_download_attachment_payload**

在 `kissbot-channel/src/messenger.rs`：

```rust
#[async_trait]
pub trait Messenger: Send + Sync + 'static {
    async fn get_info(&self) -> Result<Arc<MessengerInfo>>;
    async fn send_message(&self, message: OutgoingMessage, attachment_sn: Arc<AtomicU32>) -> Result<Arc<OutgoingMessageResponse>>;
    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> Result<()>;
    async fn download_attachment_header(&self, request: AttachmentDownloadRequest, attachment_sn: Arc<AtomicU32>) -> Result<Arc<AttachmentInfoResponse>>;
    async fn start_send_download_attachment_payload(&self, key: &str) -> Result<()>;
}
```

- [ ] **Step 3: 删除 attachment_key_map 中 att_id 的使用**

在 `kissbot-channel/src/attachment.rs` 中：
- 删除 `attachment_key_map.insert(info.att_id.to_string(), key_arc.clone())` 两处（第 50 行和 74 行）
- 注意：`attachment_key_map` 变量本身不再需要，可以删除
- 函数体简化（去掉 `let attachment_key_map = Arc::new(DashMap::new());`）

同时删除不再需要的 import：`use dashmap::DashMap;`

- [ ] **Step 4: 编译验证**

```bash
cd kissbot-channel && cargo check
```

Expected: 编译通过（可能有未实现 trait 方法的报错，Task 3/4 会修复）。

- [ ] **Step 5: Commit**

```bash
git add kissbot-channel/src/data.rs kissbot-channel/src/messenger.rs kissbot-channel/src/attachment.rs
git commit -m "refactor: Channel trait 变更 — prepare_send/send 签名更新，新增 start_send_download_attachment_payload

- prepare_send 返回 (BytesMut, u32)
- send 返回 DownloadAttachmentPayloadResponse
- Messenger trait 新增 start_send_download_attachment_payload
- attachment.rs 删除 att_id 相关的 key_map 代码

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: ChannelManager 下载 reply 实现

**Files:**
- Modify: `kissbot-channel/src/channel_manager.rs` — `prepare_send` 返回 sn；`send` 用 `send_bin_with_json_response`；新增 `DownloadResponseHandler`；`AttachmentDownloadRequestProcessor` 改为直接实现 `WsJsonProcessor`

**Interfaces:**
- Consumes: Task 1 的 `DownloadAttachmentPayloadResponse`；Task 2 的新 trait 签名
- Produces: 完整的 `AttachmentDownloadPayloadSender` 实现

- [ ] **Step 1: 修改 AttachmentDownloadPayloadSender 实现——prepare_send**

`prepare_send` 返回 `(BytesMut, u32)`，在构造完毕后同时返回 sn：

```rust
fn prepare_send(&self, key: &str, size: u32, pos: u64) -> Result<(BytesMut, u32)> {
    let sender_entry = self.attachment_sender_map.get(key)
        .ok_or_else(|| Error::AttachmentNotFound(key.to_string()))?;
    let (internal_id, ref _connect_weak) = *sender_entry;
    drop(sender_entry);

    let sn = self.global_attachment_sn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let capacity = OFFSET_ATT_DATA + size as usize;
    let mut buf = BytesMut::with_capacity(capacity);
    buf.put_u32(sn);
    buf.put_u32(TYPE_ATTACHMENT_PAYLOAD);
    buf.put_u32(CODE_SUCCESS);
    buf.put_u32(internal_id);
    buf.put_u32(size);
    buf.put_u64(pos);
    Ok((buf, sn))
}
```

- [ ] **Step 2: 修改 send——用 send_bin_with_json_response**

```rust
async fn send(&self, key: &str, size: u32, _pos: u64, buf: BytesMut) -> Result<DownloadAttachmentPayloadResponse> {
    let sender_entry = self.attachment_sender_map.get(key)
        .ok_or_else(|| Error::AttachmentNotFound(key.to_string()))?;
    let (internal_id, ref connect_weak) = *sender_entry;
    let connect_context = connect_weak.upgrade()
        .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

    if size == 0 {
        self.attachment_sender_map.remove(key);
        // size=0 的结束标记，不需要 await response
        connect_context.ws_context.send_bin(buf.freeze()).await?;
        return Ok(DownloadAttachmentPayloadResponse {
            key: Arc::new(key.to_string()),
            error_code: 0,
            error_msg: None,
        });
    }

    // 从 buf 中提取 sn（prepare_send 已写入）
    let sn = {
        let arr: [u8; 4] = buf[..4].try_into().unwrap();
        u32::from_be_bytes(arr)
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    let handler = Arc::new(DownloadResponseHandler {
        response_tx: tokio::sync::Mutex::new(Some(tx)),
    });

    connect_context.ws_context.send_bin_with_json_response(sn, buf.freeze(), handler).await?;

    let response = rx.await.map_err(|_| Error::InternalError("download response channel closed".to_string()))?;
    Ok(response)
}
```

- [ ] **Step 3: 新增 DownloadResponseHandler**

在 `channel_manager.rs` 中，`AttachmentDownloadRequestProcessor` 之前或之后添加：

```rust
struct DownloadResponseHandler {
    response_tx: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<DownloadAttachmentPayloadResponse>>>,
}

#[async_trait]
impl WsJsonProcessor for DownloadResponseHandler {
    async fn process_json(&self, data: WsMessage, _context: Arc<WsContext>) {
        if let Some(tx) = self.response_tx.lock().unwrap().take() {
            let response = data.payload
                .and_then(|v| serde_json::from_value::<DownloadAttachmentPayloadResponse>(v).ok())
                .unwrap_or_else(|| DownloadAttachmentPayloadResponse {
                    key: Arc::new(String::new()),
                    error_code: data.status_code,
                    error_msg: None,
                });
            let _ = tx.send(response);
        }
    }
}
```

需要添加 import：`use kissbot_api::channel::DownloadAttachmentPayloadResponse;`

- [ ] **Step 4: 重新实现 AttachmentDownloadRequestProcessor**

不再使用 `JsonProcessorWrapper`，直接实现 `WsJsonProcessor`：

```rust
struct AttachmentDownloadRequestProcessor {
    connect_context: Weak<ConnectContext>,
    manager: Weak<ChannelManager>,
}

#[async_trait]
impl WsJsonProcessor for AttachmentDownloadRequestProcessor {
    async fn process_json(&self, data: WsMessage, context: Arc<WsContext>) {
        let result = self.handle_download_request(data, context).await;
        if let Err(e) = result {
            error!("attachment_download_request error: {:?}", e);
        }
    }
}

impl AttachmentDownloadRequestProcessor {
    async fn handle_download_request(&self, data: WsMessage, context: Arc<WsContext>) -> Result<()> {
        let payload = data.payload
            .ok_or_else(|| Error::InvalidMessage("payload is None".to_string()))?;
        let request = serde_json::from_value::<AttachmentDownloadRequest>(payload)?;

        let manager = self.manager.upgrade()
            .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;
        let connect_context = self.connect_context.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

        let messenger_context = manager.messenger_map.get(request.messenger_id.as_str())
            .ok_or_else(|| Error::MessengerNotFound(request.messenger_id.to_string()))?;

        let bound_info = messenger_context.bound_map.get(request.user_id.as_str())
            .ok_or_else(|| Error::UserNotFound(request.user_id.to_string()))?;
        if bound_info.connect_id != connect_context.connect_id {
            return Err(Error::UserNotBound(request.user_id.to_string()));
        }
        let messenger = messenger_context.messenger.clone();

        let att_info_response = messenger.download_attachment_header(request, manager.global_attachment_sn.clone()).await?;
        let key = att_info_response.key.clone();
        let internal_id = manager.global_attachment_sn.fetch_add(1, Ordering::SeqCst);
        manager.attachment_sender_map.insert(key.to_string(), (internal_id, Arc::downgrade(&connect_context)));

        // 构造 WsAttachmentDownloadResponseHeader 返回给 agent
        let ws_response = WsAttachmentDownloadResponseHeader {
            download_id: internal_id,
            response: att_info_response,
        };
        let response_value = serde_json::to_value(ws_response)?;

        // 先返回 header，再启动 payload 推送
        context.send_json(WsMessage {
            sn: data.sn,
            status_code: CODE_SUCCESS,
            payload_type: TYPE_RESPONSE,
            payload: Some(response_value),
        }).await?;

        // start_send_download_attachment_payload 内部自己 spawn
        messenger.start_send_download_attachment_payload(&key).await?;

        Ok(())
    }
}
```

注意：需要添加 `use kissbot_api::channel::DownloadAttachmentPayloadResponse;` 和确保 `kai_ws::{WsJsonProcessor, ...}` 等 import 已存在。

- [ ] **Step 5: 注册 AttachmentDownloadRequestProcessor（更新初始化代码）**

在 `ChannelManagerInitializer::init` 中，`attachment_download_request_handler` 的注册部分不变：
```rust
let attachment_download_request_handler = Arc::new(AttachmentDownloadRequestProcessor {
    connect_context: Arc::downgrade(&connect_context),
    manager: Arc::downgrade(&manager),
});
ws_context.set_json_processor(TYPE_ATTACHMENT_DOWNLOAD_REQUEST, attachment_download_request_handler);
```

（初始化代码不变，因为 processor 结构体字段没变）

- [ ] **Step 6: 编译验证**

```bash
cd kissbot-channel && cargo check
```

Expected: 编译通过。

- [ ] **Step 7: Commit**

```bash
git add kissbot-channel/src/channel_manager.rs
git commit -m "feat: ChannelManager 下载 reply 实现 — send_bin_with_json_response + DownloadResponseHandler

- prepare_send 返回 (BytesMut, u32) 携带 sn
- send 用 send_bin_with_json_response + oneshot await agent response
- 新增 DownloadResponseHandler 桥接 WsJsonProcessor → oneshot
- AttachmentDownloadRequestProcessor 直接实现 WsJsonProcessor
- 先 send_json 返回 header，再调用 start_send_download_attachment_payload

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 4: WebMessenger 改动

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs` — 删除 `global_attachment_sn`/`next_attachment_sn`；`download_attachment_header` 取消 spawn；新增 `start_send_download_attachment_payload` 实现
- Modify: `kissbot-channel-web/src/attachment.rs` — 删除 `AttachmentMeta.att_id`

**Interfaces:**
- Consumes: Task 2 的 trait 签名

- [ ] **Step 1: 删除 AttachmentMeta.att_id**

在 `kissbot-channel-web/src/attachment.rs` 中：

```rust
pub struct AttachmentMeta {
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
    pub has_thumbnail: bool,
}
```

删除 `save_attachment` 中构造 att_id 的代码：
```rust
Ok(AttachmentMeta {
    file_name: Arc::new(filename.to_string()),
    mime_type: Arc::new(mime_type.to_string()),
    size_bytes: data.len() as u64,
    has_thumbnail,
})
```

删除 `get_meta_by_key` 中构造 att_id 的代码：
```rust
Ok(AttachmentMeta {
    file_name: Arc::new(filename),
    mime_type: Arc::new(mime_type.to_string()),
    size_bytes: metadata.len(),
    has_thumbnail: thumb_path.exists(),
})
```

- [ ] **Step 2: WebMessenger 删除 global_attachment_sn 相关**

从 `WebMessenger` 结构体中删除：
```rust
// 删除：
global_attachment_sn: Arc<AtomicU32>,
```

从 `WebMessenger::new()` 中删除 `global_attachment_sn` 参数和赋值：
```rust
pub fn new(
    messenger_id: Arc<String>,
    repo_path: PathBuf,
    config: Arc<RwLock<WebMessengerRepo>>,
    on_group_change: Weak<dyn GroupChangeHandler>,
    on_incoming_messages: Weak<dyn IncomingMessageHandler>,
    on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
    on_user_remove: Weak<dyn UserRemoveHandler>,
    attachment_dir: &str,
    // 删除: global_attachment_sn: Arc<AtomicU32>,
) -> Self {
    Self {
        messenger_id,
        repo_path,
        config,
        msg_id_seq: AtomicU32::new(0),
        on_group_change,
        on_incoming_messages,
        on_download_attachment_payload,
        on_user_remove,
        sse: Arc::new(SseDispatcher::new()),
        attachment_store: Arc::new(AttachmentStore::new(attachment_dir)),
        // 删除: global_attachment_sn,
        pending_uploads: DashMap::new(),
        upload_channels: Arc::new(DashMap::new()),
    }
}
```

删除 `next_attachment_sn()` 方法。

从 `WebMessengerCreator::create()` 中删除 `global_attachment_sn` 参数和对应传入。

- [ ] **Step 3: 更新 download_attachment_header**

不再 `tokio::spawn`，只读取 meta 返回 `AttachmentInfoResponse`：

```rust
async fn download_attachment_header(&self, request: AttachmentDownloadRequest, _attachment_sn: Arc<AtomicU32>) -> std::result::Result<Arc<AttachmentInfoResponse>, kissbot_channel::Error> {
    let meta = self.attachment_store.get_meta_by_key(request.key.as_str())?;
    let info = AttachmentInfo {
        file_name: meta.file_name.clone(),
        mime_type: meta.mime_type.clone(),
        size_bytes: meta.size_bytes,
    };
    let response_key = self.generate_key(request.group_id.as_str(), "", &info);

    Ok(Arc::new(AttachmentInfoResponse {
        key: Arc::new(response_key),
        info: Arc::new(info),
    }))
}
```

注意：`generate_key` 现在会收到空字符串 msg_id。确认 `generate_key` 的实现：
```rust
fn generate_key(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo) -> String {
    format!("{}/{}/{}", group_id, msg_id, info.file_name)
}
```
下载时 key 应该直接用 request.key 本身（文件已经上传完成），所以 `download_attachment_header` 中的 `response_key` 直接使用 `request.key`：

```rust
let response_key = request.key.clone();
```

- [ ] **Step 4: 实现 start_send_download_attachment_payload**

将原来 `download_attachment_header` 中的后台推送逻辑移到这里：

```rust
async fn start_send_download_attachment_payload(&self, key: &str) -> std::result::Result<(), kissbot_channel::Error> {
    let sender = self.on_download_attachment_payload.upgrade()
        .ok_or_else(|| kissbot_channel::Error::InternalError("download payload sender unavailable".to_string()))?;
    let store = self.attachment_store.clone();
    let key_owned = key.to_string();

    tokio::spawn(async move {
        const CHUNK_SIZE: u64 = 65536;

        let file_result = store.open_file(&key_owned);
        let (mut file, file_len) = match file_result {
            Ok(f) => f,
            Err(e) => {
                tracing::error!("Failed to open attachment for download: key={}, error={}", key_owned, e);
                return;
            }
        };

        let mut pos = 0u64;
        let mut ok = true;
        while pos < file_len && ok {
            let end = std::cmp::min(pos + CHUNK_SIZE, file_len);
            let chunk_size = (end - pos) as usize;
            let (mut buf, sn) = match sender.prepare_send(&key_owned, file_len as u32, pos) {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!("prepare_send error: {}", e);
                    break;
                }
            };
            // 读取到 payload 偏移处（prepare_send 已分配足够 capacity）
            use std::io::Read;
            if let Err(e) = (&mut file).read_exact(&mut buf[OFFSET_ATT_DATA..OFFSET_ATT_DATA + chunk_size]) {
                tracing::error!("Failed to read file chunk: {}", e);
                break;
            }
            ok = sender.send(&key_owned, file_len as u32, pos, buf).await.is_ok();
            pos = end;
        }
        // 发送 size=0 的结束标记
        if let Ok((buf, _sn)) = sender.prepare_send(&key_owned, 0, pos) {
            let _ = sender.send(&key_owned, 0, pos, buf).await;
        }
    });

    Ok(())
}
```

- [ ] **Step 5: 更新 send_message 和 Messenger trait 实现**

`send_message` 和 `download_attachment_header` 的实现参数 `_attachment_sn: Arc<AtomicU32>` 保留但不需要使用。

- [ ] **Step 6: 编译验证**

```bash
cd kissbot-channel-web && cargo check
cd kissbot-channel && cargo check
cargo test -p kissbot-api
```

Expected: 全部编译通过，API 测试通过。

- [ ] **Step 7: Commit**

```bash
git add kissbot-channel-web/src/messenger.rs kissbot-channel-web/src/attachment.rs
git commit -m "refactor: WebMessenger 适配 — 删除 global_attachment_sn，新增 start_send_download_attachment_payload

- AttachmentMeta 删除 att_id 字段
- WebMessenger 删除 global_attachment_sn/next_attachment_sn
- download_attachment_header 不再 spawn，只返回 header
- 新增 start_send_download_attachment_payload 内部 spawn 推送
- 每发一个 chunk 后 await agent response

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 5: HTTP handler 清理

**Files:**
- Modify: `kissbot-channel-web/src/http.rs` — 删除 `upload_id`/`next_attachment_sn`；删除 `AttachmentInfo.att_id`

**Interfaces:**
- Consumes: Task 1 的无 att_id 的 `AttachmentInfo`；Task 4 的无 `next_attachment_sn` 的 messenger

- [ ] **Step 1: handle_init_attachment 删除 upload_id**

```rust
/// POST /api/attachment/init — 初始化附件上传，创建临时文件并发送消息
async fn handle_init_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<InitAttachmentRequest>,
) -> impl IntoResponse {
    // 生成 msg_id
    let msg_id = messenger.next_msg_id();
    let key = format!("{}/{}/{}", req.group_id, msg_id, req.file_name);

    // 创建临时文件
    let (temp_path, target_path) = match messenger.attachment_store.create_temp_file(
        req.group_id.as_str(), msg_id.as_str(), req.file_name.as_str()
    ) {
        Ok(paths) => paths,
        Err(e) => return Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    };

    // 记录 PendingAttachment
    messenger.pending_uploads.insert(key.clone(), PendingAttachment {
        group_id: req.group_id.clone(),
        msg_id: msg_id.clone(),
        file_name: req.file_name.clone(),
        mime_type: req.mime_type.clone(),
        size_bytes: req.size_bytes,
        temp_path,
        target_path,
    });

    // 构造 OutgoingMessage 并发送（删除了 att_id）
    let info = AttachmentInfo {
        file_name: req.file_name.clone(),
        mime_type: req.mime_type.clone(),
        size_bytes: req.size_bytes,
    };
    let outgoing = OutgoingMessage {
        messenger_id: messenger.messenger_id.clone(),
        user_id: ADMIN_USER_ID.clone(),
        group_id: req.group_id.clone(),
        msg_type: Arc::new(MSG_TYPE_ATTACHMENT.to_string()),
        content: Content::AttachmentInfo(Arc::new(info)),
    };

    match messenger.send(outgoing).await {
        Ok(resp) => Json(ApiResponse::success(serde_json::json!({
            "key": key,
            "msg_id": resp.msg_id,
        }))),
        Err(e) => {
            // 发送失败时清理 pending 记录和临时文件
            messenger.pending_uploads.remove(&key);
            let _ = std::fs::remove_file(&temp_path_for_cleanup);
            Json(ApiResponse::<serde_json::Value>::error(e.to_string()))
        }
    }
}
```

注意：`temp_path_for_cleanup` 需要保留（在 `pending_uploads.insert` 之前或之后赋值）。错误清理分支中的 `temp_path_for_cleanup` 变量需要在 insert 之前保存。

- [ ] **Step 2: build_message_content 删除 att_id**

```rust
// 附件部分
for a in atts {
    let info = AttachmentInfo {
        file_name: a.file_name.clone(),
        mime_type: Arc::new(mime_guess::from_path(a.file_name.as_str())
            .first_or_octet_stream().to_string()),
        size_bytes: 0,
    };
    // ...
}
```

- [ ] **Step 3: 清理不再需要的 import**

检查并删除：
- `use std::sync::atomic::AtomicU32;`（如果不再需要）
- 其他不再使用的 import

- [ ] **Step 4: 编译验证**

```bash
cd kissbot-channel-web && cargo check
cargo test -p kissbot-api
```

Expected: 编译通过，全部测试通过。

- [ ] **Step 5: Commit**

```bash
git add kissbot-channel-web/src/http.rs
git commit -m "refactor: HTTP handler 清理 — 删除 upload_id 和 att_id

- handle_init_attachment 删除 next_attachment_sn/upload_id
- AttachmentInfo 构造删除 att_id 字段
- build_message_content 删除 att_id

Co-Authored-By: deepseek-v4-flash"
```
