# 移除 DataWriter，改用 Bytes 零拷贝

## 概述

配合 kai-ws 库的 `WsBinaryProcessor::process_bin` 从 `&[u8]` 改为 `Bytes`，移除 `DataWriter` trait，所有 `send_attachment_payload` 方法直接使用 `Bytes` 参数，消除不必要的拷贝和中间层。

## 变更清单

### 0. kai-ws（已完成）

`WsBinaryProcessor::process_bin` 已从 `&[u8]` 改为 `Bytes`。

### 1. 删除 DataWriter trait

**文件：** `kissbot-api/src/common.rs`

删除 `DataWriter<E>` trait 定义和其 `Send + Sync` bound。

**文件：** `kissbot-api/src/lib.rs`

删除 `pub use common::DataWriter;`（如果存在单独 re-export）。

### 2. 修改 Messenger trait

**文件：** `kissbot-channel/src/messenger.rs`

```rust
// 旧
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, write: Arc<dyn DataWriter<Error>>) -> Result<()>;
// 新
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> Result<()>;
```

**文件：** `kissbot-channel/src/data.rs`

```rust
// 旧
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, writer: Arc<dyn DataWriter<Error>>) -> Result<()>;
// 新
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> Result<()>;
```

### 3. 修改 channel_manager.rs

**`AttachmentPayloadProcessor::raw_process_bin`** — 收到 WS 二进制帧后，`data` 已经是 `Bytes`，用 `Bytes::slice()` 零拷贝提取 payload，直接传 messenger：

```rust
let payload = data.slice(OFFSET_ATT_DATA..);
messenger.send_attachment_payload(&key, header.size, header.pos, payload).await?;
```

删除 `SliceDataWriter` 结构体和相关代码。

**`ChannelManager::send_attachment_payload`（AttachmentDownloadPayloadSender impl）** — 直接从 `Bytes` 构建帧，不再需要 `DataWriter` 读取步骤：

```rust
let mut frame = BytesMut::with_capacity(OFFSET_ATT_DATA + data.len());
frame.put_u32(0); frame.put_u32(TYPE_ATTACHMENT_PAYLOAD); frame.put_u32(CODE_SUCCESS);
frame.put_u32(internal_id); frame.put_u32(size); frame.put_u64(pos);
frame.extend_from_slice(&data);
```

### 4. 修改 WebMessenger

**`send_attachment_payload`** — 参数从 `Arc<dyn DataWriter>` 改为 `Bytes`，直接写入：

```rust
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> Result<()> {
    let pending = self.pending_uploads.get(key)...;
    self.attachment_store.append_to_temp(&pending.temp_path, &data)?;
    ...
}
```

**`download_attachment_header`** — 后台任务中用 `Bytes::slice()` 零拷贝切分 chunk：

```rust
let chunk = data.slice(pos as usize..end as usize);
sender.send_attachment_payload(&key_for_sender, len as u32, pos, chunk).await?;
```

### 5. AttachmentStore 改为 Bytes

**文件：** `kissbot-channel-web/src/attachment.rs`

```rust
// 旧
fn save_attachment(&self, group_id, msg_id, filename, data: &[u8], mime_type) -> Result<AttachmentMeta>
fn get_attachment_data(&self, group_id, msg_id, filename) -> Result<Vec<u8>>
fn get_thumbnail(&self, group_id, msg_id, filename) -> Result<Vec<u8>>
fn get_attachment_by_key(&self, key) -> Result<Vec<u8>>
fn get_thumbnail_by_key(&self, key) -> Result<Vec<u8>>
fn append_to_temp(&self, temp_path, data: &[u8]) -> Result<()>

// 新
fn save_attachment(&self, group_id, msg_id, filename, data: Bytes, mime_type) -> Result<AttachmentMeta>
fn get_attachment_data(&self, group_id, msg_id, filename) -> Result<Bytes>
fn get_thumbnail(&self, group_id, msg_id, filename) -> Result<Bytes>
fn get_attachment_by_key(&self, key) -> Result<Bytes>
fn get_thumbnail_by_key(&self, key) -> Result<Bytes>
fn append_to_temp(&self, temp_path, data: &Bytes) -> Result<()>
// 内部实现不变：Bytes deref 到 &[u8]，fs::read 产生 Vec<u8> 后转为 Bytes::from(vec)
```

### 6. HTTP handler 适配

**文件：** `kissbot-channel-web/src/http.rs`

- `handle_download_attachment`: `get_attachment_by_key` 返回 `Bytes`，直接用 `(mime, data).into_response()`
- `handle_thumbnail`: `get_thumbnail_by_key` 返回 `Bytes`，同上
- `handle_upload_attachment`: `field.bytes().await` 直接返回 `Bytes`（axum Multipart），不再 `.to_vec()`

### 7. 删除的文件/代码

| 内容 | 文件 |
|------|------|
| `DataWriter` trait | `kissbot-api/src/common.rs` |
| `SliceDataWriter` | `kissbot-channel/src/channel_manager.rs` |
| `OwnedChunkWriter` | `kissbot-channel-web/src/messenger.rs` |
| 不再需要的 `use bytes::BufMut` | 多处（仅用于 DataWriter 的 import） |

### 8. 影响分析

- **上传方向**：`AttachmentPayloadProcessor` 收到 `Bytes` → `slice()` 零拷贝提取 → messenger 直接写入磁盘。减少 1 次 `to_vec()` 拷贝。
- **下载方向**：`download_attachment_header` 用 `Bytes::slice()` 切分 chunk → `sender.send_attachment_payload` → `ChannelManager` 直接构建帧 → `send_bin`。减少 2 次拷贝（`to_vec()` + DataWriter 的 `write_to`）。
- **不再需要处理 `!Send` 的 DataWriter 生命周期问题**（之前需要在 await 前 drop writer）。
