---
paths: 
  - "**/*.rs"
  - "**/*.ts"
description: API设计、配置文件、时间格式等编码规范
---

# API、配置、时间格式规范

## 必须遵守

### API 设计
- 所有输入参数放在 JSON 请求体中传递
- 所有 API 响应统一使用kissbot-api库定义的ApiResponse

### 配置文件
- 所有配置文件使用 JSON 格式存储

### 时间格式
- 时间格式使用 24 小时制：`yyyy-MM-dd HH:mm:ss`
- 日期格式：`yyyy-MM-dd`
- 年格式：`yyyy`

## 禁止
- API 路径中不嵌入动态参数
