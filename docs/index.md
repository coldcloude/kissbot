# 文档目录

```
docs/
├─ index.md                          # 本文档 — 文档目录索引
│
├─ design/                           # 设计文档（组件体系、通信、职责与流程）
│  ├─ system-design.md               # 系统设计：组件体系、组件间通信、关键设计
│  ├─ system-design-agent.md         # Agent 整体设计：Nexus 与 Station 的拆分、pipeline、会话、记忆
│  └─ components-design/             # 组件设计（每个组件一个文件）
│     ├─ kissbot-agent-nexus.md      # Nexus 模块：输入接入、命令、对话循环、会话、记忆读写、管理
│     ├─ kissbot-agent-station.md    # Station 模块：工具注册、MCP 服务端、子站级联、工具执行
│     ├─ kissbot-api.md              # API 契约库：统一响应、通用约定、各域数据类型
│     ├─ kissbot-channel.md          # 通道框架：连接、绑定、消息路由、事件、附件传输
│     ├─ kissbot-channel-client.md   # 通道客户端库与命令行调试客户端
│     ├─ kissbot-channel-web.md      # Web 通道：用户与群组、消息分发与落盘、附件、管理接口与界面
│     ├─ kissbot-config.md           # 公共配置库：只读加载与层级读取
│     ├─ kissbot-security.md         # 安全库：API key 认证、HTTP 与 WS 接入、安全配置
│     ├─ kissbot-llm.md              # LLM 基础库：模型调用、重试、会话上下文与版本
│     ├─ kissbot-mcp.md              # MCP 基础库：服务发现、工具列表与调用、分组与级联扩展
│     ├─ kissbot-memory.md           # 记忆基础库：目录、记忆范围、记录模型、索引
│     ├─ kissbot-memory-store.md     # 记忆存储服务：记录写入、查询、新数据通知
│     ├─ kissbot-memory-struct-summary.md  # 摘要记忆服务：case 森林、定位、归档、查询与重建
│     ├─ kissbot-memory-ego.md       # 自我认知服务：agent 元数据、个体识别、角色设定、搜索
│     ├─ kissbot-agent-config.md     # 智能体配置界面：当前为应用骨架
│     ├─ kissbot-memory-manage.md    # 记忆管理界面：当前为应用骨架
│     └─ ui-ux-design/
│        └─ kissbot-channel-web/     # Web 通道管理员界面的 UI/UX 原型
│           ├─ README.md             # 界面区域、元素与关联说明
│           ├─ login.html            # 连接页（正式版，含交互）
│           ├─ layout.html           # 聊天主界面（简易版）
│           ├─ group-management.html # 群组管理面板（简易版）
│           ├─ user-management.html  # 用户管理面板（简易版）
│           └─ style.css             # 原型样式
│
└─ spec/                             # 技术细节（实现机制、协议、存储与并发）
   ├─ technical-architecture.md      # 技术栈、本地基础库、模块形态、构建、测试与部署
   ├─ communication.md               # 通信方式：HTTP、WS、SSE、共享文件，超时与失败语义
   ├─ authentication.md              # 认证机制：两类密钥、HTTP 中间件与 WS 握手
   ├─ agent-runtime.md               # Agent 运行时：启动顺序、模块、运行期变更、并发模型
   ├─ agent-pipeline.md              # Agent pipeline：装配与路由、会话、合批、执行循环、命令、记忆写入
   ├─ agent-station.md               # Agent Station：工具索引、级联与路由、白名单、内置工具、MCP 端点
   ├─ llm.md                         # LLM 基础库：协议差异、重试、会话缓存与版本机制
   ├─ mcp.md                         # MCP 基础库：协议约束、扩展键、传输校验、客户端与服务端
   ├─ channel.md                     # 通道框架机制：连接、绑定、路由、事件顺序、附件传输
   ├─ channel-message.md             # 消息模型：内容类型、方向、回显识别、文本提取
   ├─ channel-attachment.md          # 附件两阶段表示与分块传输、能力边界
   ├─ channel-web.md                 # Web 通道机制：用户与群组、发送与落盘、推送、翻阅、附件
   ├─ channel-web-message-storage.md # Web 通道消息文件、批量写入、时间重写、索引与定位
   ├─ channel-web-attachment.md      # Web 通道附件文件布局、上传队列、缩略图与下载
   ├─ channel-web-ui.md              # Web 通道前端：后端列表、状态管理、会话与消息交互
   ├─ memory-directory.md            # 记忆目录布局、记忆范围后缀、各类文件位置
   ├─ memory-index.md                # 记忆索引：时间与页索引、懒更新、日期集合、查询
   ├─ memory-store.md                # 记忆存储：写入分组、顺序检查、批量、序号、通知
   ├─ memory-ego.md                  # 自我认知：文件、缓存、检索索引、标识约束
   └─ memory-struct-summary.md       # 摘要记忆：森林与路径、归档、定位、通知、重建、并发
```

## 阅读指引

- 先看 `design/system-design.md` 了解组件体系与组件间通信，再按需进入各组件设计。
- 关注 Agent 内部结构时看 `design/system-design-agent.md`，再分别看 Nexus 与 Station 两个模块文档。
- 需要实现细节（协议、存储、并发、重试）时进入 `spec/`，每个文件对应一个组件或一个跨组件概念。
- 界面原型从 `design/components-design/ui-ux-design/kissbot-channel-web/README.md` 开始。
