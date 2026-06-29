# 移除 DataWriter 改用 Bytes 零拷贝实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 移除 DataWriter trait，所有 send_attachment_payload 改 Bytes 参数；AttachmentStore 返回值改 Bytes；HTTP handler 适配零拷贝。

**Architecture:** 分 3 个任务：(1) kissbot-api 删除 DataWriter + trait 签名变更, (2) kissbot-channel 适配, (3) kissbot-channel-web AttachmentStore + HTTP + Messenger 适配。每个任务可独立编译。

**Tech Stack:** Rust, bytes, kissbot-api, kissbot-channel, kissbot-channel-web

## Global Constraints

- `Bytes` 来自 `bytes` crate（已依赖）
- `Bytes::slice()` 和 `BytesMut::extend_from_slice(&data)` 用于零拷贝操作
- `std::fs::read` 返回 `Vec<u8>`，通过 `Bytes::from(vec)` 转为 `Bytes`
- 不要删除代码中的注释

---

### Task 1: kissbot-api — 删除 DataWriter，修改 trait 签名

**Files:**
- Modify: `kissbot-api/src/common.rs` — 删除 `DataWriter` trait
- Modify: `kissbot-api/src/lib.rs` — 删除 `DataWriter` re-export（如存在）
- Modify: `kissbot-channel/src/messenger.rs` — `Messenger` trait 签名改 `Bytes`
- Modify: `kissbot-channel/src/data.rs` — `AttachmentDownloadPayloadSender` 签名改 `Bytes`

**Interfaces:**
- Consumes: 无
- Produces: 新的 `send_attachment_payload` 签名（`Bytes` 参数）

- [ ] **Step 1: 删除 DataWriter trait**

从 `kissbot-api/src/common.rs` 中删除：

```rust
pub trait DataWriter<E>: Send + Sync {
    fn write_to(&self, buf: &mut BytesMut) -> std::result::Result<(),E>;
}
```

同时删除 `use bytes::BytesMut;` import（如果不再需要）。

- [ ] **Step 2: 删除 DataWriter re-export**

检查 `kissbot-api/src/lib.rs`：
```bash
grep "DataWriter" kissbot-api/src/lib.rs
```
如果存在 `DataWriter` 的 re-export，删除。

- [ ] **Step 3: 修改 Messenger trait 签名**

修改 `kissbot-channel/src/messenger.rs`：

```rust
use bytes::Bytes;

#[async_trait]
pub trait Messenger: Send + Sync + 'static {
    async fn get_info(&self) -> Result<Arc<MessengerInfo>>;
    async fn send_message(&self, message: OutgoingMessage, attachment_sn: Arc<AtomicU32>) -> Result<Arc<OutgoingMessageResponse>>;

    // 旧: write: Arc<dyn DataWriter<Error>>
    // 新: data: Bytes
    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> Result<()>;

    async fn download_attachment_header(&self, request: AttachmentDownloadRequest, attachment_sn: Arc<AtomicU32>) -> Result<Arc<AttachmentInfoResponse>>;
}
```

移除 `use kissbot_api::{DataWriter, channel::*};` 中的 `DataWriter`。

- [ ] **Step 4: 修改 AttachmentDownloadPayloadSender 签名**

修改 `kissbot-channel/src/data.rs`：

```rust
use bytes::Bytes;

#[async_trait]
pub trait AttachmentDownloadPayloadSender: Send + Sync {
    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> Result<()>;
}
```

移除不再需要的 `use kissbot_api::DataWriter;` import。

- [ ] **Step 5: 编译验证**

```bash
cd kissbot-api && cargo test
cd kissbot-channel && cargo check
```

- [ ] **Step 6: Commit**

```bash
git add kissbot-api/src/common.rs kissbot-api/src/lib.rs \
       kissbot-channel/src/messenger.rs kissbot-channel/src/data.rs
git commit -m "refactor: 删除 DataWriter trait，send_attachment_payload 改为 Bytes 参数

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: kissbot-channel — adapter 适配 Bytes

**Files:**
- Modify: `kissbot-channel/src/channel_manager.rs` — `AttachmentPayloadProcessor`, `ChannelManager::send_attachment_payload` (AttachmentDownloadPayloadSender impl), 删除 `SliceDataWriter`

**Interfaces:**
- Consumes: Task 1（新 trait 签名）

- [ ] **Step 1: 修改 AttachmentPayloadProcessor::raw_process_bin**

当前代码（约 343-382 行）：

```rust
let header = parse_attachment_payload_header(data)?;
let manager = self.manager.upgrade()...;
let key = manager.receiver_id_to_key.get(&header.id)...;
let messenger_entry = manager.attachment_receiver_map.get(&key)...;
let messenger = messenger_entry...messenger.upgrade()...;

if header.size == 0 {
    manager.attachment_receiver_map.remove(&key);
    manager.receiver_id_to_key.remove(&header.id);
}

struct SliceDataWriter(...);
let writer: Arc<dyn DataWriter<crate::Error>> = Arc::new(SliceDataWriter(data_vec));
messenger.send_attachment_payload(&key, header.size, header.pos, writer).await?;
Ok(None)
```

改为：

```rust
let header = parse_attachment_payload_header(data)?;
let manager = self.manager.upgrade()...;
let key = manager.receiver_id_to_key.get(&header.id)...;
let messenger_entry = manager.attachment_receiver_map.get(&key)...;
let messenger = messenger_entry...messenger.upgrade()...;

if header.size == 0 {
    manager.attachment_receiver_map.remove(&key);
    manager.receiver_id_to_key.remove(&header.id);
}

// data 是 Bytes（WsBinaryProcessor::process_bin 已改为 Bytes 参数）
let payload = data.slice(OFFSET_ATT_DATA..);
messenger.send_attachment_payload(&key, header.size, header.pos, payload).await?;
Ok(None)
```

删除 `SliceDataWriter` 整个定义和 `use kissbot_api::DataWriter;` import。

- [ ] **Step 2: 修改 ChannelManager::send_attachment_payload（AttachmentDownloadPayloadSender impl）**

当前代码：

```rust
#[async_trait]
impl AttachmentDownloadPayloadSender for ChannelManager {
    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, writer: Arc<dyn DataWriter<Error>>) -> Result<()> {
        let sender_entry = self.attachment_sender_map.get(key)...;
        let (internal_id, ref connect_weak) = *sender_entry;
        let connect_context = connect_weak.upgrade()...;

        let mut buf = bytes::BytesMut::new();
        writer.write_to(&mut buf)?;

        let mut frame = bytes::BytesMut::new();
        frame.put_u32(0);
        frame.put_u32(TYPE_ATTACHMENT_PAYLOAD);
        frame.put_u32(CODE_SUCCESS);
        frame.put_u32(internal_id);
        frame.put_u32(size);
        frame.put_u64(pos);
        frame.extend_from_slice(&buf);

        if size == 0 { ... cleanup ... }
        connect_context.ws_context.send_bin(frame.freeze()).await?;
        Ok(())
    }
}
```

改为：

```rust
#[async_trait]
impl AttachmentDownloadPayloadSender for ChannelManager {
    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> Result<()> {
        let sender_entry = self.attachment_sender_map.get(key)...;
        let (internal_id, ref connect_weak) = *sender_entry;
        let connect_context = connect_weak.upgrade()...;

        let mut frame = bytes::BytesMut::with_capacity(OFFSET_ATT_DATA + data.len());
        frame.put_u32(0);
        frame.put_u32(TYPE_ATTACHMENT_PAYLOAD);
        frame.put_u32(CODE_SUCCESS);
        frame.put_u32(internal_id);
        frame.put_u32(size);
        frame.put_u64(pos);
        frame.extend_from_slice(&data);

        if size == 0 { ... cleanup ... }
        connect_context.ws_context.send_bin(frame.freeze()).await?;
        Ok(())
    }
}
```

删掉 `use kissbot_api::DataWriter;` 和不再需要的 `bytes::BufMut`（如果 `put_u32`/`put_u64` 不再需要的话——`BytesMut` 需要 `BufMut`，保留）。

- [ ] **Step 3: 编译验证**

```bash
cd kissbot-channel && cargo check
cd kissbot-channel-web && cargo check
```

Expected: kissbot-channel 编译通过，kissbot-channel-web 可能有关于 WebMessenger 实现的错误（将在 Task 3 修复）。

- [ ] **Step 4: Commit**

```bash
git add kissbot-channel/src/channel_manager.rs
git commit -m "refactor: channel_manager 适配 Bytes，删除 SliceDataWriter

- AttachmentPayloadProcessor 用 data.slice() 零拷贝提取 payload
- AttachmentDownloadPayloadSender 直接用 extend_from_slice 构建帧
- 删除 SliceDataWriter

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: kissbot-channel-web — AttachmentStore + HTTP + WebMessenger 适配

**Files:**
- Modify: `kissbot-channel-web/src/attachment.rs` — 所有方法签名改 `Bytes`
- Modify: `kissbot-channel-web/src/messenger.rs` — `send_attachment_payload`, `download_attachment_header`, 删除 `OwnedChunkWriter`
- Modify: `kissbot-channel-web/src/http.rs` — `handle_download_attachment`, `handle_thumbnail`, `handle_upload_attachment`

**Interfaces:**
- Consumes: Task 1（trait 签名）, Task 2（channel_manager 适配）

- [ ] **Step 1: AttachmentStore 方法签名改 Bytes**

修改 `kissbot-channel-web/src/attachment.rs` 中的方法签名：

```rust
/// 保存附件，返回附件索引 key
pub fn save_attachment(
    &self,
    group_id: &str,
    msg_id: &str,
    filename: &str,
    data: Bytes,        // &[u8] → Bytes
    mime_type: &str,
) -> Result<AttachmentMeta> {
    // 内部实现：Bytes 可以 deref 到 &[u8]，所以 std::fs::write(file_path, &data)?; 仍然可用
    // image::load_from_memory(&data)? 也可用
    // 仅改签名，不改内部逻辑
}
```

```rust
pub fn get_attachment_data(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<Bytes> {
    // Vec<u8> → Bytes::from(vec)
    let file_path = ...;
    Ok(Bytes::from(std::fs::read(&file_path)?))
}
```

```rust
pub fn get_thumbnail(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<Bytes> {
    // ... 从 Vec<u8> 改为 Bytes::from(vec)
}
```

```rust
pub fn get_attachment_by_key(&self, key: &str) -> Result<Bytes> {
    // 调用 get_attachment_data，返回值改为 Bytes
}
```

```rust
pub fn get_thumbnail_by_key(&self, key: &str) -> Result<Bytes> {
    // 调用 get_thumbnail，返回值改为 Bytes
}
```

```rust
pub fn append_to_temp(&self, temp_path: &Path, data: &Bytes) -> Result<()> {
    // &[u8] → &Bytes，内部 file.write_all(data) 可用（Bytes deref 到 &[u8]）
}
```

- [ ] **Step 2: WebMessenger::send_attachment_payload 改 Bytes**

当前代码（约 621-644 行）：

```rust
async fn send_attachment_payload(&self, key: &str, _size: u32, pos: u64, write: Arc<dyn DataWriter<kissbot_channel::Error>>) -> std::result::Result<(), kissbot_channel::Error> {
    let pending = self.pending_uploads.get(key)...;
    let mut buf = BytesMut::new();
    write.write_to(&mut buf).map_err(...)?;
    self.attachment_store.append_to_temp(&pending.temp_path, &buf).map_err(...)?;
    // ...
}
```

改为：

```rust
async fn send_attachment_payload(&self, key: &str, _size: u32, pos: u64, data: Bytes) -> std::result::Result<(), kissbot_channel::Error> {
    let pending = self.pending_uploads.get(key)...;
    self.attachment_store.append_to_temp(&pending.temp_path, &data).map_err(...)?;
    // ...
}
```

- [ ] **Step 3: download_attachment_header 用 Bytes::slice() 切分 chunk**

当前代码在 `tokio::spawn` 中使用 `OwnedChunkWriter`：

```rust
struct OwnedChunkWriter(Vec<u8>);
impl DataWriter<kissbot_channel::Error> for OwnedChunkWriter { ... }
// ...
let chunk = data[pos as usize..end as usize].to_vec();
let writer: Arc<dyn DataWriter<...>> = Arc::new(OwnedChunkWriter(chunk));
ok = sender.send_attachment_payload(&key_for_sender, len as u32, pos, writer).await.is_ok();
```

改为：

```rust
// 删除 OwnedChunkWriter 整个定义
// ...
let chunk = data.slice(pos as usize..end as usize);
ok = sender.send_attachment_payload(&key_for_sender, len as u32, pos, chunk).await.is_ok();
```

对于结束标记：
```rust
let _ = sender.send_attachment_payload(&key_for_sender, 0, pos, Bytes::new()).await;
```

用 `Bytes::new()` 替代 `OwnedChunkWriter(Vec::new())`。

- [ ] **Step 4: 更新 `use bytes::` imports**

`messenger.rs` 中不再需要 `use bytes::BytesMut;`（如果 DataWriter 相关 `BytesMut` 是唯一使用的地方）。`http.rs` 中不再需要 `use serde_json::Value;` 等相关 import（如果不再使用）。

检查并清理各文件的 unused import 警告。

- [ ] **Step 5: 修改 http.rs — HTTP handler 适配 Bytes**

`handle_download_attachment`：

```rust
async fn handle_download_attachment(...) -> Response {
    // ...
    match messenger.attachment_store.get_attachment_by_key(key) {
        Ok(data) => {
            let mime = mime_guess::from_path(key).first_or_octet_stream();
            // data 是 Bytes，直接用作 body
            ([(axum::http::header::CONTENT_TYPE, mime.to_string())], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}
```

`handle_thumbnail` — 同上，`data` 改为 `Bytes`。

`handle_upload_attachment` — 把 `.to_vec()` 去掉，直接用 `Bytes`：

```rust
// 旧
if let Ok(data) = field.bytes().await {
    file_data = Some(data.to_vec());
}
// 新
if let Ok(data) = field.bytes().await {
    file_data = Some(data);  // Bytes
}
```

`file_data` 类型从 `Option<Vec<u8>>` 改为 `Option<Bytes>`。传递给 `append_to_temp` 时用 `&file_data`。

- [ ] **Step 6: 编译验证**

```bash
cd kissbot-channel-web && cargo check
cd kissbot-api && cargo test
```

Expected: 全部编译通过，61+ API 测试通过。

- [ ] **Step 7: Commit**

```bash
git add kissbot-channel-web/src/attachment.rs \
       kissbot-channel-web/src/messenger.rs \
       kissbot-channel-web/src/http.rs
git commit -m "refactor: AttachmentStore 和 WebMessenger 改用 Bytes 零拷贝

- AttachmentStore 所有方法签名从 Vec<u8>/&[u8] 改为 Bytes
- send_attachment_payload 直接接收 Bytes，移除 DataWriter 读取
- download_attachment_header 用 Bytes::slice() 零拷贝切分 chunk
- 删除 OwnedChunkWriter
- HTTP handler 直接用 Bytes 作为 response body

Co-Authored-By: deepseek-v4-flash"
```
