# kissbot-api 单元测试设计

为 `kissbot-api` crate 编写单元测试，覆盖 channel / message / common / ego / store 五个模块。同时重构 `parse_attachment_payload_header` 使其使用 `data.get()` 安全解析模式。

## 重构

### channel.rs — parse_attachment_payload_header

内部改用 `data.get() + and_then + try_into().ok() + ok_or(kai_ws::Error::BinParse)` 替代 `data[a..b].try_into()?`，返回类型从 `Result<AttachmentPayloadHeader, TryFromSliceError>` 改为 `Result<AttachmentPayloadHeader, kai_ws::Error>`。

```rust
pub fn parse_attachment_payload_header(data: &[u8]) -> std::result::Result<AttachmentPayloadHeader, kai_ws::Error> {
    let id_bytes: [u8; 4] = data.get(OFFSET_ATT_ID..OFFSET_ATT_ID + LEN_ATT_ID)
        .and_then(|s| s.try_into().ok())
        .ok_or(kai_ws::Error::BinParse)?;
    let id = u32::from_be_bytes(id_bytes);
    let size_bytes: [u8; 4] = data.get(OFFSET_ATT_SIZE..OFFSET_ATT_SIZE + LEN_ATT_SIZE)
        .and_then(|s| s.try_into().ok())
        .ok_or(kai_ws::Error::BinParse)?;
    let size = u32::from_be_bytes(size_bytes);
    let pos_bytes: [u8; 8] = data.get(OFFSET_ATT_POS..OFFSET_ATT_POS + LEN_ATT_POS)
        .and_then(|s| s.try_into().ok())
        .ok_or(kai_ws::Error::BinParse)?;
    let pos = u64::from_be_bytes(pos_bytes);
    Ok(AttachmentPayloadHeader { id, size, pos })
}
```

移除不再需要的 `use std::array::TryFromSliceError;`。

## 测试文件结构

测试内联在各模块末尾的 `#[cfg(test)] mod tests` 中。所有测试同步 `#[test]`，无异步依赖。

## 无需 dev-dependencies

## 测试分组

### 1. channel.rs（12 个测试）

| 测试 | 类型 | 说明 |
|------|------|------|
| `test_parse_attachment_header_ok` | 解析 | 20 字节 buffer，验证 id/size/pos 正确 |
| `test_parse_attachment_header_too_short` | 解析 | buffer < 20 字节，验证 Err(BinParse) |
| `test_serde_group_change_notification` | roundtrip | GroupChangeNotification |
| `test_serde_user_remove_notification` | roundtrip | UserRemoveNotification |
| `test_serde_group_info` | roundtrip | GroupInfo |
| `test_serde_user_info` | roundtrip | UserInfo（含 DashMap） |
| `test_serde_messenger_info` | roundtrip | MessengerInfo（含 DashMap） |
| `test_serde_messenger_info_request` | roundtrip | MessengerInfoRequest |
| `test_serde_attachment_info` | roundtrip | AttachmentInfo |
| `test_serde_outgoing_message` | roundtrip | OutgoingMessage（含 DashMap） |
| `test_serde_outgoing_message_response` | roundtrip | OutgoingMessageResponse（含 DashMap） |
| `test_serde_attachment_download_request` | roundtrip | AttachmentDownloadRequest |
| `test_serde_attachment_download_response_header` | roundtrip | AttachmentDownloadResponseHeader |
| `test_serde_incoming_message` | roundtrip | IncomingMessage |
| `test_serde_bind_request` | roundtrip | BindRequest |

共 15 个测试。AttachmentPayloadHeader 无 serde，不做 roundtrip。

### 2. message.rs（1 个测试）

| 测试 | 说明 |
|------|------|
| `test_serde_message_item` | 构造包含所有 MSG_TYPE_* 常量的 MessageItem，验证 roundtrip |

### 3. common.rs（2 个测试）

| 测试 | 说明 |
|------|------|
| `test_api_response_success` | ApiResponse::success() roundtrip，验证 JSON 含 success=true 和 data |
| `test_api_response_error` | ApiResponse::error() roundtrip，验证 JSON 含 success=false 和 error |

### 4. ego.rs（34 个测试）

核心 struct 10 个 + request struct 24 个，全部做 roundtrip。

核心 struct：RoleKey / IndividualIdentifier / IndividualRelation / Individual / IndividualRecognition / AgentMetadata / RoleRelation / Role / OtherRole / RolePlay

Agent Management 请求：CreateAgentRequest / GetAgentRequest / UpdateAgentNameRequest / UpdateAgentDescriptionRequest / CopyAgentRequest / SearchRequest / SearchRoleRequest / RetrieveAgentsRequest / RetrieveRolesRequest / NameCompletionRequest / RoleNameCompletionRequest

Individual Recognition 请求：GetIndividualsRequest / GetIndividualRequest / ReplaceIndividualsRequest / RenameIndividualRequest / ReplaceIndividualIdentifiersRequest / ReplaceIndividualRelationsRequest

Role Play 请求：ListRolesRequest / GetRoleRequest / CreateRoleRequest / CreateRoleFromRequest / RemoveRoleRequest / RenameRoleRequest / UpdateRoleDescriptionRequest / GetOtherRoleRequest / ReplaceOtherRolesRequest / RenameOtherRoleRequest / UpdateOtherRoleIndividualNameRequest / UpdateOtherRoleDescriptionRequest / UpdateOtherRoleRelationRequest / ReplaceOtherRoleRelationsRequest

注意：含 DashMap/DashSet 的 struct（Individual / IndividualRecognition / OtherRole / RolePlay / ReplaceIndividualsRequest / ReplaceIndividualIdentifiersRequest / ReplaceIndividualRelationsRequest / ReplaceOtherRolesRequest / ReplaceOtherRoleRelationsRequest 等）在构造测试数据时需要填充内部结构。

### 5. store.rs（12 个测试）

ChannelRequest / ChannelRequests / ThinkRequest / ThinkRequests / ToolCallRequest / ToolCallRequests / ToolResultRequest / ToolResultRequests / QueryChannelRequest / QueryRequest / ChannelRecord / ThinkRecord / ToolCallRecord / ToolResultRecord — 全部 roundtrip。

共 14 个测试。

## 总数

- channel.rs: 15 个测试（2 解析 + 13 roundtrip）
- message.rs: 1 个测试
- common.rs: 2 个测试
- ego.rs: 34 个测试
- store.rs: 14 个测试
- **合计: 66 个测试用例**
