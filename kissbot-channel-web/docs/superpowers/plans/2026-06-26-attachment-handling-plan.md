# 附件处理实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 kissbot-channel-web 组件补充完整的附件处理能力，统一 admin HTTP 和普通 user（ChannelManager → Messenger）的附件上传/下载流程。

**Architecture:** 分 5 个任务逐步实现：(1) 共享 attachment SN, (2) AttachmentStore 新增方法, (3) PendingAttachment + WebMessenger 基础变更, (4) Messenger trait 实现补全, (5) HTTP handler 变更。每个任务可独立编译并测试。

**Tech Stack:** Rust, axum, tokio, kissbot-channel, kissbot-channel-web, DashMap, flume

## Global Constraints

- 所有新的 Async trait 方法必须使用 `#[async_trait]`
- `Arc<String>` / `Arc<AtomicU32>` 模式与现有代码一致
- 临时文件路径格式：`{attachment_dir}/{group_id}/{msg_id}/.{filename}.uploading`
- 目标文件路径格式：`{attachment_dir}/{group_id}/{msg_id}/{filename}`
- 缩略图文件路径格式：`{attachment_dir}/{group_id}/{msg_id}/thumb_{filename}`
- attachment SN 通过 `Arc<AtomicU32>` 共享，使用 `Ordering::SeqCst`
- 所有 API 响应统一使用 `kissbot_api::ApiResponse`
- 所有输入参数放在 JSON 请求体中传递（API 路径不嵌入动态参数）
- 不要删除代码中的注释

---

### Task 1: 共享 Attachment SN

**Files:**
- Modify: `kissbot-channel/src/messenger.rs` — `MessengerCreator::create()` 增加参数
- Modify: `kissbot-channel/src/channel_manager.rs` — `register_messenger()` 传入 sn
- Modify: `kissbot-channel-web/src/messenger.rs` — `WebMessenger` 持有 sn 并新增 `next_attachment_sn()`，`WebMessengerCreator::create()` 接收并传入

**Interfaces:**
- Consumes: 无（基础接口变更）
- Produces: `Arc<AtomicU32>` 从 ChannelManager 传递到 WebMessenger

- [ ] **Step 1: MessengerCreator trait 增加 global_attachment_sn 参数**

修改 `kissbot-channel/src/messenger.rs` 中的 `MessengerCreator` trait：

```rust
use std::sync::atomic::AtomicU32;

#[async_trait]
pub trait MessengerCreator<M: Messenger> {
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
        global_attachment_sn: Arc<AtomicU32>,
    ) -> Result<Arc<M>>;
}
```

确保 `use std::sync::Arc;` 和 `use std::sync::atomic::AtomicU32;` 已在文件顶部导入。

- [ ] **Step 2: ChannelManager::register_messenger() 传入 sn**

修改 `kissbot-channel/src/channel_manager.rs`，在 `register_messenger()` 方法中传入 `self.global_attachment_sn.clone()`：

```rust
let messenger = messenger_creator.create(
    group_change_handler,
    incoming_messages_handler,
    download_attachment_payload_handler,
    user_remove_handler,
    self.global_attachment_sn.clone(),  // 新增
).await?;
```

- [ ] **Step 3: WebMessenger 新增字段和方法**

修改 `kissbot-channel-web/src/messenger.rs`：

在 `WebMessenger` struct 中新增字段：

```rust
pub struct WebMessenger {
    // ... 现有字段
    pub messenger_id: Arc<String>,
    repo_path: PathBuf,
    config: Arc<RwLock<WebMessengerRepo>>,
    msg_id_seq: AtomicU32,
    on_group_change: Weak<dyn GroupChangeHandler>,
    on_incoming_messages: Weak<dyn IncomingMessageHandler>,
    on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
    on_user_remove: Weak<dyn UserRemoveHandler>,
    pub sse: Arc<SseDispatcher>,
    pub attachment_store: Arc<AttachmentStore>,
    // 新增：
    global_attachment_sn: Arc<AtomicU32>,
}
```

在 `WebMessenger::new()` 的参数列表和构造函数中增加 `global_attachment_sn: Arc<AtomicU32>`：

```rust
impl WebMessenger {
    pub fn new(
        messenger_id: Arc<String>,
        repo_path: PathBuf,
        config: Arc<RwLock<WebMessengerRepo>>,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
        attachment_dir: &str,
        global_attachment_sn: Arc<AtomicU32>,
    ) -> Self {
        Self {
            messenger_id,
            repo_path,
            config,
            msg_id_seq: AtomicU32::new(0),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
            on_user_remove,
            sse: Arc::new(SseDispatcher::new()),
            attachment_store: Arc::new(AttachmentStore::new(attachment_dir)),
            global_attachment_sn,
        }
    }
```

新增方法：

```rust
impl WebMessenger {
    pub fn next_attachment_sn(&self) -> u32 {
        self.global_attachment_sn.fetch_add(1, Ordering::SeqCst)
    }
}
```

末尾如有 `Mod` 声明需确认 `use std::sync::atomic::AtomicU32;` 已导入。现有文件开头已有 `use std::sync::atomic::{AtomicU32, Ordering};`——确认此行存在。

- [ ] **Step 4: WebMessengerCreator::create() 接收并传入 sn**

修改 `WebMessengerCreator` 的 `create()` 方法：

```rust
#[async_trait]
impl MessengerCreator<WebMessenger> for WebMessengerCreator {
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
        global_attachment_sn: Arc<AtomicU32>,
    ) -> std::result::Result<Arc<WebMessenger>, kissbot_channel::Error> {
        let mid = self.config.read().await.messenger_id.clone();
        let messenger = Arc::new(WebMessenger::new(
            mid,
            self.repo_path.clone(),
            self.config.clone(),
            on_group_change,
            on_incoming_messages,
            on_download_attachment_payload,
            on_user_remove,
            &self.attachment_dir,
            global_attachment_sn,
        ));
        Ok(messenger)
    }
}
```

- [ ] **Step 5: 编译验证**

运行 `cargo check -p kissbot-channel -p kissbot-channel-web`

Expected: 编译通过。

- [ ] **Step 6: Commit**

```bash
git add kissbot-channel/src/messenger.rs \
       kissbot-channel/src/channel_manager.rs \
       kissbot-channel-web/src/messenger.rs
git commit -m "refactor: MessengerCreator 新增 global_attachment_sn 参数，WebMessenger 持有共享 sn

- MessengerCreator::create() 增加 Arc<AtomicU32> 参数
- ChannelManager::register_messenger() 传入 global_attachment_sn
- WebMessenger 新增 global_attachment_sn 字段和 next_attachment_sn() 方法

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: AttachmentStore 新增临时文件 / 缩略图延迟生成方法

**Files:**
- Modify: `kissbot-channel-web/src/attachment.rs`

**Interfaces:**
- Consumes: Task 1（独立，无依赖）
- Produces: `AttachmentStore::create_temp_file()`, `AttachmentStore::append_to_temp()`, `AttachmentStore::finalize_upload()`, 改造后的 `get_thumbnail()`

- [ ] **Step 1: 新增 create_temp_file**

在 `impl AttachmentStore` 中添加：

```rust
use std::io::Write;

impl AttachmentStore {
    /// 创建临时文件，返回 (临时文件路径, 目标文件路径)
    pub fn create_temp_file(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<(PathBuf, PathBuf)> {
        let dir = self.base_path.join(group_id).join(msg_id);
        std::fs::create_dir_all(&dir)?;
        let temp_path = dir.join(format!(".{}.uploading", filename));
        let target_path = dir.join(filename);
        // 创建空临时文件
        std::fs::write(&temp_path, &[])?;
        Ok((temp_path, target_path))
    }
}
```

- [ ] **Step 2: 新增 append_to_temp**

```rust
impl AttachmentStore {
    /// 追加 payload 数据到临时文件
    pub fn append_to_temp(&self, temp_path: &Path, data: &[u8]) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(temp_path)?;
        file.write_all(data)?;
        Ok(())
    }
}
```

- [ ] **Step 3: 新增 finalize_upload**

```rust
impl AttachmentStore {
    /// 将临时文件重命名为正式文件
    pub fn finalize_upload(temp_path: &Path, target_path: &Path) -> Result<()> {
        std::fs::rename(temp_path, target_path)?;
        Ok(())
    }
}
```

- [ ] **Step 4: 改造 get_thumbnail 支持延迟生成**

修改 `get_thumbnail()` 方法（现有方法，替换原有实现）：

```rust
/// 获取缩略图数据，如果缩略图不存在则按需生成
pub fn get_thumbnail(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<Vec<u8>> {
    let dir = self.base_path.join(group_id).join(msg_id);
    let thumb_path = dir.join(format!("thumb_{}", filename));

    // 如果缩略图已存在则直接返回
    if thumb_path.exists() {
        return Ok(std::fs::read(&thumb_path)?);
    }

    // 延迟生成缩略图
    let file_path = dir.join(filename);
    if !file_path.exists() {
        return Err(Error::AttachmentNotFound(format!("{}/{}/{}", group_id, msg_id, filename)));
    }

    let mime_type = mime_guess::from_path(&filename).first_or_octet_stream();
    if mime_type.starts_with("image/") {
        if let Ok(data) = std::fs::read(&file_path) {
            if let Ok(img) = image::load_from_memory(&data) {
                let thumb = img.thumbnail(200, 200);
                if thumb.save(&thumb_path).is_ok() {
                    return Ok(std::fs::read(&thumb_path)?);
                }
            }
        }
    }

    Err(Error::ImageError("not an image or failed to generate thumbnail".to_string()))
}
```

注意：`Error` 枚举中需确认有 `ImageError` 变体。查看现有代码 `error.rs` 确认。

- [ ] **Step 5: 检查 Error 枚举**

确认 `/home/admin/project/kissbot/kissbot-channel-web/src/error.rs` 中包含 `ImageError` 变体。如不存在，添加：

```rust
#[derive(Debug)]
pub enum Error {
    // ... 现有变体
    ImageError(String),
    // ...
}
```

以及对应 `std::error::Error` 和 `Display` / `From` 实现。

- [ ] **Step 6: 编译验证**

```bash
cargo check -p kissbot-channel-web
```

Expected: 编译通过。

- [ ] **Step 7: Commit**

```bash
git add kissbot-channel-web/src/attachment.rs \
       kissbot-channel-web/src/error.rs
git commit -m "feat: AttachmentStore 新增临时文件方法和缩略图延迟生成

- 新增 create_temp_file / append_to_temp / finalize_upload 方法
- get_thumbnail 改造为缩略图不存在时按需生成
- Error 枚举补充 ImageError 变体

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: PendingAttachment 数据结构 + send() 填充 attachment_upload_id_map

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs`

**Interfaces:**
- Consumes: Task 1（`global_attachment_sn`）, Task 2（`AttachmentStore` 新方法）
- Produces: `PendingAttachment` struct, `WebMessenger.pending_uploads` 字段, `send()` 中填充 `attachment_upload_id_map`

- [ ] **Step 1: 定义 PendingAttachment 结构体**

在 `kissbot-channel-web/src/messenger.rs` 中适当地点添加（可在 `WebMessenger` 定义之前）：

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

- [ ] **Step 2: WebMessenger 新增 pending_uploads 字段**

在 `WebMessenger` struct 中新增：

```rust
pub struct WebMessenger {
    // ... 现有字段
    global_attachment_sn: Arc<AtomicU32>,
    pub pending_uploads: DashMap<u32, PendingAttachment>,
    pub attachment_store: Arc<AttachmentStore>,
    // ...
}
```

在 `WebMessenger::new()` 构造函数中初始化：

```rust
pending_uploads: DashMap::new(),
```

- [ ] **Step 3: send() 方法中填充 attachment_upload_id_map**

修改 `WebMessenger::send()` 方法。找到构建 `OutgoingMessageResponse` 返回的地方（当前在文件末尾附近）：

当前代码：
```rust
    Ok(OutgoingMessageResponse {
        msg_id,
        time,
        attachment_upload_id_map: Arc::new(DashMap::new()),
    })
```

替换为：

```rust
    // 处理附件 map，生成 upload_id
    let attachment_upload_id_map = Arc::new(DashMap::new());
    for entry in outgoing.attachment_map.iter() {
        let upload_id = self.next_attachment_sn();
        let (temp_path, target_path) = match self.attachment_store.create_temp_file(
            outgoing.group_id.as_str(), msg_id.as_str(), entry.key().as_str()
        ) {
            Ok(paths) => paths,
            Err(e) => return Err(Error::from(e)),
        };
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

    Ok(OutgoingMessageResponse {
        msg_id,
        time,
        attachment_upload_id_map,
    })
```

- [ ] **Step 4: 编译验证**

```bash
cargo check -p kissbot-channel-web
```

Expected: 编译通过。

- [ ] **Step 5: Commit**

```bash
git add kissbot-channel-web/src/messenger.rs
git commit -m "feat: 定义 PendingAttachment，send() 填充 attachment_upload_id_map

- 新增 PendingAttachment 结构体
- WebMessenger 新增 pending_uploads 字段
- send() 遍历 attachment_map 分配 upload_id 并创建临时文件

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 4: Messenger trait 实现补全（send_attachment_payload + download_attachment_header）

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs`

**Interfaces:**
- Consumes: Task 2（`append_to_temp`, `finalize_upload`, `get_meta_by_key`）, Task 3（`pending_uploads`）
- Produces: `send_attachment_payload()` 写入实现, `download_attachment_header()` 主动推送实现

- [ ] **Step 1: 实现 send_attachment_payload**

替换现有 no-op 实现：

当前代码（messenger.rs 约 560-562 行）：
```rust
async fn send_attachment_payload(&self, _id: u32, _size: u32, _pos: u64, _data: &[u8]) -> std::result::Result<(), kissbot_channel::Error> {
    Ok(())
}
```

替换为：

```rust
async fn send_attachment_payload(&self, id: u32, _size: u32, pos: u64, data: &[u8]) -> std::result::Result<(), kissbot_channel::Error> {
    use crate::error::Error as WebError;

    let pending = self.pending_uploads.get(&id)
        .ok_or_else(|| kissbot_channel::Error::InternalError(format!("upload_id {} not found", id)))?;

    self.attachment_store.append_to_temp(&pending.temp_path, data)
        .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;

    // 如果这是最后一块，重命名
    if (pos + data.len() as u64) >= pending.size_bytes {
        let temp = pending.temp_path.clone();
        let target = pending.target_path.clone();
        drop(pending);
        AttachmentStore::finalize_upload(&temp, &target)
            .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;
        self.pending_uploads.remove(&id);
    }

    Ok(())
}
```

- [ ] **Step 2: 实现 download_attachment_header 主动推送**

替换当前 `download_attachment_header` 方法：

当前代码（messenger.rs 约 564-575 行）：
```rust
async fn download_attachment_header(&self, request: AttachmentDownloadRequest, _attachment_sn: Arc<AtomicU32>) -> std::result::Result<Arc<AttachmentDownloadResponseHeader>, kissbot_channel::Error> {
    let meta = self.attachment_store.get_meta_by_key(request.key.as_str())?;

    Ok(Arc::new(AttachmentDownloadResponseHeader {
        download_id: 0,
        metadata: Arc::new(AttachmentInfo {
            att_id: meta.att_id,
            mime_type: meta.mime_type,
            size_bytes: meta.size_bytes,
        }),
    }))
}
```

替换为：

```rust
async fn download_attachment_header(&self, request: AttachmentDownloadRequest, attachment_sn: Arc<AtomicU32>) -> std::result::Result<Arc<AttachmentDownloadResponseHeader>, kissbot_channel::Error> {
    let meta = self.attachment_store.get_meta_by_key(request.key.as_str())?;
    let download_id = attachment_sn.fetch_add(1, Ordering::SeqCst);

    // 启动后台任务：逐块读取文件并推送
    let sender = self.on_download_attachment_payload.upgrade()
        .ok_or_else(|| kissbot_channel::Error::InternalError("download payload sender unavailable".to_string()))?;
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

注意：需要将 `tokio::spawn` 中用到的 `store` 和 `key` 克隆到闭包中。

- [ ] **Step 3: 编译验证**

```bash
cargo check -p kissbot-channel-web
```

Expected: 编译通过。

- [ ] **Step 4: Commit**

```bash
git add kissbot-channel-web/src/messenger.rs
git commit -m "feat: 实现 send_attachment_payload 和 download_attachment_header

- send_attachment_payload 从 no-op 改为写入临时文件并收齐后重命名
- download_attachment_header 分配 download_id 并启动后台推送

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 5: HTTP handler 变更（init + upload + thumbnail）

**Files:**
- Modify: `kissbot-channel-web/src/http.rs`
- Modify: `kissbot-channel-web/src/main.rs`（如果需要调整 state）

**Interfaces:**
- Consumes: Task 1（`next_attachment_sn()`）, Task 3（`pending_uploads`, `send()`）, Task 2（`create_temp_file`, `append_to_temp`, `finalize_upload`, `get_thumbnail`）
- Produces: `POST /api/attachment/init`, 改造后 `POST /api/attachment/upload`, 延迟缩略图

- [ ] **Step 1: 新增 InitAttachmentRequest DTO**

在 `http.rs` 的 DTOs 区域添加：

```rust
#[derive(Debug, Deserialize)]
pub struct InitAttachmentRequest {
    pub group_id: Arc<String>,
    pub filename: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Deserialize)]
pub struct UploadAttachmentRequest {
    pub upload_id: u32,
}
```

- [ ] **Step 2: 新增 /api/attachment/init handler**

在路由注册中添加（在已有 attachment 路由旁）：

```rust
.route("/api/attachment/init", post(handle_init_attachment))
```

添加 handler：

```rust
/// POST /api/attachment/init — 初始化附件上传，创建临时文件并发送消息
async fn handle_init_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    Json(req): Json<InitAttachmentRequest>,
) -> impl IntoResponse {
    // 1. 分配 upload_id
    let upload_id = messenger.next_attachment_sn();

    // 2. 生成 msg_id
    let msg_id = messenger.next_msg_id().to_string(); // 注意：next_msg_id 当前是私有的，需要改为 pub
    let msg_id_arc = Arc::new(msg_id.clone());

    // 3. 创建临时文件
    let (temp_path, target_path) = match messenger.attachment_store.create_temp_file(
        req.group_id.as_str(), &msg_id, req.filename.as_str()
    ) {
        Ok(paths) => paths,
        Err(e) => return Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    };

    // 4. 记录 PendingAttachment
    let key = format!("{}/{}/{}", req.group_id, msg_id, req.filename);
    messenger.pending_uploads.insert(upload_id, PendingAttachment {
        group_id: req.group_id.clone(),
        msg_id: msg_id_arc,
        filename: req.filename.clone(),
        mime_type: req.mime_type.clone(),
        size_bytes: req.size_bytes,
        temp_path,
        target_path,
    });

    // 5. 构造 OutgoingMessage 并发送
    let att_info = serde_json::json!([{
        "filename": req.filename,
        "key": key,
        "msg_type": if req.mime_type.starts_with("image/") { "image" } else { "file" }
    }]);
    let content = serde_json::to_string(&serde_json::json!({
        "text": "",
        "attachments": [serde_json::json!({
            "filename": req.filename,
            "key": key,
            "msg_type": if req.mime_type.starts_with("image/") { "image" } else { "file" }
        })]
    })).unwrap_or_default();

    let outgoing = OutgoingMessage {
        messenger_id: messenger.messenger_id.clone(),
        user_id: ADMIN_USER_ID.clone(),
        group_id: req.group_id.clone(),
        msg_type: Arc::new("mixed".to_string()),
        content: Arc::new(content),
        attachment_map: Arc::new(DashMap::new()),
    };

    match messenger.send(outgoing).await {
        Ok(resp) => Json(ApiResponse::success(serde_json::json!({
            "upload_id": upload_id,
            "key": key,
            "msg_id": resp.msg_id,
        }))),
        Err(e) => {
            // 发送失败时清理 pending 记录
            messenger.pending_uploads.remove(&upload_id);
            Json(ApiResponse::<serde_json::Value>::error(e.to_string()))
        }
    }
}
```

- [ ] **Step 3: 将 next_msg_id 改为 pub（如果当前是私有）**

当前 `WebMessenger::next_msg_id()` 是私有方法（无 `pub` 修饰）。在 `messenger.rs` 中将其改为 `pub`：

```rust
pub fn next_msg_id(&self) -> Arc<String> {
```

- [ ] **Step 4: 改造 /api/attachment/upload handler**

替换现有 `handle_upload_attachment`：

```rust
/// POST /api/attachment/upload — 第二步：上传文件实体
async fn handle_upload_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut upload_id: Option<u32> = None;
    let mut file_data: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().map(|s| s.to_string());
        if name.as_deref() == Some("upload_id") {
            if let Ok(data) = field.text().await {
                upload_id = data.trim().parse::<u32>().ok();
            }
        } else if name.as_deref() == Some("file") {
            if let Ok(data) = field.bytes().await {
                file_data = Some(data.to_vec());
            }
        }
    }

    let upload_id = match upload_id {
        Some(id) => id,
        None => return Json(ApiResponse::<serde_json::Value>::error("Missing or invalid upload_id".to_string())),
    };

    let file_data = match file_data {
        Some(d) => d,
        None => return Json(ApiResponse::<serde_json::Value>::error("Missing file data".to_string())),
    };

    // 查找 PendingAttachment
    let pending = match messenger.pending_uploads.get(&upload_id) {
        Some(p) => p,
        None => return Json(ApiResponse::<serde_json::Value>::error(format!("upload_id {} not found", upload_id))),
    };

    // 写入数据到临时文件
    if let Err(e) = messenger.attachment_store.append_to_temp(&pending.temp_path, &file_data) {
        return Json(ApiResponse::<serde_json::Value>::error(e.to_string()));
    }

    // 重命名
    let (temp, target) = (pending.temp_path.clone(), pending.target_path.clone());
    drop(pending);
    if let Err(e) = AttachmentStore::finalize_upload(&temp, &target) {
        return Json(ApiResponse::<serde_json::Value>::error(e.to_string()));
    }
    messenger.pending_uploads.remove(&upload_id);

    Json(ApiResponse::success(serde_json::json!({"success": true})))
}
```

注意：纯二进制数据的 `POST /api/attachment/upload` 目前通过 multipart 接收 `upload_id` + `file` 字段。这个设计有变化吗？—— 按之前的讨论，Step 2 使用 multipart，包含 `upload_id` 表单字段和 `file` 文件字段。

- [ ] **Step 5: 补充 http.rs 中的 use 导入**

确保 `http.rs` 的 use 语句覆盖了新类型：

```rust
use crate::messenger::{ADMIN_USER_ID, GroupConfig, UserConfig, WebMessenger, PendingAttachment};
```

- [ ] **Step 6: 确认缩略图 handler 已经支持延迟生成**

`handle_thumbnail` 当前调用的 `get_thumbnail_by_key` → `get_thumbnail`。Task 2 中已改造 `get_thumbnail` 支持延迟生成，所以 handler 不需要改。

- [ ] **Step 6: 编译验证**

```bash
cargo check -p kissbot-channel-web
```

Expected: 编译通过。

- [ ] **Step 7: Commit**

```bash
git add kissbot-channel-web/src/http.rs \
       kissbot-channel-web/src/messenger.rs
git commit -m "feat: HTTP 附件 API 改为两步流程

- 新增 POST /api/attachment/init（JSON 元数据 + 自动发消息）
- 改造 POST /api/attachment/upload 为第二步（upload_id + multipart 文件）
- next_msg_id 改为 pub 供 http handler 使用

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 6: 废弃旧的一步式上传（可选清理）

**Files:**
- Modify: `kissbot-channel-web/src/http.rs`
- Modify: `kissbot-channel-web/src/main.rs`

**接口：** 无外部依赖

- [ ] **Step 1: 确认旧版已废弃**

Task 5 中 `handle_upload_attachment` 已被替换。旧逻辑（`group_id="temp"`）已不存在。如果路由中存在 `/api/attachment/upload` 映射到新 handler，则旧版已自动废弃。

- [ ] **Step 2: 检查是否有前端或文档需要更新**

当前 `docs/spec/channel-web-decisions.md` 和现有文档描述的是旧的 `temp` 逻辑。但按约定 spec 文档以代码为准，暂不修改。

- [ ] **Step 3: 编译验证并运行测试**

```bash
cargo check -p kissbot-channel-web
cargo build -p kissbot-channel-web
```

Expected: 编译和构建通过。

- [ ] **Step 4: Commit**

```bash
git add kissbot-channel-web/src/http.rs
git commit -m "chore: 废弃旧版一步式附件上传 API

旧版 POST /api/attachment/upload 直接 multipart 存到 group_id='temp'
的行为已被两步流程（init + upload）取代

Co-Authored-By: deepseek-v4-flash"
```
