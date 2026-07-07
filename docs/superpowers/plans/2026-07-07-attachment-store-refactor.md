# AttachmentStore 重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** AttachmentRegistry 由 AttachmentStore 实现，WebMessenger 不维护任何附件状态。

**Architecture:** AttachmentRegistry trait 改为异步，process_attachment_message 同理。AttachmentStore 新增 LRU 缓存、上传队列、metadata 持久化。WebMessenger 删除 pending_uploads、upload_channels、pending_registrations 字段，附件相关操作透传给 AttachmentStore。

**Tech Stack:** Rust, async-trait, lru, uuid, flume, tokio, dashmap

## Global Constraints

- `AttachmentRegistry::register` 返回 `Result<Arc<String>>`（异步）
- key 格式为 `{group_id}/{uuid}`
- 附件存储路径 `{base_path}/{group_id}/{uuid}`，metadata 文件 `{uuid}.metadata`
- WebMessenger 不持有任何附件状态
- `#[async_trait]` 用于异步 trait
- 所有文件用 UTF-8 编码，\n 作为换行符
- 不要删除代码中的注释

---

### Task 1: AttachmentRegistry trait 改为异步

**Files:**
- Modify: `kissbot-channel/src/attachment.rs`（全文件重写）
- Test: `kissbot-channel/src/attachment.rs` 底部的测试（当前无测试，无需改动）

**Interfaces:**
- Consumes: 无（这是第一项改动）
- Produces:
  ```rust
  #[async_trait]
  pub trait AttachmentRegistry: Send + Sync {
      async fn register(&self, messenger_id: &str, user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> Result<Arc<String>>;
  }

  pub async fn process_attachment_message(
      outgoing: Arc<OutgoingMessage>,
      registry: &dyn AttachmentRegistry,
  ) -> Result<Arc<OutgoingMessageResponse>>;
  ```

- [ ] **Step 1: 修改 AttachmentRegistry trait**

将 `use std::sync::Arc;` 保持不变。新增 `use async_trait::async_trait;`。

```rust
use std::sync::Arc;

use async_trait::async_trait;
use kissbot_api::channel::{OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content, MessageItem};

use crate::error::Result;

/// 附件注册器。将 AttachmentInfo 注册为全局唯一的 key，并管理 key 与 info 的关系。
#[async_trait]
pub trait AttachmentRegistry: Send + Sync {
    /// 注册附件，返回生成的 key。
    async fn register(&self, messenger_id: &str, user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> Result<Arc<String>>;
}
```

- [ ] **Step 2: process_attachment_message 改为 async**

```rust
/// 处理 OutgoingMessage 中的附件类型消息。
///
/// 递归遍历 content，将所有 AttachmentInfo 替换为 AttachmentInfoResponse（嵌入 key）。
/// 注册过程由 AttachmentRegistry 完成。
pub async fn process_attachment_message(
    outgoing: Arc<OutgoingMessage>,
    registry: &dyn AttachmentRegistry,
) -> Result<Arc<OutgoingMessageResponse>> {
    let new_content = process_content(
        &outgoing.content,
        outgoing.messenger_id.as_str(),
        outgoing.user_id.as_str(),
        outgoing.group_id.as_str(),
        registry,
    ).await?;

    Ok(Arc::new(OutgoingMessageResponse {
        msg_id: Arc::new(String::new()),  // 调用方会覆写 msg_id 和 time
        time: Arc::new(String::new()),
        msg_type: outgoing.msg_type.clone(),
        content: new_content.clone(),
    }))
}
```

- [ ] **Step 3: process_content 改为 async**

将 `fn process_content` 改为 `async fn process_content`，内部调用 `registry.register(...).await` 和递归的 `process_content(...).await`。

```rust
/// 递归处理 Content，将 AttachmentInfo 替换为 AttachmentInfoResponse。
async fn process_content(
    content: &Content,
    messenger_id: &str,
    user_id: &str,
    group_id: &str,
    registry: &dyn AttachmentRegistry,
) -> Result<Content> {
    match content {
        Content::AttachmentInfo(info) => {
            let key = registry.register(messenger_id, user_id, group_id, info.clone()).await?;
            Ok(Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
                key,
                info: info.clone(),
            })))
        }
        Content::Multi(items) => {
            let mut new_items = Vec::with_capacity(items.len());
            for item in items.iter() {
                let new_content = process_content(
                    &item.content,
                    messenger_id,
                    user_id,
                    group_id,
                    registry,
                ).await?;
                new_items.push(Arc::new(MessageItem {
                    msg_type: item.msg_type.clone(),
                    content: new_content,
                }));
            }
            Ok(Content::Multi(new_items))
        }
        // 其他类型（text、group_change、user_remove、AttachmentInfoResponse），不做处理
        _ => Ok(content.clone()),
    }
}
```

- [ ] **Step 4: 验证编译**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel 2>&1 | head -30
```

Expected: 编译成功。

- [ ] **Step 5: 修复 kissbot-channel-web 中的编译错误**

`process_attachment_message` 现在是 async 函数，WebMessenger::send 中调用处需要改为 `.await`。先做一个最小修复让 `send()` 方法能编译（registry 参数在后续任务中改为 AttachmentStore）：

```rust
// 在 WebMessenger::send 中：
let response = kissbot_channel::process_attachment_message(
    outgoing.clone(),
    self,  // self 当前实现 AttachmentRegistry，后续会移除
).await.map_err(|e| Error::InternalError(e.to_string()))?;
```

- [ ] **Step 6: 验证完整编译**

```bash
cd /home/admin/project/kissbot && cargo build 2>&1 | head -30
```

Expected: 编译成功。

- [ ] **Step 7: 运行测试**

```bash
cd /home/admin/project/kissbot && cargo test -p kissbot-api 2>&1 | tail -10
```

Expected: 测试全部通过。

- [ ] **Step 8: 提交**

```bash
git add kissbot-channel/src/attachment.rs kissbot-channel-web/src/messenger.rs
git commit -m "refactor: AttachmentRegistry 和 process_attachment_message 改为异步

- AttachmentRegistry::register 改为 async fn，返回 Result<Arc<String>>
- process_attachment_message 改为 async fn
- process_content 递归调用改为 async/.await
- WebMessenger::send 调用处适配 .await

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: AttachmentStore 新增 LRU 缓存、uuid 依赖

**Files:**
- Modify: `kissbot-channel-web/Cargo.toml`
- Modify: `kissbot-channel-web/src/attachment.rs`（重写 AttachmentStore）

**Interfaces:**
- Consumes: `AttachmentMeta`（不变），`AttachmentRegistry` trait（来自 Task 1）
- Produces:
  ```rust
  impl AttachmentStore {
      pub fn new(base_path: &str, cache_capacity: usize) -> Self;
      fn parse_key(key: &str) -> Result<(&str, &str)>; // (group_id, uuid)
  }
  
  #[async_trait]
  impl AttachmentRegistry for AttachmentStore {
      async fn register(&self, ...) -> Result<Arc<String>>;
  }
  ```

- [ ] **Step 1: 添加依赖**

在 `kissbot-channel-web/Cargo.toml` 的 `[dependencies]` 中添加：

```toml
lru = "0.12"
uuid = { version = "1.0", features = ["v4"] }
```

- [ ] **Step 2: 重写 AttachmentStore**

全量替换 `kissbot-channel-web/src/attachment.rs`：

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use bytes::Bytes;
use lru::LruCache;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// 附件元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
    pub has_thumbnail: bool,
}

/// 附件存储：本地文件系统
///
/// 文件结构：{base_path}/{group_id}/{uuid}（本体），{uuid}.metadata（元数据）
/// key 格式：{group_id}/{uuid}
/// metadata 缓存：LRU 策略
pub struct AttachmentStore {
    base_path: PathBuf,
    meta_cache: Mutex<LruCache<String, Arc<AttachmentMeta>>>,
}

impl AttachmentStore {
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: PathBuf::from(base_path),
            meta_cache: Mutex::new(LruCache::new(1024)),
        }
    }

    /// 解析 key 为 (group_id, uuid)
    fn parse_key(key: &str) -> Result<(&str, &str)> {
        let parts: Vec<&str> = key.splitn(2, '/').collect();
        if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        Ok((parts[0], parts[1]))
    }

    /// 获取 metadata（走 LRU 缓存，缓存未命中时从文件读取）
    pub fn get_meta(&self, key: &str) -> Result<Arc<AttachmentMeta>> {
        let (group_id, uuid) = Self::parse_key(key)?;

        // 检查 LRU 缓存
        {
            let mut cache = self.meta_cache.lock().unwrap();
            if let Some(meta) = cache.get(uuid) {
                return Ok(Arc::clone(meta));
            }
        }

        // 从 metadata 文件读取
        let meta_path = self.base_path.join(group_id).join(format!("{}.metadata", uuid));
        if !meta_path.exists() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let content = std::fs::read_to_string(&meta_path)?;
        let meta: AttachmentMeta = serde_json::from_str(&content)?;
        let meta = Arc::new(meta);

        // 插入 LRU 缓存
        {
            let mut cache = self.meta_cache.lock().unwrap();
            cache.put(uuid.to_string(), Arc::clone(&meta));
        }

        Ok(meta)
    }

    /// 打开附件文件（下载用）
    pub fn open_file(&self, key: &str) -> Result<(std::fs::File, u64)> {
        let (group_id, uuid) = Self::parse_key(key)?;
        let file_path = self.base_path.join(group_id).join(uuid);
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let metadata = std::fs::metadata(&file_path)?;
        let file = std::fs::File::open(&file_path)?;
        Ok((file, metadata.len()))
    }

    /// 获取缩略图数据
    pub fn get_thumbnail(&self, key: &str) -> Result<Bytes> {
        let (group_id, uuid) = Self::parse_key(key)?;
        let dir = self.base_path.join(group_id);
        let thumb_path = dir.join(format!("thumb_{}", uuid));

        // 如果缩略图已存在则直接返回
        if thumb_path.exists() {
            return Ok(Bytes::from(std::fs::read(&thumb_path)?));
        }

        // 按需生成缩略图
        let file_path = dir.join(uuid);
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }

        let mime_type = mime_guess::from_path(&file_path).first_or_octet_stream();
        if mime_type.type_() == mime_guess::mime::IMAGE {
            if let Ok(data) = std::fs::read(&file_path) {
                if let Ok(img) = image::load_from_memory(&data) {
                    let thumb = img.thumbnail(200, 200);
                    if thumb.save(&thumb_path).is_ok() {
                        return Ok(Bytes::from(std::fs::read(&thumb_path)?));
                    }
                }
            }
        }

        Err(Error::InternalError("not an image or failed to generate thumbnail".to_string()))
    }
}
```

- [ ] **Step 3: 验证编译**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel-web 2>&1 | head -30
```

Expected: 编译成功（可能有一些 unused import 或 dead code 警告，但可以编译通过）。

- [ ] **Step 4: 提交**

```bash
git add kissbot-channel-web/Cargo.toml kissbot-channel-web/src/attachment.rs
git commit -m "refactor: AttachmentStore 新增 LRU 缓存和 metadata 持久化

- 添加 lru 和 uuid 依赖
- 重写 AttachmentStore，新增 get_meta 方法（LRU 缓存 + 文件回源）
- 新增 parse_key 辅助方法解析 group_id/UUID
- key 格式改为 {group_id}/{uuid}
- 文件路径改为 {base_path}/{group_id}/{uuid}

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: AttachmentStore 实现 AttachmentRegistry + 上传队列

**Files:**
- Modify: `kissbot-channel-web/src/attachment.rs`
- Modify: `kissbot-channel-web/src/lib.rs`（如果新增 pub re-export）

**Interfaces:**
- Consumes: `AttachmentRegistry` trait（来自 Task 1）
- Produces:
  ```rust
  impl AttachmentStore {
      /// 异步写入 chunk（通过 flume 队列串行处理）
      pub async fn write_chunk(&self, key: &str, pos: u64, size: u32, data: Bytes) -> Result<u64>;
      /// 发送下载 payload
      pub async fn send_download_payload(&self, key: &str, sender: &dyn AttachmentDownloadPayloadSender) -> Result<()>;
  }
  ```

- [ ] **Step 1: 在 AttachmentStore 中添加上传队列字段**

在 `attachment.rs` 的 `AttachmentStore` 结构体中添加：

```rust
use dashmap::DashMap;
use flume;
use tokio::sync::oneshot;
use async_trait::async_trait;
use kissbot_channel::{AttachmentDownloadPayloadSender, AttachmentRegistry};
use kissbot_api::channel::{AttachmentPayloadResponse, OFFSET_ATT_DATA};
use kissbot_api::message::AttachmentInfo;

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

修改 `AttachmentStore`：

```rust
pub struct AttachmentStore {
    base_path: PathBuf,
    meta_cache: Mutex<LruCache<String, Arc<AttachmentMeta>>>,
    /// 上传队列：key → (current_pos, sender)
    upload_channels: DashMap<String, (u64, flume::Sender<UploadCommand>)>,
}
```

构造函数新增 `upload_channels` 初始化：

```rust
pub fn new(base_path: &str) -> Self {
    Self {
        base_path: PathBuf::from(base_path),
        meta_cache: Mutex::new(LruCache::new(1024)),
        upload_channels: DashMap::new(),
    }
}
```

- [ ] **Step 2: 实现 write_chunk 方法**

```rust
impl AttachmentStore {
    /// 异步写入 chunk（通过 flume 队列串行处理）
    pub async fn write_chunk(&self, key: &str, pos: u64, size: u32, data: Bytes) -> Result<u64> {
        let tx = self.get_or_create_upload_channel(key).await;
        let (res_tx, res_rx) = oneshot::channel();

        tx.send(UploadCommand::Write {
            key: key.to_string(),
            pos,
            size,
            data,
            res: res_tx,
        }).map_err(|_| Error::InternalError("upload channel closed".to_string()))?;

        res_rx.await
            .map_err(|_| Error::InternalError("upload channel recv error".to_string()))?
            .map_err(|e| Error::InternalError(e))
    }

    async fn get_or_create_upload_channel(&self, key: &str) -> flume::Sender<UploadCommand> {
        if let Some(entry) = self.upload_channels.get(key) {
            return entry.value().1.clone();
        }

        let (tx, rx) = flume::unbounded::<UploadCommand>();
        let store_ref: *const AttachmentStore = self as *const AttachmentStore;
        let key_owned = key.to_string();
        let channels = self.upload_channels.clone();
        let base_path = self.base_path.clone();

        tokio::spawn(async move {
            let mut current_pos = 0u64;
            let store = unsafe { &*store_ref };

            while let Ok(cmd) = rx.recv_async().await {
                match cmd {
                    UploadCommand::Write { key, pos, size, data, res } => {
                        let result = Self::process_upload_write_inner(
                            &base_path, &key, &mut current_pos, pos, &data,
                        );

                        // 完成后清理 channel
                        if result.is_ok() {
                            channels.remove(&key);
                        }
                        let _ = res.send(result);
                    }
                }
            }
        });

        self.upload_channels.insert(key_owned, (0u64, tx.clone()));
        tx
    }

    /// 内部处理上传写入（同步执行，在 flume 异步任务中调用）
    fn process_upload_write_inner(
        base_path: &Path,
        key: &str,
        current_pos: &mut u64,
        pos: u64,
        data: &Bytes,
    ) -> std::result::Result<u64, String> {
        if pos < *current_pos {
            return Ok(*current_pos); // 已写入，幂等
        }
        if pos > *current_pos {
            return Err(format!("out of order: expected pos={}, got pos={}", *current_pos, pos));
        }

        let (group_id, uuid) = AttachmentStore::parse_key(key)
            .map_err(|e| e.to_string())?;
        let dir = base_path.join(group_id);
        let temp_path = dir.join(format!(".{}.uploading", uuid));
        let target_path = dir.join(uuid);

        // 追加写入临时文件
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&temp_path)
            .map_err(|e| e.to_string())?;
        file.write_all(data).map_err(|e| e.to_string())?;
        *current_pos = pos + data.len() as u64;

        // 从 metadata 获取 size_bytes 判断是否完成
        let meta_path = dir.join(format!("{}.metadata", uuid));
        if let Ok(content) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<AttachmentMeta>(&content) {
                if *current_pos >= meta.size_bytes {
                    // 写入完成，rename
                    std::fs::rename(&temp_path, &target_path).map_err(|e| e.to_string())?;

                    // 如果是图片则生成缩略图
                    if meta.mime_type.starts_with("image/") {
                        if let Ok(data) = std::fs::read(&target_path) {
                            if let Ok(img) = image::load_from_memory(&data) {
                                let thumb_path = dir.join(format!("thumb_{}", uuid));
                                let thumb = img.thumbnail(200, 200);
                                if thumb.save(&thumb_path).is_ok() {
                                    // 更新 metadata 中的 has_thumbnail
                                    let mut updated_meta = meta.clone();
                                    updated_meta.has_thumbnail = true;
                                    let _ = std::fs::write(&meta_path, serde_json::to_string(&updated_meta).unwrap());
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(*current_pos)
    }
}
```

- [ ] **Step 3: 实现 AttachmentRegistry for AttachmentStore**

```rust
use kissbot_channel::AttachmentRegistry;

#[async_trait]
impl AttachmentRegistry for AttachmentStore {
    async fn register(&self, _messenger_id: &str, _user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> Result<Arc<String>> {
        let uuid = uuid::Uuid::new_v4().to_string();
        let key = Arc::new(format!("{}/{}", group_id, uuid));

        // 1. 创建 group 目录
        let dir = self.base_path.join(group_id);
        std::fs::create_dir_all(&dir).map_err(|e| Error::IoError(e))?;

        // 2. 创建空临时文件（防止 upload 在 register 之前到达）
        let temp_path = dir.join(format!(".{}.uploading", uuid));
        std::fs::write(&temp_path, &[]).map_err(|e| Error::IoError(e))?;

        // 3. 写 metadata 文件
        let meta = AttachmentMeta {
            file_name: info.file_name.clone(),
            mime_type: info.mime_type.clone(),
            size_bytes: info.size_bytes,
            has_thumbnail: false,
        };
        let meta_path = dir.join(format!("{}.metadata", uuid));
        std::fs::write(&meta_path, serde_json::to_string(&meta).map_err(|e| Error::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))?)
            .map_err(|e| Error::IoError(e))?;

        // 4. 插入 LRU 缓存
        {
            let mut cache = self.meta_cache.lock().unwrap();
            cache.put(uuid, Arc::new(meta));
        }

        Ok(key)
    }
}
```

注意：`AttachmentRegistry::register` 在 `kissbot-channel` crate 中定义，返回 `crate::error::Result`（即 `kissbot_channel::Result`），但 `AttachmentStore` 使用 `crate::error::Result`（即 `kissbot_channel_web::Result`）。需要确保类型兼容。观察 `kissbot_channel_web::Error` 已经实现了 `From<kissbot_channel::Error>`（error.rs 中），所以通过 `kissbot_channel_web::Result` 实现 `AttachmentRegistry` 会返回类型不匹配。

需要让 register 返回 `kissbot_channel::Result<Arc<String>>`。在 impl 块中指定返回类型为 `std::result::Result<Arc<String>, kissbot_channel::Error>`，内部用 `map_err(|e| kissbot_channel::Error::IoError(e))` 转换。或者更好：直接让 AttachmentStore 的方法使用 `kissbot_channel::Error`。

推荐方案：AttachmentStore 内部方法使用 `kissbot_channel_web::Error`，但 `register` impl 转为 `kissbot_channel::Error`：

```rust
#[async_trait]
impl AttachmentRegistry for AttachmentStore {
    async fn register(&self, _messenger_id: &str, _user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> std::result::Result<Arc<String>, kissbot_channel::Error> {
        let uuid = uuid::Uuid::new_v4().to_string();
        // ...
        Ok(key)
    }
}
```

- [ ] **Step 4: 实现 send_download_payload 方法**

```rust
impl AttachmentStore {
    /// 发送下载 payload（内部按 CHUNK_SIZE 分块读取并调用 sender）
    pub async fn send_download_payload(&self, key: &str, sender: &dyn AttachmentDownloadPayloadSender) -> Result<()> {
        const CHUNK_SIZE: u64 = 65536;

        let (mut file, file_len) = self.open_file(key)?;

        let mut pos = 0u64;
        let mut ok = true;
        while pos < file_len && ok {
            let end = std::cmp::min(pos + CHUNK_SIZE, file_len);
            let chunk_size = (end - pos) as usize;
            let (sn, mut buf) = sender.prepare_send(key, chunk_size as u32, pos)
                .map_err(|e| Error::InternalError(e.to_string()))?;
            // 读取到 payload 偏移处
            use std::io::Read;
            if let Err(e) = (&mut file).read_exact(&mut buf[OFFSET_ATT_DATA..OFFSET_ATT_DATA + chunk_size]) {
                return Err(Error::InternalError(format!("Failed to read file chunk: {}", e)));
            }
            ok = sender.send(sn, key, chunk_size as u32, pos, buf).await.is_ok();
            pos = end;
        }
        // 发送 size=0 的结束标记
        if let Ok((sn, buf)) = sender.prepare_send(key, 0, pos) {
            let _ = sender.send(sn, key, 0, pos, buf).await;
        }

        Ok(())
    }
}
```

- [ ] **Step 5: 验证编译**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel-web 2>&1 | head -60
```

Expected: 编译成功。

- [ ] **Step 6: 提交**

```bash
git add kissbot-channel-web/src/attachment.rs
git commit -m "refactor: AttachmentStore 实现 AttachmentRegistry 和上传队列

- AttachmentStore 新增 upload_channels 管理上传队列
- 实现 write_chunk 方法（异步，flume 队列串行处理）
- 实现 send_download_payload 方法（分块读取下载）
- 实现 AttachmentRegistry trait（生成 UUID key + 写 metadata + LRU 缓存）
- 上传完成后自动生成缩略图

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: WebMessenger 移除附件状态，透传 AttachmentStore

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs`

**Interfaces:**
- Consumes: `AttachmentStore`（已实现 `AttachmentRegistry` 和上传/下载方法）
- Produces: 清理后的 `WebMessenger`

- [ ] **Step 1: 删除 WebMessenger 中的附件相关字段**

从 `WebMessenger` 结构体中删除：
- `pub pending_uploads: DashMap<String, PendingAttachment>`
- `pub upload_channels: Arc<DashMap<String, flume::Sender<UploadCommand>>>`
- `pending_registrations: tokio::sync::Mutex<Vec<(Arc<String>, Arc<AttachmentInfo>)>>`

同时删除 `PendingAttachment` 结构体定义、`UploadCommand` 枚举定义。

保留 `pub attachment_store: Arc<AttachmentStore>`。

- [ ] **Step 2: 删除无用的 use**

删除不再使用的导入：
- `use std::path::PathBuf;`（如果不再需要）
- `use bytes::Bytes;`（保留，Messenger trait 仍需要）
- `use tokio::sync::oneshot;`（如果不再需要）

删除 `use kissbot_channel::AttachmentRegistry;`（WebMessenger 不再实现）

- [ ] **Step 3: 修改 WebMessenger::new 构造函数**

删除 `pending_uploads`、`upload_channels`、`pending_registrations` 的初始化：

```rust
pub fn new(
    messenger_id: Arc<String>,
    repo_path: PathBuf,
    config: Arc<RwLock<WebMessengerRepo>>,
    on_group_change: Weak<dyn GroupChangeHandler>,
    on_incoming_messages: Weak<dyn IncomingMessageHandler>,
    on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
    on_user_remove: Weak<dyn UserRemoveHandler>,
    attachment_dir: &str,
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
    }
}
```

- [ ] **Step 4: 修改 send() 方法**

删除 `pending_registrations` 相关代码块：

```rust
// 删除这段代码：
// 为每个 key 创建临时文件（AttachmentRegistry::register 已填充 pending_registrations）
for (key, info) in self.pending_registrations.lock().await.drain(..) {
    let (temp_path, target_path) = match self.attachment_store.create_temp_file(
        outgoing.group_id.as_str(), msg_id.as_str(), info.file_name.as_str()
    ) {
        Ok(paths) => paths,
        Err(e) => return Err(Error::from(e)),
    };
    self.pending_uploads.insert(key.to_string(), PendingAttachment { ... });
}
```

将 `process_attachment_message` 调用的 registry 参数从 `self` 改为 `&*self.attachment_store`：

```rust
let response = kissbot_channel::process_attachment_message(
    outgoing.clone(),
    &*self.attachment_store,
).await.map_err(|e| Error::InternalError(e.to_string()))?;
```

- [ ] **Step 5: 删除 AttachmentRegistry impl for WebMessenger**

删除整个 `impl AttachmentRegistry for WebMessenger` 块。

- [ ] **Step 6: 删除 write_attachment_chunk 和 get_or_create_upload_channel 方法**

删除 `write_attachment_chunk`、`get_or_create_upload_channel`、`process_upload_write` 方法。这些功能已由 `AttachmentStore::write_chunk` 替代。

- [ ] **Step 7: 修改 Messenger trait 实现**

`send_attachment_payload` 改为委托给 `attachment_store.write_chunk`：

```rust
async fn send_attachment_payload(&self, key: &str, size: u32, pos: u64, data: Bytes) -> std::result::Result<AttachmentPayloadResponse, kissbot_channel::Error> {
    self.attachment_store.write_chunk(key, pos, size, data).await
        .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;
    Ok(AttachmentPayloadResponse {
        key: Arc::new(key.to_string()),
        pos,
        size,
        error_code: 0,
        error_msg: None,
    })
}
```

`download_attachment_header` 改为委托给 `attachment_store.get_meta`：

```rust
async fn download_attachment_header(&self, request: AttachmentDownloadRequest, _sn: Arc<AtomicU32>) -> std::result::Result<Arc<AttachmentInfoResponse>, kissbot_channel::Error> {
    let meta = self.attachment_store.get_meta(request.key.as_str())
        .map_err(|e| kissbot_channel::Error::AttachmentNotFound(e.to_string()))?;
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
```

`start_send_download_attachment_payload` 改为委托给 `attachment_store.send_download_payload`：

```rust
async fn start_send_download_attachment_payload(&self, key: &str) -> std::result::Result<(), kissbot_channel::Error> {
    let sender = self.on_download_attachment_payload.upgrade()
        .ok_or_else(|| kissbot_channel::Error::InternalError("download payload sender unavailable".to_string()))?;
    let store = self.attachment_store.clone();
    let key_owned = key.to_string();

    tokio::spawn(async move {
        if let Err(e) = store.send_download_payload(&key_owned, &*sender).await {
            tracing::error!("Failed to send download payload: {}", e);
        }
    });

    Ok(())
}
```

- [ ] **Step 8: 验证编译**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel-web 2>&1 | head -40
```

Expected: 编译成功。

- [ ] **Step 9: 运行测试**

```bash
cd /home/admin/project/kissbot && cargo test -p kissbot-api 2>&1 | tail -10
```

Expected: 测试全部通过。

- [ ] **Step 10: 提交**

```bash
git add kissbot-channel-web/src/messenger.rs
git commit -m "refactor: WebMessenger 移除附件状态，透传 AttachmentStore

- 删除 pending_uploads、upload_channels、pending_registrations 字段
- 删除 PendingAttachment、UploadCommand 定义
- 删除 AttachmentRegistry impl for WebMessenger
- send() 中 registry 参数改为 &*self.attachment_store
- Messenger trait 方法委托给 attachment_store
- 删除 write_attachment_chunk、get_or_create_upload_channel 方法

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: HTTP handler 适配新 AttachmentStore

**Files:**
- Modify: `kissbot-channel-web/src/http.rs`

- [ ] **Step 1: handle_init_attachment 改为直接返回 OutgoingMessageResponse**

```rust
/// POST /api/attachment/init — 初始化附件上传，发送消息并返回 OutgoingMessageResponse
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
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}
```

注意：`ApiResponse::success(resp)` 中 resp 的类型是 `Arc<OutgoingMessageResponse>`。`ApiResponse` 的 `success` 方法需要一个 `Serialize` 类型，`Arc<OutgoingMessageResponse>` 实现了 `Serialize`（通过 derive），所以可以直接用。

- [ ] **Step 2: handle_upload_attachment 改为直接委托 AttachmentStore**

```rust
/// POST /api/attachment/upload — 第二步：上传文件实体
async fn handle_upload_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut attachment_key: Option<String> = None;
    let mut file_data: Option<Bytes> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().map(|s| s.to_string());
        if name.as_deref() == Some("key") {
            if let Ok(data) = field.text().await {
                attachment_key = Some(data.trim().to_string());
            }
        } else if name.as_deref() == Some("file") {
            if let Ok(data) = field.bytes().await {
                file_data = Some(data);
            }
        }
    }

    let attachment_key = match attachment_key {
        Some(k) => k,
        None => return Json(ApiResponse::<serde_json::Value>::error("Missing key".to_string())),
    };

    let file_data = match file_data {
        Some(d) => d,
        None => return Json(ApiResponse::<serde_json::Value>::error("Missing file data".to_string())),
    };

    // 通过 AttachmentStore 写入
    match messenger.attachment_store.write_chunk(&attachment_key, 0, file_data.len() as u32, file_data).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}
```

- [ ] **Step 3: handle_download_attachment 改为使用 AttachmentStore::get_meta**

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

    // 获取文件元数据（走 LRU 缓存）
    let file_len = match messenger.attachment_store.open_file(key) {
        Ok((_, len)) => len,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let meta = match messenger.attachment_store.get_meta(key) {
        Ok(m) => m,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let mime = mime_guess::from_path(meta.file_name.as_str()).first_or_octet_stream().to_string();

    // Range header 解析（不变）
    // ...
}
```

其余代码（Range 解析、读文件、返回）保持不变。

- [ ] **Step 4: handle_thumbnail 改为使用 AttachmentStore::get_thumbnail**

```rust
/// GET /api/attachment/thumbnail
async fn handle_thumbnail(
    State(messenger): State<Arc<WebMessenger>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let key = match params.get("key") {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, "Missing key").into_response(),
    };

    match messenger.attachment_store.get_thumbnail(key) {
        Ok(data) => {
            ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], data).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}
```

- [ ] **Step 5: 清理不再需要的 use**

删除 `use std::collections::HashMap;` 之外的 std 导入。检查是否还需要 `PendingAttachment` 相关引用（不再需要）。删除 `use crate::messenger::PendingAttachment;`（如果存在）。

注意保留：
- `use kissbot_api::channel::OutgoingMessage;`
- `use kissbot_api::message::{AttachmentInfo, Content, MessageItem, MSG_TYPE_ATTACHMENT, MSG_TYPE_MULTI, MSG_TYPE_TEXT};`
- `use bytes::Bytes;`
- `use serde_json::Value;`

- [ ] **Step 6: 验证编译**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel-web 2>&1 | head -40
```

Expected: 编译成功。

- [ ] **Step 7: 运行测试**

```bash
cd /home/admin/project/kissbot && cargo test 2>&1 | tail -20
```

Expected: 测试全部通过。

- [ ] **Step 8: 提交**

```bash
git add kissbot-channel-web/src/http.rs
git commit -m "refactor: HTTP handler 适配新 AttachmentStore

- handle_init_attachment 返回完整 OutgoingMessageResponse
- handle_upload_attachment 直接委托 attachment_store.write_chunk
- handle_download_attachment 使用 get_meta 替换 get_meta_by_key
- handle_thumbnail 使用 get_thumbnail

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: 验证完整编译和测试

- [ ] **Step 1: 完整编译**

```bash
cd /home/admin/project/kissbot && cargo build 2>&1
```

Expected: 无错误。

- [ ] **Step 2: 运行所有测试**

```bash
cd /home/admin/project/kissbot && cargo test 2>&1 | tail -30
```

Expected: 所有测试通过。

- [ ] **Step 3: 检查 clippy（可选）**

```bash
cd /home/admin/project/kissbot && cargo clippy -p kissbot-channel-web -p kissbot-channel -p kissbot-api 2>&1 | tail -20
```

Expected: 无 warnings（或少量合理 warning）。

- [ ] **Step 4: 最终提交（如果有改动）**

```bash
git add -A && git commit -m "chore: 编译和测试验证通过

Co-Authored-By: Claude <noreply@anthropic.com>"
```
