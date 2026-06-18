# 通道实现模块（Web）

## 概述

Web 消息通道的实现。实现 Messenger 和 Channel 接口，提供基于 Web 的即时通讯功能。
包含后端服务（Rust）和前端界面（React）两部分。

**用户模型**：
- **管理用户（admin）**：唯一的外部用户，通过 admin_key 认证接入 Web 前台
- **普通用户（users）**：由 nexus 代表的智能体身份，可与管理员私聊或群聊。所有 user 共用 user_key
- Admin 和 users 在 JSON 配置文件中定义，agent 与 user 的绑定由 kissbot-channel 的 nexus 绑定流程动态建立

**默认群组**：添加 user 时自动创建该 user 与 admin 的单聊群组。

## 后端 — kissbot-channel-web

进程内运行 ChannelManager（kissbot-channel 库提供，带 WSS 服务），注册一个 WebMessenger 实现（提供 HTTPS + SSE 服务），两者通过 Messenger/Channel 接口交互，持续运行。

### 核心功能

1. **消息收发**：接收 admin 通过 Web 界面发送的消息，转发到对应的 agent 群组；接收 agent 回复，通过 SSE 实时推送到 Web 界面
2. **群组管理**：群组的创建、修改、删除，以及群组成员管理
3. **用户管理**：普通用户的创建和删除，自动生成与 admin 的单聊群组

### 模块划分

WebMessenger 是 Messenger 接口的实现（messenger_id 固定为 `"web"`），内部持有以下模块：

#### 1. ConfigManager — 配置管理
加载 JSON 配置文件，解析 admin / users / groups 定义。加载时自动注入默认私聊群组（每个 user 与 admin 的单聊）。管理员在 UI 中新建/编辑/删除群组时同步写回配置文件。提供运行时读取和修改群组的方法。

#### 2. GroupManager — 群组管理
Group 是独立实体，有自己的 ID、名称、成员列表、消息历史。GroupManager 负责维护 Group 实体（配置群组 + 自动生成的 admin-user 单聊群组）和构建 Messenger 视角的 user_group 映射。

**消息发送**：向某个 Group 发送消息时，GroupManager 为 Group 内每个绑定的 user 分别调用 ChannelManager 的消息发送流程（每条消息按 (user, group) 组合分发）。

**权限控制**：admin 在群组中时，消息收发权限和普通 user 一致。web 界面额外向 admin 提供群组变更功能，以及查看未加入群组的消息的功能（不可发送）。

**群组变化**：新建/修改/删除 Group 后触发 `GroupChangeHandler` 回调，由 ChannelManager 处理。

#### 3. UserManager — 用户管理
维护用户列表（普通 users，不含 admin）。新增用户时写入 JSON 配置文件，通知 GroupManager 自动生成该 user 与 admin 的单聊群组。删除用户时从配置文件中移除用户及其单聊群组，触发 `GroupChangeHandler` 回调。

#### 4. WebChannel（Channel 实现）
每 (user, group) 组合对应一个 WebChannel 实例。实现 `send_message()` 将消息通过 SSE 推送给前端。注册 `on_message_received` 回调处理前端发来的消息。

#### 5. AttachmentStore — 附件存储
使用本地文件系统存储附件。实现附件元数据查询和数据读取。上传时图片自动生成缩略图。

#### 6. HTTPServer — HTTP + SSE 服务器
基于 HTTP 框架的服务器，提供 REST API 和 SSE 长连接。所有请求通过安全认证模块统一认证。

### 功能流程

#### 消息上行（admin → agent）
1. 管理员在 Web UI 中选择群组，发送消息（可携带附件）
2. Web UI → HTTPS POST 到消息发送端点
3. WebMessenger 接收消息：如有附件则保存到 AttachmentStore，生成全局唯一 key；msg_type 为非 text 时 content 为 key，text 时 content 为实际文本
4. GroupManager 确定群组成员（遍历所属 user 列表），为每个绑定的 user 分发
5. 通过 Messenger 回调通知 ChannelManager，逐 user 处理
6. ChannelManager 后续处理（消息入队、推送等）

#### 消息下行（agent → admin）
1. ChannelManager 按 ChannelInfo 查找 WebChannel，调用 `send_message()`
2. WebChannel 通过 SSE 将消息推送至 Web UI
3. 管理员收到回复

#### 群组创建流程
1. 管理员在 Web UI 新建群组（指定名称和成员）
2. HTTPS POST 到群组创建端点
3. GroupManager 创建群组：写入配置文件加入内存，触发 GroupChangeHandler 回调
4. 后续由 ChannelManager 处理通知和绑定

#### 群组删除流程
1. 管理员在 Web UI 选择群组删除（admin-user 单聊群组不可删除）
2. HTTPS POST 到群组删除端点
3. GroupManager 校验后从配置文件和内存中移除，移除对应的所有 WebChannel 实例，触发 GroupChangeHandler 回调
4. 后续由 ChannelManager 处理通知和清理

#### Nexus 绑定流程
1. ChannelManager 通过 MessengerInfo 获取 user 的群组列表
2. 为每个 (user, group) 组合创建 WebChannel 实例
3. WebChannel 注册消息到达回调和附件下载回调

## 前端 — kissbot-channel-web-ui

React 单页应用，仅服务 admin 用户。普通 user 无独立 Web 界面。

### 页面结构

#### 连接页
输入 API key 的登录页，连接成功进入主界面。

#### 聊天主界面

界面布局和交互细节见 [ui-ux-design/kissbot-channel-web/](ui-ux-design/kissbot-channel-web/)。

**左侧 - 会话列表**：
- 展示所有群组（单聊群组 + 多人群组），无论 admin 是否已加入
- 未加入的群组在列表中展示（可查看消息），发送输入框禁用
- 按最后消息时间排序，有未读消息标记
- 底部固定区域：群组管理和用户管理按钮（仅 admin 可见），点击后右侧切换到管理界面

**右侧 - 消息区域**：
- 顶部会话标题显示群组名称
- 消息收发和历史消息查看（分页加载）
- 上行（admin → agent）和下行（agent → admin）消息展示区分
- 附件展示：图片显示缩略图，点击展示原图；文件显示文件名，点击下载
- 附件上传支持图片和文件，上传时图片自动生成缩略图
- "思考中..."状态提示（agent 正在处理时）

#### 群组管理面板
仅 admin 可见。群组列表展示所有群组（admin-user 单聊群组不可操作，仅可查看消息）。支持新建群组、修改群组名称、管理成员、删除群组。

#### 用户管理面板
仅 admin 可见。展示所有普通用户列表，支持新建用户和删除用户。新建用户时自动生成与 admin 的单聊群组。

### 与后端通信
- **HTTPS**：所有操作型请求（连接、发送消息、群组管理、附件），携带 API key header
- **SSE**：通过 SSE 连接消息事件端点，支持断线自动重连

## 附件存储

- 存储方式：本地文件系统
- 上传时图片自动生成缩略图
- 附件类型：图片（显示缩略图，点击展示原图）和文件（显示文件名，点击下载）
