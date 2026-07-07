# Transfer ID 附件传输重构 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在附件上传下载中统一使用 transfer_id，由 AttachmentRegistry 生成，嵌入 AttachmentInfoResponse，清理 Messenger trait 中的 attachment_sn 参数

**Architecture:** 分 7 个任务依次递进：kissbot-api 数据结构 → kissbot-channel trait → AttachmentStore → WebMessenger → ChannelManager → HTTP handler → 编译验证。各任务有严格依赖关系。

**Tech Stack:** Rust, async_trait, flume, DashMap, AtomicU32

## Global Constraints

- `AttachmentInfoResponse` 必须带 `transfer_id: u32` 字段
- `AttachmentPayloadResponse` 必须带 `transfer_id: u32` 字段
- 删除 `WsOutgoingMessageResponse`、`WsAttachmentDownloadResponseHeader`、`WsAttachmentPayloadResponse`
- `AttachmentRegistry` 新增 `async fn gen_transfer_id(&self, key: &str) -> u32`
- `process_attachment_message` 生成 `AttachmentInfoResponse` 时调用 `gen_transfer_id`
- `Messenger` trait：`send_message` 删除 `attachment_sn`；`download_attachment_header` 删除 `attachment_sn`；`send_attachment_payload` 增加 `transfer_id: u32`；`start_send_download_attachment_payload` 增加 `transfer_id: u32`
- `MessengerCreator::create` 删除 `global_attachment_sn: Arc<AtomicU32>`
- `AttachmentDownloadPayloadSender` 方法签名增加 `transfer_id: u32` 
- `AttachmentStore` 新增 `transfer_id_seq: AtomicU32`；`upload_channels` 主键由 `String` 改为 `u32`；所有方法和 `UploadCommand` 对应增加 transfer_id
- `attachment_sender_map` 类型由 `DashMap<String, (u32, Weak<ConnectContext>)>` 改为 `DashMap<u32, Weak<ConnectContext>>`

---

### Task 1: API 数据结构变更

**Files:**
- Modify: `kissbot-api/src/message.rs` — `AttachmentInfoResponse` 新增 `transfer_id`
- Modify: `kissbot-api/src/channel.rs` — `AttachmentPayloadResponse` 新增 `transfer_id`；删除 3 个 Ws 结构体；更新测试
- Test: `kissbot-api/src/message.rs` 中 `test_serde_content_attachment_info_response` 增加 transfer_id
- Test: `kissbot-api/src/channel.rs` 中 `test_serde_attachment_download_response_header` 增加 transfer_id

**Interfaces:**
- Produces: 新 `AttachmentInfoResponse { key, info, transfer_id }`
- Produces: 新 `AttachmentPayloadResponse { key, transfer_id, pos, size, error_code, error_msg }`
- Produces: 删除 `WsOutgoingMessageResponse`、`WsAttachmentDownloadResponseHeader`、`WsAttachmentPayloadResponse`

- [ ] **Step 1: 修改 AttachmentInfoResponse**

```rust
// kissbot-api/src/message.rs 第 65-69 行
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfoResponse {
    pub key: Arc<String>,
    pub info: Arc<AttachmentInfo>,
    pub transfer_id: u32,
}
```

- [ ] **Step 2: 修改 AttachmentPayloadResponse 并删除 Ws 结构体**

```rust
// kissbot-api/src/channel.rs
// 1. AttachmentPayloadResponse 新增 transfer_id：
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentPayloadResponse {
    pub key: Arc<String>,
    pub transfer_id: u32,
    pub pos: u64,
    pub size: u32,
    pub error_code: u32,
    pub error_msg: Option<Arc<String>>,
}

// 2. 删除以下三个结构体定义（第 73-101 行）：
// WsOutgoingMessageResponse { response, attachment_upload_id_map }
// WsAttachmentDownloadResponseHeader { download_id, response }
// WsAttachmentPayloadResponse { id, response }

// 3. 删除未使用的 use：Arc<DashMap>（如果不再使用，保留其它 use）
// 4. 删除 attachment_upload_id_map 字段（WsOutgoingMessageResponse 中）
```

- [ ] **Step 3: 更新测试**

```rust
// kissbot-api/src/channel.rs 第 325 行，test_serde_attachment_download_response_header
let response = Arc::new(AttachmentInfoResponse {
    key: Arc::new("g1/msg1/doc.pdf".to_string()),
    info: metadata,
    transfer_id: 42,  // 新增
});
// assert 补充 transfer_id
assert_eq!(deserialized.transfer_id, 42);
```

```rust
// kissbot-api/src/message.rs 第 134 行，test_serde_content_attachment_info_response
let content = Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
    key: Arc::new("g1/msg1/photo.png".to_string()),
    info,
    transfer_id: 42,  // 新增
}));
```

- [ ] **Step 4: 编译通过**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-api 2>&1 | head -20
```

Expected: 编译成功。

- [ ] **Step 5: 测试通过**

```bash
cd /home/admin/project/kissbot && cargo test -p kissbot-api 2>&1 | tail -10
```

Expected: 测试全部通过。

- [ ] **Step 6: 提交**

```bash
cd /home/admin/project/kissbot && git add -A && git commit -m "refactor: API 数据结构新增 transfer_id，删除 Ws 包装结构体

- AttachmentInfoResponse 新增 transfer_id 字段
- AttachmentPayloadResponse 新增 transfer_id 字段
- 删除 WsOutgoingMessageResponse、WsAttachmentDownloadResponseHeader、WsAttachmentPayloadResponse
- 更新对应测试

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Trait 接口变更

**Files:**
- Modify: `kissbot-channel/src/attachment.rs` — `AttachmentRegistry` 加 `gen_transfer_id`；`process_attachment_message` 调用它
- Modify: `kissbot-channel/src/messenger.rs` — `Messenger` trait、`MessengerCreator` trait 签名变更
- Modify: `kissbot-channel/src/data.rs` — `AttachmentDownloadPayloadSender` 签名变更

**Interfaces:**
- Consumes: `AttachmentInfoResponse` (带 transfer_id)、`AttachmentPayloadResponse` (带 transfer_id) from Task 1
- Produces: 新 `AttachmentRegistry` trait；新 `Messenger` trait；新 `MessengerCreator` trait；新 `AttachmentDownloadPayloadSender` trait

- [ ] **Step 1: 修改 AttachmentRegistry 和 process_attachment_message**

`kissbot-channel/src/attachment.rs`：

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use kissbot_api::channel::{OutgoingMessage, OutgoingMessageResponse};
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content, MessageItem};

use crate::error::Result;

#[async_trait]
pub trait AttachmentRegistry: Send + Sync {
    async fn register(&self, messenger_id: &str, user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> Result<Arc<String>>;
    async fn gen_transfer_id(&self, key: &str) -> u32;
}

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
        msg_id: Arc::new(String::new()),
        time: Arc::new(String::new()),
        msg_type: outgoing.msg_type.clone(),
        content: new_content.clone(),
    }))
}

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
            let transfer_id = registry.gen_transfer_id(key.as_str()).await?;
            Ok(Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
                key,
                info: info.clone(),
                transfer_id,
            })))
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

注意：当前的 `process_content` 中 `Content::AttachmentInfo` 分支没有使用 `messenger_id` 和 `user_id`，但它们在 trait 签名中保留供其他 Messenger 实现使用。

- [ ] **Step 2: 修改 Messenger trait 和 MessengerCreator trait**

`kissbot-channel/src/messenger.rs`：

```rust
use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::{channel::*, message::*};

use crate::error::Result;
use crate::data::*;
use std::sync::{Arc, Weak};

#[async_trait]
pub trait Messenger: Send + Sync + 'static {
    async fn get_info(&self) -> Result<Arc<MessengerInfo>>;
    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>>;
    async fn send_attachment_payload(&self, key: &str, transfer_id: u32, size: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse>;
    async fn download_attachment_header(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>>;
    async fn start_send_download_attachment_payload(&self, key: &str, transfer_id: u32) -> Result<()>;
}

#[async_trait]
pub trait MessengerCreator<M: Messenger> {
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
    ) -> Result<Arc<M>>;
}
```

删除 `use std::sync::atomic::AtomicU32` 和 `AttachmentRegistry` 导入（如果不再使用）。

- [ ] **Step 3: 修改 AttachmentDownloadPayloadSender**

`kissbot-channel/src/data.rs`：

```rust
#[async_trait]
pub trait AttachmentDownloadPayloadSender: Send + Sync {
    fn prepare_send(&self, key: &str, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)>;
    async fn send(&self, sn: u32, key: &str, transfer_id: u32, size: u32, pos: u64, buf: BytesMut) -> Result<AttachmentPayloadResponse>;
}
```

- [ ] **Step 4: 验证 kissbot-channel 编译**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel 2>&1 | head -30
```

Expected: 编译成功（这个 crate 不包含实现，只有 trait 定义，不应有运行时错误）。

- [ ] **Step 5: 提交（不包含 Task 1 的改动）**

```bash
cd /home/admin/project/kissbot && git add -A && git commit -m "refactor: Trait 接口变更——新增 gen_transfer_id、Messenger/Creator 签名变更

- AttachmentRegistry 新增 async fn gen_transfer_id
- process_attachment_message 调用 gen_transfer_id 写入 AttachmentInfoResponse
- Messenger trait：send_message/download_attachment_header 删除 attachment_sn
- Messenger trait：send_attachment_payload/start_send_download_attachment_payload 增加 transfer_id
- MessengerCreator::create 删除 global_attachment_sn
- AttachmentDownloadPayloadSender 方法签名增加 transfer_id

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: AttachmentStore 实现适配

**Files:**
- Modify: `kissbot-channel-web/src/attachment.rs`

**Interfaces:**
- Consumes: `AttachmentRegistry` (加 gen_transfer_id) from Task 2；`Messenger` trait 新签名 (send_attachment_payload 带 transfer_id) from Task 2
- Produces: `AttachmentStore` 新方法签名和内部结构

- [ ] **Step 1: 修改 AttachmentStore 结构体和构造函数**

```rust
use std::sync::atomic::{AtomicU32, Ordering};
// (保留其他 use 不变)

pub struct AttachmentStore {
    base_path: PathBuf,
    meta_cache: Mutex<LruCache<String, Arc<AttachmentMeta>>>,
    /// 上传队列：transfer_id → (key, current_pos, sender)
    upload_channels: DashMap<u32, (Arc<String>, u64, flume::Sender<UploadCommand>)>,
    transfer_id_seq: AtomicU32,
}

impl AttachmentStore {
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: PathBuf::from(base_path),
            meta_cache: Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1024).unwrap())),
            upload_channels: DashMap::new(),
            transfer_id_seq: AtomicU32::new(0),
        }
    }

    pub fn next_transfer_id(&self) -> u32 {
        self.transfer_id_seq.fetch_add(1, Ordering::SeqCst)
    }
    // ... 保留 parse_key, get_meta, open_file, get_thumbnail 不变 ...
```

- [ ] **Step 2: 修改 UploadCommand 和 write_chunk**

```rust
enum UploadCommand {
    Write {
        transfer_id: u32,
        key: String,
        pos: u64,
        size: u32,
        data: Bytes,
        res: oneshot::Sender<std::result::Result<u64, String>>,
    },
}

pub async fn write_chunk(&self, key: &str, transfer_id: u32, pos: u64, size: u32, data: Bytes) -> Result<u64> {
    let tx = self.get_or_create_upload_channel(transfer_id, Arc::new(key.to_string()));
    let (res_tx, res_rx) = oneshot::channel();

    tx.send(UploadCommand::Write {
        transfer_id,
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
```

- [ ] **Step 3: 修改 get_or_create_upload_channel**

```rust
fn get_or_create_upload_channel(&self, transfer_id: u32, key: Arc<String>) -> flume::Sender<UploadCommand> {
    if let Some(entry) = self.upload_channels.get(&transfer_id) {
        return entry.value().2.clone();
    }

    let (tx, rx) = flume::unbounded::<UploadCommand>();
    let channels = self.upload_channels.clone();
    let base_path = self.base_path.clone();

    tokio::spawn(async move {
        let mut current_pos = 0u64;

        while let Ok(cmd) = rx.recv_async().await {
            match cmd {
                UploadCommand::Write { transfer_id, key, pos, size: _, data, res } => {
                    let result = Self::process_upload_write_inner(
                        &base_path, &key, &mut current_pos, pos, &data,
                    );

                    // 完成后清理 channel
                    if result.is_ok() {
                        channels.remove(&transfer_id);
                    }
                    let _ = res.send(result);
                }
            }
        }
    });

    self.upload_channels.insert(transfer_id, (key, 0u64, tx.clone()));
    tx
}
```

注意：`process_upload_write_inner` 函数本身不需要改变（它只操作文件系统，不关心 transfer_id）。

- [ ] **Step 4: 修改 send_download_payload**

```rust
pub async fn send_download_payload(&self, key: &str, transfer_id: u32, sender: &dyn kissbot_channel::AttachmentDownloadPayloadSender) -> Result<()> {
    use kissbot_api::channel::OFFSET_ATT_DATA;
    const CHUNK_SIZE: u64 = 65536;

    let (mut file, file_len) = self.open_file(key)?;

    let mut pos = 0u64;
    let mut ok = true;
    while pos < file_len && ok {
        let end = std::cmp::min(pos + CHUNK_SIZE, file_len);
        let chunk_size = (end - pos) as usize;
        let (sn, mut buf) = sender.prepare_send(key, transfer_id, chunk_size as u32, pos)
            .map_err(|e| Error::InternalError(e.to_string()))?;
        use std::io::Read;
        if let Err(e) = (&mut file).read_exact(&mut buf[OFFSET_ATT_DATA..OFFSET_ATT_DATA + chunk_size]) {
            return Err(Error::InternalError(format!("Failed to read file chunk: {}", e)));
        }
        ok = sender.send(sn, key, transfer_id, chunk_size as u32, pos, buf).await.is_ok();
        pos = end;
    }
    // 发送 size=0 的结束标记
    if let Ok((sn, buf)) = sender.prepare_send(key, transfer_id, 0, pos) {
        let _ = sender.send(sn, key, transfer_id, 0, pos, buf).await;
    }

    Ok(())
}
```

- [ ] **Step 5: 修改 AttachmentRegistry 实现，新增 gen_transfer_id**

```rust
#[async_trait]
impl kissbot_channel::AttachmentRegistry for AttachmentStore {
    async fn register(&self, _messenger_id: &str, _user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> std::result::Result<Arc<String>, kissbot_channel::Error> {
        // ... 保持原有实现不变 ...
    }

    async fn gen_transfer_id(&self, _key: &str) -> u32 {
        self.next_transfer_id()
    }
}
```

- [ ] **Step 6: 编译测试**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel-web 2>&1 | head -30
```

Expected: 编译成功。

- [ ] **Step 7: 提交**

```bash
cd /home/admin/project/kissbot && git add -A && git commit -m "refactor: AttachmentStore 适配 transfer_id

- 新增 transfer_id_seq 全局自增字段
- upload_channels 主键由 String(key) 改为 u32(transfer_id)
- UploadCommand、write_chunk、get_or_create_upload_channel 适配 transfer_id
- send_download_payload 增加 transfer_id 参数
- 实现 AttachmentRegistry::gen_transfer_id

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: WebMessenger 适配新 Messenger trait

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs`

**Interfaces:**
- Consumes: 新 `Messenger`/`MessengerCreator` trait from Task 2；`AttachmentStore` (next_transfer_id) from Task 3
- Produces: `impl Messenger for WebMessenger` (新签名)

- [ ] **Step 1: 修改 MessengerCreator::create**

删除 `_global_attachment_sn: Arc<AtomicU32>` 参数：

```rust
#[async_trait]
impl MessengerCreator<WebMessenger> for WebMessengerCreator {
    async fn create(
        &self,
        on_group_change: Weak<dyn GroupChangeHandler>,
        on_incoming_messages: Weak<dyn IncomingMessageHandler>,
        on_download_attachment_payload: Weak<dyn AttachmentDownloadPayloadSender>,
        on_user_remove: Weak<dyn UserRemoveHandler>,
    ) -> std::result::Result<Arc<WebMessenger>, kissbot_channel::Error> {
        // ... 删除 _global_attachment_sn，其余不变 ...
    }
}
```

- [ ] **Step 2: 修改 send_message**

```rust
async fn send_message(&self, message: OutgoingMessage) -> std::result::Result<Arc<OutgoingMessageResponse>, kissbot_channel::Error> {
    Ok(self.send(Arc::new(message)).await?)
}
```

删除 `_attachment_sn: Arc<AtomicU32>` 参数。

- [ ] **Step 3: 修改 send_attachment_payload**

```rust
async fn send_attachment_payload(&self, key: &str, transfer_id: u32, size: u32, pos: u64, data: Bytes) -> std::result::Result<AttachmentPayloadResponse, kissbot_channel::Error> {
    self.attachment_store.write_chunk(key, transfer_id, pos, size, data).await
        .map_err(|e| kissbot_channel::Error::InternalError(e.to_string()))?;
    Ok(AttachmentPayloadResponse {
        key: Arc::new(key.to_string()),
        transfer_id,  // 新增
        pos,
        size,
        error_code: 0,
        error_msg: None,
    })
}
```

- [ ] **Step 4: 修改 download_attachment_header**

```rust
async fn download_attachment_header(&self, request: AttachmentDownloadRequest) -> std::result::Result<Arc<AttachmentInfoResponse>, kissbot_channel::Error> {
    let meta = self.attachment_store.get_meta(request.key.as_str())
        .map_err(|e| kissbot_channel::Error::AttachmentNotFound(e.to_string()))?;
    let info = AttachmentInfo {
        file_name: meta.file_name.clone(),
        mime_type: meta.mime_type.clone(),
        size_bytes: meta.size_bytes,
    };
    let transfer_id = self.attachment_store.next_transfer_id();

    Ok(Arc::new(AttachmentInfoResponse {
        key: Arc::clone(&request.key),
        info: Arc::new(info),
        transfer_id,
    }))
}
```

删除 `_attachment_sn: Arc<AtomicU32>` 参数。

- [ ] **Step 5: 修改 start_send_download_attachment_payload**

```rust
async fn start_send_download_attachment_payload(&self, key: &str, transfer_id: u32) -> std::result::Result<(), kissbot_channel::Error> {
    let sender = self.on_download_attachment_payload.upgrade()
        .ok_or_else(|| kissbot_channel::Error::InternalError("download payload sender unavailable".to_string()))?;
    let store = self.attachment_store.clone();
    let key_owned = key.to_string();

    tokio::spawn(async move {
        if let Err(e) = store.send_download_payload(&key_owned, transfer_id, &*sender).await {
            tracing::error!("Failed to send download payload: {}", e);
        }
    });

    Ok(())
}
```

- [ ] **Step 6: 清理不再使用的模块导入**

检查并删除不再需要的 `use std::sync::atomic::AtomicU32`（如果不再使用）。

- [ ] **Step 7: 编译**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel-web 2>&1 | head -30
```

Expected: 编译成功。

- [ ] **Step 8: 提交**

```bash
cd /home/admin/project/kissbot && git add -A && git commit -m "refactor: WebMessenger 适配新 Messenger trait 签名

- WebMessengerCreator::create 删除 global_attachment_sn 参数
- send_message 删除 attachment_sn 参数
- send_attachment_payload 新增 transfer_id 参数
- download_attachment_header 删除 attachment_sn，内部调用 next_transfer_id
- start_send_download_attachment_payload 新增 transfer_id 参数
- 清理不再使用的模块导入

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: ChannelManager 适配 transfer_id

**Files:**
- Modify: `kissbot-channel/src/channel_manager.rs`

**Interfaces:**
- Consumes: 新 `Messenger`/`MessengerCreator` trait from Task 2；新 `AttachmentDownloadPayloadSender` trait from Task 2

- [ ] **Step 1: 修改结构体字段**

`attachment_sender_map` 类型改为 `DashMap<u32, Weak<ConnectContext>>`：

```rust
pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
    messenger_map: DashMap<String, Arc<MessengerContext>>,
    memory_store_client: Arc<MemoryStoreClient>,
    // 删除 global_attachment_sn
    // 上传方向：transfer_id → (key, Weak<Messenger>)
    attachment_receiver_map: DashMap<u32, (String, Weak<dyn Messenger>)>,
    // 下载方向：transfer_id → Weak<ConnectContext>
    attachment_sender_map: DashMap<u32, Weak<ConnectContext>>,
}
```

- [ ] **Step 2: 修改构造函数**

```rust
impl ChannelManager {
    pub fn new() -> Self {
        Self {
            global_connect_id: AtomicU32::new(0),
            connect_map: DashMap::new(),
            messenger_map: DashMap::new(),
            memory_store_client: Arc::new(MemoryStoreClient::new()),
            // 删除 global_attachment_sn: Arc::new(AtomicU32::new(0)),
            attachment_receiver_map: DashMap::new(),
            attachment_sender_map: DashMap::new(),
        }
    }
```

- [ ] **Step 3: 修改 register_messenger**

删除 `self.global_attachment_sn.clone()` 参数：

```rust
pub async fn register_messenger<M, MC>(self: &Arc<Self>, messenger_id: &str, messenger_creator: MC) -> Result<Arc<M>>
where
    M: Messenger,
    MC: MessengerCreator<M>
{
    match self.messenger_map.entry(messenger_id.to_string()) {
        Entry::Vacant(entry) => {
            let group_change_handler = Arc::downgrade(self);
            let incoming_messages_handler = Arc::downgrade(self);
            let download_attachment_payload_handler = Arc::downgrade(self);
            let user_remove_handler = Arc::downgrade(self);
            let messenger = messenger_creator.create(
                group_change_handler,
                incoming_messages_handler,
                download_attachment_payload_handler,
                user_remove_handler,
                // 删除 self.global_attachment_sn.clone(),
            ).await?;
            // ...
        }
        // ...
    }
}
```

- [ ] **Step 4: 修改 OutgoingMessageProcessor**

删除 `collect_attachment_keys` 函数。改为遍历 content 注册已有 transfer_id：

```rust
// 替换 collect_attachment_keys（删除整个函数）
// 新增遍历注册函数：
fn register_attachment_receivers(
    content: &Content,
    manager: &ChannelManager,
    messenger_weak: Weak<dyn Messenger>,
    upload_id_map: &DashMap<String, u32>,
) {
    match content {
        Content::AttachmentInfoResponse(resp) => {
            let id = resp.transfer_id;
            manager.attachment_receiver_map.insert(id, (resp.key.to_string(), messenger_weak.clone()));
            upload_id_map.insert(resp.key.to_string(), id);
        }
        Content::Multi(items) => {
            for item in items.iter() {
                register_attachment_receivers(&item.content, manager, messenger_weak.clone(), upload_id_map);
            }
        }
        _ => {}
    }
}
```

修改 `OutgoingMessageProcessor::raw_process_json` 中的调用：

```rust
// 第 334 行：send_message 调用删除 attachment_sn
let response = messenger_context.messenger.send_message(outgoing_message).await?;

// 第 337-339 行：替换 collect_attachment_keys 调用
let attachment_upload_id_map = Arc::new(DashMap::new());
register_attachment_receivers(&response.content, &manager, messenger_weak, &attachment_upload_id_map);

// 构造 WsOutgoingMessageResponse → 由于 WsOutgoingMessageResponse 已删除，这里需要改为直接返回 response 和 upload_id_map
// 改为返回 OutgoingMessageResponse（WS 协议层改为直接包装原始 response）
```

但是——这里有个问题：`WsOutgoingMessageResponse` 已删除，WS 返回给 agent 时应该用什么结构？`OutgoingMessageProcessor::raw_process_json` 返回的是 `Result<Option<serde_json::Value>>`。原来返回的是 `WsOutgoingMessageResponse` 的 JSON。现在需要决定返回什么。

**方案：** 在 JSON 层直接返回 `OutgoingMessageResponse`（它本身就包含 content，其中已有 transfer_id）。`attachment_upload_id_map` 不再需要返回给 agent（因为 agent 可以通过 content 中的 `AttachmentInfoResponse.transfer_id` 获取 id）。

所以简化：

```rust
// OutgoingMessageProcessor::raw_process_json
let response = messenger_context.messenger.send_message(outgoing_message).await?;

// 注册到 receiver_map（内部逻辑）
register_attachment_receivers(&response.content, &manager, messenger_weak, &attachment_upload_id_map);

// 直接返回 OutgoingMessageResponse（不再包装 WsOutgoingMessageResponse）
let response_value = serde_json::to_value(response)?;
Ok(Some(response_value))
```

- [ ] **Step 5: 修改 AttachmentPayloadProcessor**

`AttachmentPayloadProcessor` 不需要改逻辑，它通过 `header.id` 查 `attachment_receiver_map`，这个 id 现在就是 transfer_id，所以查找方式不变：

```rust
// 第 374 行：按 header.id 查找 receiver_map（header.id == transfer_id，不变）
let key_messenger = manager.attachment_receiver_map.get(&header.id)
    .ok_or_else(|| Error::AttachmentNotFound(header.id.to_string()))?;

// 第 383、406 行：完成/失败时删除
manager.attachment_receiver_map.remove(&header.id);
```

另外删除 `WsAttachmentPayloadResponse` 的使用，直接序列化 `AttachmentPayloadResponse`：

```rust
// 第 390-396 行，替换 WsAttachmentPayloadResponse
match serde_json::to_value(&result) {
    Ok(value) => {
        success = response.error_code == 0;
        Ok(Some(value))
    }
```

这里的 `result` 是 `AttachmentPayloadResponse`（已有 transfer_id）。

- [ ] **Step 6: 修改 AttachmentDownloadRequestProcessor**

```rust
// 第 483 行：删除 attachment_sn 参数
let att_info_response = messenger.download_attachment_header(request).await?;
let key = att_info_response.key.clone();
// 取 response 中的 transfer_id，不再 fetch_add
let transfer_id = att_info_response.transfer_id;
// attachment_sender_map 改为 transfer_id → Weak<ConnectContext>
manager.attachment_sender_map.insert(transfer_id, Arc::downgrade(&connect_context));
```

同时修改 `process_download_request_header` 返回类型——不再返回 `WsAttachmentDownloadResponseHeader`，改为直接返回 `Arc<AttachmentInfoResponse>` 和 executor：

```rust
async fn process_download_request_header(&self, data: WsMessage) -> Result<(Arc<AttachmentInfoResponse>, StartSendDownloadAttachmentPayloadExecutor)> {
    // ...
    let att_info_response = messenger.download_attachment_header(request).await?;
    let key = att_info_response.key.clone();
    let transfer_id = att_info_response.transfer_id;
    manager.attachment_sender_map.insert(transfer_id, Arc::downgrade(&connect_context));

    Ok((att_info_response, StartSendDownloadAttachmentPayloadExecutor {
        messenger,
        key,
        transfer_id,  // 新增
    }))
}
```

`StartSendDownloadAttachmentPayloadExecutor` 相应修改：

```rust
struct StartSendDownloadAttachmentPayloadExecutor {
    messenger: Arc<dyn Messenger>,
    key: Arc<String>,
    transfer_id: u32,
}

impl StartSendDownloadAttachmentPayloadExecutor {
    pub async fn execute(&self) {
        if let Err(e) = self.messenger.start_send_download_attachment_payload(self.key.as_str(), self.transfer_id).await {
            error!("start_send_download_attachment_payload error: {:?}", e);
        }
    }
}
```

在 `process_json` 中，序列化 `AttachmentInfoResponse` 代替 `WsAttachmentDownloadResponseHeader`：

```rust
// 第 505-514 行
match serde_json::to_value(response_header) {
    Ok(response_header_value) => {
        match context.send_json(WsMessage {
            sn,
            status_code: CODE_SUCCESS,
            payload_type: TYPE_RESPONSE,
            payload: Some(response_header_value),
        }).await {
```

这里的 `response_header` 现在是 `Arc<AttachmentInfoResponse>`，直接序列化即可。

- [ ] **Step 7: 修改 AttachmentDownloadPayloadSender**

```rust
// ChannelManager 的 prepare_send 方法
fn prepare_send(&self, key: &str, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)> {
    // 用 transfer_id 查 sender_map（不再用 key 查）
    let connect_context = self.attachment_sender_map.get(&transfer_id)
        .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?;
    connect_context.value().upgrade()
        .ok_or_else(|| Error::InternalError("connect context is None".to_string()))
        .map(|_| {
            let sn = 0u32; // 不再使用 global_attachment_sn，用 ws_context.next_request_sn() 替代
            // TODO: 需要获取 ws_context 来 next_request_sn
            // 这里需要进一步调整
        })
    // ...
}
```

嗯，这里有个问题——`prepare_send` 原来用 `global_attachment_sn.fetch_add` 生成 sn。现在 `global_attachment_sn` 删除了，sn 从哪里来？sn 是 WS 协议的帧序列号，与 transfer_id 无关。它不应该删——`global_attachment_sn` 有两个作用：生成 sn 和生成 attachment id。现在 id 的生成移到 AttachmentRegistry，但 sn 仍然需要。

**纠正：** 保留 `global_attachment_sn` 用于生成 ws 帧序列号(sn)，但不再用于 attachment id。

`channel_manager.rs` 中的 `global_attachment_sn` 仅用于 `prepare_send` 中的 sn 生成，保留：

```rust
pub struct ChannelManager {
    // ...
    global_attachment_sn: Arc<AtomicU32>,  // 保留，仅用于 ws 帧 sn
    attachment_receiver_map: DashMap<u32, (String, Weak<dyn Messenger>)>,  // transfer_id → (key, messenger)
    attachment_sender_map: DashMap<u32, Weak<ConnectContext>>,  // transfer_id → connect_context
}
```

`prepare_send` 仅用 transfer_id 查 connect_context，仍用 global_attachment_sn 生成 sn：

```rust
fn prepare_send(&self, key: &str, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)> {
    let connect_context = self.attachment_sender_map.get(&transfer_id)
        .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?
        .upgrade()
        .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

    let sn = self.global_attachment_sn.fetch_add(1, Ordering::SeqCst);
    let capacity = OFFSET_ATT_DATA + size as usize;
    let mut buf = BytesMut::with_capacity(capacity);
    buf.put_u32(sn);
    buf.put_u32(TYPE_ATTACHMENT_PAYLOAD);
    buf.put_u32(CODE_SUCCESS);
    buf.put_u32(transfer_id);  // 二进制帧头部 id 字段用 transfer_id
    buf.put_u32(size);
    buf.put_u64(pos);
    Ok((sn, buf))
}
```

`send` 方法同样修改：

```rust
async fn send(&self, sn: u32, key: &str, transfer_id: u32, size: u32, pos: u64, buf: BytesMut) -> Result<AttachmentPayloadResponse> {
    let connect_context = self.attachment_sender_map.get(&transfer_id)
        .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?
        .upgrade()
        .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

    if size == 0 {
        self.attachment_sender_map.remove(&transfer_id);
        connect_context.ws_context.send_bin(buf.freeze()).await?;
        return Ok(AttachmentPayloadResponse {
            key: Arc::new(key.to_string()),
            transfer_id,
            pos,
            size,
            error_code: 0,
            error_msg: None,
        });
    }

    let result = self.send_download_attachment_payload(sn, buf, connect_context).await;

    if match result.as_ref() { Ok(res) => res.error_code != 0, Err(_) => true } {
        self.attachment_sender_map.remove(&transfer_id);
    }

    result
}
```

- [ ] **Step 8: 编译和测试**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel 2>&1 | head -40
```

Expected: 编译成功。

```bash
cd /home/admin/project/kissbot && cargo test -p kissbot-api 2>&1 | tail -10
```

Expected: 测试全部通过。

- [ ] **Step 9: 提交**

```bash
cd /home/admin/project/kissbot && git add -A && git commit -m "refactor: ChannelManager 适配 transfer_id、删除 Ws 结构体引用

- attachment_sender_map 改为 transfer_id → Weak<ConnectContext>
- 删除 collect_attachment_keys，新增 register_attachment_receivers
- register_messenger 删除 global_attachment_sn 参数
- OutgoingMessageProcessor 直接返回 OutgoingMessageResponse
- AttachmentDownloadRequestProcessor 使用 response.transfer_id
- prepare_send/send 用 transfer_id 查 sender_map
- 删除 WsAttachmentPayloadResponse/WsAttachmentDownloadResponseHeader 引用
- global_attachment_sn 保留仅用于 ws 帧 sn 生成

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: HTTP handler 适配 transfer_id

**Files:**
- Modify: `kissbot-channel-web/src/http.rs`

**Interfaces:**
- Consumes: `AttachmentStore::write_chunk(key, transfer_id, pos, size, data)` from Task 3

- [ ] **Step 1: 修改 handle_upload_attachment**

HTTP handler 从 multipart 中解析 `transfer_id` + `key` + `file`：

```rust
async fn handle_upload_attachment(
    State(messenger): State<Arc<WebMessenger>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut attachment_key: Option<String> = None;
    let mut attachment_transfer_id: Option<u32> = None;
    let mut file_data: Option<Bytes> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().map(|s| s.to_string());
        if name.as_deref() == Some("key") {
            if let Ok(data) = field.text().await {
                attachment_key = Some(data.trim().to_string());
            }
        } else if name.as_deref() == Some("transfer_id") {
            if let Ok(data) = field.text().await {
                attachment_transfer_id = data.trim().parse::<u32>().ok();
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

    let attachment_transfer_id = match attachment_transfer_id {
        Some(id) => id,
        None => return Json(ApiResponse::<serde_json::Value>::error("Missing transfer_id".to_string())),
    };

    let file_data = match file_data {
        Some(d) => d,
        None => return Json(ApiResponse::<serde_json::Value>::error("Missing file data".to_string())),
    };

    match messenger.attachment_store.write_chunk(&attachment_key, attachment_transfer_id, 0, file_data.len() as u32, file_data).await {
        Ok(_) => Json(ApiResponse::success(serde_json::json!({"success": true}))),
        Err(e) => Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
    }
}
```

- [ ] **Step 2: 编译**

```bash
cd /home/admin/project/kissbot && cargo build -p kissbot-channel-web 2>&1 | head -30
```

Expected: 编译成功。

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot && git add -A && git commit -m "refactor: HTTP handler 适配 transfer_id

- handle_upload_attachment 解析 transfer_id + key + file 字段
- 调用 write_chunk 时传入 transfer_id

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: 编译和测试验证

**Files:**
- Verify all affected crates

- [ ] **Step 1: 全量编译**

```bash
cd /home/admin/project/kissbot && cargo build 2>&1 | tail -20
```

Expected: 编译成功，无错误。

- [ ] **Step 2: 运行所有测试**

```bash
cd /home/admin/project/kissbot && cargo test 2>&1 | tail -20
```

Expected: 所有测试通过。
