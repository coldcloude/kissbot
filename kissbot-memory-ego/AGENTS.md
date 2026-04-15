# kissbot-memory-ego 模块设计

## 模块概述
自我认知模块，独立的记忆系统模块，和其他记忆模块读写同一个文件系统，负责管理agent的基础信息。

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
- 文件系统：读取配置文件，与其他记忆模块共享文件系统

## 实现决策
- 不作为memory-struct的子模块，不实现memory-struct的接口
- 使用tokio作为异步运行时
- 使用axum实现HTTPS服务器
- 使用serde进行JSON序列化
- 使用rusqlite操作SQLite数据库存储基础信息
- 支持从配置文件加载证书
