---
paths:
  - "docs/design/components-design/ui-ux-design/**"
description: UI/UX 界面设计规范
---

# UI/UX 界面设计规范

## 目录结构

每个界面设计在 `docs/design/components-design/ui-ux-design/<组件名>/` 目录下，包含 HTML + CSS + JS 原型文件和一个 README.md 描述文档。

## 原型文件

- HTML + CSS + JS 原型，根据设计阶段的详细程度分为两个版本：
  - **简易版**：仅展示布局结构，不包含交互逻辑（HTML + CSS）
  - **正式版**：包含完整的界面交互，使用原生 JS 实现（HTML + CSS + JS）
- 使用原生 HTML/CSS/JS，不引入第三方前端框架或库
- 不包含 API 路径或请求参数格式

## README.md

- README.md 必须描述界面各个部分的内容：
  - 每个界面区域的功能是什么
  - 各元素展示什么数据
  - 界面元素之间的关联关系
- 不包括呈现形式的描述（颜色、字体、间距等由 CSS 决定）
- 简易版的交互说明用自然语言描述，正式版以 JS 代码为准

## 引用方式

组件设计文档引用 UI/UX 设计时，使用相对路径指向对应组件目录。
