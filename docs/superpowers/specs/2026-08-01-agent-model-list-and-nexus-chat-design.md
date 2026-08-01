# Agent 模型列表获取 + 保留 agent 行为 + nexus-chat 测试 设计

## 概述

三部分工作：

1. **模型列表获取**：`Provider` 增加从服务商 API 获取全部模型名的能力（`list_models`）；`/model` 切换与启动时 `default_model` 都经 API 校验；`provider.models` 未配置的模型（极端 `models={}`）也能用 provider 默认值合成后切换；校验失败进入"无模型"状态（普通消息静默忽略、仅管理指令可用）。
2. **保留 agent "0" 行为**：无参 `/agent` 设置的保留 agent "0" 也创建会话（能 `/model` 与文本通信），但**不调 memory-ego**，改用 `AgentConfig` 新增的默认系统提示词；**照常调 memory-store / 读 history**。
3. **nexus-chat 测试**：channel-client-cli + channel-web + nexus（deepseek provider，模型 deepseek-v4-flash）真实文本通信；deepseek key 经脚本注入；不依赖 memory-ego/memory-store 服务。

## 现状

- `Provider` trait（provider.rs）只有 `send()`，无模型列表能力。
- `resolve_effective_config`（config_manager.rs:342）硬性要求 `provider.models[model]` 存在，`models={}` 时返回 None，`/model` 切换失败。
- `session_key_for`（coordinator.rs:83）：`agent_id` 为空或 == "0"（保留值）时返回 None——channel 脱离 agent，不建会话，普通消息不处理、`/model` 也报"channel 未关联 agent"。
- `build_initial_context`：新会话一律 `load_ego_info`（memory-ego）+ `read_history`（memory-store）。
- `AgentConfig` 无默认系统提示词字段。
- `/model` 命令格式：`/model <provider> <model>`（`AdminCommand::Model(ProviderModel)`）。
- test 框架：Playwright；`helpers/server.ts`（resetWorkspace/startBackend/startAgent/waitForPort）、`helpers/cli.ts`（spawnCli）；test/workspace-template 是测试工作区模板。

## Part 1：模型列表获取

### 1.1 `Provider` trait 增加 `list_models()`

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, effective: &EffectiveModelConfig, messages: &[MessageItem]) -> Result<ModelResponse>;
    /// 从服务商 API 获取全部可用模型名（GET /models）
    async fn list_models(&self) -> Result<Vec<String>>;
}
```

- `OpenAiProvider`：`GET {base_url}/models` → `data["data"][*]["id"]`（DeepSeek 兼容）
- `AnthropicProvider`：`GET {base_url}/v1/models` → `data["data"][*]["id"]`

### 1.2 Provider 工厂

抽出 model_client.rs:35-36 的 match 为共用工厂，供 send 与 list_models 使用：

```rust
pub fn provider_for(client: Arc<reqwest::Client>, provider_type: &str, base_url: &str, api_key: &str) -> Box<dyn Provider>
```

### 1.3 `resolve_effective_config` 支持未配置模型

`provider.models.get(&pm.model)` 改为 Option：存在则用其覆盖值，否则全部用 provider 的 `default_*` 合成，`model` 字段取 `pm.model`。`models={}` 也能合成。

### 1.4 `/model` 切换流程（每次现调 + 失败拒绝）

`set_session_model`（coordinator）：
1. 取 channel + session key（key 不存在→"channel 未关联 agent"）
2. 从 `NexusRepo.providers` 取 `ProviderConfig`（不存在→拒绝）
3. `provider_for` 构造 Provider → `list_models()`；**失败（网络/鉴权）→ 拒绝，保持原模型不变**
4. 模型名不在列表 → 拒绝（`ModelProviderNotSupported`）
5. `resolve_effective_config` 合成（配置里有则合并，否则默认值）
6. `session.model.store(Some(pm))`

### 1.5 启动校验 `default_model` + 无模型状态

- `coordinator::new`：构造 Provider → `list_models()` 校验 `default_model.model` 在列表；**失败（provider 未配置 / API 错误 / 不在列表）→ 无模型状态**，告警日志。
- `Session.model` 类型：`ArcSwap<ProviderModel>` → `ArcSwap<Option<ProviderModel>>`（None=无模型）。
- `run_agentic_loop` 前检查：model 为 None → **静默忽略**普通消息（不回复、不进入 agentic loop）；管理指令照常可执行（`/model` 可恢复）。
- 启动校验成功 → 新会话默认 `Some(default_model)`。

## Part 2：保留 agent "0" 行为

### 2.1 `session_key_for`：仅空 agent_id 脱离，"0" 也建会话

```rust
fn session_key_for(&self, ch: &ChannelConfig) -> Option<SessionKey> {
    let agent_id = ch.agent_id.to_string();
    if agent_id.is_empty() {
        return None; // 脱离 agent：只处理管理命令
    }
    // agent_id == "0"（保留）同样建会话，但初始上下文用默认系统提示词（见 build_initial_context）
    Some(SessionKey { agent_id, role_name: ch.role_name.to_string(), mode: ... })
}
```

### 2.2 `build_initial_context`：按 agent 分流 ego / 默认提示词

```rust
async fn build_initial_context(&self, session: &Arc<Session>) {
    // "0" 不调 memory-ego，用 AgentConfig 默认系统提示词；其余 agent 走 load_ego_info
    if session.key.agent_id == RESERVED_AGENT_ID {
        session.context.lock().await.set_system_message(self.config.default_system_prompt().to_string());
    } else if let Ok(ego_info) = self.load_ego_info(&session.key.agent_id, &session.key.role_name).await {
        session.context.lock().await.set_system_message(ego_info);
    }
    // history 一律照常加载（memory-store；URL 空则优雅跳过）
    if let Ok(history) = self.memory_reader.read_history(...).await {
        session.context.lock().await.load_history(history);
    }
}
```

### 2.3 `AgentConfig` 增加默认系统提示词

```rust
pub struct AgentConfig {
    pub data_dir: Arc<String>,
    pub mgmt_host: Arc<String>,
    pub mgmt_port: u16,
    pub ws_reconnect_interval_secs: u64,
    pub default_system_prompt: Arc<String>,   // 新增：agent "0" 的默认系统提示词
    pub init_agent_id: Arc<String>,
    pub init_role: Arc<String>,
    pub init_model: Arc<String>,
}
```

必填字段（沿用"去掉 serde(default)"的严格约定），repo 内 config.json（root / script / test/workspace-template）均需补该字段。

## Part 3：nexus-chat 测试

### 3.1 脚本拆分（key 注入可被 test import）

- `script/inject-key.mjs`（新）：导出 `injectApiKeys(keyFile, nexusPath)`——从 key 文件（`{"provider名":"key"}`）按 provider 名注入 `nexus.providers[].api_key`，就地写回；保留 CLI 入口（`node inject-key.mjs <keyFile> <nexusPath>`）。**test 用 ESM import 直接调用函数**。
- `script/agent-reset.sh`（改造）：mkdir 数据目录 → `cp template/nexus.json`、`cp template/station.json`（模板复制逻辑进 shell）→ `node inject-key.mjs`。可选参数：数据目录、key 文件路径（默认 workspace/agent-data + 根目录 key.local.json）。
- 删除 `script/agent-reset.mjs`。

### 3.2 test 工作区模板

- `test/workspace-template/agent-data/nexus.json`（新）：deepseek provider（api_key 空）+ models 含 `deepseek-4-flash`（现有测试用）与 `deepseek-v4-flash`（nexus-chat 用）；channel 配置预置 `agent_id: ""`（初始脱离，由测试用无参 `/agent` 挂到保留 agent "0"，配合 Part 2 行为）。
- `test/workspace-template/config.json`：agent 段补 `default_system_prompt`；`api` 段 `memory_store_url` / `memory_ego_url` 保持空（不依赖 memory 服务）。

### 3.3 测试基建

- `helpers/server.ts` 加 `injectAgentApiKeys()`：import `injectApiKeys`，传根目录 `key.local.json` + `test/workspace/agent-data/nexus.json`。

### 3.4 `agent-commands.spec.ts`

删除 /model 相关用例（TC-01 非管理员 /model 忽略、TC-04 管理员 /model 切换）——/model 测试并入 nexus-chat。其余用例保留。

### 3.5 `nexus-chat.spec.ts`（新）

```
beforeAll: resetWorkspace → injectAgentApiKeys → startBackend(8301) → startAgent(9090)
           → spawnCli(web u2 g1) 等 bound
TC-1: /agent（无参）→ 断言"已设置 agent: 0 / role: 0"（channel 挂到保留 agent "0"）
TC-2: /model deepseek deepseek-v4-flash → 断言"已切换模型为: deepseek/deepseek-v4-flash"
      （真实 API 校验；key 已注入）
TC-3: 普通文本"你好，请自我介绍" → 断言 CLI 收到 agent 非空回复（真实 LLM deepseek-v4-flash）
```

不依赖 memory-ego（agent "0" 走默认提示词）与 memory-store（URL 空，history 读取优雅跳过、写入仅记日志）。

## 关键决策

- 模型列表**每次切换现调** API（B=a），失败拒绝切换（D）。
- `models={}` 也能切换：未配置模型用 provider 默认值合成（C）。
- 启动 `default_model` 也经 API 校验，失败进入**无模型状态**：普通消息**静默忽略**，仅管理指令可用。
- 保留 agent "0"：建会话、**不调 memory-ego**、用 `AgentConfig.default_system_prompt`；**照常调 memory-store / 读 history**。
- key 注入脚本拆分为可 import 的 `inject-key.mjs`，模板复制等进 `agent-reset.sh`。

## 代码影响

| 文件 | 改动 |
|------|------|
| `kissbot-agent/src/provider.rs` | Provider trait + `list_models()`；OpenAi/Anthropic 实现；`provider_for` 工厂 |
| `kissbot-agent/src/model_client.rs` | 用 `provider_for` 工厂；`call` 逻辑不变 |
| `kissbot-agent/src/config_manager.rs` | `resolve_effective_config` 支持未配置模型；AgentConfig 加 `default_system_prompt` + getter |
| `kissbot-agent/src/session_manager.rs` | `Session.model: ArcSwap<Option<ProviderModel>>` |
| `kissbot-agent/src/coordinator.rs` | `session_key_for`（仅空脱离）；`build_initial_context`（"0" 用默认提示词）；启动校验 default_model；`set_session_model` API 校验；`run_agentic_loop` 无模型静默忽略 |
| `config.json` / `script/config.json` / `test/workspace-template/config.json` | agent 段补 `default_system_prompt` |
| `script/inject-key.mjs` | 新建：`injectApiKeys` 导出 + CLI |
| `script/agent-reset.sh` | 模板复制 + 调 inject-key.mjs |
| `script/agent-reset.mjs` | 删除 |
| `test/workspace-template/agent-data/nexus.json` | 新建：deepseek provider + 两模型 + channel agent "0" |
| `test/tests/helpers/server.ts` | 加 `injectAgentApiKeys()` |
| `test/tests/agent-commands.spec.ts` | 删 /model 用例 |
| `test/tests/nexus-chat.spec.ts` | 新建 |
