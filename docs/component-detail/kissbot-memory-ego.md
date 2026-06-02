# kissbot-memory-ego 组件内功能实现顺序

## 实现状态：🟡 部分完成（已完成基础模式三个核心模块）

### 第1阶段：基础模式实现 ✅
- [x] 调用 memory 基础模块库（DirectoryManager）
- [x] 实现 AgentManager 管理 agent 元数据
- [x] agent 元数据管理 API（新建 agent、查询 agent 元数据、修改名称描述）
- [x] HTTPS API 接口

### 第2阶段：全文搜索实现 ✅
- [x] 使用本地 kai-index 库实现全文搜索
- [x] 使用 dashmap 实现高并发数据结构
- [x] 实现脏标记机制延迟更新
- [x] 实现 name 和 description 字段的全文搜索
- [x] 添加搜索 API 接口
- [x] 启动时自动加载所有 agent 到索引
- [x] 身份标识 MD 文件生成（备用）

### 第3阶段：用户识别信息和角色设定管理 ✅
- [x] 用户识别信息、角色扮演、角色扮演关系的 JSON 管理模块
- [x] 用户识别信息、角色扮演、角色扮演关系的 API 接口
- [x] JSON 查询 API

### 第4阶段：禁止事项和自主运行目标管理 ❌ 未开始
- [ ] 在 AgentMetadata 数据结构中增加 force_items 和 autonomous_goals 字段
- [ ] 在 role-play 数据结构中增加 autonomous_goals 字段
- [ ] 实现更新 agent 必须遵守事项的功能和 API
- [ ] 实现更新 agent 自主运行目标的功能和 API
- [ ] 实现更新角色自主运行目标的功能和 API
- [ ] 完善 metadata.json 和 role-play-{role-id}.json 的读写

### 第5阶段：进阶模式改造 ❌ 未开始
- [ ] 记忆提取器实现
- [ ] 配置信息与记忆提取结合生成客观设定、角色设定 JSON 文件
- [ ] 进阶模式 API
