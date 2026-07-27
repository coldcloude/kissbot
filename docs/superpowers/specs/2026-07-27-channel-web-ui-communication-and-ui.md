# channel-web-ui 前后端通信适配与 UI 对齐

## 概述

将现有的 `kissbot-channel-web-ui` 前端与 `kissbot-channel-web` 后端对接，修复 API 通信断裂，同时按最新设计稿调整 UI。

## 背景

前端（React + TypeScript）和后端（Rust + Axum）独立开发，两者的 API 路径、数据格式、认证方式、SSE 事件格式均不匹配。设计稿已更新（layout.html / login.html / group-management.html / user-management.html），前端需同步对齐。

## 架构概要

- 前端 Vite 代理 `/api` → `http://127.0.0.1:8301`
- 后端所有 API 以 `/api/...` 开头，`AuthLayer` 通过 `X-Api-Key` header 验证
- 认证：`GET /api/info` — 验证 key 并返回管理员和 messenger 信息
- 实时推送：SSE `/api/events` — 推送原始 `IncomingMessage` JSON

## 通信契约（逐端接口）

### 1. 连接认证

**前端调用**：`GET /api/info`（`X-Api-Key` header）

**后端返回**：`MessengerAdminInfo`
```json
{
  "messenger_id": "web",
  "admin_name": "管理员",
  "users": {
    "u0": { "user_id": "u0", "user_name": "助手小A" },
    "u1": { "user_id": "u1", "user_name": "助手小B" }
  },
  "groups": {
    "a_u0": { "group_id": "a_u0", "group_name": "助手小A", "members": ["admin", "u0"] },
    "g0": { "group_id": "g0", "group_name": "开发组", "members": ["admin", "u0", "u1"] }
  }
}
```

前端处理：
- `users` 是对象（key → UserConfig），转为数组用于展示
- `groups` 是对象（key → GroupConfig），转为数组用于展示
- group_id 以 `a_` 开头 → admin-user 单聊群组 → 显示对应用户名，不可在群组管理中操作
- group_id 以 `g` 开头 → 多人群组 → 显示群组名 + `群组` 标记

### 2. 发送消息

**前端调用**：`POST /api/message/send`

请求体格式：
```json
{
  "messenger_id": "web",
  "user_id": "admin",
  "group_id": "a_u0",
  "msg_type": "text",
  "content": {"Text": "你好"}
}
```

msg_type 取值：
- `"text"` — 纯文本消息，content 为 `{"Text": "..."}`
- `"attachment"` — 附件消息（上传文件时用），content 为 `{"AttachmentInfo": {"file_name": "...", "mime_type": "...", "size_bytes": N}}`

**后端返回**：`OutgoingMessageResponse`
```json
{
  "msg_id": "20260727123456000000",
  "time": "2026-07-27T12:34:56.789Z",
  "msg_type": "text",
  "content": {"Text": "你好"}
}
```

### 3. 附件上传流程

1. 用户选择文件 → 获取 `file.name`, `file.type`, `file.size`
2. 发送消息 `POST /api/message/send`：
   ```json
   {
     "messenger_id": "web",
     "user_id": "admin",
     "group_id": "a_u0",
     "msg_type": "attachment",
     "content": {"AttachmentInfo": {"file_name": "photo.png", "mime_type": "image/png", "size_bytes": 1024}}
   }
   ```
3. 后端返回含 `AttachmentInfoResponse` 的 content：
   ```json
   {
     "content": {"AttachmentInfoResponse": {"key": "a_u0/uuid", "info": {"file_name": "photo.png", "mime_type": "image/png", "size_bytes": 1024}, "transfer_id": 42}}
   }
   ```
4. 前端提取 `transfer_id`
5. 调用 `POST /api/attachment/upload`（multipart）：
   - field `transfer_id`: `42`
   - field `file`: 文件二进制数据
6. 前端不发送 `Multi` 消息，文本和附件始终分为两条消息发送

### 4. 消息历史

首次加载：`GET /api/messages/recent?group_id=a_u0&n=20`

返回：
```json
[
  {
    "key": {"group_id": "a_u0", "date": "2026-07-27"},
    "messages": [
      {"line": 1, "message": {"msg_id": "...", "content": {"Text": "你好"}, ...}},
      {"line": 2, "message": {"msg_id": "...", "content": {"AttachmentInfoResponse": {...}}, ...}}
    ]
  }
]
```

滚动到顶部加载更早：`GET /api/messages/before?group_id=a_u0&date=2026-07-27&line=1&n=10`

`line` 参数取当前最早消息的 `line` 值（每组消息的第一条），加载该条之前的 N 条。

### 5. SSE 实时推送

连接：`GET /api/events`（`X-Api-Key` header）

事件数据：原始 `IncomingMessage` JSON，无外层包装
```json
{
  "msg_id": "...",
  "messenger_id": "web",
  "user_id": "u0",
  "group_id": "a_u0",
  "is_self": 0,
  "msg_type": "text",
  "content": {"Text": "回复内容"},
  "time": "2026-07-27T12:35:00.000Z"
}
```

前端直接解析，`content` 字段按 `Content` 枚举处理。

### 6. 群组管理 API

| 操作 | 路径 | 请求体 | 响应 |
|---|---|---|---|
| 创建 | `POST /api/groups/create` | `{"group_name": "新群组", "member_ids": ["u0"]}` | `{"group_id": "g1"}` |
| 重命名 | `POST /api/groups/rename` | `{"group_id": "g0", "group_name": "新名称"}` | `{"success": true}` |
| 管理成员 | `POST /api/groups/manage-members` | `{"group_id": "g0", "add_ids": ["u0"], "remove_ids": []}` | `{"success": true}` |
| 删除 | `POST /api/groups/delete` | `{"group_id": "g0"}` | `{"success": true}` |

### 7. 用户管理 API

| 操作 | 路径 | 请求体 | 响应 |
|---|---|---|---|
| 创建 | `POST /api/users/create` | `{"user_name": "新用户"}` | `{"user_id": "u2"}` |
| 重命名 | `POST /api/users/rename` | `{"user_id": "u2", "user_name": "新名称"}` | `{"success": true}` |
| 删除 | `POST /api/users/delete` | `{"user_id": "u2"}` | `{"success": true}` |

### 8. 管理员改名

`POST /api/admin/rename` — `{"admin_name": "新名称"}` → `{"success": true}`

### 9. 附件下载/缩略图

- `GET /api/attachment/download?key=a_u0/uuid` — 下载（支持 Range header）
- `GET /api/attachment/thumbnail?key=a_u0/uuid` — 缩略图（仅图片）

## 前端 UI 改动清单

### 文件结构（kissbot-channel-web-ui/src/）

| 文件 | 改动 |
|---|---|
| `api/client.ts` | 所有路径加 `/api` 前缀；重写 `connect()` → `GET /api/info`；重写 `sendMessage()` → 匹配 `OutgoingMessage`；重写 `uploadAttachment()` → 两步流程；新增 `renameAdmin()`、`renameUser()`；删除不存在的 `listGroups()`、`listUsers()`；修复消息历史 API |
| `api/sse.ts` | 去掉 `{type, data}` 包装解析，直接透传 `IncomingMessage` JSON |
| `types/index.ts` | 匹配后端 `MessengerAdminInfo`、`UserConfig`（无 `is_admin`）、`GroupConfig`（`members` 为数组）、`Content` 枚举 |
| `App.tsx` | 重构 header/sidebar/main 布局，管理员下拉菜单，Content 枚举渲染，分页加载，附件上传两步流程 |
| `index.css` | 同步 style.css 设计稿样式（登录页、dropdown、会话列表、管理面板等） |
| （新增）`api/config.ts` | 预置后端 URL 列表（名称 + URL）|

### 布局框架

```
┌─ Kissbot Web Chat ────────── 管理员 ▼ ─┐
│                              ├ 重命名管理员 │
│                              ├ 群组管理    │
│                              └ 用户管理    │
├──────────┬────────────────────────────────┤
│ 会话列表  │  消息区域                         │
│          │  或 管理面板                      │
├──────────┴────────────────────────────────┤
```

- 登录页、主界面之间通过 `connected` 状态切换
- 主界面的 `adminView` 状态控制右侧显示消息区域或管理面板
- 管理员下拉菜单通过 hover/click 控制显示

### 消息渲染规则

1. 从 `IncomingMessage.content` 解析 `Content` 枚举 JSON
2. 根据 JSON 的第一层 key 判断类型：
   - `Text` → 显示文本内容
   - `AttachmentInfoResponse` → 判断 `info.mime_type`：
     - `image/*` → `<img>` 缩略图，点击弹窗原图（`/api/attachment/download?key=...`）
     - 其他 → 文件链接，显示 `info.file_name`，点击下载
   - `GroupChange` → 居中系统消息"用户加入了群组"/"用户离开了群组"
   - `UserRemove` → 居中系统消息"用户已被删除"
   - 其他 → 忽略不显示
3. `is_self === 1` 靠右显示（admin 消息），`is_self === 0` 靠左显示（user 消息）

### 会话列表排序与未读

- 会话列表按最新消息时间降序排列（从 `messages` map 中取每个 group 最后一条消息的 `time`）
- 每个会话右对齐显示未读数，保存到 `unreadCounts: Map<string, number>`
- 点击会话时该会话未读清零
- 超过 99 显示 `...`

## 设计稿参考文件

- `docs/design/components-design/ui-ux-design/kissbot-channel-web/login.html` — 登录页
- `docs/design/components-design/ui-ux-design/kissbot-channel-web/layout.html` — 主布局
- `docs/design/components-design/ui-ux-design/kissbot-channel-web/group-management.html` — 群组管理面板
- `docs/design/components-design/ui-ux-design/kissbot-channel-web/user-management.html` — 用户管理面板
- `docs/design/components-design/ui-ux-design/kissbot-channel-web/style.css` — 完整样式
- `docs/design/components-design/ui-ux-design/kissbot-channel-web/README.md` — 交互说明
