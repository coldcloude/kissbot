# kissbot-agent 配置三分重构设计

## 概述

将 Agent 的配置拆成三部分，各司其职：

1. **纯静态配置**（`AgentConfig`）：进程级、启动后不变，从公共配置 `KISSBOT_CONFIG` 的 `agent` 段加载。替代现在的 `AgentConfigFile`（私有序列化结构）+ `AgentConfig`（只读）双重结构。
2. **可修改配置**（`NexusRepo` / `StationRepo`）：运行时可改、改动回写文件。`NexusRepo` 代替现在的 `AgentRuntimeConfig`，承载 nexus 的对接配置；`StationRepo` 与 `NexusRepo` 平级，本轮占位。
3. **运行状态**：当前活动身份/模型/绑定等，不落盘，直接作为管理模块（`AgentCoordinator`）的字段，启动时从 `NexusRepo` 默认值初始化。

同时消除现状的配置 split-brain：agent 不再用单独的 `CONFIG_PATH`，统一用 `KISSBOT_CONFIG`。

## 现状与问题

### 现状

agent 当前有两套配置来源：

- **公共配置**（`KISSBOT_CONFIG` → `kissbot-config` 全局单例）：
  - `ApiConfig::get()` → `memory_store_url`、`memory_ego_url`
  - `SecurityConfig::get()` → `api_key`、`admin_api_key`
- **agent 自有配置**（`CONFIG_PATH` → `ConfigManager::load()`，独立文件）：
  - `AgentConfigFile`（序列化结构）→ 拆成 `AgentConfig`（只读：`agent_id`、`llm`、`stations`、`channel_ws_url`、`memory_struct_url`、`ws_reconnect_interval_secs`）+ `AgentRuntimeConfig`（可变 `RwLock`，持久化：`current_role`、`current_mode`、`channel_bindings`、`admin_users`）

`memory_store_url` / `memory_ego_url` 在 `ApiConfig`，而 `channel_ws_url` / `memory_struct_url` 在 agent 自有配置--已经不一致。两个环境变量都默认 `config.json`，但 repo 里的 `config.json` 只有公共段、没有 agent 段，导致 agent 必须手动指定单独的 `CONFIG_PATH` 文件（该文件不在 repo 中）。

### 问题

- 配置来源 split-brain，agent 是异类（其他组件统一用 `KISSBOT_CONFIG`）。
- `AgentConfigFile` + `AgentConfig` 双重结构冗余。
- `AgentRuntimeConfig` 把"可改配置"和"运行状态"混在一起且全部落盘，导致 `/bind` `/role` 等命令都会写文件，与"运行状态不落盘"的诉求不符。
- `channel_bindings`（机器人绑定身份）与 `admin_users`（管理员权限）是两个扁平列表，没有按 channel 归组，ws_url 全局单一。

## 目标结构

| 部分 | 实体 | 来源 | 可变性 | 持久化 | 持有者 |
|------|------|------|--------|--------|--------|
| part1 静态 | `AgentConfig` | `KISSBOT_CONFIG` 的 `agent` 段 | 不可变 | 不需要 | `ConfigManager` |
| part2 nexus 可改 | `NexusRepo` | `<data_dir>/nexus.json` | 可改 | 改动回写 | `ConfigManager`（`Arc<RwLock<NexusRepo>>`） |
| part2 station 可改 | `StationRepo` | `<data_dir>/station.json` | 可改 | 改动回写 | `ConfigManager`（`Arc<RwLock<StationRepo>>`） |
| part3 运行状态 | 无单独实体类 | 启动时从 `NexusRepo` 默认值初始化 | 可改 | **不落盘** | `AgentCoordinator`（直接字段） |

`ConfigManager` 退化为纯配置存储（无运行状态）；`AgentCoordinator` 持有运行状态。废弃 `CONFIG_PATH`，统一用 `KISSBOT_CONFIG`。`AgentConfigFile` 删除。

### 关键决策

- **memory-store / memory-ego URL**：仍只读自 `ApiConfig`，不进 `NexusRepo`。"对接"只是运行时用 `agent_id` 分区记忆的概念关联。
- **memory-struct**：作为动态配置放 `NexusRepo`（可配多个，列表），**不**进 `ApiConfig`。运行状态另加"选中集合"。
- **channel**：`ws_url` 下沉到每个 channel 条目；admin 下沉为 channel 的子集合；绑定 user_id（机器人身份）拆为 `NexusRepo` 的 `default_bind_user`（默认）+ 运行状态的当前 `bound_channels`。
- **station**：本轮不做运行逻辑，仅落 `StationRepo` 占位结构 + 读写空表。
- **agent_id 运行时切换**：本轮按"单活动身份"落地（切换触发上下文重建，agentic loop 用快照语义）。多身份并发 / per-agent_id 上下文缓存的深度重设计**推迟**。

## Part1：`AgentConfig`（静态）

来源：`KISSBOT_CONFIG` 的 `agent` 段，经 `kissbot-config` 单例加载（`Config::get().get_section("agent")`），启动后不可变。缺段则 fail-fast panic，与其他段一致。

```rust
struct AgentConfig {
    data_dir: Arc<String>,                  // 本地数据根目录
    mgmt_addr: Arc<String>,                 // 管理 API 监听地址，如 "127.0.0.1:9090"
    ws_reconnect_interval_secs: u64,        // channel 重连全局默认
    // 以下为首次创建 nexus.json 时的种子值（参考 channel-web 用 Config 字段种子 WebMessengerRepo）
    init_agent_id: Arc<String>,             // 种子 NexusRepo.default_agent_id
    init_role: Arc<String>,                 // 种子 NexusRepo.default_role
    init_llm: Arc<String>,                  // 种子 NexusRepo.default_llm（名字，不是 LLM 配置对象）
}
```

派生路径（约定，不单独配置）：

```
<data_dir>/
  nexus.json              # NexusRepo
  station.json            # StationRepo
  llm_sessions/<...>      # 缓存与 LLM 交互的会话（功能本轮不实现，仅立目录）
  attachments/<...>       # channel 下载的附件（功能本轮不实现，仅立目录）
  station/<...>           # station 工作目录（功能本轮不实现，仅立目录）
```

> 程序运行日志不写入 `data_dir`，日志归程序工作目录。

## Part2a：`NexusRepo`（nexus 可改配置）

持久化到 `<data_dir>/nexus.json`，`ConfigManager` 以 `Arc<RwLock<NexusRepo>>` 持有（外层 `RwLock` 串行化"改-快照-写文件"）。

```rust
struct NexusRepo {
    channels: Arc<DashMap<String, Arc<ChannelDef>>>,            // key = messenger_id
    llms: Arc<DashMap<String, Arc<NamedLlmConfig>>>,            // key = name
    memory_structs: Arc<DashMap<String, Arc<MemoryStructConfig>>>, // key = name
    default_agent_id: Arc<String>,
    default_role: Arc<String>,
    default_llm: Arc<String>,    // 指向 llms[].name 的名字，非配置对象
}

struct ChannelDef {
    messenger_id: Arc<String>,
    ws_url: Arc<String>,
    admins: Arc<DashMap<String, Arc<AdminUser>>>,   // key = user_id；AdminUser = { messenger_id, user_id }
    default_bind_user: Arc<ChannelBinding>,          // ChannelBinding = { messenger_id, user_id }
    enabled_by_default: bool,                        // 启动是否默认绑定
}

struct NamedLlmConfig {
    name: Arc<String>,
    // 现有 LlmConfig 全部字段：provider, endpoint, api_key, model,
    // max_tokens, temperature, timeout_secs, retry_count
}

struct MemoryStructConfig {
    name: Arc<String>,
    url: Arc<String>,
}
```

> `AdminUser` 与 `ChannelBinding` 都保留 `{ messenger_id, user_id }` 二元组（admin 归属 channel，但条目自带 messenger_id 以保持自描述）。

## Part2b：`StationRepo`（与 `NexusRepo` 平级，本轮占位）

持久化到 `<data_dir>/station.json`，`ConfigManager` 以 `Arc<RwLock<StationRepo>>` 持有。

```rust
struct StationRepo {
    stations: Arc<DashMap<String, Arc<StationConfig>>>,  // key = station_id；本轮可为空
}

struct StationConfig {
    station_id: Arc<String>,
    base_url: Arc<String>,
    timeout_secs: u64,
}
```

本轮不实现 station 运行逻辑，仅落结构 + 读写空表。station 落地时再补充字段与运行状态。

## Part3：运行状态（`AgentCoordinator` 直接字段，不落盘）

无单独实体类，直接作为 `AgentCoordinator` 的字段。启动时从 `NexusRepo` 默认值初始化。

```rust
// AgentCoordinator 内：
current_agent_id: ArcSwap<String>,                          // init <- NexusRepo.default_agent_id
current_role: ArcSwap<String>,                              // init <- NexusRepo.default_role
current_llm: ArcSwap<String>,                               // init <- NexusRepo.default_llm
bound_channels: Arc<DashMap<String, Arc<ChannelBinding>>>,  // key = messenger_id
selected_memory_structs: Arc<DashMap<String, ()>>,           // key = name，选中集合
// current_mode 仍由 ModeManager 持有（init <- Role）
```

- `bound_channels` 启动初始化：遍历 `NexusRepo.channels`，对 `enabled_by_default == true` 且 `default_bind_user` 非空的 channel，绑定其 `default_bind_user`。
- `selected_memory_structs` 启动初始化为空（memory-struct 功能未实现，经管理 API 配置）。
- agentic loop 用**快照语义**：消息处理开始时读一次 `current_agent_id` / `current_role` / `current_mode`，整个 loop 用该快照；运行时切换只影响下一条消息。

## 并发模式约定

| 场景 | 选用 |
|------|------|
| 启动后只读 | `Arc<T>` |
| 确需运行时变更的标量（运行状态） | `ArcSwap<T>` |
| Map / 集合 | `Arc<DashMap<K, Arc<V>>>`，V 的方法全 `&self`（V 不可变；改 V 标量就换整个 `Arc<V>`；V 内可含嵌套 `DashMap` 做子集合） |
| Repo（需写文件） | `Arc<RwLock<XxxRepo>>`，外层 `RwLock` 串行化"改-快照-写文件"；内部集合仍用 `Arc<DashMap<K, Arc<V>>>`，标量用 `Arc<String>`（在写锁内改） |

不使用 `ArcSwapHashMap`。字符串统一用 `Arc<String>`。

> Repo 标量（`default_*`）在 `RwLock` 写锁内修改，故不需 `ArcSwap`；运行状态标量无外层锁，用 `ArcSwap` 做无锁读、写时 `store`。

## 持久化语义

| 操作 | 改动对象 | 是否回写 |
|------|----------|----------|
| `/bind` `/unbind` | `AgentCoordinator.bound_channels` | 否 |
| `/role` | `AgentCoordinator.current_role` | 否 |
| `/mode` | `ModeManager` | 否 |
| `/model <name>` | `AgentCoordinator.current_llm` | 否 |
| `/agent <id>` | `AgentCoordinator.current_agent_id`（触发上下文重建） | 否 |
| `/admin` `/unadmin` | `NexusRepo.channels[].admins` | 回写 nexus.json |
| channel / llm / memory_struct 增删改（管理 API） | `NexusRepo` 对应集合 | 回写 nexus.json |
| `default_agent_id` / `default_role` / `default_llm` 编辑（管理 API） | `NexusRepo` 标量 | 回写 nexus.json |
| station 增删改（管理 API） | `StationRepo.stations` | 回写 station.json |

> 与现状的行为变更：`/bind` `/unbind` `/role` `/mode` 不再写文件（当前代码会 `save()`）。

## 引导（Bootstrap）

- `KISSBOT_CONFIG` 必须含 `agent` 段（fail-fast）。`data_dir` 及派生子目录不存在则创建。
- `<data_dir>/nexus.json` 不存在：用 `AgentConfig` 的 `init_agent_id` / `init_role` / `init_llm` 种子 `NexusRepo` 的 3 个 default（`Arc<String>`），`channels` / `llms` / `memory_structs` 集合为空，写文件。参考 channel-web `WebMessengerCreator::new` 的"文件不存在则按 config 创建初始结构"。
- `<data_dir>/station.json` 不存在：创建空 `StationRepo`（`stations` 为空），写文件。
- 文件存在：反序列化加载。
- 首次创建后，`nexus.json` / `station.json` 为唯一权威来源，`init_*` 字段此后忽略。
- 边界：首次创建时 `llms` 集合为空，`default_llm`（由 `init_llm` 种子）可能指向尚不存在的 llm 名字--此时 agent 无法调用 LLM，需经管理 API 添加匹配该名字的 `NamedLlmConfig` 后方可思考。`default_agent_id` 同理需指向 memory-ego 中已存在的 agent。

## 代码影响

### `config_manager.rs`（重写）

- `AgentConfig`：经 `kissbot-config` 取 `agent` 段构造。
- `NexusRepo` / `StationRepo`：按 `data_dir` 拼路径，加载或首次创建；`Arc<RwLock<>>` 持有。
- `save_nexus()` / `save_station()`：写锁内快照 → 序列化 → 写文件。
- 删除运行状态及其 getter/setter（`current_role` / `current_mode` / `channel_bindings` / `set_current_role` / `add_binding` 等）--这些迁到 `AgentCoordinator`。
- 保留并改造 `NexusRepo` 侧操作：`admin_users` 读取改为查 `NexusRepo.channels[].admins`；`add_admin` / `remove_admin` 改 `NexusRepo` 并 `save_nexus`；channel / llm / memory_struct / default 的 CRUD getter/setter。
- `ConfigChangeListener` 机制重新评估：运行状态变更发生在 coordinator 内（无需监听）；`NexusRepo` 变更（channel 增删）若需通知 coordinator 连/断 channel，可保留监听，否则按需读取。本轮 channel 增删经 API 仅改配置、不自动连断（重启或显式 coordinator 动作生效）。

### `main.rs`

- 经 `kissbot-config` 取 `AgentConfig`（`agent` 段）→ 拼 `data_dir` 下两个 repo 路径 → 加载/创建 `NexusRepo` / `StationRepo` → 构造 `ConfigManager`。
- `mgmt_addr` 传给 `HttpServer`（替代硬编码 `9090`）。
- 删除 `CONFIG_PATH` 读取。

### `coordinator.rs`

- 新增运行状态字段（`current_agent_id` / `current_role` / `current_llm` 为 `ArcSwap`；`bound_channels` / `selected_memory_structs` 为 `Arc<DashMap>`）。
- `new()` 中从 `NexusRepo` 默认值初始化运行状态；按 `enabled_by_default && default_bind_user 非空` 的 channel 初始化 `bound_channels` 并连接。
- `agent_id()`：改为 `ArcSwap::load`（仍同步无锁读，返回 `Arc<String>` 快照）；agentic loop 内用快照。
- `current_role()`：改读 `ArcSwap`。
- channel 列表：从 `bound_channels` 读；`ws_url` 从 `NexusRepo.channels[messenger_id].ws_url` 读（每 channel 独立）。
- LLM 配置：按 `current_llm` 名字查 `NexusRepo.llms` 得 `NamedLlmConfig`，初始化 / 热更新 `LlmClient`。
- `/bind` `/unbind` `/role` `/model` `/agent` 改为 coordinator 方法（不回写）。

### `command_router.rs`

- `/bind` `/unbind` `/role` `/mode` `/model` `/agent`：调 coordinator 运行状态方法，不回写。
- `/admin` `/unadmin`：调 `ConfigManager` 的 `NexusRepo` 操作，回写 `nexus.json`。
- 需同时访问 coordinator（运行状态）与 `ConfigManager`（`NexusRepo`）--调整 `execute` 的参数/调用关系。

### `memory_reader.rs`

- `memory_struct_url()` 改为读 `NexusRepo.memory_structs`（列表）；feature 未实现仍静默跳过。
- `memory_store_url` 仍读 `ApiConfig`，不动。

### `http_server.rs`

- bind 到 `AgentConfig.mgmt_addr`。

### `llm_client.rs`

- `LlmConfig` → `NamedLlmConfig`（加 `name`）；`LlmClient::new` / `update_config` 适配；`/model` 切换时热更新。

### `kissbot-api` / `ApiConfig`

- **不动**。`memory_store_url` / `memory_ego_url` 不变；`memory_struct_url` **不**加入 `ApiConfig`（它在 `NexusRepo`）。

## 本轮范围

**做**：

- `AgentConfig` 重构（静态，`KISSBOT_CONFIG` `agent` 段，含 `init_*` 种子）。
- `NexusRepo` 重构（channels / llms / memory_structs + 3 default，`Arc<RwLock<>>` + DashMap 集合，持久化）。
- `StationRepo` 占位（空 stations，读写空表）。
- 运行状态从 `ConfigManager` 机械合并到 `AgentCoordinator`（`ArcSwap` 标量 + `Arc<DashMap>` 集合），从 `NexusRepo` 默认值初始化。
- 持久化语义调整（运行状态不回写、`NexusRepo`/`StationRepo` 改动回写）。
- 废弃 `CONFIG_PATH`，统一 `KISSBOT_CONFIG`；`mgmt_addr` 配置化。
- 引导：`nexus.json` / `station.json` 不存在则按种子/空创建。

**不做（推迟）**：

- 多身份并发 / per-agent_id 上下文缓存（第 4 点的"场景甲/丙"深度重设计）。
- memory-struct 功能实现（仅落配置结构 + 选中集合）。
- `llm_sessions` 会话缓存、attachments 下载、station 执行（仅立目录结构）。
- 管理 API 完整路由实现（`http_server` 当前骨架，本轮只把 `mgmt_addr` 接上）。
- `NexusRepo` channel 增删经 API 自动连断 coordinator（本轮仅改配置）。

## 兼容性与迁移

- `CONFIG_PATH` 环境变量废弃；agent 配置文件（旧 `AgentConfigFile` 形态）不再使用。
- `KISSBOT_CONFIG` 需新增 `agent` 段（`data_dir` / `mgmt_addr` / `ws_reconnect_interval_secs` / `init_agent_id` / `init_role` / `init_llm`）。repo 内 `config.json`（root / script / test/workspace）需补该段。
- 首次启动若 `nexus.json` 不存在，按 `init_*` 种子生成；既有手动 agent 配置文件中的 `llm` / `channel_bindings` / `admin_users` / `stations` 需手动迁移到 `nexus.json` / `station.json`（或经管理 API 重新配置）。
- `docs/design/components-design/kissbot-agent.md` 中"配置管理器"小节需在实现阶段同步更新。
