# session 命名前缀与重启加载最新会话设计

## 背景

`default` 与 `default_memory` 原先都用 `{agent_id}-{role}` 命名本地 session，
两个 pipeline 会复用同一份缓存；`default_memory` 的话题切换 session 与事件模式
session 也没有名字上的区分，无法在重启时按 pipeline / agent / role 找出最新的
角色模式会话。

本次调整：

- session 命名加 pipeline 前缀，并用 `topic` / `event` 标记区分话题与事件 session；
- `default_memory` 在 pipeline 构造（进程重启、pipeline 重建）或 `/mode role` 时，
  扫描该 agent/role 的角色模式 session，加载修改时间最新的一个。

## 命名规则

session 前缀是 pipeline 名中的 `_` 写作 `-`，由常量统一提供：

| pipeline | 角色模式 | 话题切换 | 事件模式 |
|---|---|---|---|
| `default` | `default-{agent}-{role}` | — | `default-{agent}-{role}-event-{event}` |
| `default_memory` | `default-memory-{agent}-{role}` | `default-memory-{agent}-{role}-topic-{uuid}` | `default-memory-{agent}-{role}-event-{event}` |
| `memory_summary` | `memory-summary-{agent}-{role}-{case}` | — | — |

- 话题 session 的 `topic` 段后固定是 UUID；扫描时按 UUID 解析识别话题 session。
- 旧的无前缀缓存不迁移，也不参与扫描；`memory_summary` 的命名值不变，只是前缀
  改用统一常量。
- event session 带 `-event-` 标记，与角色模式 / 话题 session 区分。

## 重启 / 重建加载（仅 `default_memory`）

触发：`default_memory` pipeline 构造（进程重启、pipeline 重建、绑定变化）以及从
事件模式执行 `/mode role`。切换 event 不扫描。

规则：

1. 在 `<data_dir>/context/` 中查找 `.msgs.jsonl` 缓存文件；
2. 接受 session_id 等于基础 id `{prefix}-{agent}-{role}`，或等于
   `{prefix}-{agent}-{role}-topic-{uuid}`（UUID 可解析）；event session 不参与；
3. 取文件修改时间最新的一个；
4. 找到则用该 id 建立 session：session 配置已存在则保留，缺失时按 agent/role 导入；
   上下文从该 id 的本地缓存恢复；
5. 没有匹配时回退到基础 id `{prefix}-{agent}-{role}`。

只读取当前缓存文件，不扫描 `versions/`；没有 `.msgs.jsonl` 的 session 不算候选。
加载后 token 占用记录从 0 开始。

## 边界

- `/mode event <event-id>` 仍使用确定的事件 session id；再次 `/mode role` 会重新
  扫描并加载最新的角色模式 session。
- `default` pipeline 不扫描最新 session，仍使用固定角色 / 事件 id。
- `memory_summary` 的 case session 行为不变。
- 旧的 session 配置条目与缓存文件保留在磁盘，不做清理或迁移。

## 实现位置

- `kissbot-agent/src/pipelines.rs`：
  - `SESSION_PREFIX_DEFAULT` / `SESSION_PREFIX_DEFAULT_MEMORY` /
    `SESSION_PREFIX_MEMORY_SUMMARY`；
  - `pipeline_session_id(prefix, memory_key)`：按前缀与模式拼 session id；
  - `topic_session_id`：`…-topic-{uuid}`；
  - `latest_role_session_id`：按修改时间扫描角色模式 session；
  - `new_session_with_memory_id`：按指定 id 建立 session 并补齐缺失配置；
  - `new_session_with_memory`：角色模式先扫描最新 session，否则回退基础 id。
- `kissbot-agent/src/pipelines/memory_summary_pipeline.rs`：case session id 使用
  统一前缀常量。

## 验证

- 单元测试：各 pipeline / 模式的 session id 格式；话题 id 唯一且带 `topic` 标记。
- 集成测试：
  - 构造前写入基础（较旧）、话题（较新）、event（最新）三个 session，重建
    pipeline 后加载话题 session，且不写基础与 event session；
  - `/mode event` 后 `/mode role` 加载最新角色模式 session；
  - 原有话题切换、事件模式与命令测试随命名调整更新。
- 验证命令：`cd kissbot-agent && cargo test`、`cargo build`。
