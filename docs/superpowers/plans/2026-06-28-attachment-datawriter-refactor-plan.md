# Attachment DataWriter 与 id 隔离重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 附件 id 机制改为 ChannelManager 内部管理，Messenger trait 层纯 key 操作；引入 DataWriter trait 替代 `&[u8]`。

**Architecture:** 分 3 个任务：(1) kissbot-api 类型变更, (2) kissbot-channel 适配, (3) kissbot-channel-web 适配。Messenger trait 签名已手动改好，核心是 ChannelManager 的 id→key 映射重构。

**Tech Stack:** Rust, kissbot-api, kissbot-channel, kissbot-channel-web, DashMap, DataWriter

## Global Constraints

- `DataWriter<E>` trait 已定义在 `kissbot-api/src/common.rs`
- Messenger trait 和 AttachmentDownloadPayloadSender 的 `send_attachment_payload` 已改为 `(key: &str, size: u32, pos: u64, write/writer: Arc<dyn DataWriter<Error>>)`
- 不要删除代码中的注释
- `attachment_receiver_map` / `attachment_sender_map` 改为 `String`(key) 索引
- 新增 `receiver_id_to_key: DashMap<u32, String>` 和 `sender_id_to_key: DashMap<u32, String>`
- `process_attachment_message` 去掉 `attachment_sn` 参数和 upload_id 分配

---

### Task 1: kissbot-api 类型变更

**Files:**
- Modify: `kissbot-api/src/channel.rs` — `OutgoingMessageResponse` 去 upload_id_map, `AttachmentDownloadResponseHeader` 去 download_id 加 ResponseAttachmentInfo, 新增 `WsOutgoingMessageResponse`, `WsAttachmentDownloadResponseHeader`

**Interfaces:**
- Consumes: 无（纯类型变更）
- Produces: 无 id 的 Messenger 层类型 + 带 id 的 WS 层类型

- [ ] **Step 1: 修改 OutgoingMessageResponse — 去掉 attachment_upload_id_map**

找到 `OutgoingMessageResponse` 定义（当前约 80-85 行）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub attachment_upload_id_map: Arc<DashMap<String, u32>>,
    pub attachment_key_map: Arc<DashMap<String, Arc<String>>>,
}
```

改为：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub attachment_key_map: Arc<DashMap<String, Arc<String>>>,  // att_id → key
}
```

- [ ] **Step 2: 修改 AttachmentDownloadResponseHeader — 去掉 download_id，加 ResponseAttachmentInfo**

找到 `AttachmentDownloadResponseHeader` 定义（当前约 102-106 行）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadResponseHeader {
    pub download_id: u32,
    pub metadata: Arc<AttachmentInfo>,
}
```

改为：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadResponseHeader {
    pub response: Arc<ResponseAttachmentInfo>,   // key + AttachmentInfo
}
```

- [ ] **Step 3: 新增 WsOutgoingMessageResponse 和 WsAttachmentDownloadResponseHeader**

在 `AttachmentDownloadResponseHeader` 定义之后添加：

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

- [ ] **Step 4: 更新 kissbot-api 测试**

找到 `test_serde_outgoing_message_response`（当前约 286-299 行），更新为不包含 `attachment_upload_id_map`：

```rust
#[test]
fn test_serde_outgoing_message_response() {
    let key_map = Arc::new(DashMap::new());
    key_map.insert("att1".to_string(), Arc::new("g1/msg1/photo.png".to_string()));

    let obj = OutgoingMessageResponse {
        msg_id: Arc::new("msg1".to_string()),
        time: Arc::new("2026-01-01 00:00:00".to_string()),
        attachment_key_map: key_map,
    };
    let json = serde_json::to_value(&obj).unwrap();
    let deserialized: OutgoingMessageResponse = serde_json::from_value(json).unwrap();
    assert_eq!(*deserialized.msg_id, "msg1");
    assert_eq!(deserialized.attachment_key_map.len(), 1);
}
```

更新 `test_serde_attachment_download_response_header`（当前约 314-329 行）：

```rust
#[test]
fn test_serde_attachment_download_response_header() {
    let metadata = Arc::new(AttachmentInfo {
        att_id: Arc::new("att1".to_string()),
        filename: Arc::new("doc.pdf".to_string()),
        mime_type: Arc::new("application/pdf".to_string()),
        size_bytes: 99999,
    });
    let response = Arc::new(ResponseAttachmentInfo {
        key: Arc::new("g1/msg1/doc.pdf".to_string()),
        info: metadata,
    });
    let obj = AttachmentDownloadResponseHeader {
        response,
    };
    let json = serde_json::to_value(&obj).unwrap();
    let deserialized: AttachmentDownloadResponseHeader = serde_json::from_value(json).unwrap();
    assert_eq!(*deserialized.response.key, "g1/msg1/doc.pdf");
    assert_eq!(*deserialized.response.info.att_id, "att1");
}
```

- [ ] **Step 5: 编译验证**

```bash
cargo test
```

Expected: 所有测试通过（编译通过，61 tests pass）。

- [ ] **Step 6: Commit**

```bash
git add kissbot-api/src/channel.rs
git commit -m "refactor: API 类型 — OutgoingMessageResponse 和 AttachmentDownloadResponseHeader 去 id，新增 WS 层 struct

- OutgoingMessageResponse 去掉 attachment_upload_id_map
- AttachmentDownloadResponseHeader 改为 ResponseAttachmentInfo（无 download_id）
- 新增 WsOutgoingMessageResponse（带 upload_id_map）
- 新增 WsAttachmentDownloadResponseHeader（带 download_id）

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: kissbot-channel 适配

**Files:**
- Modify: `kissbot-channel/src/channel_manager.rs` — 重构 attachment_receiver_map/attachment_sender_map 为 key 索引，新增 id→key 映射，各 processor 适配 DataWriter 签名
- Modify: `kissbot-channel/src/attachment.rs` — `process_attachment_message` 去掉 attachment_sn 和 upload_id 分配
- Modify: `kissbot-channel/src/lib.rs` — 无需修改（已导出 attachment 模块）

**Interfaces:**
- Consumes: Task 1（无 id 的 Messenger 层类型 + 带 id 的 WS 层类型）
- Produces: 适配后的 ChannelManager + process_attachment_message

- [ ] **Step 1: process_attachment_message 去掉 attachment_sn 参数**

修改 `kissbot-channel/src/attachment.rs` 中的函数签名和内部逻辑。

当前签名：

```rust
pub fn process_attachment_message(
    outgoing: &OutgoingMessage,
    msg_id: &str,
    key_generator: &dyn AttachmentKeyGenerator,
    attachment_sn: &Arc<AtomicU32>,
) -> Result<(String, OutgoingMessageResponse, Vec<(u32, Arc<AttachmentInfo>, Arc<String>)>)>
```

改为：

```rust
pub fn process_attachment_message(
    outgoing: &OutgoingMessage,
    msg_id: &str,
    key_generator: &dyn AttachmentKeyGenerator,
) -> Result<(String, OutgoingMessageResponse, Vec<(Arc<AttachmentInfo>, Arc<String>)>)>
```

内部的修改：
- 去掉 `let mut pending_attachments: Vec<(u32, ...)>` 中的 `u32` 字段
- 去掉 `attachment_upload_id_map` 的构建和填充
- 去掉 `attachment_sn.fetch_add(...)` 调用
- 构建 `OutgoingMessageResponse` 时不再传入 `attachment_upload_id_map`

处理 `"attachment"` 分支的代码修改示例：

```rust
MSG_TYPE_ATTACHMENT => {
    let info: AttachmentInfo = serde_json::from_str(outgoing.content.as_str())
        .map_err(|e| crate::Error::InternalError(format!("parse AttachmentInfo failed: {}", e)))?;
    let key = key_generator.generate_key(
        outgoing.group_id.as_str(), msg_id, &info
    );
    let info_arc = Arc::new(info);
    let key_arc = Arc::new(key.clone());
    attachment_key_map.insert(info_arc.att_id.to_string(), key_arc.clone());
    pending_attachments.push((info_arc.clone(), key_arc.clone()));
    let response = ResponseAttachmentInfo {
        key: key_arc,
        info: info_arc,
    };
    serde_json::to_string(&response)
        .map_err(|e| crate::Error::InternalError(format!("serialize ResponseAttachmentInfo failed: {}", e)))?
}
```

`OutgoingMessageResponse` 构建：

```rust
let response = OutgoingMessageResponse {
    msg_id: Arc::new(msg_id.to_string()),
    time: Arc::new(String::new()),
    attachment_key_map,
};

Ok((new_content, response, pending_attachments))
```

去掉不再需要的 import：`use std::sync::atomic::AtomicU32;`（如果不再被其他地方使用）。

- [ ] **Step 2: 重构 ChannelManager 内部映射**

修改 `ChannelManager` struct 的字段（当前约 37-45 行）：

```rust
pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
    messenger_map: DashMap<String, Arc<MessengerContext>>,
    memory_store_client: Arc<MemoryStoreClient>,
    global_attachment_sn: Arc<AtomicU32>,
    // 上传方向：key → (internal_upload_id, Weak<Messenger>)
    attachment_receiver_map: DashMap<String, (u32, Weak<dyn Messenger>)>,
    // upload_id → key（WS 二进制帧按 id 查找后转 key）
    receiver_id_to_key: DashMap<u32, String>,
    // 下载方向：key → (internal_download_id, Weak<ConnectContext>)
    attachment_sender_map: DashMap<String, (u32, Weak<ConnectContext>)>,
    // download_id → key
    sender_id_to_key: DashMap<u32, String>,
}
```

在 `new()` 或构造函数中初始化新字段。

- [ ] **Step 3: 修改 OutgoingMessageProcessor**

找到 `OutgoingMessageProcessor::raw_process_json`（当前约 287-314 行）。

当前代码在拿到 `messenger.send_message()` 的 response 后：

```rust
for id in response.attachment_upload_id_map.iter() {
    manager.attachment_receiver_map.insert(*id, Arc::downgrade(&messenger_context.messenger));
}
let response = serde_json::to_value(response)?;
Ok(Some(response))
```

改为：

```rust
// 从 response 中获取 key_map，分配内部 upload_id，构造 WsOutgoingMessageResponse
let ws_response = WsOutgoingMessageResponse {
    msg_id: response.msg_id.clone(),
    time: response.time.clone(),
    attachment_upload_id_map: Arc::new(DashMap::new()),
    attachment_key_map: response.attachment_key_map.clone(),
};

for entry in response.attachment_key_map.iter() {
    let att_id = entry.key().clone();
    let key = entry.value().clone();
    let internal_id = manager.global_attachment_sn.fetch_add(1, Ordering::SeqCst);
    ws_response.attachment_upload_id_map.insert(att_id, internal_id);
    manager.attachment_receiver_map.insert(key.to_string(), (internal_id, Arc::downgrade(&messenger_context.messenger)));
    manager.receiver_id_to_key.insert(internal_id, key.to_string());
}

let response = serde_json::to_value(ws_response)?;
Ok(Some(response))
```

需要添加 import: `use kissbot_api::channel::WsOutgoingMessageResponse;`（从 `kissbot_api::channel::*` 已导入）。

- [ ] **Step 4: 修改 AttachmentPayloadProcessor**

找到 `AttachmentPayloadProcessor::raw_process_bin`（当前约 332-352 行）。

当前代码：

```rust
let header = parse_attachment_payload_header(data)?;
let manager = self.manager.upgrade()...;
let messenger = manager.attachment_receiver_map.get(&header.id)...;
let messenger = messenger.upgrade()...;
if header.size == 0 { manager.attachment_receiver_map.remove(&header.id); }
messenger.send_attachment_payload(header.id, header.size, header.pos, data).await?;
Ok(None)
```

改为使用 DataWriter 和 key 查找：

```rust
let header = parse_attachment_payload_header(data)?;
let manager = self.manager.upgrade()...;

// 通过 id 找到 key
let key = manager.receiver_id_to_key.get(&header.id)
    .ok_or_else(|| Error::AttachmentNotFound(header.id.to_string()))?
    .clone();

// 通过 key 找到 messenger
let messenger_entry = manager.attachment_receiver_map.get(&key)
    .ok_or_else(|| Error::AttachmentNotFound(key.clone()))?;
let (_, ref messenger) = *messenger_entry;
let messenger = messenger.upgrade()...;

if header.size == 0 {
    manager.attachment_receiver_map.remove(&key);
    manager.receiver_id_to_key.remove(&header.id);
}

// 用 DataWriter 包装 &[u8]
struct SliceDataWriter<'a>(&'a [u8]);

impl DataWriter<crate::Error> for SliceDataWriter<'_> {
    fn write_to(&self, buf: &mut bytes::BytesMut) -> std::result::Result<(), crate::Error> {
        buf.extend_from_slice(self.0);
        Ok(())
    }
}

let writer: Arc<dyn DataWriter<crate::Error>> = Arc::new(SliceDataWriter(data));
messenger.send_attachment_payload(&key, header.size, header.pos, writer).await?;
Ok(None)
```

添加 import：`use bytes::BytesMut;` 和 `use kissbot_api::DataWriter;`。

- [ ] **Step 5: 修改 AttachmentDownloadRequestProcessor**

找到 `AttachmentDownloadRequestProcessor::raw_process_json`（当前约 370-400 行）。

当前代码：

```rust
let response = messenger_context.messenger.download_attachment_header(request, manager.global_attachment_sn.clone()).await?;
manager.attachment_sender_map.insert(response.download_id, Arc::downgrade(&connect_context));
```

改为：

```rust
let response = messenger_context.messenger.download_attachment_header(request, manager.global_attachment_sn.clone()).await?;
let key = response.response.key.clone();
let internal_id = manager.global_attachment_sn.fetch_add(1, Ordering::SeqCst);
manager.attachment_sender_map.insert(key.to_string(), (internal_id, Arc::downgrade(&connect_context)));
manager.sender_id_to_key.insert(internal_id, key.to_string());

// 构造 WsAttachmentDownloadResponseHeader
let ws_response = WsAttachmentDownloadResponseHeader {
    download_id: internal_id,
    response: response.response.clone(),
};
let responce = serde_json::to_value(ws_response)?;
Ok(Some(responce))
```

添加 import：`use kissbot_api::channel::WsAttachmentDownloadResponseHeader;`。

- [ ] **Step 6: 修改 ChannelManager::send_attachment_payload（AttachmentDownloadPayloadSender impl）**

找到 `AttachmentDownloadPayloadSender` 的 ChannelManager 实现（当前约 702-721 行）。当前签名和逻辑都需要改。

当前：

```rust
#[async_trait]
impl AttachmentDownloadPayloadSender for ChannelManager {
    async fn send_attachment_payload(&self, data: Bytes) -> Result<()> {
        let header = parse_attachment_payload_header(data.as_ref())?;
        let connect_context = self.attachment_sender_map.get(&header.id)...;
        if header.size == 0 { self.attachment_sender_map.remove(&header.id); }
        connect_context.ws_context.send_bin(data).await?;
        Ok(())
    }
}
```

改为：

```rust
#[async_trait]
impl AttachmentDownloadPayloadSender for ChannelManager {
    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, writer: Arc<dyn DataWriter<Error>>) -> Result<()> {
        let sender_entry = self.attachment_sender_map.get(key)
            .ok_or_else(|| Error::AttachmentNotFound(key.to_string()))?;
        let (internal_id, ref connect_weak) = *sender_entry;
        let connect_context = connect_weak.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

        // 从 writer 读取数据
        let mut buf = bytes::BytesMut::new();
        writer.write_to(&mut buf)?;
        // 构造 kai-ws 二进制帧: [sn(4) + payload_type(4) + status_code(4)] + [id(4) + size(4) + pos(8)] + [data]
        let mut frame = bytes::BytesMut::new();
        frame.put_u32(0);  // sn
        frame.put_u32(TYPE_ATTACHMENT_PAYLOAD);  // payload_type
        frame.put_u32(CODE_SUCCESS);  // status_code
        frame.put_u32(internal_id);
        frame.put_u32(size);
        frame.put_u64(pos);
        frame.extend_from_slice(&buf);

        if size == 0 {
            self.attachment_sender_map.remove(key);
            self.sender_id_to_key.remove(&internal_id);
        }

        connect_context.ws_context.send_bin(frame.freeze()).await?;
        Ok(())
    }
}
```

需要添加 import：`use bytes::BufMut;`（已通过 `kai_ws` 间接导入，但显式确认）。`use kissbot_api::DataWriter;` 也需确认。

- [ ] **Step 7: 清理不需要的 import**

`channel_manager.rs` 中不再需要 `use bytes::Bytes;`（如果原来只用于 `send_attachment_payload` 的 `Bytes` 参数）。保留 `bytes::BytesMut` 和 `bytes::BufMut`。

- [ ] **Step 8: 编译验证**

```bash
cargo check
```

Expected: kissbot-channel 和 kissbot-api 编译通过。

- [ ] **Step 9: Commit**

```bash
git add kissbot-channel/src/attachment.rs kissbot-channel/src/channel_manager.rs
git commit -m "refactor: ChannelManager 适配 DataWriter 和 key 路由

- process_attachment_message 去掉 attachment_sn 参数和 upload_id 分配
- attachment_receiver_map 改为 key 索引 + receiver_id_to_key 反向映射
- attachment_sender_map 改为 key 索引 + sender_id_to_key 反向映射
- OutgoingMessageProcessor 用 key_map 分配内部 id 构造 WsOutgoingMessageResponse
- AttachmentPayloadProcessor 用 id→key→messenger 链调用 DataWriter
- AttachmentDownloadRequestProcessor 用 key 分配内部 id 构造 WsAttachmentDownloadResponseHeader
- AttachmentDownloadPayloadSender 用 key 查内部 id 转发二进制帧

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: kissbot-channel-web 适配

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs` — `pending_uploads` 改为 key 索引，`send_attachment_payload` 用 key+DataWriter，`download_attachment_header` 返回新类型

**Interfaces:**
- Consumes: Task 1（新 AttachmentDownloadResponseHeader 无 download_id）, Task 2（新 process_attachment_message 无 attachment_sn 参数）

- [ ] **Step 1: pending_uploads 改为 key 索引**

`WebMessenger` struct 中 `pending_uploads: DashMap<u32, PendingAttachment>` 改为：

```rust
pub pending_uploads: DashMap<String, PendingAttachment>,  // key → pending
```

`send()` 方法中插入 pending_uploads 处改为用 key：

```rust
// Task 2 后 process_attachment_message 返回的 pending_attachments 是 Vec<(Arc<AttachmentInfo>, Arc<String>)>
let (new_content, response, pending_attachments) = kissbot_channel::process_attachment_message(
    &outgoing,
    msg_id.as_str(),
    self,
).map_err(|e| Error::InternalError(e.to_string()))?;

for (info, key) in pending_attachments {
    let (temp_path, target_path) = match self.attachment_store.create_temp_file(
        outgoing.group_id.as_str(), msg_id.as_str(), info.filename.as_str()
    ) {
        Ok(paths) => paths,
        Err(e) => return Err(Error::from(e)),
    };
    self.pending_uploads.insert(key.to_string(), PendingAttachment {
        group_id: outgoing.group_id.clone(),
        msg_id: msg_id.clone(),
        filename: info.filename.clone(),
        mime_type: info.mime_type.clone(),
        size_bytes: info.size_bytes,
        temp_path,
        target_path,
    });
}
```

- [ ] **Step 2: send_attachment_payload 改用 key + DataWriter**

找到 `WebMessenger::send_attachment_payload` 实现（当前约 616-634 行）。

当前：

```rust
async fn send_attachment_payload(&self, id: u32, _size: u32, pos: u64, data: &[u8]) -> std::result::Result<(), kissbot_channel::Error> {
    let pending = self.pending_uploads.get(&id)
        .ok_or_else(|| kissbot_channel::Error::InternalError(format!("upload_id {} not found", id)))?;
    self.attachment_store.append_to_temp(&pending.temp_path, data)
        .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;
    if (pos + data.len() as u64) >= pending.size_bytes {
        drop(pending);
        if let Some((_, pending)) = self.pending_uploads.remove(&id) {
            AttachmentStore::finalize_upload(&pending.temp_path, &pending.target_path)...;
        }
    }
    Ok(())
}
```

改为：

```rust
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, write: Arc<dyn DataWriter<kissbot_channel::Error>>) -> std::result::Result<(), kissbot_channel::Error> {
    let pending = self.pending_uploads.get(key)
        .ok_or_else(|| kissbot_channel::Error::InternalError(format!("key {} not found", key)))?;

    // 从 DataWriter 读取数据
    let mut buf = bytes::BytesMut::new();
    write.write_to(&mut buf)
        .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;

    self.attachment_store.append_to_temp(&pending.temp_path, &buf)
        .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;

    // 如果这是最后一块，用 remove 原子地取出并重命名
    if (pos + buf.len() as u64) >= pending.size_bytes {
        drop(pending);
        if let Some((_, pending)) = self.pending_uploads.remove(key) {
            AttachmentStore::finalize_upload(&pending.temp_path, &pending.target_path)
                .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;
        }
    }

    Ok(())
}
```

添加 import：`use kissbot_api::DataWriter;`（检查是否已导入）。`use bytes::BytesMut;`（从 `bytes::{BytesMut, BufMut}` 已导入）。

- [ ] **Step 3: download_attachment_header 返回新类型**

找到 `WebMessenger::download_attachment_header` 实现（当前约 636-695 行）。

需要改返回的 `AttachmentDownloadResponseHeader` 构造。当前：

```rust
async fn download_attachment_header(&self, request: AttachmentDownloadRequest, attachment_sn: Arc<AtomicU32>) -> ... {
    let meta = self.attachment_store.get_meta_by_key(request.key.as_str())?;
    let download_id = attachment_sn.fetch_add(1, Ordering::SeqCst);
    // ...spawn background task using sender.send_attachment_payload(download_id, ...)

    Ok(Arc::new(AttachmentDownloadResponseHeader {
        download_id,
        metadata: Arc::new(AttachmentInfo {
            att_id: meta.att_id,
            filename: meta.filename.clone(),
            mime_type: meta.mime_type,
            size_bytes: meta.size_bytes,
        }),
    }))
}
```

改为：

```rust
async fn download_attachment_header(&self, request: AttachmentDownloadRequest, _attachment_sn: Arc<AtomicU32>) -> ... {
    let meta = self.attachment_store.get_meta_by_key(request.key.as_str())?;
    let info = AttachmentInfo {
        att_id: meta.att_id.clone(),
        filename: meta.filename.clone(),
        mime_type: meta.mime_type.clone(),
        size_bytes: meta.size_bytes,
    };
    let key = self.generate_key(request.group_id.as_str(), &meta.att_id, &info);

    // 启动后台任务：逐块读取文件并推送（通过 AttachmentDownloadPayloadSender 的 DataWriter 机制）
    let sender = self.on_download_attachment_payload.upgrade()
        .ok_or_else(|| ...)?;
    let store = self.attachment_store.clone();
    let key_clone = key.clone();

    tokio::spawn(async move {
        const CHUNK_SIZE: u64 = 65536;
        match store.get_attachment_by_key(&request.key) {
            Ok(data) => {
                let len = data.len() as u64;
                let mut pos = 0u64;
                let mut ok = true;
                while pos < len && ok {
                    let end = std::cmp::min(pos + CHUNK_SIZE, len);
                    let chunk = &data[pos as usize..end as usize];
                    // 用 DataWriter 包装 chunk
                    struct ChunkWriter<'a>(&'a [u8]);
                    impl DataWriter<kissbot_channel::Error> for ChunkWriter<'_> {
                        fn write_to(&self, buf: &mut bytes::BytesMut) -> std::result::Result<(), kissbot_channel::Error> {
                            buf.extend_from_slice(self.0);
                            Ok(())
                        }
                    }
                    let writer: Arc<dyn DataWriter<kissbot_channel::Error>> = Arc::new(ChunkWriter(chunk));
                    ok = sender.send_attachment_payload(&key_clone, len as u32, pos, writer).await.is_ok();
                    pos = end;
                }
                // 发送 size=0 的结束标记
                let writer: Arc<dyn DataWriter<kissbot_channel::Error>> = Arc::new(ChunkWriter(&[]));
                let _ = sender.send_attachment_payload(&key_clone, 0, pos, writer).await;
            }
            Err(e) => {
                tracing::error!("Failed to read attachment for download: key={}, error={}", request.key, e);
            }
        }
    });

    Ok(Arc::new(AttachmentDownloadResponseHeader {
        response: Arc::new(ResponseAttachmentInfo {
            key: Arc::new(key),
            info: Arc::new(info),
        }),
    }))
}
```

注意：`request.group_id` 和 `meta.att_id` 需要用于 key 生成。`generate_key` 的签名是 `generate_key(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo)`，这里 `msg_id` 可以用 `meta.att_id` 替代，或者从 `request.key` 中解析。

- [ ] **Step 4: 清理无用 import**

检查 `messenger.rs` 中不再需要的 import，如 `AtomicU32` 如果不再用于 `global_attachment_sn` 字段（但 `WebMessenger` 还有 `msg_id_seq: AtomicU32`，所以保留）。

- [ ] **Step 5: 编译验证**

```bash
cargo check
```

Expected: kissbot-channel-web 编译通过（仅 pre-existing warnings）。

- [ ] **Step 6: Commit**

```bash
git add kissbot-channel-web/src/messenger.rs
git commit -m "refactor: WebMessenger 适配 DataWriter 和 key 路由

- pending_uploads 改为 key(String) 索引
- send_attachment_payload 用 key 查找 pending、DataWriter 获取数据
- download_attachment_header 返回无 download_id 的新类型
- 后台推送使用 DataWriter 包装 chunk

Co-Authored-By: deepseek-v4-flash"
```
