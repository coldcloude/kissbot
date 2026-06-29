# 统一附件上传下载引擎实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** WebMessenger 内部建立统一的上传/下载引擎，上传使用 flume 队列串行 + oneshot 同步，下载支持流式和 Range 断点续传。

**Architecture:** 分 3 个任务：(1) 上传引擎（UploadCommand + flume 队列 + write_attachment_chunk）, (2) Messenger trait 和 HTTP handler 适配统一上传接口, (3) 下载引擎 + HTTP Range 支持。

**Tech Stack:** Rust, flume, tokio, bytes, axum, kissbot-channel-web

## Global Constraints

- UploadCommand 参数顺序: key: String, pos: u64, size: u32, data: Bytes, res: oneshot::Sender
- write_attachment_chunk 签名: (key: &str, pos: u64, size: u32, data: Bytes) -> Result<u64>
- oneshot channel 用于同步等待结果
- 后台任务从 pending_uploads 读取 temp_path / target_path / size_bytes
- 不要删除代码中的注释

---

### Task 1: 上传引擎核心实现

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs` — 新增 UploadCommand、upload_channels 字段、write_attachment_chunk、get_or_create_upload_channel、后台任务
- Modify: `kissbot-channel-web/src/error.rs` — 确保 Error 类型支持字符串错误

**Interfaces:**
- Consumes: 现有的 `PendingAttachment`, `AttachmentStore` (append_to_temp, finalize_upload)
- Produces: `WebMessenger::write_attachment_chunk()`, `WebMessenger::upload_channels` 字段

- [ ] **Step 1: 添加 UploadCommand 枚举**

在 `kissbot-channel-web/src/messenger.rs` 中 `PendingAttachment` 之后添加：

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Weak};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use dashmap::{DashMap, DashSet};
use tokio::sync::oneshot;

use kissbot_api::channel::{...};
use kissbot_api::DataWriter;  // 不再需要
// ...

/// 上传队列命令
enum UploadCommand {
    Write {
        key: String,
        pos: u64,
        size: u32,
        data: Bytes,
        res: oneshot::Sender<std::result::Result<u64, String>>,
    },
}
```

- [ ] **Step 2: WebMessenger 新增 upload_channels 字段**

在 `WebMessenger` struct 中新增：

```rust
pub struct WebMessenger {
    // ... 现有字段
    pub pending_uploads: DashMap<String, PendingAttachment>,
    // 新增：
    pub upload_channels: Arc<DashMap<String, flume::Sender<UploadCommand>>>,
}
```

在 `WebMessenger::new()` 中初始化：

```rust
upload_channels: Arc::new(DashMap::new()),
```

- [ ] **Step 3: 实现 get_or_create_upload_channel**

```rust
impl WebMessenger {
    fn get_or_create_upload_channel(
        self: &Arc<Self>,
        key: &str,
    ) -> flume::Sender<UploadCommand> {
        if let Some(entry) = self.upload_channels.get(key) {
            return entry.value().clone();
        }

        let (tx, rx) = flume::unbounded::<UploadCommand>();
        let store = self.attachment_store.clone();
        let key_owned = key.to_string();
        let channels = self.upload_channels.clone();
        let pending_uploads = self.pending_uploads.clone();
        let this = self.clone();

        tokio::spawn(async move {
            let mut current_pos = 0u64;
            while let Ok(cmd) = rx.recv_async().await {
                match cmd {
                    UploadCommand::Write { key, pos, size, data, res } => {
                        let result = Self::process_upload_write(
                            &store, &pending_uploads, &key, &mut current_pos, pos, data
                        );

                        // 如果是最后一块，清理 channel
                        if let Ok(p) = &result {
                            if *p >= size as u64 {
                                channels.remove(&key);
                                pending_uploads.remove(&key);
                            }
                        }
                        let _ = res.send(result);
                    }
                }
            }
        });

        self.upload_channels.insert(key_owned, tx.clone());
        tx
    }
}
```

- [ ] **Step 4: 实现 process_upload_write 和 write_attachment_chunk**

```rust
impl WebMessenger {
    /// 写入附件数据。通过 flume 队列串行处理，避免竞争。
    /// 返回当前已写入位置。
    pub fn write_attachment_chunk(
        &self,
        key: &str,
        pos: u64,
        size: u32,
        data: Bytes,
    ) -> Result<u64> {
        let tx = self.get_or_create_upload_channel(key);
        let (res_tx, res_rx) = oneshot::channel();

        tx.send(UploadCommand::Write {
            key: key.to_string(),
            pos,
            size,
            data,
            res: res_tx,
        }).map_err(|_| Error::InternalError("upload channel closed".to_string()))?;

        // 等待后台任务处理完成
        res_rx.recv().map_err(|_| Error::InternalError("upload channel recv error".to_string()))?
            .map_err(|e| Error::InternalError(e))
    }

    fn process_upload_write(
        store: &AttachmentStore,
        pending_uploads: &DashMap<String, PendingAttachment>,
        key: &str,
        current_pos: &mut u64,
        pos: u64,
        data: Bytes,
    ) -> std::result::Result<u64, String> {
        let pending = pending_uploads.get(key)
            .ok_or_else(|| format!("key {} not found", key))?;

        if pos < *current_pos {
            return Ok(*current_pos);  // 已写入，幂等
        }
        if pos > *current_pos {
            return Err(format!("out of order: expected pos={}, got pos={}", *current_pos, pos));
        }

        store.append_to_temp(&pending.temp_path, &data)
            .map_err(|e| e.to_string())?;
        *current_pos = pos + data.len() as u64;

        if *current_pos >= pending.size_bytes {
            let (temp, target) = (pending.temp_path.clone(), pending.target_path.clone());
            drop(pending);
            AttachmentStore::finalize_upload(&temp, &target)
                .map_err(|e| e.to_string())?;
        }

        Ok(*current_pos)
    }
}
```

注意：`self: &Arc<Self>` 用于 `get_or_create_upload_channel` 因为要 clone 后传入 tokio::spawn。但 `write_attachment_chunk` 是 `&self`，调用方通过 `Arc<WebMessenger>` 调用时可以 `self.clone()` 或通过 `self: &Arc<Self>`。

更简单的方式——`get_or_create_upload_channel` 改成接受 `Arc<WebMessenger>`：

```rust
fn get_or_create_upload_channel(this: &Arc<Self>, key: &str) -> flume::Sender<UploadCommand> {
```

但 Rust 不允许在 `impl WebMessenger` 块中有非方法的关联函数同名。可以用 `self: &Arc<Self>` 代替 `&self`。

- [ ] **Step 5: 编译验证**

```bash
cd kissbot-channel-web && cargo check
```

Expected: 编译通过（可能有关于未使用 import 的警告，后续清理）。

- [ ] **Step 6: Commit**

```bash
git add kissbot-channel-web/src/messenger.rs
git commit -m "feat: 上传引擎核心 — flume 队列 + oneshot 串行写入

- 新增 UploadCommand 枚举
- WebMessenger 新增 upload_channels 字段
- get_or_create_upload_channel 启动后台任务串行处理
- write_attachment_chunk 公共写入入口

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: Messenger trait + HTTP handler 适配统一上传

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs` — `send_attachment_payload` 改为调用 `write_attachment_chunk`
- Modify: `kissbot-channel-web/src/http.rs` — `handle_upload_attachment` 改为调用 `write_attachment_chunk`

**Interfaces:**
- Consumes: Task 1（`write_attachment_chunk`）

- [ ] **Step 1: send_attachment_payload 适配**

替换现有实现（messenger.rs 约 620-638 行）：

```rust
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> std::result::Result<(), kissbot_channel::Error> {
    self.write_attachment_chunk(key, pos, size, data)
        .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;
    Ok(())
}
```

注意：原有的 pending_uploads 查找、append_to_temp、finalize_upload 逻辑已移到后台任务中，这里不再需要。

- [ ] **Step 2: handle_upload_attachment 适配**

在 `http.rs` 中，`handle_upload_attachment` 当前直接操作 pending_uploads、append_to_temp、finalize_upload。改为调用 `write_attachment_chunk`：

找到以下代码块（约 444-462 行）：

```rust
    // 查找 PendingAttachment
    let pending = match messenger.pending_uploads.get(&attachment_key) {
        Some(p) => p,
        None => return Json(ApiResponse::<serde_json::Value>::error(format!("key {} not found", attachment_key))),
    };

    // 写入数据到临时文件
    if let Err(e) = messenger.attachment_store.append_to_temp(&pending.temp_path, &file_data) {
        return Json(ApiResponse::<serde_json::Value>::error(e.to_string()));
    }

    // 重命名
    let (temp, target) = (pending.temp_path.clone(), pending.target_path.clone());
    drop(pending);
    if let Err(e) = crate::attachment::AttachmentStore::finalize_upload(&temp, &target) {
        return Json(ApiResponse::<serde_json::Value>::error(e.to_string()));
    }
    messenger.pending_uploads.remove(&attachment_key);
```

替换为：

```rust
    // 获取 size_bytes
    let size_bytes = match messenger.pending_uploads.get(&attachment_key) {
        Some(p) => p.size_bytes,
        None => return Json(ApiResponse::<serde_json::Value>::error(format!("key {} not found", attachment_key))),
    };

    // 通过上传引擎写入（串行处理，自动 finalize）
    if let Err(e) = messenger.write_attachment_chunk(&attachment_key, 0, size_bytes as u32, file_data) {
        return Json(ApiResponse::<serde_json::Value>::error(e.to_string()));
    }
```

注意：`file_data` 是 `Bytes`，`write_attachment_chunk` 接收 `Bytes`。

- [ ] **Step 3: 编译验证**

```bash
cd kissbot-channel-web && cargo check
cd kissbot-api && cargo test
```

Expected: 编译通过，API 测试通过。

- [ ] **Step 4: Commit**

```bash
git add kissbot-channel-web/src/messenger.rs kissbot-channel-web/src/http.rs
git commit -m "refactor: Messenger trait 和 HTTP handler 统一调用 write_attachment_chunk

- send_attachment_payload 改为委托 write_attachment_chunk
- handle_upload_attachment 改为委托 write_attachment_chunk
- 移除重复的 pending_uploads/append_to_temp/finalize_upload 代码

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: 下载引擎 + HTTP Range 支持

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs` — 新增 `open_download_reader`、`read_attachment_range`
- Modify: `kissbot-channel-web/src/attachment.rs` — `open_file` 返回 `(File, u64)`（已存在）
- Modify: `kissbot-channel-web/src/http.rs` — `handle_download_attachment` 支持 Range

**Interfaces:**
- Consumes: `AttachmentStore::open_file`, `AttachmentStore::get_meta_by_key`

- [ ] **Step 1: WebMessenger 新增 open_download_reader**

```rust
/// 打开文件读取器，返回 (File, 文件长度, MIME type)
pub fn open_download_reader(&self, key: &str) -> Result<(std::fs::File, u64, String)> {
    let meta = self.attachment_store.get_meta_by_key(key)?;
    let (file, len) = self.attachment_store.open_file(key)?;
    let mime = mime_guess::from_path(meta.file_name.as_str()).first_or_octet_stream().to_string();
    Ok((file, len, mime))
}
```

- [ ] **Step 2: WebMessenger 新增 read_attachment_range**

```rust
/// 读取指定范围数据，用于 HTTP Range 断点续传
pub fn read_attachment_range(&self, key: &str, offset: u64, length: u64) -> Result<Bytes> {
    let (mut file, _) = self.attachment_store.open_file(key)?;
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; length as usize];
    file.read_exact(&mut buf)?;
    Ok(Bytes::from(buf))
}
```

- [ ] **Step 3: 修改 handle_download_attachment 支持 Range**

```rust
/// GET /api/attachment/download — 支持 Range 断点续传
async fn handle_download_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let key = match params.get("key") {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, "Missing key").into_response(),
    };

    // 获取文件信息和 MIME 类型
    let (_, file_len, mime) = match messenger.open_download_reader(key) {
        Ok(info) => info,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };

    // 解析 Range header
    let range_header = headers.get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok());

    if let Some(range_str) = range_header {
        // 解析 "bytes=start-end" 格式
        if let Some((start, end)) = parse_range(range_str, file_len) {
            let length = end - start + 1;
            match messenger.read_attachment_range(key, start, length) {
                Ok(data) => {
                    let content_range = format!("bytes {}-{}/{}", start, end, file_len);
                    return (
                        StatusCode::PARTIAL_CONTENT,
                        [
                            (axum::http::header::CONTENT_RANGE, content_range.as_str()),
                            (axum::http::header::CONTENT_TYPE, mime.as_str()),
                        ],
                        data,
                    ).into_response();
                }
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
    }

    // 无 Range 或 Range 解析失败，返回全量文件
    match messenger.read_attachment_range(key, 0, file_len) {
        Ok(data) => {
            ([(axum::http::header::CONTENT_TYPE, mime.as_str())], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// 解析 "bytes=start-end" 格式的 Range header
fn parse_range(range_str: &str, file_len: u64) -> Option<(u64, u64)> {
    let range_str = range_str.strip_prefix("bytes=")?;
    let (start_str, end_str) = range_str.split_once('-')?;
    let start: u64 = start_str.parse().ok()?;
    let end: u64 = if end_str.is_empty() {
        file_len - 1
    } else {
        end_str.parse().ok()?
    };
    if start >= file_len || end >= file_len || start > end {
        return None;
    }
    Some((start, end))
}
```

注意：需要在 http.rs 中添加必要的 import：
```rust
use axum::http::StatusCode;
```

- [ ] **Step 4: 编译验证**

```bash
cd kissbot-channel-web && cargo check
cargo test -p kissbot-api
```

Expected: 编译通过，API 测试通过。

- [ ] **Step 5: Commit**

```bash
git add kissbot-channel-web/src/messenger.rs kissbot-channel-web/src/http.rs
git commit -m "feat: 下载引擎 + HTTP Range 断点续传

- WebMessenger 新增 open_download_reader 和 read_attachment_range
- handle_download_attachment 支持 Range header → 206 Partial Content
- 无 Range 时返回全量文件

Co-Authored-By: deepseek-v4-flash"
```
