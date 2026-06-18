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

进程内运行 ChannelManager（kissbot-channel 库提供，带 WSS 服务），注册一个 WebMessenger 实现（提供 HTTPS + SSE 服务），两者通过 Messenger/Channel 接口交互，持续运行。

### 1. WebMessenger（Messenger 实现）

kissbot-channel-web 中只有一个 Messenger 实例（messenger_id 固定为 `"web"`），注册到 ChannelManager，内部持有以下模块：

#### 1.1 ConfigManager — 配置管理
- 路径：`kissbot-channel-web-config.json`
- 加载 JSON 配置文件，解析 admin / users / groups 定义
- 加载时自动注入默认私聊群组：
  - 每个 user 自动生成 `{user_id}_admin` 群组（成员为 admin + 该 user）
  - group_name 直接设为该 user 的 user_name
- 管理员在 UI 中新建/编辑/删除群组时同步写回配置文件
- 提供运行时读取和修改群组的方法

#### 1.2 认证方式
- admin_key 用于 Web 前端认证。admin 后端无状态，每次请求独立校验
- user_key 用于 nexus 通过 WSS 连接时认证，由 ChannelManager 管理

#### 1.3 GroupManager — 群组管理
Group 是独立实体，有自己的 ID、名称、成员列表、消息历史。GroupManager 负责维护 Group 实体和构建 Messenger 视角的 user_group 映射。

**维护的 Group 实体**：配置群组 + 自动生成的 admin-user 单聊群组，每个 Group 记录 `(group_id, group_name, members, messages[])`

**构建 MessengerInfo**：向每个普通 user 的 group_map 中注入该 user 所属的所有 Group。例如群组 "dev-team" 包含 admin、user-1、user-2，则在 user-1 和 user-2 的 group_map 中各出现一次。admin 不在 user_map 中。

**消息发送**：向某个 Group 发送消息时，GroupManager 为 Group 内每个绑定的 user 分别调用 ChannelManager 的消息发送流程（每条消息按 (user, group) 组合分发）。消息的 msg_type 区分文本、图片、文件；附件信息以自定义 JSON 格式存储在 content 中。

**权限控制**：admin 在群组中时，消息收发权限和普通 user 一致。web 界面额外向 admin 提供群组变更功能，以及查看未加入群组的消息的功能（不可发送）。

**群组变化**：新建/修改/删除 Group 后触发 `GroupChangeHandler` 回调，由 ChannelManager 处理。

#### 1.4 UserManager — 用户管理
- 维护用户列表（普通 users，不含 admin）
- 新增用户：写入 JSON 配置文件，通知 GroupManager 自动生成该 user 与 admin 的单聊群组
- 删除用户：从配置文件中移除用户及其单聊群组，触发 `GroupChangeHandler` 回调
- 用户变化触发 `GroupChangeHandler` 回调，由 ChannelManager 处理

#### 1.5 WebChannel（Channel 实现）
- 每 (user, group) 组合对应一个 WebChannel 实例
- 标识由 ChannelInfo（messenger_id, group_id, user_id）三元组构成
- 实现 `send_message()`：将消息通过 SSE 推送给前端
- 注册 `on_message_received` 回调：前端发来的消息 → 回调 → ChannelManager

#### 1.5 AttachmentStore — 附件存储
- 使用本地文件系统存储附件
- 目录：`attachments/{group_id}/{msg_id}/`
- 实现 `get_attachment_metadata()` 和 `get_attachment_data()`

#### 1.6 HTTPServer — HTTP + SSE 服务器
基于 HTTP 框架的服务器，提供 REST API 和 SSE 长连接：
- 认证：所有请求携带 API key header，由安全认证模块统一认证
- REST 端点：连接验证、消息收发、群组管理（增删改查）、用户管理（增删改查）、附件上传下载
- SSE 端点：消息事件实时推送，支持断线自动重连

---

## 二、前端 — kissbot-channel-web-ui

React + Vite 单页应用，仅服务 admin 用户。普通 user 无独立 Web 界面。

### 页面结构

#### 2.1 连接页
- 输入 API key 的登录页
- 连接成功进入主界面

#### 2.2 聊天主界面

界面布局见 [ui-ux-design/kissbot-channel-web/layout.html](ui-ux-design/kissbot-channel-web/layout.html)。

**左侧 - 会话列表**：
- 展示所有群组（单聊群组 + 多人群组），无论 admin 是否已加入
- 未加入的群组在列表中展示（可查看消息），发送输入框禁用
- 按最后消息时间排序
- 未读消息标记
- 底部固定区域：群组管理和用户管理按钮（仅 admin 可见），点击后右侧切换到管理界面

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
- 附件上传：支持图片和文件。后端在上传时自动为图片生成缩略图
- "思考中..."状态提示（agent 正在处理时）

#### 2.3 群组管理面板
- 仅 admin 可见，点击左侧"群组管理"按钮后右侧切换到群组管理界面
- 群组列表：展示所有群组（admin-user 单聊群组不可操作，仅可查看消息）
- 新建群组：输入群组名称，选择成员（user 列表）
- 修改群组名称：选择已有群组，修改名称（admin-user 单聊群组不可修改）
- 管理成员：选择已有群组，添加或移除成员（admin-user 单聊群组不可操作）
- 删除群组：确认后删除（admin-user 单聊群组不可删除）

#### 2.4 用户管理面板
- 仅 admin 可见，点击左侧"用户管理"按钮后右侧切换到用户管理界面
- 用户列表：展示所有普通用户
- 新建用户：输入 user_id 和 user_name，自动生成该 user 与 admin 的单聊群组
- 删除用户：确认后删除用户及其单聊群组

### 与后端通信
- **HTTPS**：所有操作型请求（连接、发送消息、群组管理、附件），携带 API key header
- **SSE**：通过 SSE 库连接消息事件端点，支持断线自动重连

---

## 三、关键流程

### 3.1 启动流程
1. 读取 `kissbot-channel-web-config.json`
2. 加载 admin、users、groups
3. 自动生成 `{user_id}_admin` 单聊群组
4. 创建 ChannelManager 实例（启动 WSS 服务）
5. 创建 WebMessenger 实例，注册到 ChannelManager
6. 启动 WebMessenger 的 HTTPServer（HTTP + SSE）

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
1. ChannelManager 通过 MessengerInfo 获取 user 的群组列表
2. 为每个 (user, group) 组合创建 WebChannel 实例
3. WebChannel 注册消息到达回调和附件下载回调
```

---

## 四、附件存储

- 存储方式：本地文件系统
- 上传时图片自动生成缩略图
- 附件类型：图片（显示缩略图，点击展示原图）和文件（显示文件名，点击下载）
