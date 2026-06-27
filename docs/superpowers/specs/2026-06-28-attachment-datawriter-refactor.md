# 附件 DataWriter 与 id 隔离重构设计

## 概述

重构附件上传下载的 id 管理机制：ChannelManager 和 agent 经 WS 通信的内部机制使用 id 做路由，Messenger trait 层完全以 key 识别附件。同步引入 DataWriter 机制替代传统的 `&[u8]` 参数传递二进制数据。

## 核心变更

### 1. 分层隔离

```
                     Messenger trait 层（纯 key，无 id）
                    ┌─────────────────────────────────────┐
                    │  send_attachment_payload(key, ...)  │
                    │  download_attachment_header(key)    │
                    │  → ResponseAttachmentInfo           │
                    └────────────┬────────────────────────┘
                                 │ ChannelManager 做 key ↔ id 转换
                    ┌────────────▼────────────────────────┐
                    │  WS 协议层（id 路由）                 │
                    │  AttachmentPayloadHeader { id, ... } │
                    │  WsOutgoingMessageResponse           │
                    │  WsAttachmentDownloadResponseHeader  │
                    └─────────────────────────────────────┘
```

### 2. Messenger trait 变更（已手动完成）

`Messenger::send_attachment_payload` 签名改为：

```rust
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, write: Arc<dyn DataWriter<Error>>) -> Result<()>;
```

- 参数从 `(id: u32, size: u32, pos: u64, data: &[u8])` 改为 `(key: &str, size: u32, pos: u64, write: Arc<dyn DataWriter<Error>>)`
- 调用方通过 `DataWriter` trait 向被调用方提供的 `BytesMut` 写入数据
- `DataWriter` 定义在 `kissbot-api/src/common.rs`：

```rust
pub trait DataWriter<E> {
    fn write_to(&self, buf: &mut BytesMut) -> std::result::Result<(), E>;
}
```

`AttachmentDownloadPayloadSender` 签名同步改为：

```rust
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, writer: Arc<dyn DataWriter<Error>>) -> Result<()>;
```

### 3. kissbot-api 类型变更

`OutgoingMessageResponse` — 去掉 `attachment_upload_id_map`：

```rust
pub struct OutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub attachment_key_map: Arc<DashMap<String, Arc<String>>>,  // att_id → key
}
```

`AttachmentDownloadResponseHeader` — 去掉 `download_id`，改为 `ResponseAttachmentInfo`：

```rust
pub struct AttachmentDownloadResponseHeader {
    pub response: Arc<ResponseAttachmentInfo>,   // key + AttachmentInfo
}
```

新增 WS 协议层 struct（ChannelManager 补充 id 后使用）：

```rust
/// ChannelManager 返回给 agent 的 response，附加上传 id
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsOutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub attachment_upload_id_map: Arc<DashMap<String, u32>>,   // att_id → upload_id
    pub attachment_key_map: Arc<DashMap<String, Arc<String>>>,  // att_id → key
}

/// ChannelManager 返回给 agent 的下载 response，附加下载 id
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsAttachmentDownloadResponseHeader {
    pub download_id: u32,
    pub response: Arc<ResponseAttachmentInfo>,
}
```

### 4. ChannelManager 内部映射

ChannelManager 新增/修改的内部字段：

```rust
// 上传方向：key → (internal_upload_id, Weak<Messenger>)
attachment_receiver_map: DashMap<String, (u32, Weak<dyn Messenger>)>,
// upload_id → key（WS 二进制帧按 id 查找后转 key）
receiver_id_to_key: DashMap<u32, String>,

// 下载方向：key → (internal_download_id, Weak<ConnectContext>)
attachment_sender_map: DashMap<String, (u32, Weak<ConnectContext>)>,
// download_id → key
sender_id_to_key: DashMap<u32, String>,
```

### 5. process_attachment_message 变更

去掉 `attachment_sn` 参数（不再分配 id），返回值去掉 upload_id：

```rust
pub fn process_attachment_message(
    outgoing: &OutgoingMessage,
    msg_id: &str,
    key_generator: &dyn AttachmentKeyGenerator,
) -> Result<(String, OutgoingMessageResponse, Vec<(Arc<AttachmentInfo>, Arc<String>)>)>
```

第三个返回值从 `Vec<(u32, Arc<AttachmentInfo>, Arc<String>)>` 改为 `Vec<(Arc<AttachmentInfo>, Arc<String>)>`（无 id）。

### 6. ChannelManager 流程变更

#### 上传方向（OutgoingMessageProcessor）

```
agent → JSON OutgoingMessage → ChannelManager
  → messenger.send_message(outgoing) → OutgoingMessageResponse（含 attachment_key_map，无 id）
  → ChannelManager 遍历 attachment_key_map：
    分配 internal_upload_id = global_attachment_sn.fetch_add(1)
    记录 attachment_receiver_map[key] = (upload_id, Weak<Messenger>)
    记录 receiver_id_to_key[upload_id] = key
  → 构造 WsOutgoingMessageResponse（含 upload_id_map + key_map）
  → 返回给 agent
```

#### 上传二进制数据（AttachmentPayloadProcessor）

```
agent → WS 二进制帧（header.id = upload_id）
  → parse_attachment_payload_header → { id, size, pos }
  → receiver_id_to_key.get(id) → 拿到 key
  → attachment_receiver_map.get(key) → 拿到 messenger
  → messenger.send_attachment_payload(key, size, pos, writer)
  → 如 size == 0：清理 attachment_receiver_map 和 receiver_id_to_key
```

#### 下载方向（AttachmentDownloadRequestProcessor）

```
agent → JSON AttachmentDownloadRequest → ChannelManager
  → messenger.download_attachment_header(request)
    → AttachmentDownloadResponseHeader { response: ResponseAttachmentInfo }
  → 分配 internal_download_id = global_attachment_sn.fetch_add(1)
  → 记录 attachment_sender_map[key] = (download_id, Weak<ConnectContext>)
  → 记录 sender_id_to_key[download_id] = key
  → 构造 WsAttachmentDownloadResponseHeader { download_id, response }
  → 返回给 agent
```

#### 下载二进制推送（ChannelManager::send_attachment_payload — AttachmentDownloadPayloadSender impl）

```
Messenger 后台任务 → sender.send_attachment_payload(key, size, pos, writer)
  → attachment_sender_map.get(key) → 拿到 (download_id, connect_context)
  → 从 writer 写入 BytesMut
  → 构造 WS 二进制帧：kai-ws header + download_id + size + pos + data
  → send_bin 发给 agent
  → 如 size == 0：清理 attachment_sender_map 和 sender_id_to_key
```

### 7. WebMessenger 适配

`pending_uploads` 从 `DashMap<u32, PendingAttachment>` 改为 `DashMap<String, PendingAttachment>`（key → pending）：

```rust
pub pending_uploads: DashMap<String, PendingAttachment>,
```

`send_attachment_payload` 实现：

```rust
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, write: Arc<dyn DataWriter<Error>>) -> Result<()> {
    // 用 key 查找 pending_uploads
    let pending = self.pending_uploads.get(key)
        .ok_or_else(|| ... )?;
    // 从 writer 读取数据写入临时文件
    let mut buf = BytesMut::new();
    write.write_to(&mut buf)?;
    self.attachment_store.append_to_temp(&pending.temp_path, &buf)?;
    // 最后一块时重命名
    if (pos + buf.len() as u64) >= pending.size_bytes {
        ...finalize_upload...
        self.pending_uploads.remove(key);
    }
}
```

`sender.pending_uploads.insert()` 调用处从 `insert(upload_id, ...)` 改为 `insert(key, ...)`：

```rust
self.pending_uploads.insert(key, PendingAttachment { ... });
```

`download_attachment_header` 返回的 `AttachmentDownloadResponseHeader` 中通过 `AttachmentKeyGenerator` 生成 key：

```rust
async fn download_attachment_header(&self, request: AttachmentDownloadRequest, _attachment_sn: Arc<AtomicU32>) -> Result<Arc<AttachmentDownloadResponseHeader>> {
    let meta = self.attachment_store.get_meta_by_key(request.key.as_str())?;
    let info = meta_to_attachment_info(meta);  // 从 store 的元数据构造 AttachmentInfo
    let key = self.generate_key(request.group_id.as_str(), ...);  // 生成 key
    Ok(Arc::new(AttachmentDownloadResponseHeader {
        response: Arc::new(ResponseAttachmentInfo {
            key: Arc::new(key),
            info: Arc::new(info),
        }),
    }))
    // 后台推送任务仍然存在，通过 AttachmentDownloadPayloadSender 推
}
```

### 8. DataWriter 实现

agent 侧需要为二进制数据实现 `DataWriter`。ChannelManager 的 `AttachmentPayloadProcessor` 中，agent 发来的二进制数据需要包装为实现了 `DataWriter<Error>` 的类型：

```rust
// ChannelManager 内部：把 &[u8] 包装成 DataWriter
struct SliceDataWriter<'a>(&'a [u8]);

impl<'a> DataWriter<crate::Error> for SliceDataWriter<'a> {
    fn write_to(&self, buf: &mut BytesMut) -> Result<()> {
        buf.extend_from_slice(self.0);
        Ok(())
    }
}
```

WebMessenger 的 `send_attachment_payload` 实现中，通过 `write.write_to(&mut buf)` 获取数据。

## 变更文件一览

| 文件 | 变更 |
|------|------|
| `kissbot-api/src/common.rs` | 已加 `DataWriter` trait ✅ |
| `kissbot-api/src/channel.rs` | `OutgoingMessageResponse` 去 upload_id_map；`AttachmentDownloadResponseHeader` 去 download_id 加 ResponseAttachmentInfo；新增 `WsOutgoingMessageResponse`、`WsAttachmentDownloadResponseHeader` |
| `kissbot-channel/src/messenger.rs` | 已改 `send_attachment_payload` 签名为 DataWriter ✅；`download_attachment_header` 签名不变但返回类型变更 |
| `kissbot-channel/src/data.rs` | 已改 `AttachmentDownloadPayloadSender` 签名为 DataWriter ✅ |
| `kissbot-channel/src/attachment.rs` | `process_attachment_message` 去掉 `attachment_sn` 参数和 upload_id 分配 |
| `kissbot-channel/src/channel_manager.rs` | `attachment_receiver_map`/`attachment_sender_map` 改为 key 索引；新增 `receiver_id_to_key`/`sender_id_to_key`；各 processor 适配；新增 `SliceDataWriter` |
| `kissbot-channel-web/src/messenger.rs` | `pending_uploads` 改为 key 索引；`send_attachment_payload` 用 key 查 pending 和 DataWriter 写文件；`download_attachment_header` 返回新类型 |
