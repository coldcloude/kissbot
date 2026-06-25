# kissbot-security 组件设计

## 概述
安全组件，为所有模块提供安全相关能力。作为独立的库模块，各类安全功能以内部模块形式组织。

## 核心功能

1. **API key 认证**：为所有模块的 HTTP/WS 通信提供统一的 API key 认证能力
2. **安全配置管理**：定义安全相关配置的数据结构，从公共配置中读取 API key

## 内部模块

### 配置模块（config）
- 定义 `SecurityConfig`，包含 `api_key`（通用 API 密钥）和 `admin_api_key`（管理端 API 密钥）
- 从公共配置的 `security` 段读取，提供 `SecurityConfig::get()` 全局访问
- 各组件通过此模块统一获取认证所需的密钥

### 认证模块
- 定义认证相关的数据类型（认证失败的错误类型、HTTP header 名称常量）
- 提供统一的认证校验接口
- 提供 HTTP 接入（中间件方式挂载到路由上）
- 提供 WS 接入（在 WebSocket 握手阶段校验）
- 认证时从 `SecurityConfig::get()` 获取预配置的密钥进行校验

## 功能流程

### 认证流程
请求到达 → 从 HTTP header 中提取 API key → 校验接口验证 key 是否匹配预配置值 → 匹配则通过，不匹配则返回认证失败错误。
