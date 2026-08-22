# 设计：max_tokens_usage 驱动的上下文重置

日期：2026-08-23
状态：已确认

## 背景

现有上下文长度控制依赖 `context_length`（token 上下文长度）与 `max_context_messages`（消息条数上限）两个配置，
溢出判断基于消息条数（`SessionContext::is_overflow`），与实际 token 占用脱节，不符合真实上下文占用情况。

目标：改用服务商 API 返回的 `usage.total_tokens`（真实 token 占用）驱动重置——
每次模型请求完成后保存 usage，留到下次请求开头检查；超过 `max_tokens_usage` 的 80% 则触发重置
（沿用现有重建逻辑：event 模式 LLM 压缩、role 模式从记忆打包重建）。

## 决策点

1. **max_tokens_usage 必填语义**：serde 必填非 Option 字段（方案 A）。
   旧 nexus.json / 管理 API 请求缺字段直接解析失败（破坏性变更，符合本项目配置迁移先例）。
2. **检查时机**：overflow 检查保持在 `run_agentic_loop` 开头（追加用户消息前）；
   每次模型请求完成后保存 usage，留到下次请求开头触发重置；没有新消息就不重置（延迟检查）。
   工具轮次中途不打断（避免重建丢失 tool_calls 连续性）。
3. **Anthropic**：`total_tokens` 固定返回 0（Anthropic API 文档未找到），永不触发。
4. **80% 阈值**：硬编码常量（整数运算 `last_total_tokens * 10 > (max_tokens_usage as u64) * 8`，避免浮点；u64 提升防 u32 乘法溢出）。

## 变更内容

### 1. 配置变更（kissbot-agent/src/config_manager.rs）

- 删除常量 `DEFAULT_CONTEXT_LENGTH`、`DEFAULT_MAX_CONTEXT_MESSAGES`
- `ModelConfig`：删除 `context_length`、`max_context_messages` 两个 Option 字段；
  新增 `max_tokens_usage: u32`（serde 必填，无 `#[serde(default)]`，缺字段解析失败）
- `EffectiveModelConfig`：删除 `context_length`、`max_context_messages`；
  新增 `max_tokens_usage: u32`
- `merge_model_config`：`max_tokens_usage` 继承规则 `model 覆盖 → provider 默认`
  （必填字段，两处均有值，无全局默认回落）

### 2. ModelResponse 增加 total_tokens（types.rs + provider.rs）

- `types.rs`：`ModelResponse` 新增 `total_tokens: u64`
- `provider.rs` `parse_openai_response`：取 `data["usage"]["total_tokens"]`
  （DeepSeek、Kimi 均已通过官方文档确认该字段；缺字段回退 0）
- `provider.rs` `parse_anthropic_response`：固定 0

### 3. Session 记录 usage + 开头检查（session_manager.rs）

- `Session` 新增字段 `last_total_tokens: AtomicU64`（初始 0；
  每会话单消费任务串行调用 `run_agentic_loop`，原子即可，无锁）
- `run_agentic_loop` 开头（原 overflow 检查位置）改为 token 检查：
  ```
  读取 effective.max_tokens_usage
  若 last_total_tokens * 10 > (max_tokens_usage as u64) * 8（80% 阈值）
  → 执行现有重建逻辑（event 压缩 / role 记忆打包），重建后 last_total_tokens 清零
  ```
- agentic loop 内每次模型调用成功后（最终回复或工具轮次）：
  `last_total_tokens.store(model_resp.total_tokens)`；调用失败不更新（保持原值）
- 删除 `SessionContext::is_overflow`（消息条数判断废弃）

### 4. 边界语义

- 模型切换：`last_total_tokens` 按会话记录，检查用当前 effective 的 `max_tokens_usage`；
  旧 usage 至多导致提前一次重建，无害
- 系统消息切换（apply_pending_system 归档）：不清 usage，保守处理
- 会话新建：usage 初始 0，不触发
- 工具轮次内多次模型调用：每次成功后都更新 usage（取最近一次）

### 5. 配置文件与测试同步

- `script/template/nexus.json`、`script/README.md`、`test/workspace-template/agent-data/nexus.json`、
  `test/workspace/agent-data/nexus.json`：删除两字段，增加 `max_tokens_usage`（示例值 128000）
- `provider.rs` / `config_manager.rs` / `http_server.rs` 测试夹具：同步结构变更
- `test/tests/agent-config-api.spec.ts` TC-02：`context_length: 200000` 断言改为 `max_tokens_usage`
- 新增单元测试：
  - openai 解析 total_tokens（含缺字段回退 0）
  - anthropic 固定 0
  - 80% 判定触发 / 不触发
  - 重建后 usage 清零

## 影响范围

- kissbot-agent（config_manager / provider / session_manager / types / http_server 测试夹具）
- 配置模板与文档（script/template、script/README.md、test workspace nexus.json）
- e2e 测试（agent-config-api.spec.ts）
- kissbot-agent-config 前端 UI 无涉及（未引用相关字段）

## 不做的事（YAGNI）

- 不做流式响应 usage 处理（当前为非流式）
- 不做 Anthropic usage 解析（文档未找到，暂返回 0）
- 不做 80% 阈值的可配置化（硬编码常量）
- 不做 mid-loop 立即重建（保留延迟检查）
