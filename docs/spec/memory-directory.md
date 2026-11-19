# 记忆目录与文件布局

> 说明记忆根目录的组织、记忆范围的后缀规则、记录文件路径与各类数据的存放位置。

## 一、根目录与 agent 目录

记忆根目录由公共配置的记忆段给出（模板中为相对路径）。根目录下按 agent 标识划分：

```
{根目录}/
└── {agent_id}/
    ├── agent-{agent_id}             # 存在标记（空文件）
    ├── metadata.json                # agent 元数据（自我认知）
    ├── memory-ego/                  # 个体识别与角色设定
    │   ├── individual-recognition-.json
    │   └── role-play-{角色名}.json
    ├── memory-store/                # 原始通道记录（记忆存储写入）
    └── memory-struct-summary/       # 摘要记忆数据（摘要记忆写入）
```

- 目录管理器是进程级单例，负责创建与校验目录；创建目录后还会再校验一次存在性。
- 存在标记由创建 agent 的服务写入，列举 agent 时只把带标记的目录当作 agent。单独的记录写入不会创建标记，因此不会让某个标识出现在 agent 列表中。
- 各组件只写自己的子目录：记忆存储写 `memory-store/`，自我认知写元数据与 `memory-ego/`，摘要记忆写 `memory-struct-summary/`。

## 二、记忆范围与后缀

记忆范围由「角色 + 可选事件」表达，后缀只在构造文件与目录名时拼接：

- 角色模式：后缀为角色名。
- 事件模式：后缀为「角色名-事件标识」。

同一 agent 下两种模式的后缀是并列目录，角色模式不是事件模式的父目录。事件标识允许包含连字符（例如 UUID），因此后缀不用于反向解析角色与事件；调用方始终以「角色 + 可选事件」传入。

## 三、原始记录文件

```
{根目录}/{agent_id}/memory-store/{年份}-{后缀}/channel-records-{日期}.jsonl
```

- 年份取记录时间的年份，目录名为「年份 + 连字符 + 后缀」（后缀为空时目录名以连字符结尾）。
- 文件名为「channel-records- + 日期」；同一天、同一记忆范围的所有通道共用一个文件，文件内混合多个通道，记录自身携带通道身份。
- 文件为 JSON Lines：每行一条记录，包含消息标识、发送者、接收方绑定用户、是否本人发送、通道标识、群组标识、内容、时间、文件内序号，以及可选的记录元信息（摘要记忆的 case 归属）。
- 时间统一为「年-月-日 时:分:秒」，日期、年份与分钟都是前缀切片。

## 四、自我认知文件

| 路径 | 内容 |
|------|------|
| `{agent_id}/metadata.json` | agent 元数据（标识、描述） |
| `{agent_id}/memory-ego/individual-recognition-.json` | 该 agent 的整份个体识别信息 |
| `{agent_id}/memory-ego/role-play-{角色名}.json` | 每个角色一份角色设定 |

- 元数据与自我认知目录分开：元数据放在 agent 目录根部，个体与角色数据放在 `memory-ego/` 下。
- 读取个体识别时若文件不存在会自动创建空文件；读取角色时不创建。

## 五、摘要记忆文件

```
{根目录}/{agent_id}/memory-struct-summary/
├── llm/                              # 判定与摘要使用的模型会话
│   ├── {会话标识}.sys.jsonl
│   ├── {会话标识}.msgs.jsonl
│   └── versions/
├── {角色名}/                          # 角色目录
│   ├── tree.jsonl                    # case 森林（每行一个节点）
│   ├── current.json                  # 该记忆范围各通道的当前 case 指针
│   ├── watermark.json                # 该记忆范围各通道的处理水位
│   └── cases/                        # 原文副本目录
│       ├── role/{路径段...}/channel-records.jsonl
│       └── event-{事件标识}/{路径段...}/channel-records.jsonl
└── {角色名}-{事件标识}/                # 事件范围：只放 current.json / watermark.json
```

- `tree.jsonl` 的存在同时是「该目录是角色目录」的判定依据。
- 角色模式下记忆范围后缀即角色名，因此指针与水位文件与树文件同目录；事件模式下事件范围目录是角色目录的兄弟目录。
- 原文副本按 case 路径分目录，一个 case 一个文件；角色根映射为 `role`，事件根映射为 `event-{事件标识}`；复合 case 目录下没有原文文件。
- 状态文件先写临时文件再原子替换；指针为空时删除指针文件。

## 六、时间与编码约定

- 所有时间字符串为「年-月-日 时:分:秒」，无时区标记。
- 模型会话标识由记忆范围清洗得到：非字母、数字与 `-`、`_`、`.` 的字符映射为下划线，空范围使用固定占位。
- 记录文件、case 树、状态文件都是 UTF-8、以换行分隔的文本文件。

## 七、相关代码

- 目录与路径：`kissbot-memory/src/directory.rs`
- 记录与键：`kissbot-memory/src/data.rs`、`kissbot-memory/src/index.rs`
- 自我认知布局：`kissbot-memory-ego/src/`
- 摘要记忆布局：`kissbot-memory-struct-summary/src/`
