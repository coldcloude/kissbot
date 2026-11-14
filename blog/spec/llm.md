# LLM 组件

## 设计

### Session 和 Context

- Context 为 LLM 使用的上下文
- Session 将 Context 的多个步骤包裹为对外接口
- Session 由 session_id 唯一标识

## 实现细节

- 基本按照 OpenAI Completion API 的数据结构实现 struct
- Session 对 Context 加锁，实现完全非可变引用访问
- Session 在磁盘上缓存一个和内存一致的文件，该文件仅追加。如果需要修改，则将当前缓存文件重命名为一个版本，然后清空内存后重新写入
- 可以将 Session 的一个版本重新读入内存，同时当前缓存文件重命名为一个新版本（如果 messages 为空则仅清空当前缓存）。如果读入的 Session 文件不存在，则按照新建空 Session 处理
- 如果需要复用以前的 Session，应该将其交换到当前缓存，Session 内容变更只发生在当前缓存
- 版本名称自动生成，如果重名应该等一秒后重试解决
- 系统提示词和messages分两个文件保存，分别是 *.sys.jsonl 和 *.msgs.jsonl。保存为版本时同时保存两个文件
- 系统提示词在设置时仅临时记录，构建上下文时再应用。应用时判断是否一致，如果不一致再则写文件。生成版本时，直接移动文件，即使用当前系统提示词，不使用临时记录的
- 如果系统提示词文件不存在，保存版本或读入版本时均不写入，没有系统提示词文件时不报错


## 计划

- 支持 Anthropic API 的工具调用
- 支撑 OpenAI Response API
