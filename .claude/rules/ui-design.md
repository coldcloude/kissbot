---
paths:
  - "docs/design/components-design/ui-ux-design/**"
description: UI/UX 界面设计规范
---

# UI/UX 界面设计规范

## 目录结构

每个界面设计在 `docs/design/components-design/ui-ux-design/<组件名>/` 目录下，包含以下文件：

- `layout.html` — HTML 结构原型，不含 js 逻辑
- `style.css` — 样式定义
- `layout.md` — 交互行为说明文档，描述布局区域和各元素的交互逻辑

## 必须遵守

- HTML 原型只包含结构和样式类，不包含交互逻辑
- CSS 样式独立文件
- `layout.md` 文档与 HTML 原型配套使用，说明交互行为
- 交互说明使用自然语言描述，不包含具体实现细节
- 组件设计文档引用 UI/UX 设计时，使用相对路径指向对应目录

## 禁止

- 不包含具体的前端框架和库名
- 不包含 API 路径或请求参数格式
