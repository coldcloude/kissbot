# 文档目录

```
docs/
└─ docs-index.md                # 本文档 — 文档目录索引
│
├── spec/                       # 设计文档
│   ├── system-design.md        # 系统设计（组件体系、运行流程、组件间通信）
│   ├── tech-architecture.md    # 技术架构（技术栈、通信协议、数据存储）
│   └── components-design/      # 组件设计文档（每个组件一个文件）
│       ├── kissbot-agent.md           # 智能体核心
│       ├── kissbot-channel.md         # 消息通道框架
│       ├── kissbot-channel-web.md     # Web 通道（含 web-ui）
│       ├── kissbot-memory.md          # 记忆基础模块
│       ├── kissbot-memory-store.md    # 记忆存储模块
│       ├── kissbot-memory-ego.md      # 自我认知模块
│       ├── kissbot-memory-struct.md   # 记忆结构框架（含 abstract）
│       ├── kissbot-api.md             # API 定义模块
│       ├── kissbot-project.md         # 工程管理模块
│       ├── kissbot-agent-config.md    # 智能体配置 UI
│       └── kissbot-memory-manage.md   # 记忆管理 UI
│
└── plan/                       # 任务计划
    ├── system-plan.md       # 组件和流程的实现顺序规划
    └── components-plan/    # 组件内功能实现顺序（每个组件一个文件）
        ├── kissbot-memory.md           # 记忆基础模块
        ├── kissbot-memory-store.md     # 记忆存储模块
        ├── kissbot-memory-ego.md       # 自我认知模块
        ├── kissbot-api.md              # API 定义模块
        ├── kissbot-channel.md          # 消息通道框架
        ├── kissbot-agent.md            # 智能体核心
        ├── kissbot-project.md          # 工程管理模块
        ├── kissbot-memory-struct.md    # 记忆结构框架
        ├── kissbot-channel-web.md      # Web 通道
        ├── kissbot-agent-config.md     # 智能体配置 UI
        └── kissbot-memory-manage.md    # 记忆管理 UI
```
