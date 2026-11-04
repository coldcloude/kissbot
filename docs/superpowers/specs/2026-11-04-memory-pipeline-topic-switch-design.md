# default_memory 记忆读取策略设计

## 背景

`default_memory`（`DefaultAgenticLoopPipelineWithMemory`）原先由构造参数 `load_memory` 决定
超阈值时使用记忆回溯还是上下文压缩，写记忆始终进行。该参数在生产装配路径中固定为 `true`，
且没有区分事件模式与角色模式。

本次调整把读取开关落到 session 配置，并让两种模式的行为明确：

- 事件模式（`RoleEvent.event_id` 有值）：**永远不读取记忆**，忽略 `load_memory`，只写记忆。
- 角色模式（`RoleEvent.event_id` 缺省）：`load_memory = true` 时保持记忆回溯；
  `load_memory = false` 时不再回溯，改为在每次 run 前判断新消息是否延续同一件事。

## 配置

新增 session 级配置：

```rust
pub struct MemoryPipelineConfig {
    pub load_memory: Option<bool>, // 未配置时有效值默认 true
}

pub struct EffectiveMemoryPipelineConfig {
    pub load_memory: bool,
}
```

- 存入 `SessionConfigMergeMap`（agent/role 可继承）与 `SessionConfigMap`（session 级有效值），
  合并语义为「有值才覆盖」。
- 建立 `default_memory` pipeline 时通过 `set_session_config_if_not_exists` 从 agent/role 导入；
  已有 session 值优先。
- 同时导入 `memory_recover` 与 `compress` 两套降阈值配置，保证运行期切换开关后立即生效。
- 原构造参数 `load_memory` 移除，读取开关只由 session 配置决定。

## 读取判定

```text
effective_load_memory = (role_event.event_id == None) && config.load_memory
```

- 事件模式恒为 `false`：`run_once` 使用 `CompressTokenReducer`，不调用
  `build_context_from_memory_store`。
- 角色模式按配置：`true` 用 `MemoryRecoverTokenReducer`；`false` 用 `CompressTokenReducer`。
- 判定在每批 run 开始时读取 session 配置，因此 `/load-memory` 修改后下一批即生效。

## 同话题判断（角色模式 + load_memory=false）

每批消息打包完成后、进入 agentic loop 之前：

1. 取当前完整 session（含 System）与本批新消息，调用当前 session 的 LLM（不执行工具）。
2. 第一套提示词要求只回答 `YES`（同一件事）或 `NO`（不是同一件事）。
3. 若调用失败或回答无法解析，换第二套措辞更强的提示词重试，仍要求必须给出 `YES`/`NO`。
4. 两次都失败按「同一件事」处理，记 warn，不丢上下文。
5. 判定为 `NO`（不同）：
   - `Session::context_archive_and_clear_cache_and_reset_messages(vec![新消息])`：
     归档旧上下文、清空缓存、上下文只保留新消息；
   - `last_total_tokens` 归零；
   - 以空消息列表进入 run（新消息已在上下文中，避免重复追加）。
6. 判定为 `YES`（相同）：保持上下文，按正常流程追加新消息并运行。

判断请求本身不写入 session 上下文，也不计入会话 token 占用。

## 命令

`default_memory` 新增命令：

```text
/load-memory <on|off>
```

- 写当前 pipeline session 的 `MemoryPipelineConfig.load_memory`；
- 兼容 `true`/`false`，大小写不敏感；
- 事件模式下仍可写入配置，但运行时忽略，回复中注明「事件模式固定不读取记忆」。

## 边界

- 当前 session 没有任何非 System 消息时，不需要判断，直接 run。
- 会话上下文为空、判断结果为相同或判断失败时，均不会归档/清空。
- 归档/清空失败时记 warn，并按原上下文继续运行。
- `memory_summary` 与 `default` pipeline 不受影响。

## 验证

- 配置默认值、合并、序列化；
- agent/role 继承与 session 覆盖；
- 事件模式忽略配置、角色模式读取配置；
- `/load-memory` 参数解析与 session 写入；
- `YES`/`NO` 解析与两套提示词请求组装；
- 集成测试：角色模式 `load_memory=false` 时，判定为 `NO` 后旧上下文进入
  `context-history`，当前上下文只保留新消息与本次回复，且 LLM 被调用两次（判定 + 正式运行）。
