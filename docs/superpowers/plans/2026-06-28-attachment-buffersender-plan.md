# BufferSender 下载分离实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 下载方向将 `AttachmentDownloadPayloadSender` 从 `send_attachment_payload` 改为 `prepare_sender` 返回 `Box<dyn BufferSender>`，WebMessenger 流式读取文件写入发送 buffer 避免拷贝。

**Architecture:** 分 2 个任务：(1) channel-manager 实现 `prepare_sender`, (2) channel-web AttachmentStore 新增 `open_file` + WebMessenger 流式下载。

**Tech Stack:** Rust, bytes, kissbot-channel, kissbot-channel-web

## Global Constraints

- `BufferSender::get_buffer()` 返回 `&mut BytesMut`（不是 clone）
- `prepare_sender()` 返回 `Result<Box<dyn BufferSender>>`
- 上传方向 `Messenger::send_attachment_payload` 不变
- 不要删除代码中的注释

---

### Task 1: Tait 修正 + channel-manager 实现 prepare_sender

**Files:**
- Modify: `kissbot-channel/src/data.rs` — `BufferSender::get_buffer` 改为 `&mut self` + `&mut BytesMut`
- Modify: `kissbot-channel/src/channel_manager.rs` — 实现 `prepare_sender` + `ChannelBufferSender`，删除旧的 `send_attachment_payload` impl；移除 `sender_id_to_key`（ChannelManager 字段）

**Interfaces:**
- Consumes: 现有 `BufferSender` 和 `AttachmentDownloadPayloadSender` trait
- Produces: `ChannelBufferSender` + `prepare_sender` 实现

**注意：** `ChannelManager` 中的 `sender_id_to_key` 不再需要（`prepare_sender` 直接返回 BufferSender），可以移除。`AttachmentDownloadRequestProcessor` 中建立 `attachment_sender_map` 的逻辑不变（仍用 key → (id, connect)）。

- [ ] **Step 1: 修正 BufferSender trait 签名**

`data.rs` 中：

```rust
// 旧
fn get_buffer(&self) -> BytesMut;
// 新
fn get_buffer(&mut self) -> &mut BytesMut;
```

- [ ] **Step 2: 实现 ChannelBufferSender + prepare_sender**

在 `channel_manager.rs` 中，删除旧的 `AttachmentDownloadPayloadSender` impl（`send_attachment_payload`），替换为：

```rust
struct ChannelBufferSender {
    internal_id: u32,
    connect_context: Arc<ConnectContext>,
    size: u32,
    pos: u64,
    buf: BytesMut,
}

impl BufferSender for ChannelBufferSender {
    fn get_buffer(&mut self) -> &mut BytesMut {
        &mut self.buf
    }

    async fn send(&self) -> Result<()> {
        let mut frame = BytesMut::with_capacity(OFFSET_ATT_DATA + self.buf.len());
        frame.put_u32(0);
        frame.put_u32(TYPE_ATTACHMENT_PAYLOAD);
        frame.put_u32(CODE_SUCCESS);
        frame.put_u32(self.internal_id);
        frame.put_u32(self.size);
        frame.put_u64(self.pos);
        frame.extend_from_slice(&self.buf);

        if self.size == 0 {
            // 最后传个 size=0 的表示结尾，清理在 prepare_sender 中由调用方负责
            // 实际上清理需要 mutable 访问 ChannelManager，这里无法直接操作
            // 由 AttachmentDownloadRequestProcessor 在发送端清理
        }

        self.connect_context.ws_context.send_bin(frame.freeze()).await?;
        Ok(())
    }
}

#[async_trait]
impl AttachmentDownloadPayloadSender for ChannelManager {
    fn prepare_sender(&self, key: &str, size: u32, pos: u64) -> Result<Box<dyn BufferSender>> {
        let sender_entry = self.attachment_sender_map.get(key)
            .ok_or_else(|| Error::AttachmentNotFound(key.to_string()))?;
        let (internal_id, ref connect_weak) = *sender_entry;
        let connect_context = connect_weak.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

        if size == 0 {
            // 结束标记：清理
            self.attachment_sender_map.remove(key);
            self.sender_id_to_key.remove(&internal_id);
        }

        Ok(Box::new(ChannelBufferSender {
            internal_id,
            connect_context,
            size,
            pos,
            buf: BytesMut::new(),
        }))
    }
}
```

注意：`BufferSender` 和 `prepare_sender` 都来自 `kissbot_channel` 的 `data` 模块，通过 `use crate::data::*;` 导入。

- [ ] **Step 3: 移除 sender_id_to_key**

`ChannelManager` struct 中移除 `sender_id_to_key` 字段。检查其在代码中的使用：

```bash
grep -n "sender_id_to_key" kissbot-channel/src/channel_manager.rs
```

如果唯一使用是 `prepare_sender` 中的 `remove`，则删除该行和字段定义。

**注意：** `sender_id_to_key` 在 `AttachmentDownloadRequestProcessor` 中被插入，在旧 `send_attachment_payload` 中被用于查找。移除整个字段及其所有引用。

- [ ] **Step 4: 编译验证**

```bash
cd kissbot-channel && cargo check
```

Expected: 编译通过。

- [ ] **Step 5: Commit**

```bash
git add kissbot-channel/src/data.rs kissbot-channel/src/channel_manager.rs
git commit -m "refactor: ChannelManager 实现 BufferSender::prepare_sender，替代旧的 send_attachment_payload

- BufferSender::get_buffer 改为 &mut self + &mut BytesMut
- 新增 ChannelBufferSender
- 移除 sender_id_to_key 字段

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: AttachmentStore open_file + WebMessenger 流式下载

**Files:**
- Modify: `kissbot-channel-web/src/attachment.rs` — 新增 `open_file` 方法
- Modify: `kissbot-channel-web/src/messenger.rs` — `download_attachment_header` 改用 `prepare_sender` + 流式读取

**Interfaces:**
- Consumes: Task 1（`AttachmentDownloadPayloadSender::prepare_sender`）

- [ ] **Step 1: AttachmentStore 新增 open_file**

在 `kissbot-channel-web/src/attachment.rs` 中 `impl AttachmentStore` 块末尾添加：

```rust
/// 根据 key 打开附件文件，返回 (File, 文件长度)
pub fn open_file(&self, key: &str) -> Result<(std::fs::File, u64)> {
    let parts: Vec<&str> = key.split('/').collect();
    if parts.len() < 3 {
        return Err(Error::AttachmentNotFound(key.to_string()));
    }
    let file_path = self.base_path.join(parts[0]).join(parts[1]).join(parts[2..].join("/"));
    if !file_path.exists() {
        return Err(Error::AttachmentNotFound(key.to_string()));
    }
    let metadata = std::fs::metadata(&file_path)?;
    let file = std::fs::File::open(&file_path)?;
    Ok((file, metadata.len()))
}
```

- [ ] **Step 2: WebMessenger.download_attachment_header 改用 prepare_sender + 流式读取**

替换 `download_attachment_header` 中 `tokio::spawn` 内部的后台任务：

```rust
tokio::spawn(async move {
    use std::io::Read;
    const CHUNK_SIZE: u64 = 65536;

    match store.open_file(&key) {
        Ok((mut file, file_len)) => {
            let mut pos = 0u64;
            let mut ok = true;
            while pos < file_len && ok {
                let end = std::cmp::min(pos + CHUNK_SIZE, file_len);
                let chunk_size = (end - pos) as usize;
                let mut chunk_sender = match sender.prepare_sender(&key_for_sender, file_len as u32, pos) {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let buf = chunk_sender.get_buffer();
                buf.resize(chunk_size, 0);
                if let Err(e) = file.read_exact(&mut buf[..chunk_size]) {
                    tracing::error!("Failed to read file chunk: {}", e);
                    break;
                }
                ok = chunk_sender.send().await.is_ok();
                pos = end;
            }
            // 发送 size=0 的结束标记
            if let Ok(end_sender) = sender.prepare_sender(&key_for_sender, 0, pos) {
                let _ = end_sender.send().await;
            }
        }
        Err(e) => {
            tracing::error!("Failed to open attachment for download: key={}, error={}", key, e);
        }
    }
});
```

移除 `store.get_attachment_by_key` 调用和 `Bytes::slice()` 切分逻辑。

- [ ] **Step 3: 编译验证**

```bash
cd kissbot-channel-web && cargo check
cd kissbot-api && cargo test
```

Expected: 全部编译通过，API 测试通过。

- [ ] **Step 4: Commit**

```bash
git add kissbot-channel-web/src/attachment.rs kissbot-channel-web/src/messenger.rs
git commit -m "refactor: 下载方向改用 BufferSender 流式读取文件

- AttachmentStore 新增 open_file 方法
- download_attachment_header 用 prepare_sender + 文件流式读取替代
  整体读入后切片的方式，减少内存占用和拷贝

Co-Authored-By: deepseek-v4-flash"
```
