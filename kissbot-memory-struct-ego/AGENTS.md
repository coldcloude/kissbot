# kissbot-memory-struct-ego 模块设计

## 模块概述
自我认知模块，特殊的memory-struct模块，负责管理agent的基础信息。

## 职责
- 通过配置文件读取agent的基础信息
- 通过记忆提取的方式生成agent的基础信息
- 保存agent的基础信息
- 每个agent ID对应一个客观设定，对应多个角色设定
- 仅使用API，本身不封装为tool
- 支持配置可信证书文件

## 架构设计
### 核心组件
- 配置文件读取器
- 记忆提取器
- 基础信息管理器
- HTTPS API服务器
- 配置管理器（包含证书配置）

## 数据结构
### Agent基础信息
- agent ID（唯一标识）
- 客观设定（纯文本，可用MD）
- 多个角色设定（纯文本，可用MD）

## 通信接口
- 输入：通过HTTPS API接收agent的查询请求
- 输入：通过HTTPS API接收agent的更新请求
- 输出：通过HTTPS API返回agent的基础信息
- 文件系统：读取配置文件

## 实现决策
- 继承kissbot-memory-struct的trait实现
- 使用tokio作为异步运行时
- 使用axum实现HTTPS服务器
- 使用serde进行JSON序列化
- 使用rusqlite操作SQLite数据库存储基础信息
- 支持从配置文件加载证书
