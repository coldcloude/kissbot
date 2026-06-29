# 统一附件上传下载引擎设计

## 概述

WebMessenger 内部建立统一的上传/下载引擎，对外分别包装为 Messenger trait 接口和 HTTP handler。上传使用 flume 队列串行处理 + oneshot 同步等待，下载支持流式读取和 Range 断点续传。

## 上传引擎

### UploadCommand

```rust
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

- key、pos、size、data 标识本次写入，与 Messenger trait 参数顺序一致
- res 用于将写入结果（当前 pos 或错误）返回给调用方
- size_bytes、temp_path、target_path 由后台任务从 pending_uploads 中读取

### WebMessenger 新增字段

```rust
pub upload_channels: Arc<DashMap<String, flume::Sender<UploadCommand>>>,
pub pending_uploads: DashMap<String, PendingAttachment>,
```

### 核心方法

```rust
impl WebMessenger {
    pub fn write_attachment_chunk(
        &self,
        key: &str,
        pos: u64,
        _size: u32,
        data: Bytes,
    ) -> Result<u64> {
        let tx = self.get_or_create_upload_channel(key);
        let (res_tx, res_rx) = oneshot::channel();
        
        tx.send(UploadCommand::Write {
            key: key.to_string(),
            pos,
            size: _size,
            data,
            res: res_tx,
        }).map_err(|_| Error::InternalError("upload channel closed".to_string()))?;
        
        // 同步等待后台任务处理完成
        res_rx.recv().map_err(|_| Error::InternalError("upload channel recv error".to_string()))?
            .map_err(|e| Error::InternalError(e))
    }
}
```

### 后台任务

每个 key 首次调用 `get_or_create_upload_channel` 时启动一个 `tokio::spawn` 后台任务：

```rust
tokio::spawn(async move {
    let mut current_pos = 0u64;
    while let Ok(cmd) = rx.recv_async().await {
        let result = match cmd {
            UploadCommand::Write { key, pos, size, data, res } => {
                let r = Self::process_upload_write(
                    &store, &pending_uploads, &key, &mut current_pos, pos, data
                );
                // 如果是最后一块，清理 channel
                if let Ok(p) = &r {
                    if *p >= get_size_from_pending(...) {
                        channels.remove(&key);
                    }
                }
                let _ = res.send(r);
            }
        };
    }
});

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
        pending_uploads.remove(key);
    }
    
    Ok(*current_pos)
}
```

### Messenger trait 实现适配

```rust
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> Result<(), kissbot_channel::Error> {
    self.write_attachment_chunk(key, pos, size, data)
        .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;
    Ok(())
}
```

### HTTP handler 适配

```rust
// handle_upload_attachment 中
messenger.write_attachment_chunk(&attachment_key, 0, size_bytes, file_data)?;
// 不再需要直接操作 pending_uploads、append_to_temp、finalize_upload
```

## 下载引擎

```rust
impl WebMessenger {
    /// 打开文件读取器，返回 (File, 文件长度, MIME type)
    pub fn open_download_reader(&self, key: &str) -> Result<(std::fs::File, u64, String)> {
        let meta = self.attachment_store.get_meta_by_key(key)?;
        let (file, len) = self.attachment_store.open_file(key)?;
        let mime = mime_guess::from_path(meta.file_name.as_str()).first_or_octet_stream().to_string();
        Ok((file, len, mime))
    }
    
    /// 读取指定范围数据，用于 HTTP Range 断点续传
    pub fn read_attachment_range(&self, key: &str, offset: u64, length: u64) -> Result<Bytes> {
        let (mut file, _) = self.attachment_store.open_file(key)?;
        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; length as usize];
        file.read_exact(&mut buf)?;
        Ok(Bytes::from(buf))
    }
}
```

### 下载流程

**HTTP 下载（handle_download_attachment）：**

1. 解析 `Range` header
2. 无 Range：调用 `read_attachment_range(key, 0, file_len)` 返回全量（200 OK）
3. 有 Range：调用 `read_attachment_range(key, start, length)` 返回部分（206 Partial Content）
4. 流式输出：对超大文件可用 `tokio::fs::File` 或 axum `StreamBody` 流式发送

**Agent 下载（download_attachment_header）：**

1. 调用 `open_download_reader(key)` 获取 File
2. 分 chunk 读取 → `prepare_send` → `send`
3. 现有 BufferSender 流程不变

## PendingAttachment 变更

```rust
pub struct PendingAttachment {
    pub group_id: Arc<String>,
    pub msg_id: Arc<String>,
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
    pub temp_path: PathBuf,
    pub target_path: PathBuf,
    // current_pos 不再需要——在后台任务中作为局部变量维护
}
```

## 并发与顺序

- 每个 key 一个 flume 队列 + 一个后台任务
- 调用方通过 oneshot channel 同步等待结果
- 调用方收到成功响应后才发送下一块，天然保证顺序
- 后台任务维护 current_pos 作为局部变量，无竞争

## HTTP Range 断点续传

HTTP handler 解析 `Range: bytes=start-end` header：

```
无 Range header → 200 OK, body = 全量文件
有 Range header → 206 Partial Content, body = 指定范围
  Content-Range: bytes start-end/total
```

响应中支持断点续传。客户端可记录已接收的位置，中断后从该位置重新请求。
