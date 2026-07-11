# 移除 size=0 结束块实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 去掉上传和下载流程中最后发一个 size=0 块的机制，改为由最后一块实际数据块（pos+size == file_size）触发清理。

**Architecture:** 在 `channel_manager.rs` 中将 `attachment_receiver_map` 和 `attachment_sender_map` 从存简单 `Weak` 指针改为存包含 `Arc<AttachmentInfo>` 的结构体，处理器通过 `pos + size >= info.size_bytes` 判断是否最后一块；`attachment.rs` 删除末尾 size=0 发包代码。

**Tech Stack:** Rust, tokio, dashmap

**Cargo build check:** 在 kissbot-channel 和 kissbot-channel-web 目录下分别运行 `cargo build`

---

### Task 1: attachment_receiver_map 改为存 AttachmentReceiverContext

**Files:**
- Modify: `kissbot-channel/src/channel_manager.rs`

**Interfaces:**
- Produces: `AttachmentReceiverContext` 结构体，包含 `messenger: Weak<dyn Messenger>` 和 `info: Arc<AttachmentInfo>`
- Consumes: `Arc<AttachmentInfoResponse>` 中的 `info` 字段

- [ ] **Step 1: 添加 AttachmentReceiverContext 结构体和 import**

在 `ConnectContext` 结构体之后、`ChannelManager` 之前新增：

```rust
struct AttachmentReceiverContext {
    pub messenger: Weak<dyn Messenger>,
    pub info: Arc<AttachmentInfo>,
}
```

在 import 中将 `AttachmentInfoResponse` 改为 `{AttachmentInfo, AttachmentInfoResponse}`：

```rust
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse, Content};
```

- [ ] **Step 2: 修改 attachment_receiver_map 类型**

```rust
// 上传方向：transfer_id → AttachmentReceiverContext
attachment_receiver_map: DashMap<u32, AttachmentReceiverContext>,
```

- [ ] **Step 3: 更新 register_attachment_receivers()**

原代码：
```rust
Content::AttachmentInfoResponse(resp) => {
    manager.attachment_receiver_map.insert(resp.transfer_id, messenger.clone());
}
```

改为：
```rust
Content::AttachmentInfoResponse(resp) => {
    manager.attachment_receiver_map.insert(resp.transfer_id, AttachmentReceiverContext {
        messenger: messenger.clone(),
        info: resp.info.clone(),
    });
}
```

- [ ] **Step 4: 更新 AttachmentPayloadProcessor::raw_process_bin()**

原代码（第 356-391 行）：
```rust
async fn raw_process_bin(&self, data: Bytes) -> Result<Option<serde_json::Value>> {
    let header = parse_attachment_payload_header(data.as_ref())?;

    let manager = self.manager.upgrade()
    .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;

    // 通过 transfer_id 找到 messenger
    let messenger = manager.attachment_receiver_map.get(&header.id)
        .ok_or_else(|| Error::AttachmentNotFound(header.id.to_string()))?
        .upgrade()
        .ok_or_else(|| Error::InternalError("messenger is None".to_string()))?;

    if header.size == 0 {
        manager.attachment_receiver_map.remove(&header.id);
    }

    let mut success = false;
    let payload = data.slice(OFFSET_ATT_DATA..);
    let result = match messenger.send_attachment_payload(header.id, header.size, header.pos, payload).await {
        Ok(response) => {
            match serde_json::to_value(&response) {
                Ok(value) => {
                    success = response.error_code == 0;
                    Ok(Some(value))
                }
                Err(e) => Err(Error::from(e))
            }
        }
        Err(e) => Err(e)
    };

    if !success {
        manager.attachment_receiver_map.remove(&header.id);
    }

    result
}
```

改为：
```rust
async fn raw_process_bin(&self, data: Bytes) -> Result<Option<serde_json::Value>> {
    let header = parse_attachment_payload_header(data.as_ref())?;

    let manager = self.manager.upgrade()
    .ok_or_else(|| Error::InternalError("manager is None".to_string()))?;

    // 通过 transfer_id 找到 messenger 和 info
    let receiver = manager.attachment_receiver_map.get(&header.id)
        .ok_or_else(|| Error::AttachmentNotFound(header.id.to_string()))?;
    let messenger = receiver.messenger.upgrade()
        .ok_or_else(|| Error::InternalError("messenger is None".to_string()))?;
    let file_size = receiver.info.size_bytes;
    drop(receiver);

    let mut success = false;
    let payload = data.slice(OFFSET_ATT_DATA..);
    let result = match messenger.send_attachment_payload(header.id, header.size, header.pos, payload).await {
        Ok(response) => {
            match serde_json::to_value(&response) {
                Ok(value) => {
                    success = response.error_code == 0;
                    Ok(Some(value))
                }
                Err(e) => Err(Error::from(e))
            }
        }
        Err(e) => Err(e)
    };

    // 根据 pos+size 判断是否最后一块，或错误时清理
    if header.pos as u64 + header.size as u64 >= file_size || !success {
        manager.attachment_receiver_map.remove(&header.id);
    }

    result
}
```

- [ ] **Step 5: 编译检查**

```bash
cargo build
```

- [ ] **Step 6: 提交**

```bash
git add kissbot-channel/src/channel_manager.rs
git commit -m "refactor: attachment_receiver_map 改为存储 AttachmentReceiverContext

- 新增 AttachmentReceiverContext 结构体（messenger + info）
- attachment_receiver_map 从 DashMap<u32, Weak<dyn Messenger>>
  改为 DashMap<u32, AttachmentReceiverContext>
- register_attachment_receivers() 一并存入 info
- AttachmentPayloadProcessor 改为 header.pos+size >= info.size_bytes
  判断最后一块，移除 size==0 的检测

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2: attachment_sender_map 改为存 AttachmentSenderContext

**Files:**
- Modify: `kissbot-channel/src/channel_manager.rs`

**Interfaces:**
- Consumes: `Arc<AttachmentInfoResponse>` 中的 `info.size_bytes`
- Produces: `AttachmentSenderContext` 结构体

- [ ] **Step 1: 添加 AttachmentSenderContext 结构体**

在 `AttachmentReceiverContext` 之后新增：

```rust
struct AttachmentSenderContext {
    pub connect_context: Weak<ConnectContext>,
    pub info: Arc<AttachmentInfo>,
}
```

- [ ] **Step 2: 修改 attachment_sender_map 类型和 ChannelManager::new()**

```rust
// 下载方向：transfer_id → AttachmentSenderContext
attachment_sender_map: DashMap<u32, AttachmentSenderContext>,
```

- [ ] **Step 3: 更新 AttachmentDownloadRequestProcessor::process_download_request_header()**

原代码第 450-452 行：
```rust
let att_info_response = messenger.download_attachment_header(request).await?;
let transfer_id = att_info_response.transfer_id;
manager.attachment_sender_map.insert(transfer_id, Arc::downgrade(&connect_context));
```

改为：
```rust
let att_info_response = messenger.download_attachment_header(request).await?;
let transfer_id = att_info_response.transfer_id;
manager.attachment_sender_map.insert(transfer_id, AttachmentSenderContext {
    connect_context: Arc::downgrade(&connect_context),
    info: att_info_response.info.clone(),
});
```

- [ ] **Step 4: 更新 ChannelManager::send()**

原代码第 802-846 行：
```rust
#[async_trait]
impl AttachmentDownloadPayloadSender for ChannelManager {
    fn prepare_send(&self, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)> {
        let connect_context = self.attachment_sender_map.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?
            .upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

        let sn = connect_context.ws_context.next_request_sn();
        let capacity = OFFSET_ATT_DATA + size as usize;
        let mut buf = BytesMut::with_capacity(capacity);
        buf.put_u32(sn);
        buf.put_u32(TYPE_ATTACHMENT_PAYLOAD);
        buf.put_u32(CODE_SUCCESS);
        buf.put_u32(transfer_id);
        buf.put_u32(size);
        buf.put_u64(pos);
        Ok((sn, buf))
    }

    async fn send(&self, sn: u32, transfer_id: u32, size: u32, pos: u64, buf: BytesMut) -> Result<AttachmentPayloadResponse> {
        let connect_context = self.attachment_sender_map.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?
            .upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;

        if size == 0 {
            self.attachment_sender_map.remove(&transfer_id);
            connect_context.ws_context.send_bin(buf.freeze()).await?;
            return Ok(AttachmentPayloadResponse {
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
}
```

改为：
```rust
#[async_trait]
impl AttachmentDownloadPayloadSender for ChannelManager {
    fn prepare_send(&self, transfer_id: u32, size: u32, pos: u64) -> Result<(u32, BytesMut)> {
        let sender_info = self.attachment_sender_map.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?;
        let connect_context = sender_info.connect_context.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;
        drop(sender_info);

        let sn = connect_context.ws_context.next_request_sn();
        let capacity = OFFSET_ATT_DATA + size as usize;
        let mut buf = BytesMut::with_capacity(capacity);
        buf.put_u32(sn);
        buf.put_u32(TYPE_ATTACHMENT_PAYLOAD);
        buf.put_u32(CODE_SUCCESS);
        buf.put_u32(transfer_id);
        buf.put_u32(size);
        buf.put_u64(pos);
        Ok((sn, buf))
    }

    async fn send(&self, sn: u32, transfer_id: u32, size: u32, pos: u64, buf: BytesMut) -> Result<AttachmentPayloadResponse> {
        let sender_info = self.attachment_sender_map.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?;
        let connect_context = sender_info.connect_context.upgrade()
            .ok_or_else(|| Error::InternalError("connect context is None".to_string()))?;
        let file_size = sender_info.info.size_bytes;

        // 判断是否为最后一块
        let is_last = pos + size as u64 >= file_size;
        if is_last {
            self.attachment_sender_map.remove(&transfer_id);
        }
        drop(sender_info);

        let result = self.send_download_attachment_payload(sn, buf, connect_context).await;

        // 错误时清理（最后一块已清理过，remove 是幂等的）
        if match result.as_ref() { Ok(res) => res.error_code != 0, Err(_) => true } {
            self.attachment_sender_map.remove(&transfer_id);
        }

        result
    }
}
```

- [ ] **Step 5: 编译检查**

```bash
cargo build
```

- [ ] **Step 6: 提交**

```bash
git add kissbot-channel/src/channel_manager.rs
git commit -m "refactor: attachment_sender_map 改为存储 AttachmentSenderContext

- 新增 AttachmentSenderContext 结构体（connect_context + info）
- attachment_sender_map 从 DashMap<u32, Weak<ConnectContext>>
  改为 DashMap<u32, AttachmentSenderContext>
- process_download_request_header() 一并存入 info
- ChannelManager::send() 移除 size==0 分支，改为
  pos+size >= file_size 判断最后一块触发清理
- prepare_send() 适配新 map 类型

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 3: send_download_payload 移除 size=0 发包

**Files:**
- Modify: `kissbot-channel-web/src/attachment.rs`

- [ ] **Step 1: 删除 size=0 发包代码**

原代码第 227-231 行：
```rust
        // 发送 size=0 的结束标记
        if let Ok((sn, buf)) = sender.prepare_send(transfer_id, 0, pos) {
            let _ = sender.send(sn, transfer_id, 0, pos, buf).await;
        }
```

删除后 `send_download_payload()` 末尾变为：
```rust
        // 下载完成，清理 transfer_key_map
        self.transfer_key_map.remove(&transfer_id);

        Ok(())
    }
```

- [ ] **Step 2: 编译检查**

```bash
cargo build
```

- [ ] **Step 3: 提交**

```bash
git add kissbot-channel-web/src/attachment.rs
git commit -m "refactor: send_download_payload 移除 size=0 发包

- 循环结束后不再发送 size=0 的结束标记
- 最后一块的清理由 ChannelManager::send() 根据 pos+size 判断触发

Co-Authored-By: deepseek-v4-flash"
```
