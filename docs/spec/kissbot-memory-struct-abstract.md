# kissbot-memory-struct-abstract 技术规格

> 说明摘要记忆实现模块的对外接口、通知协议、触发判定、配置结构与存储格式。
> 组件设计与职责边界见 [kissbot-memory-struct-abstract 组件设计](../design/components-design/kissbot-memory-struct-abstract.md)，框架接口见 [kissbot-memory-struct 组件设计](../design/components-design/kissbot-memory-struct.md)，原始记录格式见 [memory-directory.md](memory-directory.md)。

## 位置与依赖

代码位置：`kissbot-memory-struct-abstract/`。

| 依赖 | 用途 |
|------|------|
| kissbot-api | `ApiResponse`、网络地址配置（memory-store 地址）、本组件的请求/响应类型、`ArcSwapHashMap` |
| kissbot-config | 公共配置读取（本组件配置段） |
| kissbot-security | API 认证（`X-Api-Key`） |
| kissbot-memory-struct | 记忆结构框架接口 |
| kissbot-memory | 记录类型、路径构造、目录管理与索引查询 |
| kissbot-llm | provider 调用与会话上下文 |
| kai-ws | 连接记忆存储的通知服务（WS 客户端） |
| kai-file | 原始记录的反向读取 |
| axum / tokio / serde / chrono / thiserror | HTTP 服务、异步运行时、序列化、时间、错误 |

不依赖 kissbot-agent / kissbot-channel / kissbot-mcp。

## 对外 HTTP 接口

所有参数放在 JSON 请求体中，路径不含动态参数；响应统一使用 `ApiResponse`；认证走 `X-Api-Key`。

| 接口 | 用途 |
|------|------|
| `POST /abstract/locate` | 传入 agent、角色与新消息，返回所属事件与上下文 |
| `POST /abstract/event/list` | 按作用域与父事件查询事件节点 |
| `POST /abstract/event/get` | 取事件详情（摘要、原文副本、层级） |
| `POST /abstract/tree/get` | 取某作用域的事件树 |
| `POST /abstract/status/get` | 取各作用域的水位与处理状态 |
| `POST /abstract/rebuild` | 手动触发或重建某作用域的归档 |

请求与响应类型作为模块专属类型定义在 kissbot-api（见 [system-design-api](../design/system-design-api.md)）。

### locate

```jsonc
// 请求
{
  "agent_id": "0",
  "role": "dev",                       // 记忆后缀；事件模式下形如 "dev-e1"
  "message": {
    "time": "2026-09-22 13:48:37",
    "user_name": "alice",
    "is_self": 0,
    "content": "…"                     // 统一的公共消息格式
  }
}

// 响应 data
{
  "matched": true,
  "event": { "event_id": "e12", "summary": "…", "is_current": true },
  "children": [ { "event_id": "e13", "summary": "…" } ],   // 命中事件为父事件时给出
  "records": [ /* 预算内的原文副本，四类记录按时间排序 */ ]
}
```

约定：

- 未命中返回 `matched: false`，不携带事件与记录。
- 定位顺序：先与当前事件比对，未命中再自顶层逐层下探。
- 原文副本的预算按记录条数或字符数截断，取最近的部分。

## 通知协议

记忆存储模块在写入新记录后，向已连接的本组件广播 WS 文本帧。通知不区分写入方式。

```jsonc
// 通道消息
{ "type": "channel", "agent_id": "0", "role": "dev",
  "time": "2026-09-22 13:48:37", "is_self": 0 }

// 思考 / 工具调用 / 工具结果
{ "type": "think",       "agent_id": "0", "role": "dev", "time": "2026-09-22 13:48:37" }
{ "type": "tool-call",   "agent_id": "0", "role": "dev", "time": "2026-09-22 13:48:37" }
{ "type": "tool-result", "agent_id": "0", "role": "dev", "time": "2026-09-22 13:48:37" }
```

- `role` 为记忆后缀：角色模式为角色名，事件模式为 `角色名-事件id`。
- 通道消息通知携带 `time`、`agent_id`、`role` 与 `is_self`；其余类型只用于登记与去重，不触发归档。
- 本组件按 `(agent_id, role)` 维护水位，收到通知后从共享记忆文件读取水位之后的记录。

## 触发判定

按 `(agent_id, role)` 维护最后一条消息时间与已处理水位；触发判定只看水位之后的未归档范围。

| 条件 | 归档范围 |
|------|----------|
| 未归档范围中存在本人通道消息，且其后出现非本人通道消息 | 截止到最后一条本人消息（含） |
| 当前时间 − 最后一条消息时间 > 超时阈值 | 截止到最后一条消息（含） |

- 归档后水位推进到归档范围末尾；范围之后的记录留待下次触发。
- 已归档的本人消息不再触发；其后的非本人消息由下一条本人消息或超时一并归档。
- 思考与工具记录不参与触发判定，随所属时间范围一并归档。
- 超时判定按作用域独立进行，无未归档记录的作用域不触发。

## 配置结构

```jsonc
{
  "memory_struct_abstract": {
    "data_dir": "workspace",              // 记忆根目录
    "archive_timeout_secs": 300,          // 归档触发超时阈值
    "provider": {                         // provider 配置，结构见 kissbot-llm spec
      "provider_type": "openai",
      "base_url": "https://api.deepseek.com",
      "api_key": "sk-…"
    },
    "default_model_config": { "timeout_secs": 60, "retry_count": 3 },
    "model_configs": { "deepseek-flash": { "max_tokens_usage": 131072 } },
    "llm": { "provider": "deepseek", "model": "deepseek-flash", "max_tokens": 4096 }
  }
}
```

- memory-store 的 HTTP 地址取自 kissbot-api 的网络地址配置；通知地址为其 WS 端点。
- provider 与模型配置复用 kissbot-llm 的配置结构；本组件只读取自己的配置段。

## 存储格式

事件数据位于记忆根目录下各 agent 目录的 `memory-struct-abstract/`，按记忆后缀分作用域：

```
{根目录}/{agent-id}/memory-struct-abstract/
├── {role}/
│   ├── tree.json                    # 事件节点与层级
│   ├── current.json                 # 当前事件指针
│   ├── watermark.json               # 已处理水位
│   └── events/{event-id}/
│       └── records-{date}.jsonl     # 原文副本（四类记录，JSON Lines）
```

- `{role}` 为记忆后缀；事件模式作用域只有一个顶层事件，`event-id` 取后缀中的事件标识。
- 角色模式新建事件的 `event-id` 由本组件分配（作用域内递增）。
- 原文副本按时间顺序追加，行内容与记忆存储的记录格式一致。

### tree.json

```jsonc
{
  "nodes": {
    "e12": {
      "event_id": "e12",
      "parent_id": null,
      "children": ["e13"],
      "mode": "role",                  // "role" | "event"
      "summary": "…",
      "record_count": 42,
      "start_time": "2026-09-22 10:00:00",
      "end_time": "2026-09-22 13:48:37",
      "created_at": "2026-09-22 10:05:00",
      "updated_at": "2026-09-22 13:49:00"
    }
  }
}
```

### current.json / watermark.json

```jsonc
// current.json：当前事件指针
{ "event_id": "e13" }

// watermark.json：各记录类型的已处理位置（时间 + 序号）
{
  "channel":     { "time": "2026-09-22 13:48:37", "sn": 128 },
  "think":       { "time": "2026-09-22 13:48:30", "sn": 64 },
  "tool-call":   { "time": "2026-09-22 13:48:31", "sn": 30 },
  "tool-result": { "time": "2026-09-22 13:48:32", "sn": 30 }
}
```

## 模型交互

| 环节 | 输入 | 输出 |
|------|------|------|
| 归档第 1 轮 | 批次内容 + 当前事件摘要 | 并入当前事件 / 新事件（含批内切分点） |
| 归档分层归并 | 批次片段 + 当层全部事件摘要 | 当层新开事件 / 并入某事件 |
| 摘要生成 | 事件直接记录 + 子事件摘要 | 事件摘要 |
| 事件定位 | 新消息 + 当前事件摘要（下探时改为当层事件摘要） | 命中事件 / 未命中 |

- 各环节要求结构化输出，解析失败按失败处理并保留原状态，等待下次触发重试。
- 会话标识按作用域编码，同一作用域的连续判定共享上下文。

## 错误类型

```rust
enum Error {
    NotifyError(String),        // 通知连接与帧解析
    StoreReadError(String),     // 原始记录读取
    ModelApiError(String),      // 模型调用（含映射自 kissbot-llm 的错误）
    StateError(String),         // 事件树、水位、指针的读写与一致性
    IoError(std::io::Error),
    JsonError(serde_json::Error),
    FileError(kai_file::Error),
    InternalError(String),
}
pub type Result<T> = std::result::Result<T, Error>;
```

## 验证

- 单元测试：`cd kissbot-memory-struct-abstract && cargo test`（触发判定、分层下探决策、摘要级联、水位推进、事件树读写、定位匹配、通知帧解析）。
- 集成测试：`test/` 下用固定记忆数据目录验证「通知 → 归档 → 定位」全流程，模型调用以桩实现替换。
