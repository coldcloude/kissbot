# 附件处理设计文档

## 概述

为 kissbot-channel-web 组件补充完整的附件处理能力，统一 admin HTTP 路径和普通 user（Agent → ChannelManager → Messenger）路径的附件上传/下载流程。

## 核心设计原则

1. **统一的 attachment SN 序列**：ChannelManager 的 `global_attachment_sn` 通过 `Arc<AtomicU32>` 共享给 Messenger，确保 admin HTTP 和 Messenger trait 路径使用同一套序列号
2. **两阶段上传（init + payload）**：先登记元数据并发送消息，后补文件实体
3. **临时文件 + 重命名**：文件未上传完成时存为 `.filename.uploading`，所有 chunk 收齐后重命名为正式文件名
4. **缩略图延迟生成**：调用缩略图 API 时如果缩略图不存在则即时生成

## 变更模块

### 1. 共享 Attachment SN

**涉及文件：**
- `kissbot-channel/src/messenger.rs` — `MessengerCreator::create()` 增加参数
- `kissbot-channel/src/channel_manager.rs` — `register_messenger()` 传入 sn
- `kissbot-channel-web/src/messenger.rs` — `WebMessenger` 持有 sn

**MessengerCreator trait 变更：**

```rust
#[async_trait]
pub trait MessengerCreator<M: Messenger> {
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
        global_attachment_sn: Arc<AtomicU32>,  // 新增
    ) -> Result<Arc<M>>;
}
```

**ChannelManager::register_messenger() 变更：**
在调用 `messenger_creator.create(...)` 时传入 `self.global_attachment_sn.clone()`。

**WebMessenger 新增字段和方法：**

```rust
pub struct WebMessenger {
    // ... 现有字段
    global_attachment_sn: Arc<AtomicU32>,
    pending_uploads: DashMap<u32, PendingAttachment>,
}

impl WebMessenger {
    pub fn next_attachment_sn(&self) -> u32 {
        self.global_attachment_sn.fetch_add(1, Ordering::SeqCst)
    }
}
```

### 2. PendingAttachment 管理

**涉及文件：** `kissbot-channel-web/src/messenger.rs`

```rust
/// 待完成的附件上传信息
pub struct PendingAttachment {
    pub group_id: Arc<String>,
    pub msg_id: Arc<String>,
    pub filename: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
    pub temp_path: PathBuf,
    pub target_path: PathBuf,
}
```

`WebMessenger.pending_uploads: DashMap<u32, PendingAttachment>` 用 upload_id 索引。

### 3. AttachmentStore 改造

**涉及文件：** `kissbot-channel-web/src/attachment.rs`

新增三个方法：

```rust
impl AttachmentStore {
    /// 创建临时文件，返回 (临时路径, 目标路径)
    pub fn create_temp_file(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<(PathBuf, PathBuf)>;

    /// 追加 payload 数据到临时文件
    pub fn append_to_temp(&self, temp_path: &Path, data: &[u8]) -> Result<()>;

    /// 将临时文件重命名为正式文件
    pub fn finalize_upload(temp_path: &Path, target_path: &Path) -> Result<()>;
}
```

**缩略图延迟生成：** `get_thumbnail()` 在缩略图不存在时检测文件是否为图片类型，即时生成。

### 4. Admin HTTP API

**涉及文件：** `kissbot-channel-web/src/http.rs`

#### POST /api/attachment/init（新增）

请求体（JSON）：

```json
{
    "group_id": "g1",
    "filename": "photo.png",
    "mime_type": "image/png",
    "size_bytes": 1048576
}
```

处理流程：
1. 调用 `messenger.next_attachment_sn()` 分配 upload_id
3. 生成 msg_id（时间戳+序号）
4. 创建临时文件
5. 记录 PendingAttachment
6. 构造 OutgoingMessage（msg_type="mixed"或"image"/"file"），content 包含附件引用
7. 调用 `messenger.send(outgoing)` 发送消息（触发 agent 推送 + memory_store + SSE）
8. 返回 `{ upload_id, key }`

响应：

```json
{
    "success": true,
    "data": {
        "upload_id": 42,
        "key": "g1/20260626123456000000/photo.png"
    }
}
```

#### POST /api/attachment/upload（修改）

当前 multipart 上传改为两步流程的第二步。

请求（multipart）：

```
--boundary
Content-Disposition: form-data; name="upload_id"

42
--boundary
Content-Disposition: form-data; name="file"; filename="photo.png"
Content-Type: image/png

<binary data>
--boundary--
```

处理流程：
1. 从表单读取 `upload_id`
2. 通过 upload_id 查找 PendingAttachment
3. 将 file 字段的二进制数据通过 `attachment_store.append_to_temp()` 写入临时文件
4. 调用 `AttachmentStore::finalize_upload()` 重命名
5. 从 `pending_uploads` 中移除记录
6. 返回成功

#### GET /api/attachment/download（无变化）

已支持任意 key 下载。

#### GET /api/attachment/thumbnail（微调）

调用 `get_thumbnail()` 时该函数内部支持延迟生成。

### 5. Messenger trait 实现变更

**涉及文件：** `kissbot-channel-web/src/messenger.rs`

#### send() / send_message() — 填充 attachment_upload_id_map

遍历 `outgoing.attachment_map`，为每个附件分配 upload_id、创建临时文件、记录 PendingAttachment：

```rust
let attachment_upload_id_map = Arc::new(DashMap::new());
for entry in outgoing.attachment_map.iter() {
    let upload_id = self.next_attachment_sn();
    let (temp_path, target_path) = self.attachment_store.create_temp_file(
        outgoing.group_id.as_str(), msg_id.as_str(), entry.key().as_str()
    )?;
    self.pending_uploads.insert(upload_id, PendingAttachment {
        group_id: outgoing.group_id.clone(),
        msg_id: msg_id.clone(),
        filename: entry.key().clone(),
        mime_type: entry.value().mime_type.clone(),
        size_bytes: entry.value().size_bytes,
        temp_path,
        target_path,
    });
    attachment_upload_id_map.insert(entry.key().clone(), upload_id);
}
```

返回的 `OutgoingMessageResponse.attachment_upload_id_map` 用上述 map 填充。

#### send_attachment_payload() — 从 no-op 改为写入

接收分块数据，追加写入临时文件。所有块收完后重命名：

```rust
async fn send_attachment_payload(&self, id: u32, _size: u32, pos: u64, data: &[u8]) -> Result<()> {
    let pending = self.pending_uploads.get(&id)
        .ok_or_else(|| Error::AttachmentNotFound(format!("upload_id={}", id)))?;
    
    self.attachment_store.append_to_temp(&pending.temp_path, data)?;
    
    if (pos + data.len() as u64) >= pending.size_bytes {
        let (temp, target) = (pending.temp_path.clone(), pending.target_path.clone());
        drop(pending);
        AttachmentStore::finalize_upload(&temp, &target)?;
        self.pending_uploads.remove(&id);
    }
    
    Ok(())
}
```

#### download_attachment_header() — 启动主动推送

分配 download_id 后启动 tokio::spawn 后台任务，逐块读取文件并通过 `AttachmentDownloadPayloadSender` 推送给 nexus：

```rust
async fn download_attachment_header(&self, request: AttachmentDownloadRequest, attachment_sn: Arc<AtomicU32>) -> Result<Arc<AttachmentDownloadResponseHeader>> {
    let meta = self.attachment_store.get_meta_by_key(request.key.as_str())?;
    let download_id = attachment_sn.fetch_add(1, Ordering::SeqCst);
    
    let sender = self.on_download_attachment_payload.upgrade()
        .ok_or_else(|| Error::InternalError("download payload sender unavailable".to_string()))?;
    let store = self.attachment_store.clone();
    let key = request.key.clone();
    
    tokio::spawn(async move {
        const CHUNK_SIZE: u64 = 65536;
        if let Ok(data) = store.get_attachment_by_key(&key) {
            let len = data.len() as u64;
            let mut pos = 0u64;
            while pos < len {
                let end = std::cmp::min(pos + CHUNK_SIZE, len);
                let chunk = &data[pos as usize..end as usize];
                if sender.send_attachment_payload(download_id, len as u32, pos, chunk).await.is_err() {
                    break;
                }
                pos = end;
            }
        }
    });
    
    Ok(Arc::new(AttachmentDownloadResponseHeader {
        download_id,
        metadata: Arc::new(AttachmentInfo {
            att_id: meta.att_id,
            mime_type: meta.mime_type,
            size_bytes: meta.size_bytes,
        }),
    }))
}
```

### 6. 废弃

- 旧版的 `POST /api/attachment/upload`（直接 multipart 存到 `group_id="temp"` 的行为）将被新的两步流程取代

## 数据流总览

### Admin 上传附件

```
admin POST /api/attachment/init (JSON: group_id, filename, mime_type, size_bytes)
  → WebMessenger.next_attachment_sn() → upload_id
  → AttachmentStore.create_temp_file()
  → 记录 PendingAttachment
  → 构造 OutgoingMessage → WebMessenger.send()
    → handle_incoming_message → agent + memory_store + SSE
  → 返回 { upload_id, key }

admin POST /api/attachment/upload (multipart: upload_id + file)
  → 查找 PendingAttachment
  → AttachmentStore.append_to_temp()
  → AttachmentStore.finalize_upload()
  → 清除 PendingAttachment
  → 返回成功
```

### 普通 User 上传附件（经过 Agent/Nexus）

```
nexus → OutgoingMessage (含 attachment_map)
  → ChannelManager → Messenger.send_message()
    → WebMessenger: 分配 upload_id、创建临时文件、记录 PendingAttachment
    → 返回 OutgoingMessageResponse (含 attachment_upload_id_map)
  
nexus → AttachmentPayload (upload_id, pos, data)
  → ChannelManager → Messenger.send_attachment_payload()
    → WebMessenger: append_to_temp()
    → 收齐所有 chunk 后 finalize_upload()
```

### Admin / User 下载附件

```
nexus → AttachmentDownloadRequest (key)
  → ChannelManager → Messenger.download_attachment_header()
    → WebMessenger: 查元数据、分配 download_id
    → tokio::spawn: 逐块读取文件
      → AttachmentDownloadPayloadSender.send_attachment_payload(download_id, pos, data)
      → ChannelManager → 推给 nexus

admin GET /api/attachment/download?key=...
  → AttachmentStore.get_attachment_by_key() → 返回文件（读全部到内存，小文件场景）
```

## 关于缩略图

- 上传时**不**生成缩略图，仅在 GET /api/attachment/thumbnail 调用时按需生成
- 生成条件：文件 MIME type 以 `image/` 开头，且缩略图文件不存在
- 缩略图尺寸：200×200 像素，JPEG 格式（与现有实现一致）
- 缓存：生成后写入磁盘 `thumb_{filename}`，后续直接读取
