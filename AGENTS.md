# 项目基本信息

## 项目名称
Keep It Simple Stupid BOT - kissbot

## 开发原则
- 不要删除代码中的仍符合实际代码注释，但已过时的注释应该删除
- 代码逻辑的复用和拆分应体现对业务逻辑的理解，而非为了单元测试的便利性。若二者冲突，应放弃细粒度单元测试

## 开发框架
- 后台rust+cargo
- 前台typescript+react+vite

## 目录结构
- **docs** 项目文档
  - **index.md** 文档目录索引
  - **design** 设计文档
    - **system-design.md** 系统设计文档（组件、流程、通信）
    - **components-design** 组件设计文档（每个组件一个文件）
  - **spec** 技术细节约定，每类细节单独一个文件。技术细节不是代码细节——spec 可包含 API 路径、JSON 结构等约定，与 design 文档"禁止包含代码细节"的规范不冲突
  - **roadmap** 计划中要添加或修改的功能。手工修改，不要编辑这个文件
- **kissbot-\*** 每个代表kissbot项目一个组件，按照docs下的组件设计和规划文档实现组件的功能
- **kai-rs** 本地Rust基础库
  - **docs** kai-rs各组件文档
  - **kai-\*** kai-rs各组件实现
- **kai** （未建立）本地TypeScript基础库
- **blog** 一些形而上的思考
- **test** 自动化测试用例
  - **workspace** 自动化测试中使用的配置和数据目录
- **script** 手动测试用启动脚本和配置文件
- **workspace** 手动测试用数据目录
- **AGENTS.md** 项目基本原则（agent 规则默认位置）
- **.agents/rules** 项目规范（agent 规则默认位置）

## git约定

- 提交comment应包含提交的所有改动对应的内容，而不是仅有最近一次改动。
- 使用中文写comment。

## 文本格式约定

- 所有文本文件用UTF-8编码，\n作为换行符
