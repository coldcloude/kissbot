# Transfer ID 附件传输重构设计

## 概述

在附件上传和下载中显式使用 transfer_id，替换原有 ChannelManager 自行分配 id 的逻辑，统一由 AttachmentRegistry 生成 id 并嵌入 AttachmentInfoResponse。同时清理 Messenger trait 中多余的参数（attachment_sn）。

## 动机

1. **支持同一 key 并发下载**：当前 `attachment_sender_map` 以 key 为主键，同一 key 多次下载会覆盖
2. **统一 id 管理**：由 AttachmentRegistry 集中生成 transfer_id，各层不再各自生成
3. **简化接口**：transfer_id 嵌入 AttachmentInfoResponse，无需额外包装结构体传递 id
4. **清理冗余参数**：删除 Messenger trait 中未使用的 `attachment_sn: Arc<AtomicU32>`

## 影响范围

- `kissbot-api` — 数据结构变更
- `kissbot-channel` — trait 接口变更
- `kissbot-channel-web` — 实现适配
- `kissbot-channel` — ChannelManager 适配

---

## 一、数据结构变更（kissbot-api）

### 1.1 AttachmentInfoResponse 新增 transfer_id

`kissbot-api/src/message.rs`：

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfoResponse {
    pub key: Arc<String>,
    pub info: Arc<AttachmentInfo>,
    pub transfer_id: u32,
}
```

### 1.2 AttachmentPayloadResponse 新增 transfer_id

`kissbot-api/src/channel.rs`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentPayloadResponse {
    pub key: Arc<String>,
    pub transfer_id: u32,
    pub pos: u64,
    pub size: u32,
    pub error_code: u32,
    pub error_msg: Option<Arc<String>>,
}
```

### 1.3 删除 Ws 系列包装结构体

`kissbot-api/src/channel.rs` 删除以下结构体定义及相关引用：

| 结构体 | 原因 |
|--------|------|
| `WsOutgoingMessageResponse` | transfer_id 已在 content 的 AttachmentInfoResponse 中 |
| `WsAttachmentDownloadResponseHeader` | transfer_id 已在返回的 AttachmentInfoResponse 中 |
| `WsAttachmentPayloadResponse` | 不再需要，agent 直接用 AttachmentPayloadResponse |

### 1.4 删除 attachment_upload_id_map 相关字段

`WsOutgoingMessageResponse` 删除后，其 `attachment_upload_id_map` 字段随之删除。

---

## 二、接口变更（kissbot-channel）

### 2.1 AttachmentRegistry 新增 gen_transfer_id

`kissbot-channel/src/attachment.rs`：

```rust
#[async_trait]
pub trait AttachmentRegistry: Send + Sync {
    async fn register(&self, messenger_id: &str, user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> Result<Arc<String>>;
    async fn gen_transfer_id(&self, key: &str) -> u32;
}
```

`process_attachment_message` 生成附件响应时调用 `gen_transfer_id`：

```rust
Content::AttachmentInfo(info) => {
    let key = registry.register(messenger_id, user_id, group_id, info.clone()).await?;
    let transfer_id = registry.gen_transfer_id(key.as_str()).await?;
    Ok(Content::AttachmentInfoResponse(Arc::new(AttachmentInfoResponse {
        key,
        info: info.clone(),
        transfer_id,
    })))
}
```

### 2.2 Messenger trait 清理 attachment_sn、增加 transfer_id

`kissbot-channel/src/messenger.rs`：

```rust
#[async_trait]
pub trait Messenger: Send + Sync + 'static {
    async fn get_info(&self) -> Result<Arc<MessengerInfo>>;
    async fn send_message(&self, message: OutgoingMessage) -> Result<Arc<OutgoingMessageResponse>>;
    async fn send_attachment_payload(&self, key: &str, transfer_id: u32, size: u32, pos: u64, data: Bytes) -> Result<AttachmentPayloadResponse>;
    async fn download_attachment_header(&self, request: AttachmentDownloadRequest) -> Result<Arc<AttachmentInfoResponse>>;
    async fn start_send_download_attachment_payload(&self, key: &str, transfer_id: u32) -> Result<()>;
}
```

### 2.3 MessengerCreator 清理 attachment_sn

`kissbot-channel/src/messenger.rs`：

```rust
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

### 2.4 AttachmentDownloadPayloadSender 增加 transfer_id

`kissbot-channel/src/data.rs`：

```rust
pub trait AttachmentDownloadPayloadSender: Send + Sync {
    fn prepare_send(&self, key: &str, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)>;
    async fn send(&self, sn: u32, key: &str, transfer_id: u32, size: u32, pos: u64, buf: BytesMut) -> Result<AttachmentPayloadResponse>;
}
```

---

## 三、实现变更

### 3.1 AttachmentStore（kissbot-channel-web）

`kissbot-channel-web/src/attachment.rs`：

- 新增 `transfer_id_seq: AtomicU32` 字段
- `upload_channels` 主键改为 `u32`（transfer_id），值改为 `(Arc<String>, u64, flume::Sender<UploadCommand>)`
- UploadCommand 新增 `transfer_id: u32` 字段
- `write_chunk(key, transfer_id, pos, size, data)` — 新增 transfer_id 参数
- `get_or_create_upload_channel(transfer_id, key)` — 以 transfer_id 为主键
- `send_download_payload(key, transfer_id, sender)` — 新增 transfer_id 参数
- `gen_transfer_id` 实现：`self.transfer_id_seq.fetch_add(1, Ordering::SeqCst)`

### 3.2 WebMessenger（kissbot-channel-web）

`kissbot-channel-web/src/messenger.rs`：

- `send_message(message)` — 删除 `_attachment_sn` 参数
- `send_attachment_payload(key, transfer_id, size, pos, data)` — 委托 `attachment_store.write_chunk(key, transfer_id, pos, size, data)`
- `download_attachment_header(request)` — 删除 `_attachment_sn` 参数，内部调用 `attachment_store.next_transfer_id()` 生成 id 写入返回的 `AttachmentInfoResponse`
- `start_send_download_attachment_payload(key, transfer_id)` — 委托 `attachment_store.send_download_payload(key, transfer_id, &*sender)`
- `WebMessengerCreator::create(...)` — 删除 `_global_attachment_sn` 参数

### 3.3 HTTP handler（kissbot-channel-web）

`kissbot-channel-web/src/http.rs`：

- `handle_init_attachment` — 不变。返回的 `OutgoingMessageResponse` 中自动携带 transfer_id
- `handle_upload_attachment` — multipart 中解析 `transfer_id` + `key` + `file`，调 `write_chunk(key, transfer_id, ...)`

示例 multipart 字段：
```
transfer_id: "42"
key: "g1/uuid-xxxx"
file: <binary data>
```

### 3.4 ChannelManager（kissbot-channel）

`kissbot-channel/src/channel_manager.rs`：

**register_messenger**：
- 删除 `self.global_attachment_sn.clone()` 参数

**OutgoingMessageProcessor**：
- 删除 `collect_attachment_keys` 函数（原函数分配 id 并注册）
- 新增遍历函数，读取 response.content 中已有的 transfer_id 注册到 `attachment_receiver_map`

**AttachmentDownloadRequestProcessor**：
- 删除 `global_attachment_sn.fetch_add` 调用
- 直接从 `att_info_response.transfer_id` 获取 id
- `attachment_sender_map.insert(transfer_id, Weak<ConnectContext>)` — 不存 key，只存 transfer_id

**AttachmentDownloadPayloadSender**：
- `prepare_send(key, transfer_id, size, pos)` — 用 transfer_id 查 `attachment_sender_map`
- `send(sn, key, transfer_id, size, pos, buf)` — 用 transfer_id 查 `attachment_sender_map`

**attachment_sender_map** 类型变更：
```rust
// 之前：DashMap<String, (u32, Weak<ConnectContext>)>
// 改为：DashMap<u32, Weak<ConnectContext>>
//        transfer_id → connect_context
```

---

## 四、数据流（完整示例）

### 上传

```
前端/Agent                  ChannelManager                Messenger              AttachmentStore
  │                              │                           │                      │
  │── send_message ─────────────→│                           │                      │
  │                              │── send_message ──────────→│                      │
  │                              │                           │── process_attachment  │
  │                              │                           │   → register(key)     │
  │                              │                           │   → gen_transfer_id   │
  │                              │                           │   ← AttachmentInfoRes │
  │                              │←── response(content) ────│   (含 transfer_id)    │
  │                              │                           │                      │
  │                              │ 遍历 content，注册         │                      │
  │                              │ receiver_map[transfer_id]  │                      │
  │←─ WsOutgoingMessageResponse──│                           │                      │
  │    (content 含 transfer_id)  │                           │                      │
  │                              │                           │                      │
  │── attachment_payload(id) ───→│                           │                      │
  │                              │── send_attachment_payload─│                      │
  │                              │   (key, transfer_id, ...) │── write_chunk ──────→│
  │                              │←── response ─────────────│←── ok ───────────────│
  │←─ response ─────────────────│                           │                      │
```

### 下载

```
前端/Agent                  ChannelManager                Messenger              AttachmentStore
  │                              │                           │                      │
  │── download_request ─────────→│                           │                      │
  │                              │── download_attachment_hdr→│                      │
  │                              │                           │── get_meta           │
  │                              │                           │── next_transfer_id   │
  │                              │←── AttachmentInfoRes ────│   (含 transfer_id)    │
  │                              │                           │                      │
  │                              │ 注册                       │                      │
  │                              │ sender_map[transfer_id]    │                      │
  │←─ response(含 transfer_id) ─│                           │                      │
  │                              │                           │                      │
  │                              │── start_send_download ───→│                      │
  │                              │   (key, transfer_id)      │── send_download_pload│
  │←── download_payload ────────│                           │   (key, transfer_id)  │
```

---

## 五、Impact Matrix

| 文件 | 变更类型 | 内容 |
|------|----------|------|
| `kissbot-api/src/message.rs` | 修改 | `AttachmentInfoResponse` 新增 `transfer_id` |
| `kissbot-api/src/channel.rs` | 修改+删除 | `AttachmentPayloadResponse` 新增 `transfer_id`；删除 3 个 Ws 结构体 |
| `kissbot-channel/src/attachment.rs` | 修改 | `AttachmentRegistry` 新增 `gen_transfer_id`；`process_attachment_message` 调用它 |
| `kissbot-channel/src/messenger.rs` | 修改 | `Messenger` 和 `MessengerCreator` 清理 `attachment_sn`；`send_attachment_payload` 和 `start_send_download_attachment_payload` 增加 `transfer_id`；`download_attachment_header` 删除 `attachment_sn` |
| `kissbot-channel/src/data.rs` | 修改 | `AttachmentDownloadPayloadSender` 方法增加 transfer_id 和 key |
| `kissbot-channel/src/channel_manager.rs` | 修改 | 删除自身分配 id 逻辑；改用 response 中的 transfer_id；`attachment_sender_map` 改为 `u32 → Weak<ConnectContext>`；删除 `global_attachment_sn` 相关引用 |
| `kissbot-channel-web/src/attachment.rs` | 修改 | `AttachmentStore` 新增 `transfer_id_seq`；`upload_channels` 主键改为 u32；`write_chunk`/`send_download_payload` 增加 transfer_id |
| `kissbot-channel-web/src/messenger.rs` | 修改 | 适配 Messenger trait 新签名 |
| `kissbot-channel-web/src/http.rs` | 修改 | `handle_upload_attachment` 解析 transfer_id |
| `kissbot-channel-web/Cargo.toml` | 不变 | 已有相关依赖 |

---

## 六、测试关注点

1. `AttachmentInfoResponse` 序列化/反序列化测试补充 `transfer_id` 字段
2. `AttachmentPayloadResponse` 序列化测试补充 `transfer_id` 字段
3. 删除的 Ws 结构体相关测试删除
4. 编译通过（所有 crate）
5. 单元测试通过
