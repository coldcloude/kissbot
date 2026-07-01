# 附件清理：删除 att_id + 下载 response 回调

## 概述

两个独立的改动：
1. **删除 `att_id`** — HTTP 上传下载直接用 key 标识文件，`att_id` 是冗余字段
2. **下载 response 回调** — `send` 改为 `send_bin_with_json_response` + oneshot await agent response；Messenger trait 新增 `start_send_download_attachment_payload`

## 改动一：删除 att_id

### 分析

`att_id` 的值始终等于 key（`{group_id}/{msg_id}/{file_name}`）或独立 `upload_id`。key 已经是全局唯一标识，`att_id` 没有任何独立用途。

影响范围：`AttachmentInfo`（API 层）、`AttachmentMeta`（存储层）、`attachment_key_map`（att_id→key 映射）、`global_attachment_sn`、`WsOutgoingMessageResponse.attachment_upload_id_map`。

### 变更清单

**`kissbot-api/src/message.rs`：**
- 从 `AttachmentInfo` 中删除 `att_id: Arc<String>` 字段
- 更新测试（`make_content`、`test_serde_content_attachment_info`、`test_serde_content_attachment_info_response`）

**`kissbot-api/src/channel.rs`：**
- 从 `WsOutgoingMessageResponse` 中删除 `attachment_upload_id_map: Arc<DashMap<String, u32>>`

**`kissbot-channel/src/attachment.rs`：**
- 删除 `attachment_key_map`（att_id→key 映射）及其所有使用

**`kissbot-channel/src/messenger.rs`：**
- `Messenger` trait 中 `send_message` 和 `download_attachment_header` 删除 `attachment_sn: Arc<AtomicU32>` 参数（不再需要）
- `MessengerCreator` trait 中删除 `global_attachment_sn: Arc<AtomicU32>` 参数

**`kissbot-channel/src/channel_manager.rs`：**
- 从 `OutgoingMessageProcessor::raw_process_json` 中删除 `attachment_upload_id_map: Arc::new(DashMap::new())`
- 从 `ChannelManager` 中删除 `global_attachment_sn: Arc<AtomicU32>`（HTTP 端不再需要，WS 端的上传 id 保留为 `internal_id`）

**`kissbot-channel-web/src/attachment.rs`：**
- 从 `AttachmentMeta` 中删除 `att_id: Arc<String>` 字段
- 删除 `save_attachment` 中构造 `att_id` 的代码

**`kissbot-channel-web/src/messenger.rs`：**
- 删除 `global_attachment_sn: Arc<AtomicU32>` 字段
- 删除 `next_attachment_sn()` 方法
- 从 `WebMessenger::new()`、`WebMessengerCreator::create()` 中删除 `global_attachment_sn` 参数
- `download_attachment_header` 中 `att_id: meta.att_id.clone()` → 不再需要
- `Messenger` 实现中 `send_message` 和 `download_attachment_header` 删除 `_attachment_sn` 参数

**`kissbot-channel-web/src/http.rs`：**
- `handle_init_attachment` 中删除 `upload_id` / `next_attachment_sn()` 
- 构造 `AttachmentInfo` 时删除 `att_id` 字段
- 删除 `handle_init_attachment` 返回值中的 `upload_id`

## 改动二：下载 response 回调

### 新流程

```
Agent                      ChannelManager               Messenger
  │                              │                          │
  │── TYPE_ATTACHMENT_DOWNLOAD──►│                          │
  │                              │── download_attachment_   │
  │                              │   header(request) ──────►│
  │                              │◄── AttachmentInfoResp ───│
  │                              │                          │
  │◄── JSON Response ────────────│                          │
  │     (download_id, resp)      │                          │
  │                              │── start_send_download_   │
  │                              │   attachment_payload(key)│
  │                              │   └─ tokio::spawn ──────►│
  │                              │        │                 │
  │◄── bin (TYPE_ATT_PAYLOAD) ───│───────►│                 │
  │     (sn, id, size, pos)     │        │                 │
  │── JSON Response ────────────►│◄───────│                 │
  │     (sn, payload)           │  oneshot│                 │
  │◄── next chunk ──────────────│───────►│                 │
  │         ...                 │        │                 │
```

### sn 问题

`prepare_send` 内部生成 sn 并写入 frame 头部。`send_bin_with_json_response` 也需要这个 sn 来注册 response handler。方案：`prepare_send` 返回 `(BytesMut, u32)`，把 sn 带出来。

```rust
fn prepare_send(&self, key: &str, size: u32, pos: u64) -> Result<(BytesMut, u32)>;
```

### 变更清单

**`kissbot-api/src/channel.rs`：**
- 新增 `DownloadAttachmentPayloadResponse`（agent 对 payload chunk 的 response 内容）：
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct DownloadAttachmentPayloadResponse {
      pub key: Arc<String>,
      pub error_code: u32,
      pub error_msg: Option<Arc<String>>,
  }
  ```

**`kissbot-channel/src/data.rs`：**
- `AttachmentDownloadPayloadSender::prepare_send` 返回值从 `Result<BytesMut>` 改为 `Result<(BytesMut, u32)>`（extra sn 用于注册 response handler）
- `AttachmentDownloadPayloadSender::send` 返回值从 `Result<()>` 改为 `Result<DownloadAttachmentPayloadResponse>`
- 新增 `DownloadResponseHandler` 结构体（内部使用，oneshot 桥接）

**`kissbot-channel/src/messenger.rs`：**
- `Messenger` trait 新增 `start_send_download_attachment_payload` 方法：
  ```rust
  async fn start_send_download_attachment_payload(&self, key: &str) -> Result<()>;
  ```

**`kissbot-channel/src/channel_manager.rs`：**
- `AttachmentDownloadRequestProcessor` 不再用 `JsonProcessorWrapper`，直接实现 `WsJsonProcessor`。在 `process_json` 中：先调用 `messenger.download_attachment_header` → `context.send_json` 返回 header 给 agent → 调用 `messenger.start_send_download_attachment_payload(key)`
- `start_send_download_attachment_payload` 内部 `tokio::spawn`，同步返回，调用方不需要 spawn
- `AttachmentDownloadPayloadSender::send`：用 `send_bin_with_json_response` + oneshot 实现，await agent response 后返回 `DownloadAttachmentPayloadResponse`
- 新增 `DownloadResponseHandler` 结构体：
  ```rust
  struct DownloadResponseHandler {
      response_tx: tokio::sync::Mutex<Option<oneshot::Sender<DownloadAttachmentPayloadResponse>>>,
  }
  
  #[async_trait]
  impl WsJsonProcessor for DownloadResponseHandler {
      async fn process_json(&self, data: WsMessage, _context: Arc<WsContext>) {
          if let Some(tx) = self.response_tx.lock().unwrap().take() {
              // 从 WsMessage.payload 反序列化 DownloadAttachmentPayloadResponse
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

**`kissbot-channel-web/src/messenger.rs`：**
- `download_attachment_header`：不再 `tokio::spawn` 后台推送，只读取 meta 返回 `AttachmentInfoResponse`
- 新增 `start_send_download_attachment_payload` 实现：将原来的后台推送逻辑移到这里
- 推送循环中每发一个 chunk 后通过 `on_download_attachment_payload.send()` await agent response

## Messenger trait 删除 attachment_sn 参数（仅 web 端）

`send_message` 和 `download_attachment_header` 中 `attachment_sn: Arc<AtomicU32>` 对于 web 组件不再需要。由于其他组件的 Messenger 实现可能需要，这个参数保留在 trait 中，但 WebMessenger 的实现中不再使用（`_attachment_sn`）。

`MessengerCreator::create` 中的 `global_attachment_sn` 同理，WebMessengerCreator 接受参数但不使用。
