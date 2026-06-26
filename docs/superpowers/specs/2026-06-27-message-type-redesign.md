# 消息类型与附件模型重构设计

## 概述

重构消息系统中非文本消息的 content 格式和附件数据模型。附件信息不再通过 `OutgoingMessage.attachment_map` 承载，改为统一由 `msg_type` + `content` 表达，在 channel 组件层提供统一的解析函数完成 key 生成和 upload_id 分配。

## 核心变更

### 1. msg_type 体系调整

- 移除 `MSG_TYPE_IMAGE = "image"` 和 `MSG_TYPE_FILE = "file"` 常量
- 新增 `MSG_TYPE_ATTACHMENT = "attachment"` — 统一所有附件类型
- 保留：`MSG_TYPE_TEXT = "text"`、`MSG_TYPE_MULTI = "multi"`、`MSG_TYPE_SYSTEM_JOIN`、`MSG_TYPE_SYSTEM_LEAVE`
- 附件具体的媒体类型区分由 `AttachmentInfo.mime_type` 承载，不再体现在 `msg_type` 中

### 2. OutgoingMessage 简化

**去掉 `attachment_map` 字段**。附件信息完全由 `msg_type` + `content` 表达：

- `msg_type = "text"` → `content` 是纯文本
- `msg_type = "attachment"` → `content` 是 `AttachmentInfo` 的 JSON 序列化
- `msg_type = "multi"` → `content` 是 `MessageItem[]` 的 JSON 序列化，其中每条 `MessageItem` 也遵循此规则：
  - `msg_type = "text"` → `content` 是文本
  - `msg_type = "attachment"` → `content` 是 `AttachmentInfo` JSON

### 3. 新增 ResponseAttachmentInfo

```rust
pub struct ResponseAttachmentInfo {
    pub key: Arc<String>,              // 生成的附件 key，格式 "{group_id}/{msg_id}/{filename}"
    pub info: Arc<AttachmentInfo>,     // 附件元数据，含 filename
}
```

`IncomingMessage` 中 `content` 的格式：
- `msg_type = "attachment"` → `content` 是 `ResponseAttachmentInfo` JSON
- `msg_type = "multi"` → `content` 是 `MessageItem[]` JSON，其中 attachment 项的 `content` 是 `ResponseAttachmentInfo` JSON

### 4. AttachmentInfo 增加 filename 字段

```rust
pub struct AttachmentInfo {
    pub att_id: Arc<String>,        // 附件标识
    pub filename: Arc<String>,      // 文件名（新增）
    pub mime_type: Arc<String>,     // MIME 类型
    pub size_bytes: u64,            // 文件大小
}
```

### 5. OutgoingMessageResponse 扩展

```rust
pub struct OutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub attachment_upload_id_map: Arc<DashMap<String, u32>>,   // att_id → upload_id
    pub attachment_key_map: Arc<DashMap<String, Arc<String>>>, // att_id → key（新增）
}
```

## 新增组件层抽象

### AttachmentKeyGenerator trait

放置在 `kissbot-channel` 组件中：

```rust
/// 附件 key 生成器。将 AttachmentInfo 映射为全局唯一的 attachment key。
pub trait AttachmentKeyGenerator: Send + Sync {
    fn generate_key(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo) -> String;
}
```

`WebMessenger` 实现此 trait，生成 `"{group_id}/{msg_id}/{filename}"` 格式的 key。

### process_attachment_message 函数

放置在 `kissbot-channel` 组件中：

```rust
/// 处理 OutgoingMessage 中的附件类型消息。
///
/// 根据 msg_type：
/// - "text"：原样返回，upload_id_map 和 key_map 为空
/// - "attachment"：解析 content 中 AttachmentInfo → 生成 key + upload_id → 返回 ResponseAttachmentInfo 内容
/// - "multi"：逐项处理，attachment 类型项同上处理
///
/// 返回 (新 content, OutgoingMessageResponse)。
/// 新 content 中附件类型的 ResponseAttachmentInfo 已包含生成的 key。
pub fn process_attachment_message(
    outgoing: &OutgoingMessage,
    msg_id: &str,
    key_generator: &dyn AttachmentKeyGenerator,
    attachment_sn: &Arc<AtomicU32>,
) -> Result<(String, OutgoingMessageResponse)>
```

**处理逻辑：**

1. 解析 `outgoing.msg_type`：
   - `"text"` → 直接返回 `(outgoing.content, empty_response)`
2. 对于 `"attachment"`：
   - 解析 `outgoing.content` 为 `AttachmentInfo`
   - 调用 `key_generator.generate_key(group_id, msg_id, &info)` 生成 key
   - `attachment_sn.fetch_add(1, ...)` 分配 upload_id
   - 构造 `ResponseAttachmentInfo { key, info }` 序列化为新 content
   - 填充 `attachment_upload_id_map[att_id] = upload_id`
   - 填充 `attachment_key_map[att_id] = key`
3. 对于 `"multi"`：
   - 解析 `outgoing.content` 为 `Vec<MessageItem>`
   - 逐项处理：`"text"` 项跳过，`"attachment"` 项按上述流程处理
   - 将所有项重新序列化为新 content

## 使用场景示例

### 发送图片附件

**nexus 构造 OutgoingMessage：**
```json
{
    "messenger_id": "web",
    "user_id": "u0",
    "group_id": "g1",
    "msg_type": "attachment",
    "content": {
        "att_id": "att_42",
        "filename": "photo.png",
        "mime_type": "image/png",
        "size_bytes": 1048576
    }
}
```

**经 channel 处理后，IncomingMessage 内容：**
```json
{
    "msg_type": "attachment",
    "content": {
        "key": "g1/20260627120000000000/photo.png",
        "info": {
            "att_id": "att_42",
            "filename": "photo.png",
            "mime_type": "image/png",
            "size_bytes": 1048576
        }
    }
}
```

### 发送图文混合消息

**OutgoingMessage content（multi 类型）：**
```json
[
    {"msg_type": "text", "content": "看这张照片"},
    {"msg_type": "attachment", "content": {"att_id": "att_42", "filename": "photo.png", "mime_type": "image/png", "size_bytes": 1048576}}
]
```

**处理后 IncomingMessage content：**
```json
[
    {"msg_type": "text", "content": "看这张照片"},
    {"msg_type": "attachment", "content": {"key": "g1/20260627120000000000/photo.png", "info": {"att_id": "att_42", "filename": "photo.png", "mime_type": "image/png", "size_bytes": 1048576}}}
]
```

### admin 通过 HTTP 上传

`POST /api/attachment/init` 中构造的 OutgoingMessage：
- `msg_type = "attachment"`
- `content = AttachmentInfo` JSON（不含 key）

经 `process_attachment_message` 处理后得到带 key 的 content 和含 `attachment_upload_id_map`/`attachment_key_map` 的 response。

## 变更影响

### kissbot-api
- `OutgoingMessage`：去掉 `attachment_map` 字段
- `AttachmentInfo`：新增 `filename` 字段
- 新增 `ResponseAttachmentInfo` 结构
- `OutgoingMessageResponse`：新增 `attachment_key_map` 字段
- `message.rs`：新增 `MSG_TYPE_ATTACHMENT` 常量，删除 `MSG_TYPE_IMAGE`/`MSG_TYPE_FILE`

### kissbot-channel
- 新增 `AttachmentKeyGenerator` trait
- 新增 `process_attachment_message()` 函数
- 调用方（如 `ChannelManager` 中的 OutgoingMessageProcessor）需要适配新的 Msg 类型

### kissbot-channel-web
- `Messenger::send_message()` 签名不变，但内部调用 `process_attachment_message` 替代手工遍历 `attachment_map`
- `WebMessenger` 实现 `AttachmentKeyGenerator`
- `handle_init_attachment` 构造新的 OutgoingMessage（无 attachment_map）
- `build_message_content` / `handle_send_message` 改用 `"attachment"` 替代 `"mixed"` + 自定义 JSON 格式
- `handle_upload_attachment` 不变（两阶段上传流程不受影响）

### 向后兼容
- 旧的 `"image"`、`"file"`、`"mixed"` msg_type 不再被识别。已存储的历史消息保持原格式，无需迁移。
