# AttachmentStore 重构设计

> **设计目标：** AttachmentRegistry 由 AttachmentStore 实现，WebMessenger 不维护任何附件状态。附件 key 改为 group_id+UUID 格式，存储按 group 分目录、UUID 为文件名，metadata 持久化 + LRU 缓存。

**架构变更概要：**

- `AttachmentRegistry` trait 改为异步（`async fn register`）
- `process_attachment_message` 改为异步
- `AttachmentStore` 实现 `AttachmentRegistry`，管理所有附件状态
- 上传/下载队列由 `AttachmentStore` 内部管理
- `WebMessenger` 删除 `pending_uploads`、`upload_channels`、`pending_registrations` 字段

**Tech Stack:** Rust, tokio, flume, lru crate, serde, kissbot-channel (AttachmentRegistry trait)

---

## 1. AttachmentRegistry trait 变更

**位置:** `kissbot-channel/src/attachment.rs`

**变更：**
- `fn register(...)` 改为 `async fn register(...)`
- `process_attachment_message` 改为 `async fn process_attachment_message(...)`

```rust
#[async_trait]
pub trait AttachmentRegistry: Send + Sync {
    async fn register(&self, messenger_id: &str, user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> Result<Arc<String>>;
}

pub async fn process_attachment_message(
    outgoing: Arc<OutgoingMessage>,
    registry: &dyn AttachmentRegistry,
) -> Result<Arc<OutgoingMessageResponse>> {
    // 内部 process_content 也改为 async
}
```

`process_content` 内部递归调用改为 async，`Content::Multi` 分支用 for 循环处理每个子项（当前已如此），await 每个子项的 `process_content`。

## 2. 存储结构

**位置:** `kissbot-channel-web/src/attachment.rs`（部分重写）

### 文件系统布局

```
{base_path}/
  {group_id}/
    {uuid}              -- 附件本体文件
    {uuid}.metadata     -- JSON 格式的 AttachmentMeta
```

key 格式：`{group_id}/{uuid}`

### AttachmentMeta（不变）

```rust
pub struct AttachmentMeta {
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
    pub has_thumbnail: bool,
}
```

### LRU 缓存

使用 `lru::LruCache<String, Arc<AttachmentMeta>>`（线程安全需包 `Mutex` 或 `RwLock`）。

- register 时：写 metadata 文件后插入缓存
- 读缓存未命中时：从 `{uuid}.metadata` 文件读取并插入缓存
- 缓存淘汰策略：LRU，容量可配置（默认 1024）
- 上传/下载方向查询 meta 时走缓存

### 上传队列

`AttachmentStore` 内部维护：
```rust
// key → (pos, flume::Sender<UploadCommand>)
upload_channels: DashMap<String, (u64, flume::Sender<UploadCommand>)>,
```

`UploadCommand` 与当前的类似，但由 AttachmentStore 处理：

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

每次 chunk 写入后更新 `pos`，当 `pos >= size_bytes` 时说明文件完整，finalize（如果 register 时创建了临时文件则 rename；否则直接视为完成）。

### 新方法

```rust
impl AttachmentStore {
    /// 异步写入 chunk（外部调用入口）
    async fn write_chunk(&self, key: &str, pos: u64, size: u32, data: Bytes) -> Result<u64>;

    /// 获取 metadata（走 LRU 缓存）
    fn get_meta(&self, key: &str) -> Result<Arc<AttachmentMeta>>;

    /// 打开附件文件（下载用）
    fn open_file(&self, key: &str) -> Result<(std::fs::File, u64)>;

    /// 获取缩略图数据
    fn get_thumbnail(&self, key: &str) -> Result<Bytes>;

    /// 发送下载 payload（内部按 CHUNK_SIZE 分块读取并调用 sender）
    async fn send_download_payload(&self, key: &str, sender: &dyn AttachmentDownloadPayloadSender) -> Result<()>;
}
```

## 3. AttachmentStore 实现 AttachmentRegistry

```rust
#[async_trait]
impl AttachmentRegistry for AttachmentStore {
    async fn register(&self, _messenger_id: &str, _user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> Result<Arc<String>> {
        let uuid = uuid::Uuid::new_v4().to_string();
        let key = Arc::new(format!("{}/{}", group_id, uuid));

        // 1. 创建 group 目录
        let dir = self.base_path.join(group_id);
        std::fs::create_dir_all(&dir)?;

        // 2. 创建空临时文件（防止 upload 在 register 之前到达）
        let temp_path = dir.join(format!(".{}.uploading", uuid));
        std::fs::write(&temp_path, &[])?;

        // 3. 写 metadata 文件
        let meta = AttachmentMeta {
            file_name: info.file_name.clone(),
            mime_type: info.mime_type.clone(),
            size_bytes: info.size_bytes,
            has_thumbnail: false, // 上传完成后由 check_thumbnail 更新
        };
        let meta_path = dir.join(format!("{}.metadata", uuid));
        std::fs::write(&meta_path, serde_json::to_string(&meta)?)?;

        // 4. 插入 LRU 缓存
        self.meta_cache.lock().unwrap().put(uuid.clone(), Arc::new(meta));

        Ok(key)
    }
}
```

## 4. WebMessenger 变更

**删除的字段：**
- `pending_uploads: DashMap<String, PendingAttachment>`
- `upload_channels: Arc<DashMap<String, flume::Sender<UploadCommand>>>`
- `pending_registrations: tokio::sync::Mutex<Vec<(Arc<String>, Arc<AttachmentInfo>)>>`

**`AttachmentRegistry` 实现移除：** `WebMessenger` 不再实现 `AttachmentRegistry`。

**`send()` 方法变更：**
- `process_attachment_message` 调用改为 await，registry 参数改为 `&*self.attachment_store`
- 删除 `pending_registrations.lock().await.drain(..)` 后创建临时文件的逻辑

### Messenger trait 实现调整

```rust
#[async_trait]
impl Messenger for WebMessenger {
    async fn send_message(&self, message: OutgoingMessage, _sn: Arc<AtomicU32>) -> Result<Arc<OutgoingMessageResponse>, kissbot_channel::Error> {
        Ok(self.send(Arc::new(message)).await?)
    }

    async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse, kissbot_channel::Error> {
        self.attachment_store.write_chunk(key, pos, size, data).await?;
        Ok(AttachmentPayloadResponse {
            key: Arc::new(key.to_string()),
            pos,
            size,
            error_code: 0,
            error_msg: None,
        })
    }

    async fn download_attachment_header(&self, request: AttachmentDownloadRequest, _sn: Arc<AtomicU32>) -> Result<Arc<AttachmentInfoResponse>, kissbot_channel::Error> {
        let meta = self.attachment_store.get_meta(request.key.as_str())?;
        let info = AttachmentInfo {
            file_name: meta.file_name.clone(),
            mime_type: meta.mime_type.clone(),
            size_bytes: meta.size_bytes,
        };
        Ok(Arc::new(AttachmentInfoResponse {
            key: Arc::clone(&request.key),
            info: Arc::new(info),
        }))
    }

    async fn start_send_download_attachment_payload(&self, key: &str) -> Result<(), kissbot_channel::Error> {
        let sender = self.on_download_attachment_payload.upgrade()
            .ok_or_else(|| kissbot_channel::Error::InternalError("download payload sender unavailable".to_string()))?;
        let store = self.attachment_store.clone();
        let key_owned = key.to_string();

        tokio::spawn(async move {
            // AttachmentStore 提供按 key 分块读取的方法
            if let Err(e) = store.send_download_payload(&key_owned, &sender).await {
                tracing::error!("Failed to send download payload: {}", e);
            }
        });

        Ok(())
    }
}
```

## 5. HTTP handler 调整

### `handle_init_attachment` 简化

不再自行生成 msg_id、key、创建临时文件。直接构造 OutgoingMessage 调用 send：

```rust
async fn handle_init_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<InitAttachmentRequest>,
) -> impl IntoResponse {
    let info = AttachmentInfo {
        file_name: req.file_name.clone(),
        mime_type: req.mime_type.clone(),
        size_bytes: req.size_bytes,
    };
    let outgoing = OutgoingMessage {
        messenger_id: messenger.messenger_id.clone(),
        user_id: ADMIN_USER_ID.clone(),
        group_id: req.group_id.clone(),
        msg_type: Arc::new(MSG_TYPE_ATTACHMENT.to_string()),
        content: Content::AttachmentInfo(Arc::new(info)),
    };

    match messenger.send(Arc::new(outgoing)).await {
        Ok(resp) => Json(ApiResponse::success(resp)),
        Err(e) => Json(ApiResponse::<OutgoingMessageResponse>::error(e.to_string())),
    }
}
```

### `handle_upload_attachment` 简化

不再读取 pending_uploads，直接委托给 AttachmentStore：

```rust
async fn handle_upload_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // 解析 key 和 file_data（不变）
    // ...
    // 直接委托
    match messenger.attachment_store.write_chunk(&key, 0, size_bytes as u32, file_data).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}
```

## 6. 依赖变更

**kissbot-channel:**
- 添加 `async-trait` 依赖（如果尚未添加）
- `AttachmentRegistry` 和 `process_attachment_message` 使用 `#[async_trait]`

**kissbot-channel-web:**
- 添加 `lru` crate 依赖
- 添加 `uuid` crate 依赖（feature: "v4"）
- 移除不再需要的与上传队列相关的字段

## 7. 缩略图处理

当前缩略图在 `save_attachment` / `get_thumbnail` 中按需生成。新设计中：

- `register` 时 `has_thumbnail = false`
- 文件上传完成后（write_chunk 检测 pos >= size_bytes），AttachmentStore 检查 mime_type，如果是图片则生成缩略图
- 更新 metadata 文件中的 `has_thumbnail = true`，更新 LRU 缓存
- `get_thumbnail` 按 key 查找缩略图文件（`{uuid}.thumb`），不存在且是图片则按需生成

## 8. Migration 说明

旧存储路径为 `{base}/{group_id}/{msg_id}/{filename}`，新路径为 `{base}/{group_id}/{uuid}`。旧数据不会自动迁移——新系统从干净存储开始。

---

## 设计确认

以上设计已与用户确认：流程、trait 变更、文件结构、WebMessenger 简化均审核通过。
