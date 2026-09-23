# 文档目录

```
docs/
└─ index.md                # 本文档 — 文档目录索引
│
├── design/                     # 设计文档
│   ├── system-design.md        # 系统设计（组件体系、运行流程、组件间通信）
│   ├── system-design-api.md    # API 定义模块
│   ├── system-design-agent.md  # Agent 组件（Nexus + Station 内部模块）
│   └── components-design/      # 组件设计文档（每个组件一个文件）
│       ├── kissbot-agent-nexus.md     # Agent Nexus 模块 — LLM 通信枢纽
│       ├── kissbot-agent-station.md   # Agent Station 模块 — Tool 执行主机
│       ├── kissbot-llm.md             # LLM 基础库 — provider 调用与会话上下文
│       ├── kissbot-mcp.md             # MCP 基础库 — 极简 MCP 实现
│       ├── kissbot-channel.md         # 消息通道框架
│       ├── kissbot-channel-web.md     # Web 通道（含 web-ui）
│       ├── kissbot-memory.md          # 记忆基础模块
│       ├── kissbot-memory-store.md    # 记忆存储模块
│       ├── kissbot-memory-ego.md      # 自我认知模块
│       ├── kissbot-memory-struct.md   # 记忆结构框架
│       ├── kissbot-memory-struct-abstract.md  # 摘要记忆实现模块（事件切分与上下文定位）
│       ├── kissbot-agent-config.md    # 智能体配置 UI
│       ├── kissbot-memory-manage.md   # 记忆管理 UI
│       ├── kissbot-config.md          # 公共配置组件
│       ├── kissbot-security.md        # 安全组件（认证 + 安全配置）
│       └── ui-ux-design/              # UI/UX 设计
│           └── kissbot-channel-web/
│               ├── layout.html         # Web 聊天页面原型
│               ├── style.css           # 样式定义
│               └── README.md           # 界面内容说明
│
├── spec/                       # 技术细节约定
│   ├── technical-architecture.md    # 技术架构（技术栈、模块类型）
│   ├── communication.md             # 组件间通信协议
│   ├── authentication.md            # 认证方式
│   ├── channel-message.md           # 消息模型
│   ├── channel-attachment.md        # channel 附件注册与传输
│   ├── channel.md                   # channel 通道框架
│   ├── channel-web.md               # channel-web 设计实现的关键决策
│   ├── channel-web-message.md       # channel-web 消息流转
│   ├── channel-web-attachment.md    # channel-web 附件存储
│   ├── channel-web-message-storage.md  # channel-web 消息本地存储
│   ├── channel-web-ui.md            # channel-web 前端界面约定
│   ├── memory-directory.md          # 记忆存储文件结构
│   ├── memory-store.md              # memory-store 记录写入
│   ├── memory-index.md              # 记忆索引机制
│   ├── memory-ego.md                # 自我认知模块文件存储
│   ├── memory-struct-abstract.md    # 摘要记忆技术约定（接口/通知/懒处理与分组/存储）
│   ├── agent-modules.md             # Agent 组件模块调用关系
│   ├── agent-nexus.md               # Agent Nexus 技术约定
│   ├── agent-station.md             # Agent Station 技术约定（group/递归平铺/工具路由/MCP 端点）
│   ├── llm.md                       # LLM 基础库技术约定（provider 配置/API 适配/会话上下文格式）
│   └── mcp.md                       # MCP 基础库技术约定（消息结构/HTTP 端点/kissbot 扩展）
│
└── roadmap/                    # 计划中要添加或修改的功能（手工维护，不要编辑）
    ├── new.md                  # 待规划/新增项
    └── pending.md              # 进行中/待完成项
```
