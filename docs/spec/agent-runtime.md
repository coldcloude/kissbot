# Agent 运行时机制

> 说明 kissbot-agent 进程内的模块装配、启动顺序、后台任务、运行期变更与并发模型。业务行为见 `docs/design/`。

## 一、启动顺序

```
初始化日志
→ ConfigManager 初始化（读公共配置的 agent 段；建数据目录；读或生成 nexus.json / station.json；注册单例）
→ Station 构建（读 station.json；注册本级分组与已启用的直接子站；注册单例）
→ Nexus 初始化（构造记忆/自我认知/摘要记忆客户端、Provider 管理、Pipeline 管理、通道管理；注册单例）
→ 后台启动管理接口服务
→ 后台启动 station 的 MCP 端点
→ Nexus 主循环：按配置装配全部 pipeline、绑定输入身份、建立通道连接
→ 保活循环（每小时一次 sleep）
```

失败处理：配置、Station、Nexus 的初始化失败直接 panic（fail-fast）；单条 pipeline 装配失败、单个输入身份冲突只记日志并跳过；两个后台服务启动失败只记日志，进程继续运行。

## 二、模块与调用方向

| 模块 | 职责 | 主要调用方 |
|------|------|------------|
| `config_manager` | 公共配置读取、两份可写配置的引导与落盘、可继承配置与会话级配置、管理接口的写入入口 | main、各模块 |
| `nexus` | 运行时组件集合与单例、启动装配 | main |
| `channel_manager` | 通道客户端回调实现、连接运行态、回显过滤、通道级命令、输入身份路由、下行发送 | nexus、pipelines、管理接口 |
| `pipelines` | 三种 pipeline 实现、pipeline 注册表、合批、会话、命令、记忆写入、工具调用 | nexus、channel_manager |
| `station` | 工具索引、子站客户端、工具路由、MCP 处理器 | pipelines、管道管理接口、station 端点 |
| `station_http` | 把 Station 挂到独立的 MCP 端点 | main |
| `http_server` | 管理接口 | main、管理界面 |
| `provider` | 按名缓存模型访问实现、解析有效模型参数、按协议分派、模型列表 | pipelines |
| `memory_store_client` / `memory_ego_client` / `memory_summary_client` | 三个记忆服务的客户端 | pipelines、nexus |
| `ego_md` | 把自我认知数据拼装为系统提示词的 markdown 片段 | pipelines |
| `message` | 上行批次打包、记忆记录打包为对话消息 | pipelines |
| `types` | 错误类型、记忆范围键、记录构造辅助 | 全部模块 |

Nexus 本身不实现执行流程、不接收消息、不持有会话；它只提供组件集合与启动装配。所有跨模块取用一律通过单例进行，不传引用。

## 三、数据目录布局

| 路径 | 内容 | 是否使用 |
|------|------|----------|
| `<data_dir>/nexus.json` | 通道、provider、agent/角色配置、pipeline 指派、会话配置 | 是 |
| `<data_dir>/station.json` | station 标识、本级分组、直接子站连接信息 | 是 |
| `<data_dir>/context/` | 会话上下文缓存（系统提示词与消息两类文件） | 是 |
| `<data_dir>/context/versions/` | 上下文版本文件 | 是（只写不读） |
| `<data_dir>/sessions/`、`attachments/`、`station/` | 启动时创建 | 当前无用途 |

配置写入采用「持写锁 → 改内存 → 整份序列化 → 覆盖写文件」，没有临时文件与原子替换，也没有文件变更监听；运行期不重读磁盘，手工修改配置文件需要重启（通道管理员名单这类每次消息现读的配置除外）。

## 四、运行期变更（管理接口）

管理接口使用管理密钥认证，业务失败以成功响应的失败标志表达。它提供的能力与运行态同步方式：

| 能力 | 同步动作 | 生效范围 |
|------|----------|----------|
| 读配置快照 | 无 | 即时 |
| provider 增 / 删 | 只写配置 | 下一次调用按名取用；已缓存的实现实例不重建，改地址或密钥需重启 |
| 通道增 / 删 / 启停 | 结束或建立连接循环、移除运行态、通知绑定该通道的 pipeline 清理资源；输入身份路由保留 | 即时 |
| 通道管理员增 / 删 | 只写配置 | 下一条消息按新名单判断 |
| pipeline 增 / 删 / 重载 / 改名 | 装配或卸载 pipeline、重新绑定或解绑输入身份 | 即时 |
| 输入身份增 / 删 | 写 agent/角色配置并同步路由；被其他 pipeline 占用时拒绝 | 即时 |
| station 分组增 / 删、子站启停 | 注册/注销该分组工具、建立/拆除该子站客户端、清空工具路由表 | 即时 |

不会持久化的运行态：pipeline 的当前事件（含摘要记忆按通道维护的事件映射）、已入队但未处理的消息、已定位但未执行的 case、摘要记忆的内存模型覆盖。重启后这些状态回到初始值，配置则从文件重新读取。

## 五、并发模型

| 资源 | 机制 |
|------|------|
| 配置仓库 | 两把异步读写锁（agent/角色配置、会话配置）；读取返回快照克隆，写入全程持锁并落盘 |
| 会话上下文 | 每个会话一把异步读写锁：只读构建可并发，追加与重置互斥 |
| 通道运行态、输入身份路由、pipeline 表、工具索引、子站表 | 无锁并发映射（条目级锁） |
| 工具路由表 | 原子交换的不可变快照：读取无锁，写入整体替换；子站集合变化时清空，下一次工具发现时重建 |
| pipeline 任务 | 每条 pipeline 一条无界队列，单消费者串行；命令通过一次性回传通道返回结果 |
| 摘要记忆 pipeline | 按通道的合批队列、单一批次 worker、单一 case worker 三级串行 |
| 待回显消息标识 | 并发映射 + 60 秒 TTL 懒清理 |
| 模型访问实现 | 按 provider 名缓存，同名复用实例与 HTTP 连接 |

跨异步边界不持锁：并发映射的条目在 `await` 前先克隆出共享引用再使用。

## 六、相关代码

- 启动与装配：`kissbot-agent/src/main.rs`、`kissbot-agent/src/nexus.rs`
- 配置：`kissbot-agent/src/config_manager.rs`、`kissbot-agent/src/configs/`
- 通道适配：`kissbot-agent/src/channel_manager.rs`
- 管理接口：`kissbot-agent/src/http_server.rs`
- station 端点：`kissbot-agent/src/station_http.rs`
- 测试装配：`kissbot-agent/src/test_support.rs`
