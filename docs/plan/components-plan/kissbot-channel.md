# kissbot-channel 组件内功能实现顺序

## 实现状态：🟡 框架代码已有基础实现，开发计划列出的阶段尚未正式完成

### 第1阶段：基础结构搭建 ⚠️ 代码已有部分实现
- [ ] 配置 Cargo.toml，添加依赖
- [ ] 定义 Messenger trait（含回调注册接口和附件管理接口）
- [ ] 定义 Channel trait
- [ ] 定义核心数据结构（MessageRecord、OutgoingMessage、Attachment 等）
- [ ] 定义错误类型和事件类型

### 第2阶段：核心组件实现 ⚠️ 代码已有部分实现
- [ ] 实现 ChannelManager（注册多个 Messenger，消息队列，回调注册，nexus 绑定）
- [ ] 实现 WSS Server
- [ ] 实现 MemoryStoreClient

### 第3阶段：Messenger/Channel 实现示例 ❌ 未开始
- [ ] 实现 channel-web 作为参考实现
- [ ] 实现 nexus 绑定流程
- [ ] 实现 channel 实例创建与回调注册
- [ ] 实现附件存储

### 第4阶段：与 memory-store 集成 ❌ 未开始
- [ ] 实现 MemoryStoreClient 与 kissbot-memory-store 的通信
- [ ] 实现 ChannelManager 消息队列驱动推送

### 第5阶段：Channel Web 完善 ❌ 未开始
- [ ] 完善与前端 Web 界面的通信
- [ ] 实现 group 变化事件通知
- [ ] 实现消息收发

### 第6阶段：测试和完善 ❌ 未开始
- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化
