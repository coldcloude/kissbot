# channel-web 设计实现的关键决策

## 一、用户模型

### admin 与 user 的区分
- 只支持一个 admin 用户（通过 `admin_key` 认证），多个普通 users（共用 `user_key`）
- admin 通过 Web HTTPS 接入，users 通过 agent 的 WSS 连接接入
- admin 不在 MessengerInfo 的 user_map 中返回
- admin 无状态，每次请求通过 `X-Api-Key` header 单独认证

### admin 的消息权限
- admin 在群组中时，消息收发权限和普通 user 一致
- web 界面额外向 admin 提供群组变更、用户管理功能
- admin 可以查看未加入的群组的消息，但不能发送

## 二、群组设计

### 群组生成规则
- 配置文件中 users 定义普通用户，groups 定义多人群组
- admin-user 单聊群组不记录在 groups 中，群组名为 `a_{user_id}`
  - agent 获取群组信息时动态拼装
  - admin 向群组发消息时使用 `a_{user_id}` 作为 group_id
- 单聊群组的 group_name 即为该 user 的 user_name
- 允许 0 人、1 人、2 人群组

### Group 的独立性
- Group 是独立实体，有自己的 ID、名称、成员列表、消息历史
- MessengerInfo 中每个 group 在所属 user 的 group_map 中各出现一次
- 向群组发消息时，为每个绑定的 user 分发到各自的 channel

## 三、前后端通信

### 实时推送：SSE
- 前端到后端不使用 WebSocket（浏览器对自签名 WSS 证书限制严格）
- 改用 SSE（Server-Sent Events），端点 `GET /api/events`
- 使用 `@microsoft/fetch-event-source` 库支持自定义 header
- API key 通过 `X-Api-Key` header 传递

### REST API 设计
- 群组管理 API 拆分：`rename`（改名称）和 `manage-members`（增减成员）
- `api/info` 可以获取全部 group 和 user，不再单独设 list 接口

## 四、消息处理

### 统一的消息格式与 ID 分配
- 发送方不决定消息时间，由 messenger 在 out 转 in 时填入当前时间
- 消息 ID 格式：时间（`yyyyMMddHHmmss`）+ 自增序号（固定 6 位，超过回 0）
- user_id 用 `u{数字}` 格式，group_id 用 `g{数字}` 格式
- admin-user 单聊群组用 `a_{user_id}` 格式
- user_id 和 group_id 即使删除也不复用，配置中保持当前最大 ID，创建时递增
- 创建 user 和 group 不传入 id，由系统生成

### Outgoing→Incoming 转换
- 后台统一 OutgoingMessage 转 IncomingMessage 的机制
- admin 发消息和 WSS 接到 Outgoing 走同一套处理
- 转换后调用 group 各成员（不含 admin）的 `on_incoming`，同时推 SSE 给 admin
- 生成 members vec 时直接把 admin 排除

### 附件
- 图片和文件使用不同的 msg_type
- 图片上传时后端自动生成缩略图

