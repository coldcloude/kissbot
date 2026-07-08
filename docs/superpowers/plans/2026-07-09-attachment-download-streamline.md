# 附件下载流程优化与 Registry 合并实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 优化附件下载流程（先读 metadata 再 parse_range，合并 open_file 和 read_attachment_range），合并 AttachmentRegistry::register 与 gen_transfer_id，拆分 upload_channels 与 transfer_key_map 职责。

**Architecture:** 三个独立但有序的任务：1) AttachmentRegistry trait 变更（register 返回 Arc<AttachmentInfoResponse>，删除 gen_transfer_id）+ AttachmentStore 对应实现；2) AttachmentStore 新增 read_attachment_range，删除 open_file，upload_channels 抽象为 struct，write_chunk 简化，send_download_payload 末尾清理；3) http handler handle_download_attachment 流程重构。

**Tech Stack:** Rust, async_trait, flume, DashMap, LRU cache

## Global Constraints

- `AttachmentRegistry::register` 返回类型改为 `Result<Arc<AttachmentInfoResponse>>`
- 删除 `AttachmentRegistry::gen_transfer_id` 方法
- AttachmentStore 新增 `read_attachment_range(key, start, length) -> Result<Bytes>` 方法
- AttachmentStore 删除 `open_file` 方法
- `upload_channels` value 从 `(Arc<String>, u64, flume::Sender<UploadCommand>)` 改为 `UploadChannel` struct
- 删除 `get_or_create_upload_channel` 方法：upload_channels 中没有 transfer_id 直接报错
- `write_chunk` 直接用 transfer_id 查 `upload_channels`
- `send_download_payload` 末尾发完 size=0 后清理 `transfer_key_map`
- http.rs 中独立函数 `read_attachment_range` 删除，用 `AttachmentStore::read_attachment_range` 替代
- 所有代码中的注释不要删除

---

### Task 1: AttachmentRegistry trait 变更 + AttachmentStore register 实现

**Files:**
- Modify: `kissbot-channel/src/attachment.rs` — trait 声明 + process_attachment_message
- Modify: `kissbot-channel-web/src/attachment.rs` — register 实现 + UploadChannel struct
- Modify: `kissbot-channel-web/src/messenger.rs` — download_attachment_header 适配（不变，已使用 next_transfer_id_for）

**Interfaces:**
- Consumes: `AttachmentInfoResponse` 已有 transfer_id 字段
- Produces: `AttachmentRegistry::register` 返回 `Arc<AttachmentInfoResponse>`；`UploadChannel` struct

**Step 1: 修改 AttachmentRegistry trait**

`kissbot-channel/src/attachment.rs`：

将注册器 trait 中 `register` 返回类型从 `Result<Arc<String>>` 改为 `Result<Arc<AttachmentInfoResponse>>`，删除 `gen_transfer_id` 方法：

```rust
#[async_trait]
pub trait AttachmentRegistry: Send + Sync {
    /// 注册附件，返回包含 key、info、transfer_id 的响应。
    /// transfer_id 用于上传时的 write_chunk 路由。
    async fn register(
        &self,
        messenger_id: &str,
        user_id: &str,
        group_id: &str,
        info: Arc<AttachmentInfo>,
    ) -> std::result::Result<Arc<AttachmentInfoResponse>, Error>;
}
```

**Step 2: 修改 process_attachment_message**

`kissbot-channel/src/attachment.rs`：

```rust
async fn process_content(
    content: &Content,
    messenger_id: &str,
    user_id: &str,
    group_id: &str,
    registry: &dyn AttachmentRegistry,
) -> Result<Content> {
    match content {
        Content::AttachmentInfo(info) => {
            let resp = registry.register(messenger_id, user_id, group_id, info.clone()).await?;
            Ok(Content::AttachmentInfoResponse(resp))
        }
        Content::Multi(items) => {
            let mut new_items = Vec::with_capacity(items.len());
            for item in items.iter() {
                let new_content = Box::pin(process_content(
                    &item.content,
                    messenger_id,
                    user_id,
                    group_id,
                    registry,
                )).await?;
                new_items.push(Arc::new(MessageItem {
                    msg_type: item.msg_type.clone(),
                    content: new_content,
                }));
            }
            Ok(Content::Multi(new_items))
        }
        _ => Ok(content.clone()),
    }
}
```

注意 `use` 中 `AttachmentInfo` 仍需保留（match 模式使用），`AttachmentInfoResponse` 已在 use 中。

**Step 3: 在 AttachmentStore 中定义 UploadChannel struct**

`kissbot-channel-web/src/attachment.rs`：

在 `UploadCommand` struct 之后添加：

```rust
/// 上传通道
struct UploadChannel {
    key: Arc<String>,
    current_pos: u64,
    sender: flume::Sender<UploadCommand>,
}
```

**Step 4: 修改 AttachmentStore 的 upload_channels 字段类型**

将：

```rust
upload_channels: DashMap<u32, (Arc<String>, u64, flume::Sender<UploadCommand>)>,
```

改为：

```rust
upload_channels: DashMap<u32, UploadChannel>,
```

**Step 5: 修改 register 实现**

`kissbot-channel-web/src/attachment.rs`：

将 `register` 方法改为直接返回 `Arc<AttachmentInfoResponse>`，合并 gen_transfer_id 逻辑：

```rust
#[async_trait]
impl kissbot_channel::AttachmentRegistry for AttachmentStore {
    async fn register(&self, _messenger_id: &str, _user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> std::result::Result<Arc<AttachmentInfoResponse>, kissbot_channel::Error> {
        let uuid = uuid::Uuid::new_v4().to_string();
        let key = Arc::new(format!("{}/{}", group_id, uuid));
        let transfer_id = self.next_transfer_id();

        // 创建 group 目录
        let dir = self.base_path.join(group_id);
        std::fs::create_dir_all(&dir)?;

        // 创建空临时文件
        let temp_path = dir.join(format!(".{}.uploading", uuid));
        std::fs::write(&temp_path, &[])?;

        // 写 metadata 文件
        let meta = AttachmentMeta {
            key: key.clone(),
            info: info.clone(),
            has_thumbnail: false,
        };
        let meta_path = dir.join(format!("{}.metadata", uuid));
        let meta_json = serde_json::to_string(&meta)?;
        std::fs::write(&meta_path, &meta_json)?;

        // 插入 LRU 缓存
        {
            let mut cache = self.meta_cache.lock().unwrap();
            cache.put(uuid, Arc::new(meta));
        }

        // 创建上传队列并注册到 upload_channels（不上 transfer_key_map）
        let (tx, rx) = flume::unbounded::<UploadCommand>();
        let channels = self.upload_channels.clone();
        let base_path = self.base_path.clone();
        let key_ch = key.clone();
        let id = transfer_id;
        tokio::spawn(async move {
            let mut current_pos = 0u64;
            while let Ok(cmd) = rx.recv_async().await {
                let result = Self::process_upload_write_inner(
                    &base_path, key_ch.as_str(), &mut current_pos, cmd.header.pos, &cmd.data,
                );
                if result.is_ok() {
                    channels.remove(&id);
                }
                let _ = cmd.res.send(result);
            }
        });

        self.upload_channels.insert(transfer_id, UploadChannel {
            key: key.clone(),
            current_pos: 0,
            sender: tx,
        });

        Ok(Arc::new(AttachmentInfoResponse {
            key,
            info,
            transfer_id,
        }))
    }
}
```

删除 `gen_transfer_id` 方法（原来的 `gen_transfer_id` 块整个删除）。

**Step 6: 调整 use 导入**

如果 `kissbot-channel-web/src/attachment.rs` 中原来有 `use kissbot_api::message::AttachmentInfoResponse;`，确认已存在；如没有则添加。

**Step 7: 编译检查**

```bash
cd /home/admin/project/kissbot && cargo check 2>&1
```

**Step 8: 提交**

```bash
git add kissbot-channel/src/attachment.rs kissbot-channel-web/src/attachment.rs
git commit -m "refactor: AttachmentRegistry::register 返回 Arc<AttachmentInfoResponse>，合并 gen_transfer_id；upload_channels 用 UploadChannel struct"
```

---

### Task 2: AttachmentStore 新增 read_attachment_range，简化 upload/write_chunk，send_download_payload 末尾清理

**Files:**
- Modify: `kissbot-channel-web/src/attachment.rs` — 新增 read_attachment_range，删除 open_file，简化 write_chunk 和 get_or_create_upload_channel，send_download_payload 末尾清理

**Interfaces:**
- Consumes: Task 1 的 UploadChannel struct，register 简化
- Produces: `AttachmentStore::read_attachment_range(key, start, length) -> Result<Bytes>`

**Step 1: 新增 read_attachment_range 方法**

在 `send_download_payload` 方法之后（或 open_file 原来位置附近）添加：

```rust
/// 根据 key 和范围读取附件数据
/// 内部 parse_key → open file → seek → read_exact
pub fn read_attachment_range(&self, key: &str, start: u64, length: u64) -> Result<Bytes> {
    let (group_id, uuid) = Self::parse_key(key)?;
    let file_path = self.base_path.join(group_id).join(uuid);
    let mut file = std::fs::File::open(&file_path)?;
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; length as usize];
    file.read_exact(&mut buf)?;
    Ok(Bytes::from(buf))
}
```

需要添加 use 导入（如果在文件顶部还没有）：
```rust
use std::io::{Read, Seek, SeekFrom};
```
检查文件顶部已有的 `use std::io::Write;`，补充缺失的导入。

**Step 2: 删除 open_file 方法**

删除整个 `open_file` 方法（从 `/// 打开附件文件（下载用）` 到方法结束的 `}`）。

**Step 3: 简化 write_chunk 和删除 get_or_create_upload_channel**

将 `write_chunk` 改为直接查 `upload_channels`：

```rust
/// 异步写入 chunk（通过 flume 队列串行处理）
pub async fn write_chunk(&self, transfer_id: u32, pos: u64, size: u32, data: Bytes) -> Result<u64> {
    let sender = self.upload_channels.get(&transfer_id)
        .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?
        .value().sender.clone();
    let (res_tx, res_rx) = oneshot::channel();

    sender.send(UploadCommand {
        header: AttachmentPayloadHeader {
            id: transfer_id,
            size,
            pos,
        },
        data,
        res: res_tx,
    })?;

    res_rx.await?
}
```

删除整个 `get_or_create_upload_channel` 方法（从 `fn get_or_create_upload_channel` 到 `tx` 返回的整个块）。

**Step 4: send_download_payload 末尾添加清理**

找到 `send_download_payload` 方法末尾，在发完 size=0 结束标记之后添加：

```rust
// 下载完成，清理 transfer_key_map
self.transfer_key_map.remove(&transfer_id);
```

注意：需要将 transfer_id 从参数传入的 `u32` 值直接使用（send_download_payload 的签名已是 `transfer_id: u32`），不需要额外处理。

修改后的 `send_download_payload` 完整方法：

```rust
/// 发送下载 payload（内部按 CHUNK_SIZE 分块读取并调用 sender）
pub async fn send_download_payload(&self, transfer_id: u32, sender: &dyn kissbot_channel::AttachmentDownloadPayloadSender) -> Result<()> {
    use kissbot_api::channel::OFFSET_ATT_DATA;
    const CHUNK_SIZE: u64 = 65536;

    let key = self.transfer_key_map.get(&transfer_id)
        .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?
        .clone();
    let file_len = {
        let (group_id, uuid) = Self::parse_key(key.as_str())?;
        let file_path = self.base_path.join(group_id).join(uuid);
        std::fs::metadata(&file_path)?.len()
    };

    let mut pos = 0u64;
    let mut ok = true;
    while pos < file_len && ok {
        let end = std::cmp::min(pos + CHUNK_SIZE, file_len);
        let chunk_size = (end - pos) as usize;
        let (sn, mut buf) = sender.prepare_send(transfer_id, chunk_size as u32, pos)?;
        use std::io::Read;
        if let Err(e) = (&mut file).read_exact(&mut buf[OFFSET_ATT_DATA..OFFSET_ATT_DATA + chunk_size]) {
            return Err(Error::InternalError(format!("Failed to read file chunk: {}", e)));
        }
        ok = sender.send(sn, transfer_id, chunk_size as u32, pos, buf).await.is_ok();
        pos = end;
    }
    // 发送 size=0 的结束标记
    if let Ok((sn, buf)) = sender.prepare_send(transfer_id, 0, pos) {
        let _ = sender.send(sn, transfer_id, 0, pos, buf).await;
    }

    // 下载完成，清理 transfer_key_map
    self.transfer_key_map.remove(&transfer_id);

    Ok(())
}
```

> 注意：这里做了修改——原来 `send_download_payload` 调用了 `self.open_file(key.as_str())?` 来获取 `(file, file_len)`。由于 `open_file` 被删除，改为通过 `std::fs::metadata` 获取 `file_len`，并通过 `std::fs::File::open` 直接打开文件。

**Step 5: 编译检查**

```bash
cd /home/admin/project/kissbot && cargo check 2>&1
```

**Step 6: 提交**

```bash
git add kissbot-channel-web/src/attachment.rs
git commit -m "refactor: AttachmentStore 新增 read_attachment_range；删除 open_file；write_chunk 简化；send_download_payload 末尾清理 transfer_key_map"
```

---

### Task 3: http handler handle_download_attachment 流程重构

**Files:**
- Modify: `kissbot-channel-web/src/http.rs`

**Interfaces:**
- Consumes: Task 2 的 `AttachmentStore::read_attachment_range(key, start, length) -> Result<Bytes>`
- Consumes: `AttachmentStore::get_meta(key) -> Result<Arc<AttachmentMeta>>`（已有）
- Produces: 无（重构内部逻辑）

**Step 1: 重构 handle_download_attachment**

`kissbot-channel-web/src/http.rs`，将整个 `handle_download_attachment` 函数替换为：

```rust
/// GET /api/attachment/download — 支持 Range 断点续传
async fn handle_download_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let key = match params.get("key") {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, "Missing key").into_response(),
    };

    // 1. 读取 metadata 获取文件大小和类型
    let meta = match messenger.attachment_store.get_meta(key) {
        Ok(m) => m,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let file_len = meta.info.size_bytes;
    let mime = meta.info.mime_type.as_str().to_string();

    // 2. 解析 Range header
    let range_header = headers.get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok());

    if let Some(range_str) = range_header {
        // 解析 "bytes=start-end" 格式
        if let Some((start, end)) = parse_range(range_str, file_len) {
            let length = end - start + 1;
            match messenger.attachment_store.read_attachment_range(key, start, length) {
                Ok(data) => {
                    let content_range = format!("bytes {}-{}/{}", start, end, file_len);
                    return (
                        StatusCode::PARTIAL_CONTENT,
                        [
                            (axum::http::header::CONTENT_RANGE, content_range),
                            (axum::http::header::CONTENT_TYPE, mime),
                        ],
                        data,
                    ).into_response();
                }
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
    }

    // 3. 无 Range 或 Range 解析失败，返回全量文件
    match messenger.attachment_store.read_attachment_range(key, 0, file_len) {
        Ok(data) => {
            ([(axum::http::header::CONTENT_TYPE, mime)], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}
```

**Step 2: 删除独立的 read_attachment_range 函数**

删除 `http.rs` 中独立的 `read_attachment_range` 函数（以 `/// 读取指定范围的文件数据` 注释开头到函数结束 `}` 的整个块）。

`parse_range` 函数保留不变。

**Step 3: 清理 use 导入（可选）**

检查 `http.rs` 顶部 use 导入中如果不再需要的项（如 `use std::io::{Read, Seek, SeekFrom}` 如果在 read_attachment_range 函数被删除后不再被其他代码使用），可以移除。

**Step 4: 编译检查**

```bash
cd /home/admin/project/kissbot && cargo check 2>&1
```

**Step 5: 提交**

```bash
git add kissbot-channel-web/src/http.rs
git commit -m "refactor: handle_download_attachment 先读 metadata 再 parse_range，使用 store.read_attachment_range"
```
