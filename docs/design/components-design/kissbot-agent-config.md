# 智能体配置界面

## 概述
提供 Web 界面的智能体配置工具。用于配置 nexus 和 station 的各项参数。

## 职责
- 配置 LLM API（base_url、api_key、model 等参数）
- 配置工具 skill 和 Station 连接
- 配置消息通道地址
- 配置记忆相关组件地址
- 配置管理功能的前端界面

## 外部通信

| 对端 | 协议 | 通信时机 | 内容 |
|------|------|----------|------|
| nexus 后端 | HTTPS | 用户操作时 | 读取/更新 nexus 配置 |
| station 后端 | HTTPS | 用户操作时 | 读取/更新 station 配置 |
