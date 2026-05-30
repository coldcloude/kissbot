# kissbot-memory-store 组件内功能实现顺序

## 实现状态：🟡 基础功能已完成，WSS 通知待验证

### 第1阶段：基础结构搭建 ✅
- [x] 配置 Cargo.toml，添加依赖
- [x] 调用 memory 基础模块库（DirectoryManager）
- [x] 定义模块结构
- [x] 定义错误类型

### 第2阶段：记录管理实现 ✅
- [x] 实现 RecordManager
- [x] 按日期自动创建目录和文件
- [x] 实现 JSON Lines 格式读写
- [x] 实现追加记录功能

### 第3阶段：API 实现 ✅
- [x] 实现记忆推送 API
- [x] 实现记忆查询 API
- [x] HTTPS 服务器启动

### 第4阶段：WSS 通知服务 ⚠️ 待确认
- [ ] 实现 WSSNotificationServer
- [ ] 实现 WSS 服务器和客户端连接管理
- [ ] 实现新数据通知机制
- [ ] 实现心跳检测
- [ ] WSS 服务器启动

### 第5阶段：测试和完善 ❌ 未开始
- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化
