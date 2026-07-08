# 附件下载流程优化与 Registry 合并设计

## 概述

在上一轮 transfer_id 重构基础上，进一步优化附件下载流程和 Registry 接口。核心变更：
1. `handle_download_attachment` 流程改为先读 metadata 再 parse_range，合并 open_file 和 read_attachment_range
2. `AttachmentRegistry::register` 直接返回 `Arc<AttachmentInfoResponse>`，合并 `gen_transfer_id`；upload_channels 和 transfer_key_map 职责拆分
3. 上传完成自动清理 upload_channels，下载完成自动清理 transfer_key_map

## 动机

1. **消除重复文件打开**：当前下载流程中 `open_file` 被调用两次（第一次拿 size，第二次读数据），改为 metadata 提供 size
2. **简化 Registry 接口**：`register` + `gen_transfer_id` 两步合并为一步，减少调用方心智负担
3. **职责分离**：upload_channels 只处理上传，transfer_key_map 只处理下载，互不依赖
4. **资源自动清理**：上传/下载完成后自动清除对应的 transfer_id，防止泄漏

## 影响范围

- `kissbot-channel/src/attachment.rs` — AttachmentRegistry trait 变更
- `kissbot-channel-web/src/attachment.rs` — AttachmentStore 实现变更
- `kissbot-channel-web/src/http.rs` — handle_download_attachment 重构
- `kissbot-channel-web/src/messenger.rs` — 适配注册和下载

---

## 一、`handle_download_attachment` 流程优化

### 当前流程

```rust
// 1. open_file 拿 size（重复打开）
let (_, file_len) = store.open_file(key)?;
// 2. get_meta 拿 mime_type（二次读 metadata 文件）
let meta = store.get_meta(key)?;
let mime = mime_guess::from_path(meta.info.file_name);
// 3. parse_range 用 file_len
let (start, length) = parse_range(range_str, file_len);
// 4. 再次 open_file 读数据
let (file, _) = store.open_file(key)?;
let data = read_attachment_range(file, start, length);
```

### 优化流程

```rust
// 1. get_meta 一次获取 size + mime_type
let meta = store.get_meta(key)?;
let file_len = meta.info.size_bytes;
let mime = meta.info.mime_type.clone();
// 2. parse_range
let (start, length) = parse_range(range_str, file_len);
// 3. 合并函数：打开文件 + seek + read
let data = store.read_attachment_range(key, start, length)?;
```

### AttachmentStore 新增方法

`kissbot-channel-web/src/attachment.rs`：

```rust
/// 根据 key 和范围读取附件数据
/// 内部 parse_key → open file → seek → read_exact
/// 不返回文件大小（调用方已从 get_meta 获取）
pub fn read_attachment_range(&self, key: &str, start: u64, length: u64) -> Result<Bytes> {
    let (group_id, uuid) = Self::parse_key(key)?;
    let file_path = self.base_path.join(group_id).join(uuid);
    let mut file = std::fs::File::open(&file_path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; length as usize];
    file.read_exact(&mut buf)?;
    Ok(Bytes::from(buf))
}
```

### 删除

- `AttachmentStore::open_file` 方法移除
- `http.rs` 中的独立函数 `read_attachment_range` 移除
- `http.rs` 中的独立函数 `parse_range` 保留（仍是纯函数，无状态依赖）

---

## 二、AttachmentRegistry 合并 gen_transfer_id

### Trait 变更（`kissbot-channel/src/attachment.rs`）

```rust
#[async_trait]
pub trait AttachmentRegistry: Send + Sync {
    /// 注册附件，返回包含 key、info、transfer_id 的响应
    /// transfer_id 用于上传时的 write_chunk 路由
    async fn register(
        &self,
        messenger_id: &str,
        user_id: &str,
        group_id: &str,
        info: Arc<AttachmentInfo>,
    ) -> std::result::Result<Arc<AttachmentInfoResponse>, Error>;

    // gen_transfer_id 已删除，合并到 register 中
}
```

### process_attachment_message 变更

原来：
```rust
let key = registry.register(messenger_id, user_id, group_id, info.clone()).await?;
let transfer_id = registry.gen_transfer_id(key.as_str()).await?;
Ok(Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse { key, info, transfer_id })))
```

改为：
```rust
let resp = registry.register(messenger_id, user_id, group_id, info.clone()).await?;
Ok(Content::AttachmentInfoResponse(resp))
```

---

## 三、AttachmentStore 实现变更

### upload_channels 抽象为 struct

```rust
/// 上传通道
struct UploadChannel {
    key: Arc<String>,
    current_pos: u64,
    sender: flume::Sender<UploadCommand>,
}
```

### register 实现

```rust
async fn register(&self, _messenger_id: &str, _user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> Result<Arc<AttachmentInfoResponse>> {
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
    let meta = AttachmentMeta { key: key.clone(), info: info.clone(), has_thumbnail: false };
    let meta_path = dir.join(format!("{}.metadata", uuid));
    std::fs::write(&meta_path, serde_json::to_string(&meta)?)?;

    // 插入 LRU 缓存
    self.meta_cache.lock().unwrap().put(uuid, Arc::new(meta));

    // 直接注册到 upload_channels（不上 transfer_key_map）
    let (tx, rx) = flume::unbounded();
    let channels = self.upload_channels.clone();
    let base_path = self.base_path.clone();
    let key_ch = key.clone();
    tokio::spawn(async move {
        let mut current_pos = 0u64;
        while let Ok(cmd) = rx.recv_async().await {
            let result = Self::process_upload_write_inner(
                &base_path, key_ch.as_str(), &mut current_pos, cmd.header.pos, &cmd.data,
            );
            if result.is_ok() {
                channels.remove(&transfer_id);
            }
            let _ = cmd.res.send(result);
        }
    });
    self.upload_channels.insert(transfer_id, UploadChannel { key: key.clone(), current_pos: 0, sender: tx });

    Ok(Arc::new(AttachmentInfoResponse { key, info, transfer_id }))
}
```

### write_chunk 简化

```rust
pub async fn write_chunk(&self, transfer_id: u32, pos: u64, size: u32, data: Bytes) -> Result<u64> {
    let tx = self.upload_channels.get(&transfer_id)
        .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?
        .value().sender.clone();
    let (res_tx, res_rx) = oneshot::channel();
    tx.send(UploadCommand {
        header: AttachmentPayloadHeader { id: transfer_id, size, pos },
        data,
        res: res_tx,
    })?;
    res_rx.await?
}
```

注意：`get_or_create_upload_channel` 方法删除。如果 transfer_id 不在 upload_channels 中，视为错误直接返回。

### send_download_payload 末尾清理

```rust
pub async fn send_download_payload(&self, transfer_id: u32, sender: &dyn AttachmentDownloadPayloadSender) -> Result<()> {
    let key = self.transfer_key_map.get(&transfer_id)
        .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?
        .clone();
    // ... 分块读取发送 ...

    // 发送 size=0 结束标记
    if let Ok((sn, buf)) = sender.prepare_send(transfer_id, 0, pos) {
        let _ = sender.send(sn, transfer_id, 0, pos, buf).await;
    }

    // 下载完成，清理
    self.transfer_key_map.remove(&transfer_id);

    Ok(())
}
```

---

## 四、数据流（完整示例）

### 上传

```
前端/Agent           ChannelManager             WebMessenger          AttachmentStore
  │                        │                        │                     │
  │── send_message ───────→│                        │                     │
  │                        │── send_message ───────→│                     │
  │                        │                        │── register ───────→│
  │                        │                        │   (生成 key+id,     │
  │                        │                        │    写 metadata,     │
  │                        │                        │    注册到 upload_ch)│
  │                        │                        │← AttachmentInfoRes │
  │                        │                        │   (含 transfer_id)  │
  │                        │←── response(content) ─│                     │
  │                        │ 遍历 content 注册       │                     │
  │                        │ receiver_map[id]        │                     │
  │←─ response(content) ──│                        │                     │
  │  (含 transfer_id)      │                        │                     │
  │                        │                        │                     │
  │── attachment_payload ─→│                        │                     │
  │   (transfer_id)        │── send_attachment_payload                     │
  │                        │   (transfer_id, ...)  │                     │
  │                        │                        │── write_chunk ────→│
  │                        │                        │   (transfer_id)     │
  │                        │                        │←─ ok ─────────────│
  │                        │                        │   完成后自动清理     │
  │←─ response ───────────│                        │   upload_channels   │
```

### 下载

```
前端/Agent           ChannelManager             WebMessenger          AttachmentStore
  │                        │                        │                     │
  │── download_request ───→│                        │                     │
  │                        │── download_attachment_header ───────────────→│
  │                        │                        │── get_meta          │
  │                        │                        │── next_transfer_id  │
  │                        │                        │   写入 transfer_key │
  │                        │                        │← AttachmentInfoRes  │
  │                        │  注册 sender_map[id]   │   (含 transfer_id)  │
  │←─ response(含 tid) ───│                        │                     │
  │                        │                        │                     │
  │                        │── start_send_download_payload ──────────────→│
  │                        │   (transfer_id)        │── send_download_payload
  │                        │                        │   (分块读取发送)    │
  │←── download_payload ──│                        │   发完 size=0 后    │
  │                        │                        │   清理 transfer_key │
```

---

## 五、Impact Matrix

| 文件 | 变更类型 | 内容 |
|------|----------|------|
| `kissbot-channel/src/attachment.rs` | 修改 | `AttachmentRegistry::register` 返回类型改为 `Result<Arc<AttachmentInfoResponse>>`；删除 `gen_transfer_id` |
| `kissbot-channel-web/src/attachment.rs` | 修改 | `register` 实现合并 gen_transfer_id；新增 `UploadChannel` struct；新增 `read_attachment_range` 方法；删除 `open_file` 方法；删除 `get_or_create_upload_channel`；`write_chunk` 直接查 upload_channels；`send_download_payload` 末尾清理 transfer_key_map |
| `kissbot-channel-web/src/http.rs` | 修改 | `handle_download_attachment` 流程改为 get_meta → parse_range → read_attachment_range；删除独立的 `read_attachment_range` 函数 |
| `kissbot-channel-web/src/messenger.rs` | 修改 | `download_attachment_header` 调 `next_transfer_id_for` 写入 transfer_key_map（下载用） |

---

## 六、测试关注点

1. `AttachmentRegistry` 相关测试更新 register 返回类型
2. `process_attachment_message` 单元测试验证直接返回 Arc<AttachmentInfoResponse>
3. handle_download_attachment 流程测试（正常、Range、Range 越界、文件不存在）
4. 编译通过（所有 crate）
5. 单元测试通过
