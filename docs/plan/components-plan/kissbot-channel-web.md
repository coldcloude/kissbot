# kissbot-channel-web 组件内功能实现顺序

## 实现状态：🔴 未开始

### kissbot-channel-web（Rust 后端）

#### 第1阶段：基础结构 ❌ 未开始
- [ ] 定义 JSON 配置文件结构（admin_key、user_key），编写 ConfigManager（加载 + 自动注入单聊群组，group_name 取 user_name）
- [ ] 实现 WebMessenger（Messenger trait）：get_info、get_available_users、get_user_groups、create_channel
- [ ] 实现 WebChannel（Channel trait）：send_message、send_attachment_payload、download_attachment_header、register_on_incoming_messages、register_on_download_attachment_payload
- [ ] 实现 UserSessionManager：admin_key 用于 Web 前端认证，user_key 用于 WSS 连接认证
- [ ] 实现 GroupManager（群组增删改查 + 配置文件同步写回），admin-user 单聊群组禁止改名/改成员/删除

#### 第2阶段：HTTP + SSE 服务器 ❌ 未开始
- [ ] 实现 HTTPServer（Axum），集成 kissbot-security 认证中间件
- [ ] 实现 REST API：/api/connect（验证 API key）
- [ ] 实现 REST API：/api/message/send
- [ ] 实现 REST API：/api/groups/create
- [ ] 实现 REST API：/api/groups/rename（admin-user 单聊群组禁止修改）
- [ ] 实现 REST API：/api/groups/manage-members（admin-user 单聊群组禁止修改成员）
- [ ] 实现 REST API：/api/groups/delete（admin-user 单聊群组禁止删除）
- [ ] 实现 REST API：/api/attachment/upload & /api/attachment/download
- [ ] 实现 REST API：/api/messages（历史消息查询，支持 before_id/after_id/time 参数，默认返回最新 10 条）
- [ ] 实现 SSE 端点 GET /api/events（通过 `@microsoft/fetch-event-source` 连接，header 传递 API key）

#### 第3阶段：完整集成 ❌ 未开始
- [ ] AttachmentStore（本地文件系统附件管理，支持图片和文件两种类型）
- [ ] WebMessenger 注册到全局 ChannelManager
- [ ] 实现消息上行流程（admin → ChannelManager → nexus）
- [ ] 实现消息下行流程（nexus → ChannelManager → Web UI）
- [ ] 实现群组变化通知流程（GroupChangeHandler → nexus）

#### 第4阶段：测试和完善 ❌ 未开始
- [ ] 单元测试
- [ ] 集成测试

### kissbot-channel-web-ui（TypeScript 前端）

#### 第1阶段：基础搭建 🟡 骨架阶段
- [x] 初始化 React + Vite 项目
- [ ] 实现连接页（API key 输入认证）
- [ ] 实现聊天主界面布局（左侧会话列表 + 右侧消息区域）
- [ ] 实现与后端的 HTTPS 通信（API client 封装）
- [ ] 添加 `@microsoft/fetch-event-source` 依赖
- [ ] 实现与后端的 SSE 连接（使用 fetchEventSource 库，自定义 header 传递 API key，配置自动重连）

#### 第2阶段：功能完善 ❌ 未开始
- [ ] 实现会话列表展示（单聊 + 群聊，未加入群组也展示但不可发送消息）
- [ ] 实现消息收发（消息气泡、上行/下行区分），消息区域标题显示 group_name
- [ ] 实现历史消息滚动加载（默认 10 条，上滚前 10 条，下滚后 10 条，切换群组保留已加载消息）
- [ ] 实现时间搜索定位历史消息
- [ ] 实现附件上传（图片/文件）和展示（图片显示原图，文件显示文件名可下载）
- [ ] 实现群组管理面板（创建/重命名/管理成员/删除，admin-user 单聊群组不可操作）
- [ ] 实现 agent "思考中..."状态提示
