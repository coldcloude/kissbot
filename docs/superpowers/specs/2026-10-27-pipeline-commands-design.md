# pipeline 命令检测与 model / outgoing 配置下沉 设计

## 背景

channel 文本命令目前只有默认一层：`Nexus::incoming_message` 校验管理员后调用
`CommandRouter::execute`，由它内联解析并执行全部命令。其中 `/model` 经
`Nexus::set_channel_pipeline_model` → `verify_model`（拉取 API 模型列表）→
`AgentPipeline::set_model` 写 session 的 `LLMConfig`；`/bind-outgoing`、`/unbind-outgoing`
把 `out_channel` 写到 (agent, role) 级 `OutChannelConfig`。

这带来两个问题：

- 模型与 outgoing 属于具体 pipeline 的执行资源，却由 Nexus 与默认命令路由器固定处理；
- 不同 pipeline（`default` / `default_memory` / `abstract_memory`）需要各自决定支持哪些命令、
  写到哪些 session，现有接口无法表达。

## 目标

- 默认命令检测之后增加 pipeline 命令检测执行接口，由 pipeline 自行识别与解析命令。
- 去掉固定的模型切换接口与固定层级的 outgoing 配置写入，改由 pipeline 命令处理。
- 三条 pipeline 复用相同的命令解析 / 配置写入代码，共享函数集中到 `pipelines.rs`。
- `default` 保留 `/model`，不接 outgoing 命令。

## 方案

### 1. 两层命令检测

默认层（`ChannelManager` 内）继续处理 bind / unbind / admin / unadmin / agent / role / mode；
返回值改为 `Result<Option<ArcStr>>`，`Ok(None)` 表示「不是默认命令」。

pipeline 层新增接口：

```rust
// pipeline.rs
#[async_trait]
pub trait AgentPipeline: IncomingMessageProcessor + Send + Sync {
    /// 识别并执行本 pipeline 支持的管理命令；不是本 pipeline 的命令返回 None。
    /// channel_id 为来源 channel（outgoing 等命令据此校验来源身份），content 为原始命令文本
    async fn execute_command(&self, channel_id: ArcStr, content: ArcStr) -> Option<Result<ArcStr>>;
}
```

ChannelManager 的 `Terminal::incoming_message` 派发顺序：

1. 管理员判定不变（非管理员发送的命令忽略，不回复、不入 pipeline）。
2. 默认层命中 → 回复；`Ok(None)` → 按来源 channel 定位 `MemoryKey`，装配 pipeline 后调用
   `execute_command`。
3. pipeline 返回 `None` → `⚠️ 未知命令: {命令}`；`InvalidCommand` → `⚠️ {描述}`；其余错误 →
   `❌ 命令执行失败: {描述}`。回复文案与现状一致。

### 2. 命令归属

| pipeline | `/model` | `/bind-outgoing`、`/unbind-outgoing` |
|----------|----------|--------------------------------------|
| `default` | ✓ | — |
| `default_memory` | ✓ | ✓ |
| `abstract_memory` | ✓ | ✓ |

### 3. 配置与存储

- `/model`：写入当前 pipeline 的 session `LLMConfig`。`abstract_memory` 继续写基准 session 与
  已有 case session，并保存 `model_override` 供新建 case session 应用。
- `out_channel`：从 (agent, role) 级下沉到 pipeline/session 级。
  - `default_memory`：写自己的 `session_id`。
  - `abstract_memory`：写基准 session `memory-abstract-{encode}-none`，即该 pipeline 的
    pipeline 级配置存放处；运行期从同一 session 读取，不为每个 case session 各存一份。
  - agent/role 继承表中的 `out_channel` **保留**，两条记忆 pipeline 建立 session 时仍从
    agent/role 初始化 `out_channel`（现有 `import_session_config_from_agent_role` 不变）。
- 运行期：`run_once` 从上述 session 读取 `OutChannelConfig` 后发送，不再按 (agent, role) 实时合成。
- 删除 `ConfigManager::set_out_channel` 与 `ConfigManager::out_channel_config`（命令不再使用
  (agent, role) 级读写）；`OutChannelConfig` 类型与继承表条目保留。

### 4. pipelines.rs 共享函数

- `command_parts(content) -> Vec<ArcStr>`：按空白切分一次，pipeline 的 `execute_command` 用
  `match parts[0]` 分发（不再逐个尝试命令）。
- `parse_model_command(parts) -> Result<(ArcStr, ArcStr)>`：`/model` 参数解析（参数个数不符报错）。
- `handle_model_command(parts, session_id) -> Result<ArcStr>`：解析 + 校验 + 写入单 session；
  `default` 与 `default_memory` 共用。
- `switch_model(session_ids, provider, model) -> Result<ArcStr>`：拉模型列表校验后逐个 session
  写入，返回回复文案；`abstract_memory` 多 session 复用。
- `bind_outgoing(session_id, channel_id, parts)` / `unbind_outgoing(session_id, parts)`：执行
  `/bind-outgoing`、`/unbind-outgoing`；两条记忆 pipeline 共用（`abstract_memory` 传基准 session id）。
- `outgoing_channel(session_id) -> Option<Arc<OutChannel>>`：发送前读取 pipeline 的 outgoing 配置。
- 内部复用：`verify_model`（从 Nexus 移入）。

### 5. 删除项

- `Nexus::set_pipeline_model`、`Nexus::set_channel_pipeline_model`、`Nexus::verify_model`。
- `AgentPipeline::set_model`。
- `CommandRouter` 整体删除：默认命令处理迁入 `ChannelManager::execute_default_command`。
- `default` pipeline 的 outgoing 相关内容：命令检测只认 `/model`，不导入 outgoing 配置。

### 6. 通用 pipeline 配置命令

在默认命令层（先于 pipeline 命令检测）增加两条通用命令：

- `/pipeline <pipeline_name>`：设置来源 channel 的 (agent, role) 级 `pipeline_name`；默认 role（空）不可设置。
- `/agent-pipeline <pipeline_name>`：设置来源 channel 的 agent（role 空）级 `pipeline_name`；默认 agent "0" 不可设置。
- 名称校验：仅接受 `default` / `default_memory` / `abstract_memory`。
- 清理：修改前保存受影响 role（当前已装配 pipeline 的 role）merge 后的配置，写入后重新 merge；
  pipeline 名称变化的已装配 pipeline 从 `PipelineManager` 移除，下次按新配置装配。
- 实现：`ChannelManager::execute_default_command` 负责解析与校验，`PipelineManager::set_pipeline_config`
  负责保存 / 比较 / 失效。

## 测试与文档

- 单测：命令识别（非本 pipeline 命令返回 `None`）、`/model` 参数个数错误、
  outgoing 缺参 / 身份未绑定 / channel 不存在、outgoing 写入后可读、解除后为空。
- 集成用例文案保持不变：`/send-channel` → `⚠️ 未知命令`，`/model` 参数错误 →
  `⚠️ 格式: /model <provider> <model>`。
- 更新 `docs/spec/agent-nexus.md`、`docs/spec/agent-modules.md` 与
  `docs/design/components-design/kissbot-agent-nexus.md` 中的命令与 out_channel 说明。

## 影响

- outgoing 配置不再随 (agent, role) 共享：event 模式 / role 切换后各 pipeline 独立配置。
- 未知命令会经 `ensure_pipeline` 定位并（必要时）装配来源 pipeline，回复文案不变。
- `/model`、`/bind-outgoing`、`/unbind-outgoing` 的回复文案与参数校验文案保持在 pipeline 实现中，
  对外行为不变。
