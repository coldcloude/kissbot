# kissbot-memory-store 组件设计

## 概述
记忆存储模块，收集 nexus 和消息通道的全部原始消息（包括 channel 文本、思考内容、工具调用、工具结果），按统一格式持久化存储。填充记忆系统的原始数据层。

## 内部模块

### 1. RecordManager - 记录管理器
- 管理多种类型记录的存储和读取
- 按日期自动创建目录和文件
- 使用 JSON Lines 格式高效读写
- 追加记录、按时间范围查询

存储的文件分类：
- `channel-{messenger_id}={user_id}={group_id}-records-{date}.jsonl`：channel 文本记录（按 messenger、user、group 和时间组织）
- `think-records-{date}.jsonl`：思考内容记录
- `tool-call-records-{date}.jsonl`：工具调用记录
- `tool-result-records-{date}.jsonl`：工具调用结果记录

### 2. WSSNotificationServer - WSS 通知服务器
- 作为 WSS 服务器，接受记忆结构实现模块的连接
- 维护已连接的客户端列表
- 新数据到达时通知所有已连接客户端
- 支持心跳检测、连接管理

### 3. HTTPS API 服务器
- 提供记忆推送 API（接收 nexus/通道推送的记忆记录）
- 提供记忆查询 API

## 记忆来源处理

### channel 消息
- 文本内容直接存储
- 非文本内容（图片、音频等）：通道将其保存为附件后存入 key，记忆存储模块对 key 与文本同等处理

### 大模型输出
- 思考内容：全文存入思考记录文件，仅将反查 key 发送到 channel
- 工具调用指令：name 和 parameter 存入工具调用记录文件，仅反查 key 到 channel
- 回复文本：全文经通道推入 channel 文本记录
- 生成的非文本内容：经通道推入 channel 文本记录

### 工具输出
- tool call 直接返回的内容存入工具调用结果记录文件
- 副产物（写入的文件等）不包含
- 记忆查询工具（nexus 内置 tool）的输出不送入记忆系统

## 内部流程

### 记忆写入流程
收到 HTTPS 推送请求 → RecordManager 解析 → 按路径中的 year-suffix 构建文件目录 → 按日期追加写入 → WSSNotificationServer 广播通知

### 记忆查询流程
收到 HTTPS 查询请求 → RecordManager 使用 MemoryIndexer 定位 → 读取记录 → 返回

## 外部通信

| 对端 | 协议 | 通信时机 | 内容 |
|------|------|----------|------|
| nexus/通道 | HTTPS | 消息产生时 | 接收记忆推送 |
| 记忆结构实现模块 | WSS | 新数据到达时 | 新数据通知 |
| 记忆结构实现模块 | 文件系统 | 持续 | 共享读取记忆文件 |
| 记忆管理界面 | HTTPS | 用户操作时 | 管理 API |
