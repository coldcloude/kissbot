# kissbot-memory-struct-abstract 模块设计

## 模块概述
记忆结构实现模块（摘要搜索），纯后台模块，实现处理记忆文件生成摘要结构，实现HTTP服务器接收推送。

## 职责
- 处理记忆文件生成摘要结构
- 实现HTTP服务器接收memory-store的推送
- 提供记忆搜索HTTPS tool接口
- 与memory-store共同读写文件系统
- 需要配置摘要用LLM API
- 支持配置可信证书文件

## 架构设计
### 核心组件
- 摘要生成器（使用LLM API）
- HTTPS API服务器（提供搜索tool）
- 文件系统访问层
- 推送接收器
- 配置管理器（包含证书配置和LLM API配置）

## 存储结构
- 摘要索引数据库（SQLite）
- 按会话存储摘要

## 通信接口
- 输入：通过HTTPS接收memory-store的推送通知
- 输入：读取memory-store的文件系统
- 输出：通过HTTPS向agent提供搜索tool

## 实现决策
- 继承kissbot-memory-struct的trait实现
- 使用tokio作为异步运行时
- 使用axum实现HTTPS服务器
- 使用rusqlite操作SQLite数据库
- 支持从配置文件加载证书和LLM API配置
