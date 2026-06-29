# BufferSender 下载分离设计

## 概述

下载方向将 `AttachmentDownloadPayloadSender::send_attachment_payload` 替换为 `prepare_sender`，返回 `Box<dyn BufferSender>`。调用方通过 `get_buffer()` 获取 `&mut BytesMut` 直接写入数据，然后 `send()` 发送。每个 chunk 一个 `BufferSender`。

## BufferSender trait

```rust
pub trait BufferSender {
    fn get_buffer(&mut self) -> &mut BytesMut;
    async fn send(&self) -> Result<()>;
}
```

## AttachmentDownloadPayloadSender trait

```rust
pub trait AttachmentDownloadPayloadSender: Send + Sync {
    fn prepare_sender(&self, key: &str, size: u32, pos: u64) -> Result<Box<dyn BufferSender>>;
}
```

## ChannelManager 实现

`prepare_sender` 查找 `attachment_sender_map`，创建 `ChannelBufferSender`：

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
        if self.size == 0 { /* cleanup sender_map */ }
        self.connect_context.ws_context.send_bin(frame.freeze()).await?;
        Ok(())
    }
}
```

## AttachmentStore 新增 open_file

```rust
pub fn open_file(&self, key: &str) -> Result<(std::fs::File, u64)>
```

解析 key 中的路径，打开文件返回 `File` + 文件长度。

## WebMessenger download_attachment_header 变更

```rust
// 启动后台任务：流式读取文件并推送
let sender = self.on_download_attachment_payload.upgrade()...;
let store = self.attachment_store.clone();
let key_for_sender = response_key.clone();

tokio::spawn(async move {
    const CHUNK_SIZE: u64 = 65536;
    match store.open_file(&key) {
        Ok((mut file, file_len)) => {
            let mut pos = 0u64;
            while pos < file_len {
                let end = std::cmp::min(pos + CHUNK_SIZE, file_len);
                let chunk_size = (end - pos) as usize;
                let mut chunk_sender = sender.prepare_sender(&key_for_sender, file_len as u32, pos)?;
                let buf = chunk_sender.get_buffer();
                buf.resize(chunk_size, 0);
                file.read_exact(&mut buf[..chunk_size])?;
                chunk_sender.send().await?;
                pos = end;
            }
            // 结束标记
            let end_sender = sender.prepare_sender(&key_for_sender, 0, pos)?;
            end_sender.send().await?;
        }
        Err(e) => { tracing::error!(...); }
    }
});
```

## 上传方向不变

`Messenger::send_attachment_payload` 保留原有 `(key, size, pos, data: Bytes)` 签名不变。

## 变更文件

| 文件 | 变更 |
|------|------|
| `kissbot-channel/src/data.rs` | BufferSender 和 AttachmentDownloadPayloadSender trait 已定义 ✅ |
| `kissbot-channel/src/channel_manager.rs` | 实现 `prepare_sender`，删除旧 `send_attachment_payload` impl，清理 `sender_id_to_key`（不再需要） |
| `kissbot-channel-web/src/attachment.rs` | 新增 `open_file` 方法 |
| `kissbot-channel-web/src/messenger.rs` | `download_attachment_header` 改用 `prepare_sender` + 流式文件读取 |
