# 移除 size=0 结束块设计文档

## 概述

去掉上传和下载流程中最后发送一个 size=0 的数据块作为结束标记的机制，改为由最后一块实际数据块（pos + size == file_size）直接触发结束动作。

## 动机

原设计中，上传/下载完成后需要额外发送一个 size=0 的空数据包来触发清理（移除 transfer ID 记录等）。这种方式增加了一次不必要的网络往返。改为通过最后一块数据包本身判断结束，减少一次发包，简化流程。

## 设计原则

1. **存储 file_size 信息**：在 routing map 中存储 `Arc<AttachmentInfo>`，使处理器能通过 `pos + size >= info.size_bytes` 判断是否最后一块
2. **原子清理**：最后一块发送/处理成功后立即清理 map，无需额外包
3. **兼容性**：二进制协议格式不变（header 中依然有 size/pos 字段），仅改变结束判断逻辑

## 变更模块

### 1. 新增结构体

**涉及文件：** `kissbot-channel/src/channel_manager.rs`

```rust
struct AttachmentReceiverContext {
    messenger: Weak<dyn Messenger>,
    info: Arc<AttachmentInfo>,       // 含 size_bytes（上传时由 nexus 传入）
}

struct AttachmentSenderContext {
    connect_context: Weak<ConnectContext>,
    info: Arc<AttachmentInfo>,       // 含 size_bytes（下载时由 metadata 获取）
}
```

### 2. attachment_receiver_map — 上传方向

**涉及文件：** `kissbot-channel/src/channel_manager.rs`

**当前：**
```rust
attachment_receiver_map: DashMap<u32, Weak<dyn Messenger>>
```

**改为：**
```rust
attachment_receiver_map: DashMap<u32, AttachmentReceiverContext>
```

**受影响的位置：**

- `register_attachment_receivers()`：将 `AttachmentInfoResponse.info` 连同 messenger 一起存入
- `AttachmentPayloadProcessor::raw_process_bin()`：
  - 原来 `if header.size == 0 { remove }` 改为
  - 在处理完数据后判断 `if header.pos + header.size >= receiver.info.size_bytes { remove }`
  - 错误时同样清理

### 3. attachment_sender_map — 下载方向

**涉及文件：** `kissbot-channel/src/channel_manager.rs`

**当前：**
```rust
attachment_sender_map: DashMap<u32, Weak<ConnectContext>>
```

**改为：**
```rust
attachment_sender_map: DashMap<u32, AttachmentSenderContext>
```

**受影响的位置：**

- `AttachmentDownloadRequestProcessor::process_download_request_header()`：在 `messenger.download_attachment_header()` 返回的 `AttachmentInfoResponse.info` 中获取 `size_bytes`，连同 ConnectContext 一起存入
- `ChannelManager::send()`：
  - 去掉 `if size == 0` 的特殊处理分支
  - 正常发送前判断 `if pos + size >= info.size_bytes { remove }`
  - 错误时同样清理

### 4. send_download_payload — 移除 size=0 发包

**涉及文件：** `kissbot-channel-web/src/attachment.rs`

**当前：**
```rust
// 发送 size=0 的结束标记
if let Ok((sn, buf)) = sender.prepare_send(transfer_id, 0, pos) {
    let _ = sender.send(sn, transfer_id, 0, pos, buf).await;
}
// 下载完成，清理 transfer_key_map
self.transfer_key_map.remove(&transfer_id);
```

**改为：** 直接删除 size=0 的发送代码，仅保留 `transfer_key_map.remove()`

## 数据流对比

### 上传方向（nexus → channel-web）

```
当前：
 nexus 发 data chunk → AttachmentPayloadProcessor → write_chunk
 nexus 发 size=0    → AttachmentPayloadProcessor → receiver_map.remove(id)

改后：
 nexus 发 data chunk（最后一个 pos+size==file_size）→ AttachmentPayloadProcessor
   → write_chunk + receiver_map.remove(id)
```

### 下载方向（channel-web → nexus）

```
当前：
 send_download_payload 发 data chunk → ChannelManager::send → nexus 响应
 send_download_payload 发 size=0    → ChannelManager::send → sender_map.remove(id)

改后：
 send_download_payload 发 data chunk（最后一个 pos+size==file_size）
   → ChannelManager::send（发送前 sender_map.remove(id)）→ nexus 响应
```

## 涉及的 crate 和文件

| Crate | 文件 | 改动 |
|-------|------|------|
| kissbot-channel | src/channel_manager.rs | 新增 `*Context` 结构体、改 map 类型、改判断逻辑 |
| kissbot-channel-web | src/attachment.rs | 删除 size=0 发包代码 |
