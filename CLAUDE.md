# 项目基本信息

## 项目名称
Keep It Simple Stupid BOT - kissbot

## 开发原则
- 不要删除注释！

## 开发框架
- 后台rust+cargo
- 前台typescript+react+vite

## 项目结构
- draft目录，开发者决策.txt文件、开发者决策-*.txt文件，保存项目需求和开发者决策。不要修改这写文件，仅在需要生成docs目录下的文件时读取
- docs目录，旧版项目设计文档（保留以供参考）
- docs_new目录，新版重组后的项目设计文档（推荐使用），结构如下：
  - 文档规划.md — 本文档规划说明
  - 01-system-design.md — 系统设计文档（组件、流程、通信）
  - components/ — 组件设计文档（每个组件一个文件）
  - 03-tech-architecture.md — 技术架构文档（技术栈、协议）
  - 04-implementation-sequence-components.md — 组件和流程的实现顺序规划
  - component-detail/ — 组件内功能实现顺序（每个组件一个文件）
- kissbot开头的目录，每个代表kissbot项目一个模块，按照docs_new下的模块设计和规划文档实现模块的功能
- 其他目录为kissbot项目依赖的基础功能的工程，或者临时工程，具体如下
  - kai-rs目录，本地Rust基础库，文档位于kai-rs/docs目录下，包含如下模块：
    - kai-index：倒排索引模块，提供文档索引功能
  - kai目录，本地TypeScript基础库（暂未用到）
