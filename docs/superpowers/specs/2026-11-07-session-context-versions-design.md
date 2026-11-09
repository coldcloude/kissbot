# 会话上下文版本机制与双文件缓存改造设计

> 日期：2026-11-07
> 范围：`kissbot-llm` 的 SessionContext/Session、`kissbot-agent` 的 ConfigManager 与各 pipeline、`kissbot-memory-struct-summary` 的会话归档调用、相关技术规格文档。
> 不改 `blog/`（人工文档，禁止修改）。

## 背景

当前 SessionContext 把「当前缓存」与「历史归档」分成两个概念：

- 缓存：`<data_dir>/context/<session_id>.jsonl`（系统消息与消息同一个文件）
- 历史：`<data_dir>/context-history/<session_id>-<时间戳>.jsonl`

本次改造做三件事：

1. **缓存拆成两个文件**：系统提示词与消息分别保存为 `.sys.jsonl` / `.msgs.jsonl`；
2. **历史改为版本**：归档自动生成版本后缀，版本不可覆盖（同秒重名自动追加序号），并支持与当前缓存互换；
3. **系统提示词维持待定语义**：`set` 只写待定值；构建完整上下文时对比应用，需要变更才重写 sys 文件；生成版本时直接移动原 sys 文件，不使用待定值。

## 1. 磁盘布局

`SessionContext::new(data_dir, session_id)` 的 `data_dir` 就是当前缓存所在目录，不再在其下追加 `context` 层级。

| 项 | 路径 | 说明 |
|---|---|---|
| 系统提示词 | `{data_dir}/{session-id}.sys.jsonl` | 单行 System Message JSON；没有系统提示词时不创建该文件 |
| 消息 | `{data_dir}/{session-id}.msgs.jsonl` | 每行一条 Message JSON，追加写、不截断 |
| 版本 | `{data_dir}/versions/{session-id}-{后缀}.sys.jsonl` | 版本的系统提示词（从当前缓存移动） |
| 版本 | `{data_dir}/versions/{session-id}-{后缀}.msgs.jsonl` | 版本的消息 |

版本后缀由调用方指定，非空且只能包含 ASCII 字母、数字与下划线。版本与当前缓存的同名文件格式完全一致。

### 调用方目录

| 调用方 | SessionContext 的 `data_dir` | 当前缓存 | 版本 |
|---|---|---|---|
| kissbot-agent pipeline | `ConfigManager::context_dir()` = `{agent.data_dir}/context` | `{root}/context/{sid}.sys.jsonl` / `.msgs.jsonl`（路径在原来 `context/` 目录内） | `{root}/context/versions/{sid}-{后缀}.sys.jsonl` / `.msgs.jsonl` |
| kissbot-memory-struct-summary | `{agent目录}/memory-struct-summary/llm` | `llm/{sid}.sys.jsonl` / `.msgs.jsonl`（从 `llm/context/` 上移） | `llm/versions/{sid}-{后缀}.sys.jsonl` / `.msgs.jsonl` |

旧的单文件缓存、`context-history` 目录与含 `-` 的日期格式都不做兼容；memory-struct-summary 旧 `llm/context/` 下的缓存也不再读取。

## 2. API

### SessionContext

```rust
pub fn new(data_dir: &str, session_id: ArcStr) -> Self;

/// 只保存待定值（不写文件、不会失败）
pub fn set_system_message(&mut self, content: ArcStr);

/// 构建完整上下文时应用待定值：需要变更则重写 sys 文件（不归档、不动消息）
pub async fn apply_pending_system(&mut self) -> Result<()>;

pub async fn append(&mut self, messages: Vec<Message>) -> Result<()>;
pub async fn recover_from_cache(&mut self) -> Result<()>;

/// 归档当前缓存为版本并清空缓存/内存（append 由 Session 封装）
pub async fn archive_and_clear(&mut self) -> Result<()>;

pub async fn swap_version_with_cache(&mut self, version_suffix: &str) -> Result<()>;
pub fn build(&self) -> Vec<Message>;
```

### Session 包装

```rust
/// 只读构建：当前系统消息 + 内存消息（不应用待定、不写文件）
pub async fn build_context(&self) -> Vec<Message>;

/// 发送前入口：应用待定系统消息后构建（待定变更时重写 sys 文件）
pub async fn build_context_full(&self) -> Vec<Message>;

pub async fn context_append(&self, messages: Vec<Message>) -> Result<()>;
pub async fn context_archive_and_clear_cache_and_reset_messages(
    &self,
    new_messages: Vec<Message>,
) -> Result<()>;
pub async fn context_swap_version_with_cache(&self, version_suffix: &str) -> Result<()>;

/// 只保存待定值（不写文件、不会失败）
pub async fn set_system_message(&self, content: ArcStr);
```

## 3. 校验与错误

`kissbot-llm::Error` 新增：

归档后缀由 SessionContext 自动生成，不再校验调用方后缀，因此没有版本相关错误变体；互换按调用方给的版本后缀定位版本。

## 4. 行为

### 4.1 设置系统提示词

1. `set_system_message` 只写内存中的待定值，同步返回、不会失败，不写文件、不生成版本；
2. 多次 set 只保留最近一次；
3. `build_context` 只读，不应用待定；
4. `build_context_full` 对比待定值与当前值：
   - 一致 → 仅清空待定；
   - 不一致 → 重写 `.sys.jsonl`，把待定值切成当前系统消息；不归档、不改动 `.msgs.jsonl`。

### 4.2 归档

`SessionContext::archive_and_clear` 只负责归档 + 清空，写新消息由 Session 封装：

1. 以内存消息为准：内存为空则无可归档内容，仍清空缓存；
2. msgs 非空：自动生成版本后缀（本地时间 `%Y%m%d%H%M%S`，重名时等一秒后重试）；
3. 生成版本：把当前缓存的 `.sys.jsonl` 与 `.msgs.jsonl` 直接移动到版本路径（不重新写入；sys 使用原先的系统提示词，不应用待定值；缓存没有 sys 时版本也不写 sys、不报错）；
4. 清空当前缓存文件（已移走的文件不存在，幂等），清空内存（system 保留）。

`Session::context_archive_and_clear_cache_and_reset_messages` 在 `archive_and_clear` 之后调用 `append(new_messages)` 重建当前上下文（append 的调用位置在 Session，方法仍在 SessionContext）。

归档自动生成版本后缀（本地时间 `%Y%m%d%H%M%S`，重名时等一秒后重试）；每个版本都是只读快照，复用旧 session 时通过互换换回当前缓存，后续变更只发生在当前缓存。

### 4.3 互换

全部使用 rename，不复制：

1. 校验版本后缀；格式不合法按版本不存在处理（不报错）；
2. 复用 `archive_and_clear()`：当前缓存有 msgs 时先移动成一个新版本，并清空当前缓存/内存（只有 sys 的缓存在这里丢弃）；
3. 目标版本存在：把它的 sys / msgs 移入当前缓存，目标版本被消耗；不存在：当前缓存保持为空；
4. 清除待定系统消息，调用 `recover_from_cache()` 重新读取当前缓存。

## 5. 调用点改造

| 位置 | 改动 |
|---|---|
| `kissbot-llm/src/session.rs` | 双文件路径、待定系统提示词、`archive_and_clear`、版本互换、测试；append 调用移到 Session |
| `kissbot-llm/src/error.rs` | 不新增版本相关错误变体 |
| `kissbot-agent/src/config_manager.rs` | 新增 `context_dir()`；路径在 `new()` 中计算一次并保存为 `ArcStr` |
| `kissbot-agent/src/pipelines.rs` | 建 session 改用 `context_dir()`；发送路径使用 `build_context_full` |
| `kissbot-agent/src/pipelines/token_reducer.rs` | 只读压缩路径继续用 `build_context` |
| `kissbot-agent/src/pipelines/memory_summary_pipeline.rs` | 系统提示词 set 保持不会失败 |
| `kissbot-agent/src/types/error.rs` | kissbot-llm 新错误映射为内部错误 |
| `kissbot-memory-struct-summary/src/model.rs` | 读取上下文用 `build_context_full` |
| `kissbot-agent/src/nexus.rs` | 测试改用 `context_dir()` 与 `context/versions`；读取缓存用双文件恢复 |

## 6. 测试

- 缓存为 `.sys.jsonl` / `.msgs.jsonl` 两个文件；没有系统提示词时不创建 sys 文件。
- set 只写待定：不写 sys 文件、不改变已构建上下文；`build_context` 不应用，`build_context_full` 应用。
- 待定值与当前一致时不重写 sys；不一致时只重写 sys，不归档、不改消息。
- 归档时 sys 文件直接移动：即使存在待定值，版本也保存原先的系统提示词；原 sys 不存在时版本不写 sys、不报错。
- 归档直接移动 sys / msgs 两个缓存文件成版本，再用新消息新建当前缓存；内存为空不产生版本。
- 归档自动生成唯一后缀；重名时等一秒后重试，不覆盖旧版本；复用旧版本通过互换完成。
- 互换时当前缓存非空：当前缓存先存成新版本，目标版本移入当前缓存并被消耗，内存重新读取目标版本。
- 当前缓存为空（msgs 为空）时互换：目标版本文件移到缓存路径，版本消失；只有 sys 的当前缓存被丢弃。
- 目标版本不存在或后缀非法时互换：不报错；当前缓存非空则先存成新版本，然后得到空上下文。
- 版本不存在时互换按新建空上下文处理：当前缓存非空则先存成新版本，然后当前缓存与内存清空。
- `Session` 包装层转发归档、互换、待定设置与完整构建。
- 日期后缀只含数字。
- agent 集成测试中的归档目录改为 `context/versions`，读取缓存走双文件恢复。

## 7. 文档

更新 `docs/spec/llm.md`、`docs/spec/agent-nexus.md`、`docs/spec/memory-struct-summary.md`、`docs/spec/agent-modules.md` 与 `docs/design/` 下相关组件设计中的缓存/版本路径、系统提示词待定/应用语义；代码注释同步。
