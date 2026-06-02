# 项目基本信息

## 项目名称
Keep It Simple Stupid BOT - kissbot

## 开发原则
- 不要删除注释！

## 开发框架
- 后台rust+cargo
- 前台typescript+react+vite

## 项目结构
- draft目录，开发者决策.txt文件、开发者决策-*.txt文件，保存项目原始需求和开发者决策。不要修改这写文件。
- docs目录，项目文档，结构如下：
  - 文档目录.md — 文档目录索引
  - spec/ — 设计文档
    - system-design.md — 系统设计文档（组件、流程、通信）
    - technical-architecture.md — 技术架构文档（技术栈、协议）
    - components-design/ — 组件设计文档（每个组件一个文件）
  - plan/ — 任务计划
    - system-plan.md — 组件和流程的实现顺序规划
    - components-plan/ — 组件内功能实现顺序（每个组件一个文件）
- .claude/rules/文档规范.md — 文档编写规范（五类文档约定）
- kissbot开头的目录，每个代表kissbot项目一个模块，按照docs下的模块设计和规划文档实现模块的功能
- 其他目录为kissbot项目依赖的基础功能的工程，或者临时工程，具体如下
  - kai-rs目录，本地Rust基础库，文档位于kai-rs/docs目录下，包含如下模块：
    - kai-index：倒排索引模块，提供文档索引功能
  - kai目录，本地TypeScript基础库（暂未用到）
