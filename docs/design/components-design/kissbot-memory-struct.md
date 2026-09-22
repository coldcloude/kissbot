# kissbot-memory-struct 组件设计

## 概述
记忆结构框架，定义记忆结构构建的标准接口，供具体的记忆结构实现模块使用。记忆存储模块有新数据时通过 WSS 通知记忆结构实现模块，后者从共享文件系统读取后构建索引。

记忆结构实现模块可使用记忆基础模块的 MemoryIndexer 在内存中维护索引，并将 memory-store 的变更通知对接到 MemoryIndexer 的变更通知接口，以便在追加或重建时自动更新索引。

当前实现模块为 kissbot-memory-struct-abstract（摘要记忆）：把原始记忆切分为事件、生成事件摘要，并按新消息定位事件提供上下文，详见 [kissbot-memory-struct-abstract](kissbot-memory-struct-abstract.md)。

## 框架接口
定义标准接口规范，包括：
- 记忆结构构建接口：连接新数据通知、按作用域构建与更新结构
- 记忆结构查询接口：按作用域与条件查询结构内容
- 记忆搜索工具接口：供 nexus 在 agentic loop 内通过 tool call 调用（预留）

## 记忆结构实现模块的工作方式

### 工作流程
1. 连接到记忆存储模块的 WSS 服务
2. 收到新数据通知后读取共享文件系统的记忆记录
3. 将 memory-store 的变更通知对接到 MemoryIndexer 的变更通知接口 → MemoryIndexer 自动追加或重建索引
4. 对每条记忆执行处理（如提取摘要、构建搜索索引等）
5. 提供搜索 API（由 nexus 在 agentic loop 内通过内置 tool 调用）

### 配置需求
各个实现模块可配置自己的处理参数（如摘要生成用的 LLM API 等）。
