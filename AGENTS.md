# 项目基本信息

## 项目名称
Keep It Simple Stupid BOT - kissbot

## 开发原则
- 不要删除注释！

## 开发环境
- 开发操作系统为Windows系统，调试后台产物应为exe文件
- 开发命令行环境为PowerShell，执行命令时使用PowerShell的语法

## 开发框架
- 后台rust+cargo
- 前台typescript+react+vite

## 项目结构
- 开发者决策.txt文件、开发者决策-*.txt文件，保存项目需求和开发者决策。不要修改这写文件，仅在需要生成docs目录下的文件时读取
- docs目录，项目设计文档，其中
  - kissbot.md文件，项目整体设计和规划
  - <模块名>.md文件，每个模块的设计和规划
- kissbot开头的目录，每个代表kissbot项目一个模块，按照docs下的模块设计和规划文档实现模块的功能
- 其他目录为kissbot项目依赖的基础功能的工程，或者临时工程，具体如下
  - kai-rs目录，本地Rust基础库，文档位于kai-rs/docs目录下，包含如下模块：
    - kai-index：倒排索引模块，提供文档索引功能
  - kai目录，本地TypeScript基础库（暂未用到）
