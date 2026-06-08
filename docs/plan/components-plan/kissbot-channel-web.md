# kissbot-channel-web 组件内功能实现顺序

## 实现状态：🔴 未开始

### kissbot-channel-web（Rust 后端）

#### 第1阶段：基础结构 ❌ 未开始
- [ ] 定义 JSON 配置文件结构，编写 ConfigManager（加载 + 自动注入单聊群组）
- [ ] 实现 WebMessenger（Messenger trait）：get_info、get_available_users、get_user_groups、create_channel
- [ ] 实现 WebChannel（Channel trait）：send_message、send_attachment_payload、download_attachment_header、register_on_incoming_messages、register_on_download_attachment_payload
- [ ] 实现 GroupManager（群组增删改查 + 配置文件同步写回）

#### 第2阶段：HTTP + SSE 服务器 ❌ 未开始
- [ ] 实现 HTTPServer（Axum），集成 kissbot-security 认证中间件
- [ ] 实现 REST API：/api/connect（验证 API key）
- [ ] 实现 REST API：/api/message/send
- [ ] 实现 REST API：/api/groups/create
- [ ] 实现 REST API：/api/groups/rename（双人群组禁止修改）
- [ ] 实现 REST API：/api/groups/manage-members（双人群组禁止修改成员）
- [ ] 实现 REST API：/api/groups/delete（双人群组禁止删除）
- [ ] 实现 REST API：/api/attachment/upload & /api/attachment/download
- [ ] 实现 SSE 端点 POST /api/events（POST 长连接，从 response body 流式推送 SSE 事件）

#### 第3阶段：完整集成 ❌ 未开始
- [ ] AttachmentStore（本地文件系统附件管理）
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
- [ ] 实现与后端的 SSE 连接（fetch POST + ReadableStream 流式读取，API key 在 header 中）

#### 第2阶段：功能完善 ❌ 未开始
- [ ] 实现会话列表展示（单聊 + 群聊）
- [ ] 实现消息收发（消息气泡、上行/下行区分）
- [ ] 实现附件上传和下载
- [ ] 实现群组管理面板（创建/编辑/删除群组）
- [ ] 实现 agent "思考中..."状态提示
