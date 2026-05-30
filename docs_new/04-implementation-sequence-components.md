# 组件和流程的实现顺序规划

## 实现阶段总览

| 阶段 | 内容 | 状态 |
|------|------|------|
| 第1阶段 | 核心模块初始化 | ✅ 已完成 |
| 第2阶段 | memory 基础模块实现 | ✅ 已完成 |
| 第3阶段 | memory-store 实现 | ✅ 已完成 |
| 第4阶段 | channel 实现 | ❌ 未开始 |
| 第5阶段 | agent 基础实现 | ❌ 未开始 |
| 第6阶段 | memory-ego 实现（基础模式） | ✅ 部分完成 |
| 第7阶段 | memory-struct 实现 | ❌ 未开始 |
| 第8阶段 | agent 自主模式进阶实现 | ❌ 未开始 |
| 第9阶段 | agent skill 实现 | ❌ 未开始 |
| 第10阶段 | memory-ego 进阶模式改造 | ❌ 未开始 |
| 第11阶段 | UI 实现 | ❌ 未开始 |
| 第12阶段 | project 模块实现 | ❌ 未开始 |
| 第13阶段 | agent 工程模式实现 | ❌ 未开始 |

## 各阶段详细说明

### 第1阶段：核心模块初始化 ✅ 已完成
- [x] kissbot-api Rust 项目初始化（6 个源文件，已完成 trait 定义和 API 类型）
- [x] kissbot-memory Rust 项目初始化（6 个源文件，已完成）
- [x] kissbot-agent Rust 项目初始化（骨架）
- [x] kissbot-channel Rust 项目初始化（7 个源文件，框架代码基本完成）
- [x] kissbot-memory-store Rust 项目初始化（5 个源文件，已完成）
- [x] kissbot-memory-ego Rust 项目初始化（9 个源文件，大部分完成）
- [x] kissbot-memory-struct Rust 项目初始化（骨架）
- [x] kissbot-memory-struct-abstract Rust 项目初始化（骨架）
- [x] kissbot-channel-web Rust 项目初始化（骨架）
- [x] 所有前端项目（React + Vite）初始化：agent-config、memory-manage、channel-web-ui

### 第2阶段：memory 基础模块实现 ✅ 已完成
- [x] 模块架构设计
- [x] 定义记忆存储目录结构
- [x] 提供基础库（DirectoryManager、MemoryIndexer）供其他记忆模块使用
- [x] 实现记忆原文按时间索引搜索

### 第3阶段：memory-store 实现 ✅ 已完成
- [x] 模块架构设计
- [x] 三种记录类型的存储（JSON Lines 格式）
- [x] 记忆推送 HTTPS API 接口
- [x] WSS 通知服务器功能（已规划，代码中待验证是否完成）
- [x] 记忆查询 API

### 第4阶段：channel 实现 🔴 未开始
- [ ] 模块架构设计
- [ ] 框架 trait 定义（Messenger trait、Channel trait）
- [ ] 附件存储管理
- [ ] WSS 服务器实现
- [ ] HTTPS API 接口
- [ ] channel-web 后台实现
- [ ] channel-web-ui 前台实现

> **备注**：kissbot-channel 的 Rust 源码目录中有 7 个源文件（channel_manager.rs、channel.rs、data.rs、error.rs、lib.rs、memory_store_client.rs、messenger.rs），说明框架代码已有部分基础实现。设计文档已完整编写，但开发计划中的阶段均为 [ ] 未勾选状态。

### 第5阶段：agent 基础实现 🔴 未开始
- [ ] 模块架构设计
- [ ] LLM API 集成
- [ ] WSS 客户端与 channel 集成
- [ ] agent 问答模式实现
- [ ] agent 自主模式实现
- [ ] 与 memory-store 的集成

> **备注**：kissbot-agent 目前只有 main.rs 一个骨架文件。

### 第6阶段：memory-ego 实现（基础模式）🟡 部分完成
- [x] 模块架构设计
- [x] 实现 agent 元数据 JSON 文件存储（带读写锁）
- [x] agent 元数据管理 API（新建、查询、更新）
- [x] HTTPS API 接口
- [x] 用户识别信息、角色设定的 JSON 管理模块
- [x] 用户识别信息、角色设定的查询 API
- [x] 全文搜索实现（kai-index 库）
- [ ] 在 AgentMetadata 中增加 forbidden_items 和 autonomous_goals 字段
- [ ] 在 role-play 数据结构中增加 autonomous_goals 字段
- [ ] 实现更新禁止事项和自主运行目标的功能和 API

### 第7阶段：memory-struct 实现 🔴 未开始
- [ ] 模块架构设计
- [ ] 框架 trait 定义
- [ ] memory-store 实现向 memory-struct 的 WSS 通知机制
- [ ] memory-struct-abstract 实现（摘要搜索）
- [ ] 摘要搜索记忆 HTTPS API 和 tool

### 第8阶段：agent 自主模式进阶实现 🔴 未开始
- [ ] 与 memory-ego 的集成
- [ ] 与 memory-struct-* 的集成（非 tool）
- [ ] agent tool call 实现
- [ ] agentic loop 实现
- [ ] 上下文重置功能
- [ ] 自主运行目标触发机制

### 第9阶段：agent skill 实现 🔴 未开始
- [ ] 渐进披露（Skill）tool 实现
- [ ] 问答模式固定 skill 集成
- [ ] 自主模式固定 skill 集成

### 第10阶段：memory-ego 进阶模式改造 🔴 未开始
- [ ] 记忆提取器实现
- [ ] 配置信息与记忆提取结合生成设定 JSON 文件
- [ ] 进阶模式 API

### 第11阶段：UI 实现 🔴 未开始
- [ ] agent-config（配置 agent）
- [ ] memory-manage（管理记忆）
- [ ] channel-web-ui 完善

### 第12阶段：project 模块实现 🔴 未开始
- [ ] 模块架构设计
- [ ] 职位管理功能
- [ ] Tool 提供者实现
- [ ] 笔记管理功能
- [ ] 指导文件加载器

### 第13阶段：agent 工程模式实现 🔴 未开始
- [ ] 工程模式支持
- [ ] 与 project 模块集成
- [ ] 职位切换功能
- [ ] 工作区目录绑定
- [ ] 自定义指导文件加载
- [ ] 两种上下文压缩方式（LLM 压缩、记忆丢弃）

## 关键流程实现状态

| 流程 | 状态 |
|------|------|
| 消息上行（外部 → agent） | ❌ 未实现（依赖 channel 和 agent 基础实现） |
| 消息下行（agent → 外部） | ❌ 未实现 |
| agentic loop | ❌ 未实现 |
| agent 绑定 channel | ❌ 未实现 |
| 记忆存储（推送至 memory-store） | ✅ 已有推送 API，但推送方（agent/channel）未接入 |
| 记忆查询（tool call 查询 memory-struct） | ❌ 未实现 |
| 自我认知读取（agent 查询 memory-ego） | ✅ memory-ego API 已实现，但 agent 未集成 |
| 上下文重置 | ❌ 未实现 |
| 自主触发主动行为 | ❌ 未实现 |
| Group 变化通知 | ❌ 未实现 |
| 附件下载 | ❌ 未实现 |
