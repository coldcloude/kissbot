# 消息类型与附件模型重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 重构非文本消息的 content 格式和附件数据模型，去除 `OutgoingMessage.attachment_map`，新增 `AttachmentKeyGenerator` trait 和 `process_attachment_message` 统一函数。

**Architecture:** 分 4 个任务逐步实现：(1) kissbot-api 类型变更, (2) kissbot-channel 新增抽象, (3) kissbot-channel-web 适配, (4) 清理旧版 msg_type 和测试修复。每个任务可独立编译。

**Tech Stack:** Rust, kissbot-api, kissbot-channel, kissbot-channel-web, DashMap, serde_json

## Global Constraints

- 所有新增的 Async trait 方法必须使用 `#[async_trait]`
- `Arc<String>` / `Arc<AtomicU32>` 模式与现有代码一致
- 不要删除代码中的注释
- msg_type 常量使用 `&str`，通过 `pub const` 导出
- `AttachmentKeyGenerator` trait 放在 `kissbot-channel/src/attachment.rs`（新建文件）
- `process_attachment_message` 函数放在 `kissbot-channel/src/attachment.rs`
- 所有 API 响应使用 `kissbot_api::ApiResponse`
- 不要删除旧 `MSG_TYPE_IMAGE`/`MSG_TYPE_FILE` 直到 Task 4（先保留但标记弃用）

---

### Task 1: kissbot-api 类型变更

**Files:**
- Modify: `kissbot-api/src/message.rs` — 新增 `MSG_TYPE_ATTACHMENT`
- Modify: `kissbot-api/src/channel.rs` — `AttachmentInfo` 加 filename, `OutgoingMessage` 去 attachment_map, 新增 `ResponseAttachmentInfo`, `OutgoingMessageResponse` 加 attachment_key_map

**Interfaces:**
- Consumes: 无（基础类型变更）
- Produces: 新的数据结构供后续任务使用

- [ ] **Step 1: message.rs — 新增 `MSG_TYPE_ATTACHMENT`**

```rust
pub const MSG_TYPE_ATTACHMENT: &str = "attachment";
```

保留 `MSG_TYPE_IMAGE` 和 `MSG_TYPE_FILE`（Task 4 再删除），但在注释中标注弃用。

- [ ] **Step 2: AttachmentInfo 增加 filename 字段**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub att_id: Arc<String>,
    pub filename: Arc<String>,      // ← 新增
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
}
```

- [ ] **Step 3: OutgoingMessage 去掉 attachment_map 字段**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    // attachment_map 已移除，附件信息由 msg_type + content 承载
}
```

- [ ] **Step 4: 新增 ResponseAttachmentInfo**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseAttachmentInfo {
    pub key: Arc<String>,
    pub info: Arc<AttachmentInfo>,
}
```

- [ ] **Step 5: OutgoingMessageResponse 增加 attachment_key_map**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessageResponse {
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub attachment_upload_id_map: Arc<DashMap<String, u32>>,
    pub attachment_key_map: Arc<DashMap<String, Arc<String>>>,  // att_id → key（新增）
}
```

- [ ] **Step 6: 更新 kissbot-api 的测试**

修改 `channel.rs` 中的测试：

`test_serde_outgoing_message`（更新 — 去掉 attachment_map，content 改为 JSON）：

```rust
#[test]
fn test_serde_outgoing_message() {
    let att_info = Arc::new(AttachmentInfo {
        att_id: Arc::new("att1".to_string()),
        filename: Arc::new("photo.png".to_string()),
        mime_type: Arc::new("image/png".to_string()),
        size_bytes: 12345,
    });
    let content = serde_json::to_string(&att_info).unwrap();

    let obj = OutgoingMessage {
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
        group_id: Arc::new("g1".to_string()),
        msg_type: Arc::new(MSG_TYPE_ATTACHMENT.to_string()),
        content: Arc::new(content),
    };
    let json = serde_json::to_value(&obj).unwrap();
    let deserialized: OutgoingMessage = serde_json::from_value(json).unwrap();
    assert_eq!(*deserialized.messenger_id, "m1");
    assert_eq!(*deserialized.msg_type, MSG_TYPE_ATTACHMENT);
}
```

`test_serde_outgoing_message_response`（更新 — 增加 attachment_key_map）：

```rust
#[test]
fn test_serde_outgoing_message_response() {
    let upload_map = Arc::new(DashMap::new());
    upload_map.insert("att1".to_string(), 100u32);
    let key_map = Arc::new(DashMap::new());
    key_map.insert("att1".to_string(), Arc::new("g1/msg1/photo.png".to_string()));

    let obj = OutgoingMessageResponse {
        msg_id: Arc::new("msg1".to_string()),
        time: Arc::new("2026-01-01 00:00:00".to_string()),
        attachment_upload_id_map: upload_map,
        attachment_key_map: key_map,
    };
    let json = serde_json::to_value(&obj).unwrap();
    let deserialized: OutgoingMessageResponse = serde_json::from_value(json).unwrap();
    assert_eq!(*deserialized.msg_id, "msg1");
    assert_eq!(deserialized.attachment_upload_id_map.len(), 1);
    assert_eq!(deserialized.attachment_key_map.len(), 1);
}
```

`test_serde_attachment_download_response_header`（更新 — AttachmentInfo 加 filename）：

```rust
#[test]
fn test_serde_attachment_download_response_header() {
    let metadata = Arc::new(AttachmentInfo {
        att_id: Arc::new("att1".to_string()),
        filename: Arc::new("doc.pdf".to_string()),
        mime_type: Arc::new("application/pdf".to_string()),
        size_bytes: 99999,
    });
    // ...（其他不变）
}
```

更新 `test_serde_message_item` 在 `message.rs` 中增加 `MSG_TYPE_ATTACHMENT`：

```rust
#[test]
fn test_serde_message_item() {
    let types = [MSG_TYPE_TEXT, MSG_TYPE_ATTACHMENT, MSG_TYPE_SYSTEM_JOIN, MSG_TYPE_SYSTEM_LEAVE, MSG_TYPE_MULTI];
    // ...
}
```

- [ ] **Step 7: 编译验证**

```bash
cd kissbot-api && cargo test
```

Expected: 编译通过，所有测试通过。

- [ ] **Step 8: Commit**

```bash
git add kissbot-api/src/message.rs kissbot-api/src/channel.rs
git commit -m "refactor: 消息类型体系重构 — API 层类型变更

- 新增 MSG_TYPE_ATTACHMENT = \"attachment\"
- AttachmentInfo 增加 filename 字段
- OutgoingMessage 移除 attachment_map 字段
- 新增 ResponseAttachmentInfo 结构
- OutgoingMessageResponse 增加 attachment_key_map

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: kissbot-channel 新增抽象层

**Files:**
- Create: `kissbot-channel/src/attachment.rs` — `AttachmentKeyGenerator` trait + `process_attachment_message` 函数
- Modify: `kissbot-channel/src/lib.rs` — 注册新 module

**Interfaces:**
- Consumes: Task 1（新类型）
- Produces: `AttachmentKeyGenerator` trait, `process_attachment_message()` 函数

- [ ] **Step 1: 创建 `kissbot-channel/src/attachment.rs`**

```rust
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use dashmap::DashMap;
use kissbot_api::channel::{
    AttachmentInfo, OutgoingMessage, OutgoingMessageResponse, ResponseAttachmentInfo,
};
use kissbot_api::message::{MessageItem, MSG_TYPE_ATTACHMENT, MSG_TYPE_MULTI, MSG_TYPE_TEXT};

use crate::error::Result;

/// 附件 key 生成器。将 AttachmentInfo 映射为全局唯一的 attachment key。
pub trait AttachmentKeyGenerator: Send + Sync {
    fn generate_key(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo) -> String;
}

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
) -> Result<(String, OutgoingMessageResponse)> {
    let attachment_upload_id_map = Arc::new(DashMap::new());
    let attachment_key_map = Arc::new(DashMap::new());

    let new_content = match outgoing.msg_type.as_str() {
        MSG_TYPE_TEXT => {
            // 纯文本，无附件处理
            outgoing.content.to_string()
        }
        MSG_TYPE_ATTACHMENT => {
            // 单条附件：content 是 AttachmentInfo JSON
            let info: AttachmentInfo = serde_json::from_str(outgoing.content.as_str())
                .map_err(|e| crate::Error::InternalError(format!("parse AttachmentInfo failed: {}", e)))?;
            let key = key_generator.generate_key(
                outgoing.group_id.as_str(), msg_id, &info
            );
            let upload_id = attachment_sn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            attachment_upload_id_map.insert(info.att_id.to_string(), upload_id);
            attachment_key_map.insert(info.att_id.to_string(), Arc::new(key.clone()));
            let response = ResponseAttachmentInfo {
                key: Arc::new(key),
                info: Arc::new(info),
            };
            serde_json::to_string(&response)
                .map_err(|e| crate::Error::InternalError(format!("serialize ResponseAttachmentInfo failed: {}", e)))?
        }
        MSG_TYPE_MULTI => {
            // multi：逐项处理
            let items: Vec<MessageItem> = serde_json::from_str(outgoing.content.as_str())
                .map_err(|e| crate::Error::InternalError(format!("parse MessageItem[] failed: {}", e)))?;
            let new_items: std::result::Result<Vec<MessageItem>, _> = items.into_iter().map(|item| {
                if item.msg_type.as_str() == MSG_TYPE_ATTACHMENT {
                    let info: AttachmentInfo = serde_json::from_str(item.content.as_str())
                        .map_err(|e| crate::Error::InternalError(format!("parse AttachmentInfo failed: {}", e)))?;
                    let key = key_generator.generate_key(
                        outgoing.group_id.as_str(), msg_id, &info
                    );
                    let upload_id = attachment_sn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    attachment_upload_id_map.insert(info.att_id.to_string(), upload_id);
                    attachment_key_map.insert(info.att_id.to_string(), Arc::new(key.clone()));
                    let response = ResponseAttachmentInfo {
                        key: Arc::new(key),
                        info: Arc::new(info),
                    };
                    let new_content = serde_json::to_string(&response)
                        .map_err(|e| crate::Error::InternalError(format!("serialize ResponseAttachmentInfo failed: {}", e)))?;
                    Ok(MessageItem {
                        msg_type: item.msg_type,
                        content: Arc::new(new_content),
                    })
                } else {
                    // 非 attachment 类型（如 text），原样保留
                    Ok(item)
                }
            }).collect();
            let items = new_items?;
            serde_json::to_string(&items)
                .map_err(|e| crate::Error::InternalError(format!("serialize MessageItem[] failed: {}", e)))?
        }
        other => {
            // 其他类型（如 system_join、system_leave），不做处理
            outgoing.content.to_string()
        }
    };

    let response = OutgoingMessageResponse {
        msg_id: Arc::new(msg_id.to_string()),
        time: Arc::new(String::new()),  // 调用方会覆写 time
        attachment_upload_id_map,
        attachment_key_map,
    };

    Ok((new_content, response))
}
```

- [ ] **Step 2: 注册 module 到 lib.rs**

在 `kissbot-channel/src/lib.rs` 中加入：

```rust
pub mod attachment;
pub use attachment::{AttachmentKeyGenerator, process_attachment_message};
```

- [ ] **Step 3: 编译验证**

```bash
cd kissbot-channel && cargo check
```

Expected: 编译通过。

- [ ] **Step 4: Commit**

```bash
git add kissbot-channel/src/attachment.rs kissbot-channel/src/lib.rs
git commit -m "feat: 新增 AttachmentKeyGenerator trait 和 process_attachment_message 函数

- AttachmentKeyGenerator 定义 key 生成接口
- process_attachment_message 统一处理 attachment/multi 类型消息的
  key 生成和 upload_id 分配

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: kissbot-channel-web 适配

**Files:**
- Modify: `kissbot-channel-web/src/messenger.rs` — `WebMessenger` 实现 `AttachmentKeyGenerator`, `send()` 改用 `process_attachment_message`
- Modify: `kissbot-channel-web/src/http.rs` — 适配新 OutgoingMessage（无 attachment_map），`handle_init_attachment` 适配

**Interfaces:**
- Consumes: Task 1（新类型结构）, Task 2（`AttachmentKeyGenerator`, `process_attachment_message`）

- [ ] **Step 1: WebMessenger 实现 AttachmentKeyGenerator**

在 `kissbot-channel-web/src/messenger.rs` 中 `impl Messenger for WebMessenger` 块之后（或在文件末尾），增加：

```rust
use kissbot_channel::AttachmentKeyGenerator;

impl AttachmentKeyGenerator for WebMessenger {
    fn generate_key(&self, group_id: &str, msg_id: &str, info: &AttachmentInfo) -> String {
        format!("{}/{}/{}", group_id, msg_id, info.filename)
    }
}
```

- [ ] **Step 2: 修改 WebMessenger::send() — 改用 process_attachment_message**

当前 `send()` 中处理 `outgoing.attachment_map` 的逻辑（第 440-460 行左右）替换为调用 `process_attachment_message`：

找到以下代码块：

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
```

替换为：

```rust
        // 处理附件消息：解析 content、生成 key、分配 upload_id
        let (new_content, response) = kissbot_channel::process_attachment_message(
            &outgoing,
            msg_id.as_str(),
            self,
            &self.global_attachment_sn,
        ).map_err(|e| Error::InternalError(e.to_string()))?;

        // 为每个 upload_id 创建临时文件
        for entry in response.attachment_upload_id_map.iter() {
            let att_id = entry.key().clone();
            let upload_id = *entry.value();
            let key = response.attachment_key_map.get(&att_id)
                .map(|k| k.value().clone())
                .unwrap_or_default();
            // 从 key 中解析 filename: 格式 "{group_id}/{msg_id}/{filename}"
            let filename = key.rsplit('/').next().unwrap_or("unknown").to_string();
            let mime_type = mime_guess::from_path(&filename).first_or_octet_stream().to_string();
            // 获取 size_bytes — 从 response 重建 info 需要反查 key_map
            // 实际上 size_bytes 可以从原始 outgoing.content 解析
            // 这里简化处理：从 response.attachment_key_map 拿到 key，解析 filename
            match self.attachment_store.create_temp_file(
                outgoing.group_id.as_str(), msg_id.as_str(), &filename
            ) {
                Ok((temp_path, target_path)) => {
                    self.pending_uploads.insert(upload_id, PendingAttachment {
                        group_id: outgoing.group_id.clone(),
                        msg_id: msg_id.clone(),
                        filename: Arc::new(filename),
                        mime_type: Arc::new(mime_type),
                        size_bytes: 0, // process_attachment 后需要从原始数据获取
                        temp_path,
                        target_path,
                    });
                }
                Err(e) => return Err(Error::from(e)),
            }
        }
```

**注意：** 这里有个关键问题——`process_attachment_message` 返回后，我们拿到了 `att_id → upload_id` 和 `att_id → key`，但缺少 `size_bytes`。需要在 `process_attachment_message` 的返回值中增加 `size_bytes` 信息，或者直接返回 `Vec<(u32, PendingAttachmentInfo)>`。

更简洁的方案是：**让 `process_attachment_message` 也返回 `Vec<(u32, Arc<AttachmentInfo>, Arc<String>)>`**（upload_id, info, key）三元组。

修改 `attachment.rs` 中 `process_attachment_message` 的返回类型为：

```rust
pub fn process_attachment_message(
    ...
) -> Result<(String, OutgoingMessageResponse, Vec<(u32, Arc<AttachmentInfo>, Arc<String>)>)>
```

第三个返回值是 `Vec<(upload_id, AttachmentInfo, key)>`，调用方可以直接用它创建临时文件。

**实际的实现步骤：**

- [ ] **Step 2a: 更新 `process_attachment_message` 返回三元组**

在 Task 2 的文件中，将返回值改为 `(String, OutgoingMessageResponse, Vec<(u32, Arc<AttachmentInfo>, Arc<String>)>)`。在 "attachment" 和 "multi" 的处理分支中，每次分配 upload_id 时 push 到列表中：

```rust
let mut pending_attachments: Vec<(u32, Arc<AttachmentInfo>, Arc<String>)> = Vec::new();
// ... 在处理 attachment 项时：
pending_attachments.push((upload_id, Arc::new(info), Arc::new(key)));
```

- [ ] **Step 2b: WebMessenger::send() 中使用三元组创建临时文件**

```rust
let (new_content, response, pending_attachments) = kissbot_channel::process_attachment_message(
    &outgoing,
    msg_id.as_str(),
    self,
    &self.global_attachment_sn,
).map_err(|e| Error::InternalError(e.to_string()))?;

// 为每个 upload_id 创建临时文件
for (upload_id, info, key) in pending_attachments {
    let (temp_path, target_path) = match self.attachment_store.create_temp_file(
        outgoing.group_id.as_str(), msg_id.as_str(), info.filename.as_str()
    ) {
        Ok(paths) => paths,
        Err(e) => return Err(Error::from(e)),
    };
    self.pending_uploads.insert(upload_id, PendingAttachment {
        group_id: outgoing.group_id.clone(),
        msg_id: msg_id.clone(),
        filename: info.filename.clone(),
        mime_type: info.mime_type.clone(),
        size_bytes: info.size_bytes,
        temp_path,
        target_path,
    });
}
```

- [ ] **Step 3: 修改 IncomingMessage 构造 — 使用 new_content**

在 `send()` 中找到 IncomingMessage 构造处（~421-430 行），将 `outgoing.content.clone()` 改为 `Arc::new(new_content.clone())`：

```rust
let incoming = Arc::new(IncomingMessage {
    msg_id: msg_id.clone(),
    messenger_id: messenger_id.clone(),
    user_id: outgoing.user_id.clone(),
    group_id: outgoing.group_id.clone(),
    is_self,
    msg_type: outgoing.msg_type.clone(),
    content: Arc::new(new_content.clone()),   // 使用处理后的新 content
    time: time.clone(),
});
```

同时更新 SSE 事件构造（~467-472 行）也使用 new_content：

```rust
let sse_event = SseMessage {
    msg_id: msg_id.clone(),
    messenger_id,
    user_id: outgoing.user_id,
    group_id: outgoing.group_id,
    is_self: 1,
    msg_type: outgoing.msg_type,
    content: Arc::new(new_content),   // 处理后的新 content
    time: time.clone(),
};
```

注意 new_content 在 SSE 处被消费，所以 IncomingMessage 之前需要用 `.clone()`。

- [ ] **Step 4: 修改 outgoing 的返回 — 使用 response**

将 `send()` 方法末尾的 `Ok(OutgoingMessageResponse { ... })` 改为使用 `process_attachment_message` 返回的 `response`，但用自己生成的 `msg_id` 和 `time`：

```rust
Ok(OutgoingMessageResponse {
    msg_id,
    time,
    attachment_upload_id_map: response.attachment_upload_id_map,
    attachment_key_map: response.attachment_key_map,
})
```

- [ ] **Step 5: 修改 http.rs — `handle_init_attachment`**

`handle_init_attachment` 中构造 OutgoingMessage 时不再设置 `attachment_map`：

```rust
let outgoing = OutgoingMessage {
    messenger_id: messenger.messenger_id.clone(),
    user_id: ADMIN_USER_ID.clone(),
    group_id: req.group_id.clone(),
    msg_type: Arc::new(MSG_TYPE_ATTACHMENT.to_string()),
    content: Arc::new(content),    // 序列化后的 AttachmentInfo JSON
    // attachment_map 已移除
};
```

同时更新 `handle_send_message` 和 `build_message_content`：

- `build_message_content` 中附件相关的逻辑（"mixed" 类型）需要改为使用 `MSG_TYPE_ATTACHMENT`
- 将 `msg_type` 从 `"mixed"` 改为 `"multi"`，content 改为 `MessageItem[]` JSON 格式

```rust
fn build_message_content(req: &SendMessageRequest) -> (String, String) {
    let atts = req.attachments.as_deref().unwrap_or_default();
    if atts.is_empty() {
        return (req.content.to_string(), MSG_TYPE_TEXT.to_string());
    }
    // 构建 multi 类型消息
    let mut items = Vec::new();
    // 文本部分
    if !req.content.is_empty() {
        items.push(serde_json::json!({
            "msg_type": MSG_TYPE_TEXT,
            "content": req.content,
        }));
    }
    // 附件部分
    for a in atts {
        let info = AttachmentInfo {
            att_id: Arc::new(a.key.clone()),  // 暂用 key 作为 att_id
            filename: a.filename.clone(),
            mime_type: Arc::new(mime_guess::from_path(a.filename.as_str())
                .first_or_octet_stream().to_string()),
            size_bytes: 0,  // HTTP 上传时未知，由 init 阶段确定
        };
        items.push(serde_json::json!({
            "msg_type": MSG_TYPE_ATTACHMENT,
            "content": serde_json::to_value(&info).unwrap(),
        }));
    }
    let content = serde_json::to_string(&items).unwrap();
    (content, MSG_TYPE_MULTI.to_string())
}
```

- [ ] **Step 6: 编译验证**

```bash
cd kissbot-channel-web && cargo check
```

Expected: 编译通过。

- [ ] **Step 7: Commit**

```bash
git add kissbot-channel-web/src/messenger.rs kissbot-channel-web/src/http.rs
git commit -m "refactor: channel-web 适配新消息类型体系

- WebMessenger 实现 AttachmentKeyGenerator trait
- send() 改用 process_attachment_message 统一处理附件
- http handler 适配无 attachment_map 的 OutgoingMessage
- build_message_content 改为 multi + attachment 格式

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 4: 清理与测试

**Files:**
- Modify: `kissbot-api/src/message.rs` — 移除 `MSG_TYPE_IMAGE`/`MSG_TYPE_FILE`
- Modify: `kissbot-api/src/channel.rs` — 清理测试中旧的 attachment_map 引用

**接口：** 无外部依赖

- [ ] **Step 1: 移除旧的 msg_type 常量**

从 `message.rs` 中删除 `MSG_TYPE_IMAGE` 和 `MSG_TYPE_FILE`：

```rust
// 以下常量已移除，由 MSG_TYPE_ATTACHMENT + mime_type 替代：
// pub const MSG_TYPE_IMAGE: &str = "image";
// pub const MSG_TYPE_FILE: &str = "file";
```

- [ ] **Step 2: 确保所有测试通过**

```bash
cd kissbot-api && cargo test
cd kissbot-channel && cargo test
cd kissbot-channel-web && cargo check
```

Expected: 全部通过。

- [ ] **Step 3: 确认无旧版 msg_type 引用**

```bash
grep -rn "MSG_TYPE_IMAGE\|MSG_TYPE_FILE\|msg_type.*\"image\"\|msg_type.*\"file\"\|msg_type.*\"mixed\"" --include="*.rs" | grep -v target
```

如果有残留引用，更新。

- [ ] **Step 4: Commit**

```bash
git add kissbot-api/src/message.rs
git commit -m "cleanup: 移除旧的 MSG_TYPE_IMAGE/MSG_TYPE_FILE 常量

- 由 MSG_TYPE_ATTACHMENT + mime_type 替代
- 清理测试中残留的旧类型引用

Co-Authored-By: deepseek-v4-flash"
```
