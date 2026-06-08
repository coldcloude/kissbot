# 通道实现模块（Web）

## 概述

Web 消息通道的实现。实现 Messenger 和 Channel 接口，提供基于 Web 的即时通讯功能。
包含后端服务（Rust）和前端界面（React）两部分。

**用户模型**：
- **管理用户（admin）**：唯一的外部用户，通过 admin_key 认证接入 Web 前台
- **普通用户（users）**：由 nexus 代表的智能体身份，可与管理员私聊或群聊。所有 user 共用 user_key
- Admin 和 users 在 JSON 配置文件中定义，agent 与 user 的绑定由 kissbot-channel 的 nexus 绑定流程动态建立

**默认群组**：添加 user 时自动创建该 user 与 admin 的单聊群组，group_id 为 `{user_id}_admin`，group_name 为该 user 的 user_name。

---

## 一、后端 — kissbot-channel-web

整体以 WebMessenger 为核心，向全局 ChannelManager 注册，对外提供 HTTPS API + SSE（Server-Sent Events）服务于 Web 前端。

### 1. WebMessenger（Messenger 实现）

kissbot-channel-web 中只有一个 Messenger 实例（messenger_id 固定为 `"web"`），它向全局 ChannelManager 注册，内部持有以下模块：

#### 1.1 ConfigManager — 配置管理
- 路径：`kissbot-channel-web-config.json`
- 加载 JSON 配置文件，解析 admin / users / groups 定义
- 加载时自动注入默认私聊群组：
  - 每个 user 自动生成 `{user_id}_admin` 群组（成员为 admin + 该 user）
  - group_name 直接设为该 user 的 user_name
- 管理员在 UI 中新建/编辑/删除群组时同步写回配置文件
- 提供运行时读取和修改群组的方法

#### 1.2 UserSessionManager — 会话管理
- 验证 HTTP 请求中的 `X-Api-Key` header
- admin_key 用于 Web 前端认证（用户身份：admin）
- user_key 用于 nexus 通过 WSS 连接时认证（用户身份：user）

#### 1.3 GroupManager — 群组管理
Group 是独立实体，有自己的 ID、名称、成员列表、消息历史。GroupManager 负责维护 Group 实体和构建 Messenger 视角的 user_group 映射。

**维护的 Group 实体**：配置群组 + 自动生成的 admin-user 单聊群组，每个 Group 记录 `(group_id, group_name, members, messages[])`

**构建 MessengerInfo**：向每个普通 user 的 group_map 中注入该 user 所属的所有 Group。例如群组 "dev-team" 包含 admin、user-1、user-2，则在 user-1 和 user-2 的 group_map 中各出现一次。admin 不在 user_map 中。

**消息发送**：向某个 Group 发送消息时，GroupManager 为 Group 内每个绑定的 user 分别调用 ChannelManager 的消息发送流程（每条消息按 (user, group) 组合分发）。

**权限控制**：admin 在群组中时，消息收发权限和普通 user 一致。web 界面额外向 admin 提供群组变更功能，以及查看未加入群组的消息的功能（不可发送）。

**群组变化**：新建/修改/删除 Group 后触发 `GroupChangeHandler` 回调，由 ChannelManager 处理。

#### 1.4 WebChannel（Channel 实现）
- 每 (user, group) 组合对应一个 WebChannel 实例
- 标识由 ChannelInfo（messenger_id, group_id, user_id）三元组构成
- 实现 `send_message()`：将消息通过 SSE 推送给前端
- 注册 `on_message_received` 回调：前端发来的消息 → 回调 → ChannelManager

#### 1.5 AttachmentStore — 附件存储
- 使用本地文件系统存储附件
- 目录：`attachments/{group_id}/{msg_id}/`
- 实现 `get_attachment_metadata()` 和 `get_attachment_data()`

#### 1.6 HTTPServer — HTTP + SSE 服务器
基于 Axum 的服务器，提供以下 API：

**REST 端点：**

| 端点 | 方法 | 功能 |
|------|------|------|
| `GET /api/connect` | GET | 验证 API key，返回用户身份（admin/user）和对应的 Messenger 信息（users、groups） |
| `POST /api/message/send` | POST | 发送消息：`{ group_id, content, attachments? }` |
| `GET /api/groups` | GET | 获取群组列表 |
| `POST /api/groups/create` | POST | 创建群组：`{ group_name, member_ids? }` |
| `POST /api/groups/rename` | POST | 修改群组名称：`{ group_id, group_name }`。admin 与 user 的单聊群组（`{user_id}_admin`）不允许修改 |
| `POST /api/groups/manage-members` | POST | 增/删群组成员：`{ group_id, add_ids?, remove_ids? }`。仅 admin 可操作。admin 与 user 的单聊群组不允许修改成员 |
| `POST /api/groups/delete` | POST | 删除群组：`{ group_id }`。admin 与 user 的单聊群组不允许删除 |
| `POST /api/attachment/upload` | POST | 上传附件（multipart），图片自动生成缩略图 |
| `GET /api/attachment/download` | GET | 下载原图/文件 |
| `GET /api/attachment/thumbnail` | GET | 读取图片缩略图 |
| `GET /api/messages` | GET | 获取历史消息：`{ group_id, before_id?, after_id?, time? }`。默认返回最新 10 条，指定 `before_id` 获取更早 10 条，`after_id` 获取之后 10 条，`time` 时间搜索定位（返回该时间点前后各 10 条） |

所有端点统一携带 `X-Api-Key` header，由 kissbot-security 中间件认证。

**SSE 端点：**
| 端点 | 方法 | 功能 |
|------|------|------|
| `GET /api/events` | GET | SSE 长连接，使用 `@microsoft/fetch-event-source` 库连接，`X-Api-Key` 通过该库的自定义 header 配置传递。 |

---

## 二、前端 — kissbot-channel-web-ui

React + Vite 单页应用，仅服务 admin 用户。普通 user 无独立 Web 界面。

### 页面结构

#### 2.1 连接页
- 输入 API key 的登录页
- 连接成功进入主界面

#### 2.2 聊天主界面
```
┌─────────────────────────────────────────────────────┐
│  Kissbot Web Chat                       [用户名 ▼] │
├──────────┬──────────────────────────────────────────┤
│ 会话列表 │          消息区域                         │
│          │   会话标题                                │
│  ● userA │                                           │
│  ○ userB │   消息气泡（上行/下行）                    │
│  ○ 群组名 │                                           │
│          │                                           │
│  ───────  │                                           │
│  群组管理 │                                           │
│  ○ 开发组 │                                           │
│  ○ 项目组 │                                           │
│          │                                           │
├──────────┼──────────────────────────────────────────┤
│          │  ▎ 输入消息...                 📎 发送  │
└──────────┴──────────────────────────────────────────┘
```

**左侧 - 会话列表**：
- 展示所有群组（单聊群组 + 多人群组），无论 admin 是否已加入
- 未加入的群组在列表中展示（可查看消息），发送输入框禁用
- 按最后消息时间排序
- 未读消息标记

**右侧 - 消息区域**：
- 顶部会话标题显示群组名称（group_name），无前后缀
- 消息收发和消息历史查看：
  - 进入群组时默认加载最新 10 条消息
  - 向上滚动加载更早 10 条消息（追加模式）
  - 向下滚动加载之后 10 条消息（追加模式）
  - 已加载的消息在切换群组再切回后仍然保留
  - 时间搜索：输入时间点，跳转到该时间前后各 10 条消息
- 上行（admin → agent）和下行（agent → admin）消息展示区分
- 附件展示：图片显示缩略图，点击后展示原图；文件显示文件名，点击下载
- 附件上传：支持图片和文件。后端在上传时自动为图片生成缩略图，前端通过独立 URL 读取
- "思考中..."状态提示（agent 正在处理时）

#### 2.3 群组管理面板
- 仅 admin 可见
- 群组列表：展示所有群组（admin-user 单聊群组不可操作，仅可查看消息）
- 新建群组：输入群组名称，选择成员（user 列表）
- 修改群组名称：选择已有群组，修改名称（admin-user 单聊群组不可修改）
- 管理成员：选择已有群组，添加或移除成员（admin-user 单聊群组不可操作）
- 删除群组：确认后删除（admin-user 单聊群组不可删除）

### 与后端通信
- **HTTPS**：所有操作型请求（连接、发送消息、群组管理、附件），携带 `X-Api-Key` header
- **SSE**：通过 `@microsoft/fetch-event-source` 库连接 `GET /api/events`，`X-Api-Key` 通过该库的 header 配置传递。支持断线自动重连

---

## 三、关键流程

### 3.1 启动流程
1. 读取 `kissbot-channel-web-config.json`
2. 加载 admin、users、groups
3. 自动生成 `{user_id}_admin` 单聊群组
4. 创建 WebMessenger 实例，注册到全局 ChannelManager
5. 启动 HTTPServer（HTTP + SSE）

### 3.2 消息上行（admin → agent）
```
1. 管理员在 Web UI 中选择群组，发送消息
2. Web UI → HTTPS POST /api/message/send { group_id, content, attachments? }
3. WebMessenger 接收消息：
   ├─ 如有附件，保存到 AttachmentStore
   └─ 构建 IncomingMessage
4. GroupManager 确定群组成员（遍历所属 user 列表），为每个绑定的 user 分发
5. 通过 Messenger 回调通知 ChannelManager，逐 user 处理
6. ChannelManager 后续处理（消息入队、推送等）
```

### 3.3 消息下行（agent → admin）
```
1. ChannelManager 按 ChannelInfo 查找 WebChannel，调用 send_message()
2. WebChannel.send_message() → 通过 SSE 推送至 Web UI
3. 管理员看到回复
```

### 3.4 群组创建流程
```
1. 管理员在 Web UI 新建群组
2. Web UI → HTTPS POST /api/groups/create { group_name, member_ids }
3. GroupManager 创建群组：
   ├─ 写入 JSON 配置文件
   ├─ 群组加入内存
   └─ 触发 GroupChangeHandler 回调
4. 后续由 ChannelManager 处理通知和绑定
```

### 3.5 群组删除流程
```
1. 管理员在 Web UI 选择群组删除
2. 前端校验：admin-user 单聊群组隐藏删除按钮
3. Web UI → HTTPS POST /api/groups/delete { group_id }
4. GroupManager：
   ├─ 校验非 admin-user 单聊群组
   ├─ 从配置文件和内存中移除
   ├─ 移除对应的所有 WebChannel 实例
   └─ 触发 GroupChangeHandler 回调
5. 后续由 ChannelManager 处理通知和清理
```

### 3.6 Nexus 绑定流程
```
1. ChannelManager 调用 WebMessenger.get_user_groups(user_id)
2. WebMessenger 返回该 user 所在的所有群组（含单聊群组 user-1_admin）
3. ChannelManager 按返回结果创建 WebChannel 实例
4. WebMessenger 创建 WebChannel 并注册回调
```

---

## 四、配置文件格式

文件位置：`kissbot-channel-web-config.json`

```json
{
  "admin_key": "admin-api-key-xxx",
  "user_key": "user-api-key-xxx",
  "admin": {
    "user_id": "admin",
    "user_name": "管理员"
  },
  "users": [
    { "user_id": "user-1", "user_name": "助手小A" },
    { "user_id": "user-2", "user_name": "助手小B" }
  ],
  "groups": [
    { "group_id": "dev-team", "group_name": "开发组", "members": ["admin", "user-1", "user-2"] },
    { "group_id": "project-x", "group_name": "项目X", "members": ["user-1", "user-2"] }
  ]
}
```

加载时自动注入的单聊群组（不在配置文件中存储，group_name 直接取 user_name）：
- `{ "group_id": "user-1_admin", "group_name": "助手小A", "members": ["admin", "user-1"] }`
- `{ "group_id": "user-2_admin", "group_name": "助手小B", "members": ["admin", "user-2"] }`

---

## 五、附件存储

- 存储方式：本地文件系统
- 根目录：`attachments/`
- 文件路径：`attachments/{group_id}/{msg_id}/{filename}`
- 上传接口接收 multipart/form-data，图片上传时后端自动生成缩略图
- 下载接口分两种：
  - `GET /api/attachment/download?key=...` — 下载原图/文件
  - `GET /api/attachment/thumbnail?key=...` — 读取缩略图（仅图片类型）
- 两种附件类型：
  - **图片**：前端显示缩略图，点击后展示原图（支持 jpg/png/gif/webp）
  - **文件**：前端显示文件名，点击触发下载

---

## 六、外部通信

| 对端 | 协议 | 通信时机 | 内容 |
|------|------|----------|------|
| ChannelManager | 库调用 | 持续 | 通过 Messenger/Channel 接口交互 |
| Web 前端（浏览器） | HTTPS | 用户操作时 | 消息收发、群组管理、附件操作 |
| Web 前端（浏览器） | SSE | 持续 | 实时推送新消息 |

---

## 七、技术栈

| 层 | 技术 | 用途 |
|----|------|------|
| 后端 Runtime | tokio | 异步运行时 |
| 后端 HTTP | axum | HTTP + SSE 服务器 |
| 后端认证 | kissbot-security | API key 验证中间件 |
| 后端序列化 | serde / serde_json | JSON 处理 |
| 前端框架 | React 19 | UI 框架 |
| 前端构建 | Vite 8 | 构建工具 |
| 前端语言 | TypeScript 6 | 开发语言 |
| 前端 SSE 库 | @microsoft/fetch-event-source | SSE 连接（支持自定义 header 和自动重连） |
