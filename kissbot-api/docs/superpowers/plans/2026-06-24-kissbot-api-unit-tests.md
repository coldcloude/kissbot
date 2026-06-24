# kissbot-api 单元测试实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 kissbot-api crate 编写 66 个单元测试，覆盖 channel/message/common/ego/store 五个模块，同时重构 parse_attachment_payload_header

**Architecture:** 测试内联在各模块末尾的 `#[cfg(test)] mod tests` 中。全部同步 `#[test]`。先重构 channel.rs 的二进制解析函数，再按模块逐个添加测试。ego.rs 和 store.rs 的 roundtrip 测试较多，各拆为多个 task。

**Tech Stack:** Rust, serde, serde_json, kai-ws, dashmap

**设计文档:** `kissbot-api/docs/superpowers/specs/2026-06-24-kissbot-api-test-design.md`

---

## 文件结构

- **Modify:** `kissbot-api/src/channel.rs` — 重构 parse_attachment_payload_header + 添加测试
- **Modify:** `kissbot-api/src/message.rs` — 添加测试
- **Modify:** `kissbot-api/src/common.rs` — 添加测试
- **Modify:** `kissbot-api/src/ego.rs` — 添加测试
- **Modify:** `kissbot-api/src/store.rs` — 添加测试

---

### Task 1: 重构 parse_attachment_payload_header

**Files:**
- Modify: `kissbot-api/src/channel.rs`

- [ ] **Step 1: 重构函数**

修改 `parse_attachment_payload_header`，内部改用 `data.get() + and_then + try_into().ok() + ok_or(kai_ws::Error::BinParse)` 替代 `data[a..b].try_into()?`：

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

同时移除文件顶部的 `use std::array::TryFromSliceError;`。

- [ ] **Step 2: 编译验证**

```bash
cd /home/admin/project/kissbot/kissbot-api && cargo check 2>&1
```

Expected: 编译通过，无错误。

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-api && git add src/channel.rs && git commit -m "refactor: parse_attachment_payload_header 改用 data.get() 安全解析

替换 data[a..b].try_into()? 为 data.get() + and_then + try_into().ok() + ok_or，
返回类型改为 Result<..., kai_ws::Error>。

Co-Authored-By: deepseek-v4-flash"
```

---

### Task 2 — Task 9

（完整 plan 过长，在实施时逐 task 提供完整代码）
