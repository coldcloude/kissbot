# 聊天主界面交互与布局说明（管理后台）

配合 [login.html](login.html)、[layout.html](layout.html)、[group-management.html](group-management.html)、[user-management.html](user-management.html) 和 [style.css](style.css) 使用。

> 本页面为管理员专用后台，普通用户不使用此界面。

## 页面结构

### 连接页（login.html）

输入后端 URL 和管理员 API Key 的登录页。

- **后端选择区**：预置多个后端地址，以卡片形式展示，每个卡片大字显示名称，小字显示完整 URL。选中项有高亮边框和背景色。
- **Admin Key 输入**：密码输入框，输入管理员连接密钥。
- **连接按钮**：提交连接。

### 聊天主界面（layout.html）

#### 顶部标题栏（header）
- 左侧显示应用名称 "Kissbot Web Chat"
- 右侧显示当前管理员名称和下拉菜单
- 下拉菜单项：重命名管理员、群组管理、用户管理

#### 左侧 - 会话列表（sidebar）
- 展示所有会话（admin-user 单聊群组 + 多人群组）
- admin-user 单聊群组显示对应用户名；多人群组显示群组名，其后附加 `群组` 标记
- 当前选中项有高亮背景
- 右对齐显示未读消息数，切换到该群组时清零；超过 99 条显示 `...`
- 按最新消息时间降序排列
- 未加入的群组展示但输入框禁用

#### 右侧 - 消息区域（main）
- **顶部标题**：显示当前选中的会话名称
- **消息列表**（messages）：
  - 不同 Content 类型的展示规则：
    - `Content::Text`：直接显示文本内容
    - `Content::AttachmentInfoResponse`（图片，`mime_type` 以 `image/` 开头）：显示缩略图，点击弹窗显示原图
    - `Content::AttachmentInfoResponse`（非图片文件）：显示文件链接，文件名为 `file_name`，点击下载
    - `Content::Multi`：在同一个气泡框内逐项渲染多条内容，项与项之间以分隔线隔开
    - `Content::GroupChange` / `Content::UserRemove`：居中系统消息样式显示
    - 其他未知类型：忽略不显示
  - admin 消息靠右、用户消息靠左，气泡颜色区分
  - 分页加载历史消息（首次加载 recent，滚动到顶部时调用 before 追加）
- **底部输入区**（footer）：
  - 文本输入框
  - 附件上传按钮（流程见下方"附件上传"章节）
  - 发送按钮

### 群组管理面板（group-management.html）

通过顶部下拉菜单进入，替代消息区域。

- **新建群组**：输入群组名称 + 点选成员 → 创建
- **重命名群组**：下拉选择群组 → 输入新名称 → 重命名
- **管理成员**：下拉选择群组 → 点选添加/删除成员 → 提交
- **群组列表**：
  - admin-user 单聊群组不可操作（显示 `仅可查看消息`）
  - 多人群组显示名称、ID、成员列表，提供删除按钮

### 用户管理面板（user-management.html）

通过顶部下拉菜单进入，替代消息区域。

- **新建用户**：输入用户名称 → 创建。用户 ID 由系统自动生成，创建时自动建立与 admin 的单聊群组。
- **用户列表**：显示用户名称和 ID，支持重命名（点击弹出编辑框）和删除。

## 页面切换

- 管理员下拉菜单中的"群组管理"和"用户管理"点击后，右侧消息区域切换为对应管理面板
- 管理面板左上角的 `<--` 按钮返回聊天界面

## 与后端通信

- **认证**：所有请求携带 `X-Api-Key` header
- **连接**：`GET /api/info` 验证 API Key 并获取管理员及 messenger 信息
- **操作型请求**：发送消息、群组管理、用户管理、附件上传下载等操作
- **实时推送**：通过 `/api/events` SSE 长连接接收消息事件，支持断线自动重连

## 附件上传流程

管理员在 UI 中选择附件时：
1. 发送一条消息，`content` 中附带 `AttachmentInfo`（文件名、MIME 类型、文件大小）
2. 后端处理消息时注册附件，返回 `AttachmentInfoResponse` 含 `transfer_id`
3. UI 获取 `transfer_id` 后，通过 `/api/attachment/upload` 上传文件数据

## 消息内容类型（Content 枚举）

后端使用 `Content` 枚举序列化为 JSON，前端按 serde 格式解析：

```json
{"Text": "你好"}
{"Multi": [{"msg_type": "text", "content": {"Text": "..."}}, {"msg_type": "attachment", "content": {"AttachmentInfoResponse": {...}}}]}
{"AttachmentInfoResponse": {"key": "g0/uuid", "info": {"file_name": "photo.png", "mime_type": "image/png", "size_bytes": 1024}, "transfer_id": 42}}
```

前端根据 JSON 中的变体标签（`Text` / `Multi` / `AttachmentInfoResponse` / `GroupChange` / `UserRemove`）判断渲染方式。
