# default_memory 角色模式同话题切换新 session 设计

> 后续调整：session 命名增加 pipeline 前缀，topic / event session 加标记；重启/重建与
> `/mode role` 会加载最新的角色模式 session。见
> [2026-11-12 session 命名与重启恢复设计](2026-11-12-session-naming-restart-resume-design.md)。

## 背景

`default_memory`（`DefaultAgenticLoopPipelineWithMemory`）原先只在角色模式且
`memory_pipeline.load_memory=false` 时做同话题判断，判定为不同后归档并清空当前
session 的上下文。本次调整：

- 只要是角色模式就做同话题判断，不再看 `load_memory`；
- 判定为不同后换用自动生成的新 `session_id` 与新的空 session，不再归档清空；
- 旧 session 不保存版本，直接释放。

判定发生在两次 run 之间，`load_memory` 只决定一次 run 内上下文超阈值时用记忆回溯
还是压缩，两件事互不干扰。

## 同话题判断

- 判定条件：当前 event 为角色模式（`RoleEvent.event_id` 缺省）。事件模式不判定。
- 每批消息打包完成后、进入 agentic loop 之前，用当前完整 session（含 System）
  与本批新消息直接调用 LLM：不执行工具、不读取 memory-store、不经过 `run_once`。
- 首次使用 `TOPIC_CHECK_PROMPT`，要求只回答 `YES`/`NO`；回答先 `trim` 再转大写，
  精确匹配 `YES`（同一件事）/ `NO`（不同）。
- 首次调用失败或回答不是精确的 `YES`/`NO`：写死用 `TOPIC_CHECK_PROMPT_RETRY`
  再调用一次（不循环），重试仍精确匹配。
- 两次都失败或都不是精确 `YES`/`NO`：按同一件事处理。
- 当前 session 只有 System 消息时跳过判定。
- 判定请求不写入 session 上下文，也不计入会话 token 占用。

## 判定为不同

1. 生成新 `session_id`：`{prefix}-{agent_id}-{role}-topic-{uuid}`（prefix 为
   `default-memory`），保证不重复。
2. `ConfigManager::copy_session_config(from, to)` 把切换前 session 的整份 session 级配置
   复制给新 id 并落盘。因此 `/model`、`/bind-outgoing`、`/load-memory` 等命令写入的
   当前配置以及 agent/role 导入值全部保留。
3. 用新 id 建立空 session（新 id 没有本地缓存），系统提示词按 ego 重新生成。
4. 替换 pipeline 当前 session 与 token 占用记录（`last_total_tokens` 归零）；
   `memory_key`（记忆范围）不变。
5. 不调用归档/清空，不生成版本；旧 session 随引用释放，旧缓存文件保留在磁盘
   但不再使用。
6. 批次末尾统一调用一次 `run`，把本批新消息追加到新 session。

`copy_session_config` 失败时忽略错误，仍按新 session 处理。

## 判定为相同或无法判定

沿用当前 session，本批消息正常追加。

## 其他行为

- 角色模式的 `load_memory` 仍在 run 前从当前 session 读取，只决定超阈值时用
  `MemoryRecoverTokenReducer` 还是 `CompressTokenReducer`。
- `/mode event` / `/mode role`：切换 event 时释放当前 session，按 event 的固定
  session_id 新建或从缓存读取；切回角色模式时加载该 agent/role 修改时间最新的
  角色模式 session（基础或 topic）。
- pipeline 重启或重建后加载该 agent/role 修改时间最新的角色模式 session；
  没有可加载的 session 时新建话题 session（不再使用无话题的基础 id）。
- 旧 session 的 session 配置条目与缓存文件都保留在磁盘，不做清理。
- `default`、`memory_summary` 与事件模式不受影响。

## 实现位置

- `kissbot-agent/src/pipelines.rs`：
  - `topic_session_id`：生成不重复的新 session_id；
  - `create_session_with_prompt`：建立 session 并应用 ego 提示词；
  - `new_session_with_memory_id`：按指定 id 建立 session 并补齐 agent/role 配置；
  - 话题切换的配置复制与空 session 建立内联在 `incoming_message_batch`；
  - `DefaultAgenticLoopWithMemoryExecutor::topic_changed`：同话题判断；
  - `incoming_message_batch` 改为 `&mut self`，判定为不同时替换 session；
  - `PipelineTaskExecutor` 的批次入口同步改为 `&mut self`。
- `kissbot-agent/src/config_manager.rs`：新增 `copy_session_config(from, to)`。

## 验证

- 单元测试：`copy_session_config` 整体复制、落盘、复制后两边独立；
  `topic_session_id` 唯一且带角色前缀。
- 集成测试：
  - 角色模式 `load_memory=true` 也判定；判定为不同后出现新 session_id 的缓存，
    新 session 只有新对话，旧 session 缓存保留且没有版本，session 配置整体继承；
  - 回答非精确 / 重试确认相同时不创建新 session；
  - 事件模式不做判定。
- 验证命令：`cd kissbot-agent && cargo test`、`cargo build`。
