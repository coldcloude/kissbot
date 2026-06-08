# 通道实现模块（Web）

## 概述

Web 消息通道的实现。实现 Messenger 和 Channel 接口，提供基于 Web 的即时通讯功能。
包含后端服务（Rust）和前端界面（React）两部分。

**用户模型**：
- **管理用户（admin）**：唯一的外部用户，通过 API key 认证接入 Web 前台
- **普通用户（users）**：由 nexus 代表的智能体身份，可与管理员私聊或群聊
- Admin 和 users 在 JSON 配置文件中定义，agent 与 user 的绑定由 nexus 绑定流程动态建立

**默认群组**：添加 user 时自动创建该 user 与 admin 的双人群组，group_id 为 `{user_id}_admin`。

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
  - 名称格式：`"与 {user_name} 的对话"`
- 管理员在 UI 中新建/编辑/删除群组时同步写回配置文件
- 提供运行时读取和修改群组的方法

#### 1.2 UserSessionManager — 会话管理
- 验证 HTTP 请求中的 `X-Api-Key` header
- 通过验证的请求映射为 admin 用户（kissbot-security 模块负责认证中间件）
- 只有 admin 用户可以访问 Web 前端
- 不需要多用户 session 管理

#### 1.3 GroupManager — 群组管理
- 维护群组列表（配置群组 + 自动生成的单聊群组）
- 实现 `Messenger` trait 的 `get_user_groups()`：
  - admin 用户：看到所有群组（系统自动生成的单聊群组 + 配置的多人群组）
  - user 用户：看到该 user 参与的所有群组（自己的单聊群组 + 所在的多人群组）
- 实现 `Messenger` trait 的 `get_available_users()`：
  - 返回 admin 和所有 users
- 群组变化时触发 `GroupChangeHandler` 回调，通知 ChannelManager 推送给 nexus

#### 1.4 WebChannel（Channel 实现）
- 每 (user, group) 组合对应一个 WebChannel 实例
- 标识由 ChannelInfo（messenger_id, group_id, user_id）三元组构成
- 实现 `send_message()`：将消息通过 SSE 推送给前端
- 注册 `on_message_received` 回调：前端发来的消息 → 回调 → ChannelManager → nexus

#### 1.5 AttachmentStore — 附件存储
- 使用本地文件系统存储附件
- 目录：`attachments/{group_id}/{msg_id}/`
- 实现 `get_attachment_metadata()` 和 `get_attachment_data()`

#### 1.6 HTTPServer — HTTP + SSE 服务器
基于 Axum 的服务器，提供以下 API：

**REST 端点：**

| 端点 | 方法 | 功能 |
|------|------|------|
| `GET /api/connect` | GET | 验证 API key，返回 Messenger 信息（users、groups） |
| `POST /api/message/send` | POST | 发送消息：`{ group_id, content, attachments? }` |
| `GET /api/groups` | GET | 获取群组列表 |
| `POST /api/groups/create` | POST | 创建群组：`{ group_name, member_ids? }` |
| `POST /api/groups/rename` | POST | 修改群组名称：`{ group_id, group_name }`。自动生成的双人群组（`{user_id}_admin`）不允许修改 |
| `POST /api/groups/manage-members` | POST | 增/删群组成员：`{ group_id, add_ids?, remove_ids? }`。仅 admin 可操作。双人群组不允许修改成员 |
| `POST /api/groups/delete` | POST | 删除群组：`{ group_id }`。双人群组不允许删除 |
| `POST /api/attachment/upload` | POST | 上传附件（multipart） |
| `GET /api/attachment/download` | GET | 下载附件 |

所有端点统一携带 `X-Api-Key` header，由 kissbot-security 中间件认证。

**SSE 端点：**
| 端点 | 方法 | 功能 |
|------|------|------|
| `POST /api/events` | POST | SSE 长连接，通过 fetch POST 建立，从 response body 流式读取 SSE 事件。API key 在 header 中传递。 |

---

## 二、前端 — kissbot-channel-web-ui

React + Vite 单页应用，仅服务 admin 用户。

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
- 展示所有可用的会话（单聊群组 + 多人群组）
- 按最后消息时间排序
- 未读消息标记

**右侧 - 消息区域**：
- 消息历史和消息收发
- 上行（admin → agent）和下行（agent → admin）消息展示区分
- 附件上传/下载（图片预览、文件链接）
- "思考中..."状态提示（agent 正在处理时）

#### 2.3 群组管理面板
- 仅 admin 可见
- 群组列表：展示所有多人群组（自动生成的双人群组不可在面板中操作）
- 新建群组：输入群组名称，选择成员（user 列表）
- 修改群组名称：选择已有群组，修改名称
- 管理成员：选择已有群组，添加或移除成员（仅 admin 可操作）
- 删除群组：确认后删除（双人群组不可删除）

### 与后端通信
- **HTTPS**：所有操作型请求（连接、发送消息、群组管理、附件），携带 `X-Api-Key` header
- **SSE（POST）**：通过 fetch POST /api/events 建立长连接，从 response body 流式读取 SSE 事件。API key 通过 header 传递，前端使用 ReadableStream 处理实时推送

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
1. 管理员在 Web UI 中选择会话，发送消息
2. Web UI → HTTPS POST /api/message/send
3. WebMessenger 接收消息：
   ├─ 如有附件，保存到 AttachmentStore
   └─ 构建 IncomingMessage
4. 通过 Messenger 回调通知 ChannelManager
5. ChannelManager 消息入队
6. 处理队列：
   ├─ 推送至 memory-store
   └─ 通过 WSS 发送至 nexus
7. Nexus 接收 → 进入 Agentic Loop
```

### 3.3 消息下行（agent → admin）
```
1. Nexus 生成回复 → WSS 发送给 ChannelManager
2. ChannelManager 按 ChannelInfo 查找 WebChannel
3. 消息入队
4. 处理队列：
   ├─ 推送至 memory-store
   └─ WebChannel.send_message() → 通过 SSE 推送至 Web UI
5. 管理员看到回复
```

### 3.4 群组创建流程
```
1. 管理员在 Web UI 新建群组
2. Web UI → HTTPS POST /api/groups/create { group_name, member_ids }
3. GroupManager 创建群组：
   ├─ 写入 JSON 配置文件
   ├─ 群组加入内存
   └─ 触发 GroupChangeHandler 回调
4. ChannelManager 通过 WSS 通知 nexus
5. Nexus 按需绑定新群组的 channel
```

### 3.5 群组删除流程
```
1. 管理员在 Web UI 选择群组删除
2. 前端校验：自动生成的双人群组（{user_id}_admin）隐藏删除按钮
3. Web UI → HTTPS POST /api/groups/delete { group_id }
4. GroupManager：
   ├─ 校验非双人群组
   ├─ 从配置文件和内存中移除
   ├─ 移除对应的所有 WebChannel 实例
   └─ 触发 GroupChangeHandler 回调
5. ChannelManager 通过 WSS 通知 nexus
6. Nexus 解除对应 channel
```

### 3.6 Nexus 绑定流程
```
1. Nexus 通过 WSS 发送 bind 请求（messenger_id = "web", user_id = "user-1"）
2. ChannelManager → WebMessenger.get_user_groups("user-1")
3. 返回 user-1 所在的所有群组（含单聊群组 user-1_admin）
4. ChannelManager 为每组创建 WebChannel 实例
5. 注册回调 → 加入索引 → 返回绑定确认
```

---

## 四、配置文件格式

文件位置：`kissbot-channel-web-config.json`

```json
{
  "admin": {
    "user_id": "admin",
    "user_name": "管理员"
  },
  "users": [
    { "user_id": "user-1", "user_name": "助手小A" },
    { "user_id": "user-2", "user_name": "助手小B" }
  ],
  "groups": [
    { "group_id": "dev-team", "group_name": "开发组", "members": ["admin", "user-1", "user-2"] }
  ]
}
```

加载时自动注入的单聊群组（不在配置文件中存储）：
- `{ "group_id": "user-1_admin", "group_name": "与 助手小A 的对话", "members": ["admin", "user-1"] }`
- `{ "group_id": "user-2_admin", "group_name": "与 助手小B 的对话", "members": ["admin", "user-2"] }`

---

## 五、附件存储

- 存储方式：本地文件系统
- 根目录：`attachments/`
- 文件路径：`attachments/{group_id}/{msg_id}/{filename}`
- 上传接口接收 multipart/form-data
- 下载接口返回原始文件内容 + Content-Type

---

## 六、外部通信

| 对端 | 协议 | 通信时机 | 内容 |
|------|------|----------|------|
| ChannelManager | 库调用 | 持续 | 通过 Messenger/Channel 接口交互 |
| Web 前端（浏览器） | HTTPS | 用户操作时 | 消息收发、群组管理、附件操作 |
| Web 前端（浏览器） | SSE | 持续 | 实时推送新消息 |
| Nexus | WSS（通过 ChannelManager） | 持续 | 收发消息、绑定/解绑、群组变化通知 |
| 记忆存储模块 | HTTPS（通过 ChannelManager） | 消息产生时 | 推送消息记录 |

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
