# kissbot-project 组件设计

## 概述
工程管理模块，管理工程中的职位配置，为 agent 工程模式提供 tool 封装和职位设定。以库形式提供给 agent 模块使用。

## 内部模块

### 1. ProjectManager - 工程管理器
- 管理工程整体配置（project.json）
- 初始化工程目录结构
- 协调其他组件

### 2. RoleManager - 职位管理器
- 管理各个职位配置
- 提供职位列表
- 切换当前职位
- 提供职位设定（系统提示词附加内容、职责、技能、可用 tool）

### 3. ToolProvider - Tool 提供者
- 根据职位配置封装标准 tool 集合：
  - 文件操作（读、写、编辑）
  - 命令执行
  - 扩展 skill
- 支持自定义 tool

### 4. NoteManager - 笔记管理器
- 管理各职位的笔记读写
- 按职位组织笔记目录
- 笔记包含时间、主题、内容，存储为 JSON 文件并保留历史记录

### 5. GuideLoader - 指导文件加载器
- 从工作区加载自定义指导文件（AGENTS.md）
- 提取指导内容用于系统提示词

## 工作区目录结构
```
{workspace-path}/
├── .kissbot/
│   ├── project.json           # 工程配置
│   ├── roles/                 # 职位配置
│   ├── notes/                 # 笔记
│   └── tools/                 # 自定义 tool（可选）
├── AGENTS.md                  # 指导文件
└── (工程实际文件)
```

## 外部通信
以库形式被 agent 模块直接调用，不启动独立进程。
