# Agent 多会话适配实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 kissbot-agent 从单会话改造为多会话：nexus 按 (agent_id, role_name, mode) 三元组管理会话，各会话独立上下文/模型/模式状态，消息按来源 channel 的绑定配置路由，回复经会话发送 channel 发出。

**Architecture:** 新增 SessionManager 模块（合并原 ContextBuilder 逻辑）；ChannelConfig 扁平化为「配置即运行值」（bind_user/agent_id/role_name/is_send_channel/enabled 更新即回写 nexus.json），运行态仅保留 per-channel mode；Coordinator 按来源 channel 绑定三元组定位会话并路由，回复走会话发送 channel。

**Tech Stack:** Rust + tokio + dashmap + arc-swap + axum；测试用 cargo test + Playwright（channel-web + cli）。

**Spec:** `docs/superpowers/specs/2026-08-01-agent-multi-session-design.md`

## Global Constraints

- 不要删除代码中的注释（项目 CLAUDE.md 约定；本计划改写旧逻辑时注释同步更新语义，不删注释）
- 提交 comment 用中文，包含该次提交全部改动内容
- 文本文件 UTF-8、\n 换行
- 读写文件必须使用 Read/Write/Edit 工具，禁止 sed/python 改文件
- 保留值：`agent_id = "0"`、`role_name = "0"`（agent="0" 或空 = 脱离 agent，该 channel 只处理管理命令）
- 事件记忆编码分隔符为横线：`{role_name}-{event}`，只改 agent 侧，memory-store 不动
- /unbind 暂不进行任何操作（回复提示）
- /model 调整**来源 channel 所属会话**的模型（运行态，不回写）；会话模型初始取 `NexusRepo.default_model`
- 运行态仅保存 per-channel mode（DashMap<channel_id, Mode>，不回写，重启回 Role）
- **任务顺序约束**：Task 1、Task 2 必须保持 crate 可编译（纯增量）；Task 3 是原子集成改造（coordinator/command_router/ChannelConfig 强耦合，本任务内一次性落地，完成后整体编译）

---

### Task 1: types.rs 基础类型改造（纯增量，保持可编译）

**Files:**
- Modify: `kissbot-agent/src/types.rs`
- Test: `kissbot-agent/src/types.rs`（文件内 `#[cfg(test)]`）

**Interfaces:**
- Consumes: 现有 `Mode`、`AdminCommand`、`ProviderModel`
- Produces:
  - `Mode` 增加 `PartialEq, Eq, Hash` 派生
  - `pub struct SessionKey { pub agent_id: String, pub role_name: String, pub mode: Mode }`（derive `Debug, Clone, PartialEq, Eq, Hash`）
  - `AdminCommand`：**保留** `Agent(String)`（Task 3 删除），新增 `SetAgent { agent_id: Option<String>, role: Option<String> }`、`SendChannel(bool)`
  - `pub enum CommandEffect { None, Relocate, ResetSession }`（derive `Debug, Clone, Copy, PartialEq, Eq`）
  - `pub fn memory_role(key: &SessionKey) -> String`：事件模式返回 `format!("{}-{}", role_name, event_id)`，角色模式返回 role_name
- 约束：本任务结束时 `cd kissbot-agent && cargo build` 必须通过（旧代码未受影响）

- [ ] **Step 1: 写失败测试**

在 `kissbot-agent/src/types.rs` 末尾新增 `#[cfg(test)]` 模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_hash_eq_by_value() {
        use std::collections::HashSet;
        let a = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let b = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let c = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 SessionKey 应命中 HashSet");
        assert!(!set.contains(&c), "不同 mode 不应命中");
    }

    #[test]
    fn memory_role_encodes_event_only() {
        let role_key = SessionKey { agent_id: "a1".into(), role_name: "dev".into(), mode: Mode::Role };
        assert_eq!(memory_role(&role_key), "dev");
        let event_key = SessionKey { agent_id: "a1".into(), role_name: "dev".into(), mode: Mode::Event("e1".into()) };
        assert_eq!(memory_role(&event_key), "dev-e1");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd kissbot-agent && cargo test types::tests -v 2>&1 | tail -20`
Expected: 编译失败，`SessionKey` 未定义 / `memory_role` 未定义

- [ ] **Step 3: 实现**

在 `kissbot-agent/src/types.rs` 中：

`Mode` 派生增加：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    Role,
    Event(String),
}
```

在 `Mode` 之后新增：

```rust
// ========== 会话标识 ==========

/// 会话唯一标识：agent_id + role_name + mode 三元组
/// 所有绑定 channel 的信息去重，每个三元组 = 一个会话
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub agent_id: String,
    pub role_name: String,
    pub mode: Mode,
}

/// 记忆读写边界的 role 编码：事件模式拼 {role}-{event}（对 memory-store 透明），角色模式原样
pub fn memory_role(key: &SessionKey) -> String {
    match &key.mode {
        Mode::Event(event_id) => format!("{}-{}", key.role_name, event_id),
        Mode::Role => key.role_name.clone(),
    }
}
```

`AdminCommand` 中 `Agent(String)` 变体**保留不动**，在其之后新增：

```rust
    /// 设置 channel 绑定的 agent 与 role（缺省用保留值 "0"）；旧 Agent 变体 Task 3 删除
    SetAgent { agent_id: Option<String>, role: Option<String> },
```

在 `Reenter(String)` 与 `Events` 之间插入：

```rust
    SendChannel(bool),
```

在 `AdminCommand` 之后新增：

```rust
// ========== 管理命令执行效果 ==========

/// 命令执行后协调器需做的后续动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEffect {
    None,
    /// 绑定/模式变化，来源 channel 需按新三元组重定位会话
    Relocate,
    /// 重置来源 channel 所属会话的上下文
    ResetSession,
}
```

- [ ] **Step 4: 运行测试确认通过 + 编译验证**

Run: `cd kissbot-agent && cargo test types::tests -v 2>&1 | tail -20 && cargo build 2>&1 | tail -5`
Expected: 2 个测试 PASS，且 `cargo build` 无错误（旧代码兼容）

- [ ] **Step 5: 提交**

```bash
git add kissbot-agent/src/types.rs
git commit -m "feat(agent): types 增加 SessionKey/CommandEffect/memory_role，Mode 支持 Hash，AdminCommand 增加 SetAgent/SendChannel（保留旧 Agent 变体）"
```

---

### Task 2: session_manager.rs 新模块（纯增量，暂不删 context_builder）

**Files:**
- Create: `kissbot-agent/src/session_manager.rs`
- Modify: `kissbot-agent/src/main.rs`（仅增加 `mod session_manager;`，**保留** `mod context_builder;`）
- Test: `kissbot-agent/src/session_manager.rs`（文件内 `#[cfg(test)]`）

**Interfaces:**
- Consumes: Task 1 的 `SessionKey` / `Mode`；现有 `ProviderModel` / `ChannelConfig` / `ContextMessage` / `MessageItem`
- Produces:
  - `pub struct SessionContext`（原 ContextBuilder 全部方法：`new` / `set_system_message` / `load_history` / `push_user_message` / `push_assistant` / `push_tool_call` / `push_tool_result` / `record_sent_content` / `is_self_echo` / `build` / `is_overflow` / `clear`）
  - `pub struct Session { pub key: SessionKey, pub context: tokio::sync::Mutex<SessionContext>, pub model: ArcSwap<ProviderModel> }`，`Session::new(key, model)`
  - `pub struct SessionManager { sessions: DashMap<SessionKey, Arc<Session>>, channel_modes: DashMap<String, Mode> }`
  - `SessionManager::new() -> Arc<Self>`
  - `get(&self, key: &SessionKey) -> Option<Arc<Session>>`
  - `get_or_create(&self, key: &SessionKey, model: ProviderModel) -> (Arc<Session>, bool)`（bool = 是否新建）
  - `retain(&self, keys: &HashSet<SessionKey>)`：只保留集合内的会话
  - `set_channel_mode(&self, channel_id: &str, mode: Mode)` / `channel_mode(&self, channel_id: &str) -> Mode`（缺省 `Mode::Role`）
  - `resolve_send_channel(&self, key: &SessionKey, channels: Vec<(String, Arc<ChannelConfig>)>) -> Option<String>`：`is_send_channel=true` 优先，否则首个绑定；都不匹配返回 None
- 约束：本任务结束时 `cd kissbot-agent && cargo test` 必须全部通过（含既有测试），`cargo build` 无错误

- [ ] **Step 1: 创建实现文件（含单测）**

创建 `kissbot-agent/src/session_manager.rs`，完整内容见 Step 2（实现 + `#[cfg(test)]` 单测在一个文件中）。

- [ ] **Step 2: 实现（完整文件）**

`kissbot-agent/src/session_manager.rs` 全文：

```rust
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use arc_swap::ArcSwap;
use dashmap::DashMap;

use crate::config_manager::{ChannelConfig, ProviderModel};
use crate::types::{ContextMessage, MessageItem, Mode, SessionKey};

/// 最大上下文消息数量，超过时触发重置
const MAX_CONTEXT_MESSAGES: usize = 100;

/// 会话上下文（原 ContextBuilder 逻辑，按会话持有）
pub struct SessionContext {
    messages: VecDeque<ContextMessage>,
    system_message: Option<String>,
    /// 保存已发送的消息 content，用于 is_self=1 对比
    sent_contents: VecDeque<String>,
}

impl SessionContext {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            system_message: None,
            sent_contents: VecDeque::with_capacity(64),
        }
    }

    /// 设置系统消息（会话创建或重置时）
    pub fn set_system_message(&mut self, content: String) {
        self.system_message = Some(content);
    }

    /// 从 MemoryReader 加载历史记录重建上下文
    pub fn load_history(&mut self, messages: Vec<ContextMessage>) {
        self.messages.clear();
        for msg in messages {
            self.messages.push_back(msg);
        }
    }

    /// 追加用户消息
    pub fn push_user_message(&mut self, msg: ContextMessage) {
        self.messages.push_back(msg);
    }

    /// 追加 assistant 回复
    pub fn push_assistant(&mut self, content: String, time: String) {
        self.messages.push_back(ContextMessage::Assistant { content, time });
    }

    /// 追加 tool call
    pub fn push_tool_call(&mut self, tool_name: String, parameters: serde_json::Value, time: String) {
        self.messages.push_back(ContextMessage::ToolCall { tool_name, parameters, time });
    }

    /// 追加 tool result
    pub fn push_tool_result(&mut self, tool_name: String, result: serde_json::Value, time: String) {
        self.messages.push_back(ContextMessage::ToolResult { tool_name, result, time });
    }

    /// 记录已发送的消息内容（用于 is_self=1 识别）
    pub fn record_sent_content(&mut self, content: String) {
        if self.sent_contents.len() >= 64 {
            self.sent_contents.pop_front();
        }
        self.sent_contents.push_back(content);
    }

    /// 检查内容是否为最近发出的消息回显
    pub fn is_self_echo(&self, content: &str) -> bool {
        self.sent_contents.iter().any(|s| s == content)
    }

    /// 构建模型消息列表
    pub fn build(&self) -> Vec<MessageItem> {
        let mut items = Vec::new();

        if let Some(system) = &self.system_message {
            items.push(MessageItem {
                role: "system".to_string(),
                content: system.clone(),
            });
        }

        for msg in &self.messages {
            match msg {
                ContextMessage::User { content, .. } => {
                    items.push(MessageItem {
                        role: "user".to_string(),
                        content: content.clone(),
                    });
                }
                ContextMessage::Assistant { content, .. } => {
                    items.push(MessageItem {
                        role: "assistant".to_string(),
                        content: content.clone(),
                    });
                }
                ContextMessage::ToolCall { tool_name, parameters, .. } => {
                    items.push(MessageItem {
                        role: "assistant".to_string(),
                        content: format!("工具调用: {} ({})", tool_name, parameters),
                    });
                }
                ContextMessage::ToolResult { tool_name, result, .. } => {
                    items.push(MessageItem {
                        role: "user".to_string(),
                        content: format!("工具 {} 返回: {}", tool_name, result),
                    });
                }
            }
        }

        items
    }

    /// 检查上下文是否超长
    pub fn is_overflow(&self) -> bool {
        self.messages.len() >= MAX_CONTEXT_MESSAGES
    }

    /// 清空上下文（重置时调用）
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

/// 单个会话：独立上下文、模型与模式状态
pub struct Session {
    pub key: SessionKey,
    pub context: tokio::sync::Mutex<SessionContext>,
    /// 会话级模型（创建时取 default_model，/model 调整）
    pub model: ArcSwap<ProviderModel>,
}

impl Session {
    pub fn new(key: SessionKey, model: ProviderModel) -> Self {
        Self {
            key,
            context: tokio::sync::Mutex::new(SessionContext::new()),
            model: ArcSwap::from_pointee(model),
        }
    }
}

/// 会话管理器：汇总所有绑定 channel 的 (agent_id, role_name, mode) 去重维护会话集合
pub struct SessionManager {
    sessions: DashMap<SessionKey, Arc<Session>>,
    /// 运行态 per-channel mode（不回写，重启回 Role）
    channel_modes: DashMap<String, Mode>,
}

impl SessionManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
            channel_modes: DashMap::new(),
        })
    }

    /// 按 key 取会话
    pub fn get(&self, key: &SessionKey) -> Option<Arc<Session>> {
        self.sessions.get(key).map(|e| e.value().clone())
    }

    /// 定位会话，不存在则创建（model 为初始模型）；返回 (会话, 是否新建)
    pub fn get_or_create(&self, key: &SessionKey, model: ProviderModel) -> (Arc<Session>, bool) {
        if let Some(s) = self.get(key) {
            return (s, false);
        }
        let session = Arc::new(Session::new(key.clone(), model));
        self.sessions.insert(key.clone(), session.clone());
        (session, true)
    }

    /// 只保留仍在绑定集合中的会话（绑定信息变化后清理无绑定会话）
    pub fn retain(&self, keys: &HashSet<SessionKey>) {
        self.sessions.retain(|k, _| keys.contains(k));
    }

    /// 设置来源 channel 的运行态模式（不回写）
    pub fn set_channel_mode(&self, channel_id: &str, mode: Mode) {
        self.channel_modes.insert(channel_id.to_string(), mode);
    }

    /// 读取来源 channel 的运行态模式（缺省角色模式）
    pub fn channel_mode(&self, channel_id: &str) -> Mode {
        self.channel_modes.get(channel_id).map(|m| m.value().clone()).unwrap_or(Mode::Role)
    }

    /// 从绑定该会话的多个 channel 中选定发送 channel：
    /// is_send_channel=true 优先，否则选首个绑定；无匹配返回 None
    pub fn resolve_send_channel(
        &self,
        key: &SessionKey,
        channels: Vec<(String, Arc<ChannelConfig>)>,
    ) -> Option<String> {
        let mut first = None;
        for (cid, ch) in channels {
            if ch.agent_id.as_str() != key.agent_id || ch.role_name.as_str() != key.role_name {
                continue;
            }
            if self.channel_mode(&cid) != key.mode {
                continue;
            }
            if !ch.enabled {
                continue;
            }
            if first.is_none() {
                first = Some(cid.clone());
            }
            if ch.is_send_channel {
                return Some(cid);
            }
        }
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_manager::ChannelUser;

    fn sample_channel(id: &str, agent: &str, role: &str, is_send: bool) -> ChannelConfig {
        ChannelConfig {
            channel_id: Arc::new(id.into()),
            ws_url: Arc::new("ws://127.0.0.1:8201".into()),
            admins: Arc::new(HashSet::new()),
            bind_user: ChannelUser { messenger_id: Arc::new("web".into()), user_id: Arc::new("u1".into()) },
            agent_id: Arc::new(agent.into()),
            role_name: Arc::new(role.into()),
            is_send_channel: is_send,
            enabled: true,
        }
    }

    fn key(agent: &str, role: &str) -> SessionKey {
        SessionKey { agent_id: agent.into(), role_name: role.into(), mode: Mode::Role }
    }

    #[test]
    fn get_or_create_dedupes() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k = key("a1", "r1");
        let (s1, created1) = mgr.get_or_create(&k, model.clone());
        assert!(created1, "首次创建");
        let (s2, created2) = mgr.get_or_create(&k, model.clone());
        assert!(!created2, "同 key 复用");
        assert!(Arc::ptr_eq(&s1, &s2), "同 key 应返回同一 Session");
        // 不同 mode 是不同会话
        let k_event = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let (_s3, created3) = mgr.get_or_create(&k_event, model);
        assert!(created3, "事件模式是独立会话");
    }

    #[test]
    fn retain_prunes_unbound() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k1 = key("a1", "r1");
        let k2 = key("a2", "r2");
        mgr.get_or_create(&k1, model.clone());
        mgr.get_or_create(&k2, model);
        let mut keep = HashSet::new();
        keep.insert(k1.clone());
        mgr.retain(&keep);
        assert!(mgr.get(&k1).is_some(), "仍在绑定集合的会话保留");
        assert!(mgr.get(&k2).is_none(), "无绑定会话销毁");
    }

    #[test]
    fn resolve_send_channel_flag_then_first() {
        let mgr = SessionManager::new();
        let k = key("a1", "r1");
        let channels = vec![
            ("c1".to_string(), Arc::new(sample_channel("c1", "a1", "r1", false))),
            ("c2".to_string(), Arc::new(sample_channel("c2", "a1", "r1", true))),
            ("c3".to_string(), Arc::new(sample_channel("c3", "a1", "r1", false))),
        ];
        assert_eq!(mgr.resolve_send_channel(&k, channels.clone()).as_deref(), Some("c2"), "is_send_channel 优先");

        // 全 false → 首个绑定
        let channels_all_false = vec![
            ("c1".to_string(), Arc::new(sample_channel("c1", "a1", "r1", false))),
            ("c3".to_string(), Arc::new(sample_channel("c3", "a1", "r1", false))),
        ];
        assert_eq!(mgr.resolve_send_channel(&k, channels_all_false).as_deref(), Some("c1"));

        // 不同三元组 → None
        let other = vec![("c9".to_string(), Arc::new(sample_channel("c9", "a9", "r9", true)))];
        assert_eq!(mgr.resolve_send_channel(&k, other), None);
    }

    #[test]
    fn channel_mode_default_role_and_set() {
        let mgr = SessionManager::new();
        assert_eq!(mgr.channel_mode("c1"), Mode::Role, "缺省角色模式");
        mgr.set_channel_mode("c1", Mode::Event("e9".into()));
        assert_eq!(mgr.channel_mode("c1"), Mode::Event("e9".into()));
        assert_eq!(mgr.channel_mode("c2"), Mode::Role, "未设置仍为角色模式");
    }
}
```

注意：本任务中 `ChannelConfig` 仍是旧结构（`default_bind_user` / `enabled_by_default`）——session_manager 的测试用新字段构造会在 Task 3 完成前编译失败。因此本任务先**不写测试中的 `sample_channel` 新字段**，改为：先创建 `session_manager.rs`（**不含** `#[cfg(test)]` 模块），跑 `cargo test` 确认既有测试仍全绿、编译通过并提交；测试随 Task 3 的 ChannelConfig 扁平化一并落地（Task 3 步骤 1 会给出完整测试代码）。

- [ ] **Step 3: main.rs 声明**

`kissbot-agent/src/main.rs` 的模块声明区增加一行（**保留** `mod context_builder;`）：

```rust
mod session_manager;
```

- [ ] **Step 4: 编译验证（crate 必须仍编译）**

Run: `cd kissbot-agent && cargo build 2>&1 | tail -10 && cargo test 2>&1 | tail -10`
Expected: 编译通过；既有测试全部 PASS（本任务不新增可运行单测，单测随 Task 3 落地）

- [ ] **Step 5: 提交**

```bash
git add kissbot-agent/src/session_manager.rs kissbot-agent/src/main.rs
git commit -m "feat(agent): 新增 SessionManager 模块（会话集合/上下文/发送 channel/per-channel mode），main 声明新模块"
```

---

### Task 3: 一体化集成改造（ChannelConfig 扁平化 + Coordinator/CommandRouter 重写，原子落地）

**Files:**
- Modify: `kissbot-agent/src/config_manager.rs`（ChannelConfig 扁平化 + update_channel + channels() 去 allow + 3 个新测试）
- Rewrite: `kissbot-agent/src/coordinator.rs`（多会话路由）
- Rewrite: `kissbot-agent/src/command_router.rs`（命令新语义）
- Modify: `kissbot-agent/src/types.rs`（删除旧 `Agent(String)` 变体）
- Modify: `kissbot-agent/src/main.rs`（删除 `mod context_builder;`，保留 `mod session_manager;`）
- Delete: `kissbot-agent/src/context_builder.rs`
- Modify: `kissbot-agent/src/memory_reader.rs`（事件编码冒号改横线）
- Modify: `kissbot-agent/src/http_server.rs`（测试 channel JSON 适配新结构）
- Test: `kissbot-agent/src/session_manager.rs`（本任务补充 4 个单测，见步骤 4 完整代码）

**Interfaces:**
- Consumes: Task 1（SessionKey/CommandEffect/memory_role/SetAgent/SendChannel）、Task 2（SessionManager/Session/SessionContext）
- Produces:
  - `ChannelConfig { channel_id, ws_url, admins, bind_user: ChannelUser, agent_id: Arc<String>, role_name: Arc<String>, is_send_channel: bool, enabled: bool }`（serde 别名 `default_bind_user`/`enabled_by_default` 兼容旧文件，新字段 `#[serde(default)]`）
  - `ConfigManager::update_channel<F>(&self, channel_id: &str, f: F) -> Result<()> where F: FnOnce(&mut ChannelConfig) + Send`
  - `pub const RESERVED_AGENT_ID: &str = "0";` / `pub const RESERVED_ROLE_NAME: &str = "0";`
  - `AgentCoordinator` 方法：`set_channel_mode` / `set_send_channel(channel_id, on) -> Result<()>` / `set_session_model(channel_id, pm) -> Result<()>` / `list_events(channel_id) -> Result<String>` / `relocate_channel(channel_id)` / `reset_session_for(channel_id)`
  - `CommandRouter::execute(command, config, coordinator, channel_id) -> Result<(String, CommandEffect)>`
- 约束：本任务结束后 `cd kissbot-agent && cargo test` 全部 PASS、`cargo build` 无 warning 级错误

- [ ] **Step 1: config_manager.rs ChannelConfig 扁平化 + update_channel + 测试**

在 `kissbot-agent/src/config_manager.rs` 中，将 `ChannelConfig` 定义替换为：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub channel_id: Arc<String>,         // agent 内部唯一标识，与消息方 messenger 无关
    pub ws_url: Arc<String>,
    pub admins: Arc<HashSet<ChannelUser>>,
    /// 绑定用户（必填；auto-bind 功能以后再做）
    /// 旧字段名 default_bind_user 别名兼容旧 nexus.json
    #[serde(alias = "default_bind_user")]
    pub bind_user: ChannelUser,
    /// 绑定的 agent_id（"0" 或空 = 脱离 agent，该 channel 只处理管理命令）
    #[serde(default)]
    pub agent_id: Arc<String>,
    #[serde(default)]
    pub role_name: Arc<String>,
    /// 是否选为该会话的发送 channel
    #[serde(default)]
    pub is_send_channel: bool,
    /// 是否启用（连接由 enabled 控制）
    /// 旧字段名 enabled_by_default 别名兼容旧 nexus.json
    #[serde(alias = "enabled_by_default")]
    pub enabled: bool,
}
```

`channels()` 方法去掉 `#[allow(dead_code)]`：

```rust
    // ---------- channels ----------
    /// 返回所有 channel 配置快照（channel_id -> Arc<ChannelConfig>）
    pub async fn channels(&self) -> Vec<(String, Arc<ChannelConfig>)> {
        let repo = self.nexus_repo.read().await;
        repo.channels.iter().map(|(k, v)| (k.clone(), v.load().clone())).collect()
    }
```

在 `remove_channel` 之后新增：

```rust
    /// 修改 channel 配置并落盘（绑定/agent/role/is_send_channel 等运行时回写统一入口）
    /// channel 不存在返回 ConfigNotFound
    pub async fn update_channel<F>(&self, channel_id: &str, f: F) -> Result<()>
    where
        F: FnOnce(&mut ChannelConfig) + Send,
    {
        {
            let repo = self.nexus_repo.write().await;
            let swap = repo.channels.get(channel_id)
                .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
            let mut ch = swap.load().clone();
            let ch_mut = Arc::make_mut(&mut ch);
            f(ch_mut);
            swap.store(ch);
        }
        self.save_nexus().await
    }
```

在 `#[cfg(test)]` 模块（`mod tests` 内）新增 3 个测试：

```rust
    fn sample_channel(id: &str) -> ChannelConfig {
        ChannelConfig {
            channel_id: Arc::new(id.into()),
            ws_url: Arc::new("ws://127.0.0.1:8201".into()),
            admins: Arc::new(HashSet::new()),
            bind_user: ChannelUser { messenger_id: Arc::new("web".into()), user_id: Arc::new("u1".into()) },
            agent_id: Arc::new("0".into()),
            role_name: Arc::new("0".into()),
            is_send_channel: true,
            enabled: true,
        }
    }

    #[test]
    fn channel_config_new_shape_serde_roundtrip() {
        let ch = sample_channel("web-main");
        let json = serde_json::to_string(&ch).unwrap();
        assert!(json.contains("\"bind_user\""), "应序列化 bind_user");
        assert!(json.contains("\"agent_id\""));
        assert!(json.contains("\"role_name\""));
        assert!(json.contains("\"is_send_channel\""));
        assert!(json.contains("\"enabled\""));
        let back: ChannelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(*back.channel_id, "web-main");
        assert_eq!(*back.bind_user.user_id, "u1");
    }

    #[test]
    fn channel_config_old_shape_alias_migration() {
        // 旧格式：default_bind_user / enabled_by_default，缺 agent_id/role_name/is_send_channel
        let old = r#"{
            "channel_id": "web-main",
            "ws_url": "ws://127.0.0.1:8201",
            "admins": [],
            "default_bind_user": { "messenger_id": "web", "user_id": "u1" },
            "enabled_by_default": true
        }"#;
        let ch: ChannelConfig = serde_json::from_str(old).unwrap();
        assert_eq!(*ch.bind_user.messenger_id, "web");
        assert!(ch.enabled, "旧字段 enabled_by_default 应映射到 enabled");
        assert!(ch.agent_id.is_empty(), "缺省 agent_id 应为空（脱离态）");
        assert!(ch.role_name.is_empty());
        assert!(!ch.is_send_channel);
    }

    #[tokio::test]
    async fn update_channel_mutates_and_persists() {
        let dir = tempdir().unwrap();
        let cfg = agent_config(dir.path().to_str().unwrap());
        let manager = ConfigManager {
            agent_config: cfg,
            nexus_repo: Arc::new(RwLock::new(NexusRepo::default())),
            station_repo: Arc::new(RwLock::new(StationRepo::default())),
            nexus_path: dir.path().join("nexus.json").to_str().unwrap().to_string(),
            station_path: dir.path().join("station.json").to_str().unwrap().to_string(),
            listeners: DashMap::new(),
        };
        manager.add_channel(sample_channel("web-main")).await.unwrap();

        // 修改 agent_id/role_name/is_send_channel
        manager.update_channel("web-main", |c| {
            c.agent_id = Arc::new("a1".into());
            c.role_name = Arc::new("r1".into());
            c.is_send_channel = false;
        }).await.unwrap();

        // 内存可见
        let ch = manager.channels().await.into_iter()
            .find(|(id, _)| id == "web-main").map(|(_, c)| c).unwrap();
        assert_eq!(*ch.agent_id, "a1");
        assert!(!ch.is_send_channel);

        // 落盘可见（重新读文件）
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("nexus.json")).unwrap()).unwrap();
        assert_eq!(saved["channels"]["web-main"]["agent_id"], "a1");

        // channel 不存在报错
        let err = manager.update_channel("nope", |_| {}).await.unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound(_)));
    }
```

- [ ] **Step 2: session_manager.rs 补充单测（ChannelConfig 新字段可用了）**

在 `kissbot-agent/src/session_manager.rs` 文件末尾追加 `#[cfg(test)]` 模块（完整代码，含 `sample_channel` 新字段构造）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_manager::ChannelUser;

    fn sample_channel(id: &str, agent: &str, role: &str, is_send: bool) -> ChannelConfig {
        ChannelConfig {
            channel_id: Arc::new(id.into()),
            ws_url: Arc::new("ws://127.0.0.1:8201".into()),
            admins: Arc::new(HashSet::new()),
            bind_user: ChannelUser { messenger_id: Arc::new("web".into()), user_id: Arc::new("u1".into()) },
            agent_id: Arc::new(agent.into()),
            role_name: Arc::new(role.into()),
            is_send_channel: is_send,
            enabled: true,
        }
    }

    fn key(agent: &str, role: &str) -> SessionKey {
        SessionKey { agent_id: agent.into(), role_name: role.into(), mode: Mode::Role }
    }

    #[test]
    fn get_or_create_dedupes() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k = key("a1", "r1");
        let (s1, created1) = mgr.get_or_create(&k, model.clone());
        assert!(created1, "首次创建");
        let (s2, created2) = mgr.get_or_create(&k, model.clone());
        assert!(!created2, "同 key 复用");
        assert!(Arc::ptr_eq(&s1, &s2), "同 key 应返回同一 Session");
        // 不同 mode 是不同会话
        let k_event = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let (_s3, created3) = mgr.get_or_create(&k_event, model);
        assert!(created3, "事件模式是独立会话");
    }

    #[test]
    fn retain_prunes_unbound() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k1 = key("a1", "r1");
        let k2 = key("a2", "r2");
        mgr.get_or_create(&k1, model.clone());
        mgr.get_or_create(&k2, model);
        let mut keep = HashSet::new();
        keep.insert(k1.clone());
        mgr.retain(&keep);
        assert!(mgr.get(&k1).is_some(), "仍在绑定集合的会话保留");
        assert!(mgr.get(&k2).is_none(), "无绑定会话销毁");
    }

    #[test]
    fn resolve_send_channel_flag_then_first() {
        let mgr = SessionManager::new();
        let k = key("a1", "r1");
        let channels = vec![
            ("c1".to_string(), Arc::new(sample_channel("c1", "a1", "r1", false))),
            ("c2".to_string(), Arc::new(sample_channel("c2", "a1", "r1", true))),
            ("c3".to_string(), Arc::new(sample_channel("c3", "a1", "r1", false))),
        ];
        assert_eq!(mgr.resolve_send_channel(&k, channels.clone()).as_deref(), Some("c2"), "is_send_channel 优先");

        // 全 false → 首个绑定
        let channels_all_false = vec![
            ("c1".to_string(), Arc::new(sample_channel("c1", "a1", "r1", false))),
            ("c3".to_string(), Arc::new(sample_channel("c3", "a1", "r1", false))),
        ];
        assert_eq!(mgr.resolve_send_channel(&k, channels_all_false).as_deref(), Some("c1"));

        // 不同三元组 → None
        let other = vec![("c9".to_string(), Arc::new(sample_channel("c9", "a9", "r9", true)))];
        assert_eq!(mgr.resolve_send_channel(&k, other), None);
    }

    #[test]
    fn channel_mode_default_role_and_set() {
        let mgr = SessionManager::new();
        assert_eq!(mgr.channel_mode("c1"), Mode::Role, "缺省角色模式");
        mgr.set_channel_mode("c1", Mode::Event("e9".into()));
        assert_eq!(mgr.channel_mode("c1"), Mode::Event("e9".into()));
        assert_eq!(mgr.channel_mode("c2"), Mode::Role, "未设置仍为角色模式");
    }
}
```

- [ ] **Step 3: coordinator.rs 完整重写**

完整替换 `kissbot-agent/src/coordinator.rs` 为以下代码（注意保留原注释语义，勿删注释）：

```rust
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Local;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::types::{
    Mode, WriteTask, ContextMessage, Result, Error, SessionKey, memory_role,
};
use crate::config_manager::{ConfigManager, ProviderModel};
use crate::command_router::CommandRouter;
use crate::model_client::ModelClient;
use crate::session_manager::{Session, SessionManager};
use crate::memory_reader::MemoryReader;
use crate::memory_writer::MemoryWriter;
use crate::memory_store_client::{MemoryStoreClient, ChannelRecord};

use kissbot_api::channel::{IncomingMessage, OutgoingMessage, BindRequest};
use kissbot_api::message::{Content, AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel_client::{ChannelClient, Terminal};

/// 保留 agent/role：agent_id 为 "0" 或空 = 脱离 agent（该 channel 只处理管理命令）
pub const RESERVED_AGENT_ID: &str = "0";
pub const RESERVED_ROLE_NAME: &str = "0";

pub struct AgentCoordinator {
    config: Arc<ConfigManager>,
    memory_reader: Arc<MemoryReader>,
    memory_writer: Arc<MemoryWriter>,
    memory_store_client: Arc<MemoryStoreClient>,
    session_manager: Arc<SessionManager>,
    model_client: Arc<tokio::sync::Mutex<ModelClient>>,
    /// 按 agent 内部 channel_id 索引的 ChannelClient
    channel_clients: Arc<DashMap<String, Arc<ChannelClient>>>,
    /// 断线通知：channel_id → Notify，closed() 通知重连循环
    disconnect_notify: Arc<DashMap<String, Arc<tokio::sync::Notify>>>,
}

impl AgentCoordinator {
    pub async fn new(
        config: Arc<ConfigManager>,
        memory_writer: MemoryWriter,
    ) -> Result<Arc<Self>> {
        let memory_reader = Arc::new(MemoryReader::new());
        let memory_writer = Arc::new(memory_writer);
        let memory_store_client = Arc::new(MemoryStoreClient::new());
        let session_manager = SessionManager::new();
        let model_client = ModelClient::new(config.clone());

        let coordinator = Arc::new(Self {
            config: config.clone(),
            memory_reader,
            memory_writer,
            memory_store_client,
            session_manager,
            model_client: Arc::new(tokio::sync::Mutex::new(model_client)),
            channel_clients: Arc::new(DashMap::new()),
            disconnect_notify: Arc::new(DashMap::new()),
        });

        // 按全部 channel 的绑定三元组初始化会话集合（agent 脱离态跳过）
        for (_, ch) in config.channels().await {
            if let Some(key) = coordinator.session_key_for(&ch) {
                coordinator.ensure_session(&key).await;
            }
        }

        // 连接所有 enabled 的 channel
        coordinator.connect_channels().await;

        info!("AgentCoordinator 初始化完成");
        Ok(coordinator)
    }

    // ==================== 会话定位与构建 ====================

    /// 按来源 channel 的绑定配置 + 运行态 mode 计算会话 key；agent 脱离态返回 None
    fn session_key_for(&self, ch: &crate::config_manager::ChannelConfig) -> Option<SessionKey> {
        let agent_id = ch.agent_id.to_string();
        if agent_id.is_empty() || agent_id == RESERVED_AGENT_ID {
            return None; // 脱离 agent：只处理管理命令
        }
        let mode = self.session_manager.channel_mode(&ch.channel_id);
        Some(SessionKey {
            agent_id,
            role_name: ch.role_name.to_string(),
            mode,
        })
    }

    /// 定位会话，新建时构建初始上下文；返回 (会话, 是否新建)
    async fn ensure_session(&self, key: &SessionKey) -> (Arc<Session>, bool) {
        let (session, created) =
            self.session_manager.get_or_create(key, self.config.default_model().await);
        if created {
            self.build_initial_context(&session).await;
        }
        (session, created)
    }

    /// 会话创建/重置时：加载 ego + 历史记录 + 顶层记忆索引构建初始上下文
    async fn build_initial_context(&self, session: &Arc<Session>) {
        // 读取自我认知（agent_id/role_name 取会话 key 原始值，不带事件编码）
        if let Ok(ego_info) = self.load_ego_info(&session.key.agent_id, &session.key.role_name).await {
            session.context.lock().await.set_system_message(ego_info);
        }
        // 读取历史记忆（事件模式由 memory_role 编码）
        if let Ok(history) = self.memory_reader
            .read_history(&self.config, &session.key.agent_id, &session.key.role_name, &session.key.mode)
            .await
        {
            session.context.lock().await.load_history(history);
        }
        // 读取顶层记忆索引（memory-struct 未实现时静默跳过）
        let _ = self.memory_reader
            .read_memory_struct_index(&self.config, &session.key.agent_id, &session.key.role_name, &session.key.mode)
            .await;
    }

    /// 来源 channel 绑定信息变化后重定位会话：清理无绑定会话 + 为新三元组创建会话
    async fn relocate_channel(&self, channel_id: &str) {
        // 1. 清理无任何 channel 绑定的会话
        self.prune_sessions().await;
        // 2. 新三元组对应会话不存在则创建并构建初始上下文
        if let Some(ch) = self.channel_config(channel_id).await {
            if let Some(key) = self.session_key_for(&ch) {
                self.ensure_session(&key).await;
            }
        }
    }

    /// 按当前全部 channel 的绑定集合清理无绑定会话
    async fn prune_sessions(&self) {
        let channels = self.config.channels().await;
        let mut keys = HashSet::new();
        for (_, ch) in &channels {
            if let Some(key) = self.session_key_for(ch) {
                keys.insert(key);
            }
        }
        self.session_manager.retain(&keys);
    }

    /// 重置来源 channel 所属会话的上下文
    async fn reset_session_for(&self, channel_id: &str) {
        if let Some(ch) = self.channel_config(channel_id).await {
            if let Some(key) = self.session_key_for(&ch) {
                if let Some(session) = self.session_manager.get(&key) {
                    self.reset_context(&session).await;
                    return;
                }
            }
        }
        warn!("reset: channel {} 无会话可重置", channel_id);
    }

    /// 上下文重置：清空后重建初始上下文
    async fn reset_context(&self, session: &Arc<Session>) {
        session.context.lock().await.clear();
        self.build_initial_context(session).await;
        info!("会话上下文已重置: {:?}", session.key);
    }

    /// 读取自我认知（agent 元数据 + 角色设定），agent_id/role_name 取会话 key
    async fn load_ego_info(&self, agent_id: &str, role_name: &str) -> Result<String> {
        let ego_url = kissbot_api::ApiConfig::get().memory_ego_url.clone();

        let client = reqwest::Client::new();

        let mut system_parts = vec![];

        // 获取 agent 元数据
        if let Ok(agent_resp) = client.post(&format!("{}/agent/list", ego_url))
            .json(&serde_json::json!({}))
            .send()
            .await
        {
            if let Ok(data) = agent_resp.json::<serde_json::Value>().await {
                if let Some(name) = data["data"]["individual_name"].as_str() {
                    system_parts.push(format!("你的名字是: {}", name));
                }
                if let Some(desc) = data["data"]["description"].as_str() {
                    system_parts.push(format!("你的描述: {}", desc));
                }
            }
        }

        // 获取角色设定
        if !role_name.is_empty() {
            if let Ok(role_resp) = client.post(&format!("{}/role/get", ego_url))
                .json(&serde_json::json!({
                    "agent_id": agent_id,
                    "role_name": role_name,
                }))
                .send()
                .await
            {
                if let Ok(data) = role_resp.json::<serde_json::Value>().await {
                    if let Some(desc) = data["data"]["description"].as_str() {
                        system_parts.push(format!("角色: {} - {}", role_name, desc));
                    }
                }
            }
        }

        if system_parts.is_empty() {
            system_parts.push("你是 kissbot 智能助手".to_string());
        }

        Ok(system_parts.join("\n"))
    }

    // ==================== 运行状态修改（管理命令入口） ====================

    /// 切换来源 channel 的运行态模式（不回写，会话重定位由调用方触发）
    pub async fn set_channel_mode(&self, channel_id: &str, mode: Mode) {
        self.session_manager.set_channel_mode(channel_id, mode);
    }

    /// 设置/取消来源 channel 为其会话的发送 channel（回写配置）
    /// on 时清除同会话其他 channel 的 is_send_channel 标志
    pub async fn set_send_channel(&self, channel_id: &str, on: bool) -> Result<()> {
        let Some(ch) = self.channel_config(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        let Some(key) = self.session_key_for(&ch) else {
            return Err(Error::InvalidCommand("channel 未关联 agent，无法设置发送 channel".to_string()));
        };
        if on {
            // 同会话其他 channel 的发送标志清除（保持会话内只有一个发送 channel）
            let channels = self.config.channels().await;
            for (cid, other) in channels {
                if cid == channel_id {
                    continue;
                }
                let same_key = other.agent_id.as_str() == key.agent_id
                    && other.role_name.as_str() == key.role_name
                    && self.session_manager.channel_mode(&cid) == key.mode;
                if same_key && other.is_send_channel {
                    self.config.update_channel(&cid, |c| c.is_send_channel = false).await?;
                }
            }
        }
        self.config.update_channel(channel_id, |c| c.is_send_channel = on).await
    }

    /// 设置来源 channel 所属会话的模型（运行态，不回写；校验 provider/model 存在）
    pub async fn set_session_model(&self, channel_id: &str, pm: ProviderModel) -> Result<()> {
        let Some(ch) = self.channel_config(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        let Some(key) = self.session_key_for(&ch) else {
            return Err(Error::InvalidCommand("channel 未关联 agent，无法设置模型".to_string()));
        };
        // 校验 provider 与 model 存在
        if self.config.resolve_effective_config(&pm).await.is_none() {
            return Err(Error::ModelProviderNotSupported(format!(
                "provider/model 不存在: {}/{}", pm.provider, pm.model)));
        }
        let (session, _) = self.ensure_session(&key).await;
        session.model.store(Arc::new(pm));
        Ok(())
    }

    /// 查询来源 channel 所属会话的事件列表
    pub async fn list_events(&self, channel_id: &str) -> Result<String> {
        let Some(ch) = self.channel_config(channel_id).await else {
            return Err(Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)));
        };
        let Some(key) = self.session_key_for(&ch) else {
            return Err(Error::InvalidCommand("channel 未关联 agent".to_string()));
        };
        let events = self.memory_reader
            .list_events(&self.config, &key.agent_id, &key.role_name)
            .await?;
        if events.is_empty() {
            Ok("📋 暂无事件".to_string())
        } else {
            Ok(format!("📋 事件列表:\n{}", events.join("\n")))
        }
    }

    // ==================== 通道连接 ====================

    /// 从配置中按 channel_id 取 channel 配置
    async fn channel_config(&self, channel_id: &str) -> Option<Arc<crate::config_manager::ChannelConfig>> {
        self.config.channels().await.into_iter()
            .find(|(id, _)| id == channel_id)
            .map(|(_, ch)| ch)
    }

    /// 连接所有 enabled 的 channel（NexusRepo channel 配置为连接来源）
    /// 连接与绑定统一由 ChannelConfig 描述：enabled 控制连接，bind_user 为绑定身份
    async fn connect_channels(self: &Arc<Self>) {
        let reconnect_secs = self.config.ws_reconnect_interval_secs();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
        let coordinator = self.clone();

        // 遍历 NexusRepo 中所有 channel，enabled 才连接
        for (_, ch) in self.config.channels().await {
            if !ch.enabled {
                continue; // 未启用：不连接
            }
            let channel_id = ch.channel_id.to_string();
            let ws_url = ch.ws_url.to_string();
            // 绑定身份来自 ChannelConfig.bind_user
            let bound_user = ch.bind_user.clone();

            let client = ChannelClient::new(
                channel_id.clone(),
                Arc::downgrade(&(coordinator.clone() as Arc<dyn Terminal>)),
            );

            // 断线通知
            let notify = Arc::new(tokio::sync::Notify::new());
            coordinator.disconnect_notify.insert(channel_id.clone(), notify.clone());
            coordinator.channel_clients.insert(channel_id.clone(), client);

            let client_clone = coordinator.channel_clients.get(&channel_id).unwrap().clone();
            let api_key = api_key.clone();

            tokio::spawn(async move {
                loop {
                    match client_clone.connect(&ws_url, &api_key).await {
                        Ok(()) => {
                            info!("已连接 channel: {}", channel_id);
                            // 绑定用户（BindRequest.messenger_id 用绑定身份的 messenger 标识，如 "web"）
                            let _ = client_clone.bind(BindRequest {
                                messenger_id: bound_user.messenger_id.clone(),
                                user_id: bound_user.user_id.clone(),
                            }).await;
                            // 等待断线通知（closed() 回调中 notify_one）
                            notify.notified().await;
                        }
                        Err(e) => {
                            warn!("连接 channel {} 失败: {:?}，{}秒后重连", channel_id, e, reconnect_secs);
                            tokio::time::sleep(Duration::from_secs(reconnect_secs)).await;
                        }
                    }
                }
            });
        }
    }

    /// 启动主循环（保持进程运行）
    pub async fn run(&self) {
        info!("AgentCoordinator 启动，等待外部输入...");
        // channel-client 通过 Terminal 回调驱动，此处保持进程不退出
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
}

// ==================== Terminal trait 实现 ====================

#[async_trait]
impl Terminal for AgentCoordinator {
    /// 收到上行消息
    async fn incoming_message(&self, channel_id: &str, message: Arc<IncomingMessage>) {
        // 1. 来源 channel 必须在配置中
        let Some(ch) = self.channel_config(channel_id).await else { return; };

        // 2. 推上行消息到记忆（agent/role 取来源 channel 绑定，事件模式编码）
        if let Some(key) = self.session_key_for(&ch) {
            let role_name = memory_role(&key);
            self.memory_store_client.push_channel_record(ChannelRecord {
                agent_id: Arc::new(key.agent_id.clone()),
                role_name: Arc::new(role_name),
                messenger_id: message.messenger_id.clone(),
                user_id: message.user_id.clone(),
                group_id: message.group_id.clone(),
                is_self: message.is_self,
                content: message.content.clone(),
                time: message.time.clone(),
            }).await;
        }

        // 3. 处理消息（channel_id 为 agent 内部连接标识，来自连接 id）
        self.handle_incoming(channel_id, ch, message).await;
    }

    async fn join_group(&self, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组加入事件，当前暂不处理
    }

    async fn leave_group(&self, _id: &str, _notification: Arc<GroupChangeNotification>) {
        // 群组离开事件，当前暂不处理
    }

    async fn user_removed(&self, _id: &str, _notification: Arc<UserRemoveNotification>) {
        // 用户删除事件，当前暂不处理
    }

    async fn download_chunk(&self, _id: &str, _info: Arc<AttachmentInfoResponse>, _pos: u64, _data: Bytes) -> std::result::Result<(), kissbot_channel_client::Error> {
        // 当前未使用附件下载
        Ok(())
    }

    async fn closed(&self, id: &str) {
        info!("channel 连接关闭: {}，准备重连", id);
        // 通知重连循环
        if let Some(notify) = self.disconnect_notify.get(id) {
            notify.notify_one();
        }
    }
}

// ==================== 消息处理 ====================

impl AgentCoordinator {
    async fn handle_incoming(
        &self,
        channel_id: &str,
        ch: Arc<crate::config_manager::ChannelConfig>,
        incoming: Arc<IncomingMessage>,
    ) {
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let is_self = incoming.is_self;
        let content_text = extract_text(&incoming.content);

        // 1. 自身发送回显识别（会话级 sent_contents）
        if is_self == 1 {
            if let Some(key) = self.session_key_for(&ch) {
                if let Some(session) = self.session_manager.get(&key) {
                    let ctx = session.context.lock().await;
                    if ctx.is_self_echo(&content_text) {
                        return; // 自己发出的回显，丢弃
                    }
                }
            }
            return;
        }

        // 2. 管理命令
        if CommandRouter::is_command(&content_text) {
            if CommandRouter::check_admin(&self.config, &messenger_id, &user_id).await {
                self.handle_admin_command(channel_id, &content_text, &group_id).await;
            }
            // 非管理员发送的管理命令忽略，不回复也不进入 agentic loop
            return;
        }

        // 3. 普通消息：脱离 agent 的 channel 丢弃，否则进入该会话的 agentic loop
        let Some(key) = self.session_key_for(&ch) else { return; };
        let (session, _) = self.ensure_session(&key).await;
        self.run_agentic_loop(channel_id, &session, incoming).await;
    }

    async fn handle_admin_command(
        &self,
        channel_id: &str,
        content: &str,
        group_id: &str,
    ) {
        match CommandRouter::parse(content) {
            Ok(cmd) => {
                match CommandRouter::execute(&cmd, &self.config, self, channel_id).await {
                    Ok((reply, effect)) => {
                        // 回复：会话存在走发送 channel，脱离态/无会话回退来源 channel
                        self.reply(channel_id, group_id, reply).await;

                        // 应用命令执行效果
                        match effect {
                            crate::types::CommandEffect::Relocate => {
                                self.relocate_channel(channel_id).await;
                            }
                            crate::types::CommandEffect::ResetSession => {
                                self.reset_session_for(channel_id).await;
                            }
                            crate::types::CommandEffect::None => {}
                        }
                    }
                    Err(e) => {
                        self.reply(channel_id, group_id,
                            format!("❌ 命令执行失败: {}", e)).await;
                    }
                }
            }
            Err(e) => {
                self.reply(channel_id, group_id,
                    format!("⚠️ {}", e)).await;
            }
        }
    }

    /// 回复消息：解析会话发送 channel，脱离态/无会话回退来源 channel
    async fn reply(&self, channel_id: &str, group_id: &str, content: String) {
        let send_channel = self.resolve_send_channel(channel_id).await
            .unwrap_or_else(|| channel_id.to_string());
        self.send_reply(&send_channel, group_id, content).await;
    }

    /// 解析来源 channel 所属会话的发送 channel
    async fn resolve_send_channel(&self, channel_id: &str) -> Option<String> {
        let ch = self.channel_config(channel_id).await?;
        let key = self.session_key_for(&ch)?;
        self.session_manager
            .resolve_send_channel(&key, self.config.channels().await)
            .or(Some(channel_id.to_string()))
    }

    async fn run_agentic_loop(&self, channel_id: &str, session: &Arc<Session>, incoming: Arc<IncomingMessage>) {
        let content_text = extract_text(&incoming.content);
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let time = incoming.time.to_string();

        // 1. 追加用户消息到该会话上下文
        {
            let mut ctx = session.context.lock().await;
            ctx.push_user_message(ContextMessage::User {
                messenger_id: messenger_id.clone(),
                user_id: user_id.clone(),
                group_id: group_id.clone(),
                content: content_text.clone(),
                time: time.clone(),
            });
        }

        // 2. 调用模型（用该会话的模型）
        let response = {
            let ctx = session.context.lock().await;
            let messages = ctx.build();
            let model = session.model.load_full();
            let mc = self.model_client.lock().await;
            mc.call(&model, &messages).await
        };

        match response {
            Ok(model_resp) => {
                let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

                // 3. 记录已发送内容
                {
                    let mut ctx = session.context.lock().await;
                    ctx.push_assistant(model_resp.content.clone(), now.clone());
                    ctx.record_sent_content(model_resp.content.clone());
                }

                // 4. 推送 think 到 MemoryWriter（事件模式编码）
                let agent_id = session.key.agent_id.clone();
                let role_name = memory_role(&session.key);
                let _ = self.memory_writer.push(WriteTask::Think {
                    agent_id,
                    role_name: Some(role_name),
                    content: model_resp.content.clone(),
                    time: now,
                });

                // 5. 发送回复到该会话的发送 channel
                self.reply(channel_id, &group_id, model_resp.content).await;

                // 6. 检查上下文超长
                let overflow = {
                    let ctx = session.context.lock().await;
                    ctx.is_overflow()
                };
                if overflow {
                    warn!("会话上下文超长，触发重置: {:?}", session.key);
                    self.reset_context(session).await;
                }
            }
            Err(e) => {
                warn!("模型调用失败: {:?}", e);
                self.reply(channel_id, &group_id,
                    format!("❌ 模型调用失败: {}", e)).await;
            }
        }
    }

    /// 发送回复消息到通道（send_channel_id 为该会话发送 channel），成功后推记忆（is_self=1）
    /// 发件人身份为发送 channel 配置的 bind_user
    async fn send_reply(&self, send_channel_id: &str, group_id: &str, content: String) {
        let Some(client) = self.channel_clients.get(send_channel_id) else {
            warn!("send_reply: 未找到 channel client: {}", send_channel_id);
            return;
        };
        let Some(ch) = self.channel_config(send_channel_id).await else {
            warn!("send_reply: 未找到 channel 配置: {}", send_channel_id);
            return;
        };
        let bound = ch.bind_user.clone();

        let msg = OutgoingMessage {
            messenger_id: bound.messenger_id.clone(),   // 对端 messenger 标识（如 "web"）
            user_id: bound.user_id.clone(),             // agent 绑定的用户
            group_id: Arc::new(group_id.to_string()),
            content: Content::Text(Arc::new(content.clone())),
        };

        match client.send_message(msg).await {
            Ok(response) => {
                // 下行成功后推记忆（is_self=1，使用返回的 content）
                if let Some(key) = self.session_key_for(&ch) {
                    let role_name = memory_role(&key);
                    self.memory_store_client.push_channel_record(ChannelRecord {
                        agent_id: Arc::new(key.agent_id.clone()),
                        role_name: Arc::new(role_name),
                        messenger_id: bound.messenger_id.clone(),
                        user_id: bound.user_id.clone(),
                        group_id: Arc::new(group_id.to_string()),
                        is_self: 1,
                        content: response.content.clone(),
                        time: response.time.clone(),
                    }).await;

                    // 记录已发送内容（用于 is_self echo 检测，会话级）
                    if let Some(session) = self.session_manager.get(&key) {
                        session.context.lock().await.record_sent_content(content);
                    }
                }
            }
            Err(e) => {
                warn!("send_reply 失败: {:?}", e);
            }
        }
    }
}

/// 从 Content 枚举中提取文本
fn extract_text(content: &Content) -> String {
    match content {
        Content::Text(t) => t.as_str().to_string(),
        Content::Multi(items) => items.iter()
            .filter_map(|c| match c { Content::Text(t) => Some(t.as_str().to_string()), _ => None })
            .collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}
```

注意：原 coordinator.rs 末尾的 `#[cfg(test)] mod tests`（`bound_channels_init_logic` 测试）整体删除——旧逻辑已删除，coordinator 依赖全局单例 KISSBOT_CONFIG 难以单测，路由核心由 SessionManager 单测 + Task 4 集成测试覆盖。

- [ ] **Step 4: command_router.rs 完整重写**

完整替换 `kissbot-agent/src/command_router.rs`：

```rust
use std::sync::Arc;

use crate::types::{AdminCommand, CommandEffect, Error, Mode, Result};
use crate::config_manager::{ChannelUser, ConfigManager, ProviderModel};
use crate::coordinator::{AgentCoordinator, RESERVED_AGENT_ID, RESERVED_ROLE_NAME};

pub struct CommandRouter;

impl CommandRouter {
    /// 检查消息是否为管理命令（以 "/" 开头）
    pub fn is_command(content: &str) -> bool {
        content.starts_with('/')
    }

    /// 检查发送者是否为管理权限用户
    pub async fn check_admin(
        config: &ConfigManager,
        messenger_id: &str,
        user_id: &str,
    ) -> bool {
        let admins = config.admin_users().await;
        admins.iter().any(|a| *a.messenger_id == messenger_id && *a.user_id == user_id)
    }

    /// 解析管理命令
    pub fn parse(content: &str) -> Result<AdminCommand> {
        let trimmed = content.trim();
        if !trimmed.starts_with('/') {
            return Err(Error::InvalidCommand("命令必须以 / 开头".to_string()));
        }

        let without_prefix = &trimmed[1..];
        let parts: Vec<&str> = without_prefix.split_whitespace().collect();
        if parts.is_empty() {
            return Err(Error::InvalidCommand("空命令".to_string()));
        }

        match parts[0] {
            "bind" => {
                if parts.len() < 4 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /bind messenger <messenger_id> <user_id>".to_string()
                    ));
                }
                Ok(AdminCommand::Bind {
                    messenger_id: parts[2].to_string(),
                    user_id: parts[3].to_string(),
                })
            }
            "unbind" => {
                if parts.len() < 3 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /unbind messenger <messenger_id>".to_string()
                    ));
                }
                Ok(AdminCommand::Unbind {
                    messenger_id: parts[2].to_string(),
                })
            }
            "admin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /admin <messenger_id> <user_id>".to_string()
                    ));
                }
                Ok(AdminCommand::Admin {
                    messenger_id: parts[1].to_string(),
                    user_id: parts[2].to_string(),
                })
            }
            "unadmin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /unadmin <messenger_id> <user_id>".to_string()
                    ));
                }
                Ok(AdminCommand::Unadmin {
                    messenger_id: parts[1].to_string(),
                    user_id: parts[2].to_string(),
                })
            }
            "agent" => {
                // /agent [id] [role]：缺省 id 用保留 agent "0"，缺省 role 用保留 role "0"
                let agent_id = parts.get(1).map(|s| s.to_string());
                let role = parts.get(2).map(|s| s.to_string());
                Ok(AdminCommand::SetAgent { agent_id, role })
            }
            "role" => {
                // /role [name]：缺省用保留 role "0"
                let role = parts.get(1).map(|s| s.to_string());
                Ok(AdminCommand::SetRole(role))
            }
            "mode" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand(
                        "格式: /mode event [event-id] 或 /mode role".to_string()
                    ));
                }
                match parts[1] {
                    "event" => {
                        if parts.len() >= 3 {
                            Ok(AdminCommand::ModeEvent(Some(parts[2].to_string())))
                        } else {
                            Ok(AdminCommand::ModeEvent(None))
                        }
                    }
                    "role" => Ok(AdminCommand::ModeRole),
                    _ => Err(Error::InvalidCommand(format!("未知模式: {}", parts[1]))),
                }
            }
            "reenter" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand(
                        "格式: /reenter <event-id>".to_string()
                    ));
                }
                Ok(AdminCommand::Reenter(parts[1].to_string()))
            }
            "send-channel" => {
                if parts.len() < 2 || !matches!(parts[1], "on" | "off") {
                    return Err(Error::InvalidCommand(
                        "格式: /send-channel on|off".to_string()
                    ));
                }
                Ok(AdminCommand::SendChannel(parts[1] == "on"))
            }
            "events" => Ok(AdminCommand::Events),
            "reset" => Ok(AdminCommand::Reset),
            "model" => {
                if parts.len() != 3 {
                    return Err(Error::InvalidCommand("格式: /model <provider> <model>".to_string()));
                }
                Ok(AdminCommand::Model(ProviderModel {
                    provider: parts[1].to_string(),
                    model: parts[2].to_string(),
                }))
            }
            _ => Err(Error::InvalidCommand(format!("未知命令: {}", parts[0]))),
        }
    }

    /// 执行管理命令（返回回复文本和协调器后续动作）
    /// bind/agent/role/send-channel/admin/unadmin 走 ConfigManager 回写；
    /// mode/reenter 改运行态模式（coordinator）；model 改会话模型（运行态）。
    pub async fn execute(
        command: &AdminCommand,
        config: &ConfigManager,
        coordinator: &AgentCoordinator,
        channel_id: &str,
    ) -> Result<(String, CommandEffect)> {
        match command {
            AdminCommand::Bind { messenger_id, user_id } => {
                config.update_channel(channel_id, |c| {
                    c.bind_user = ChannelUser {
                        messenger_id: Arc::new(messenger_id.clone()),
                        user_id: Arc::new(user_id.clone()),
                    };
                }).await?;
                Ok((format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id), CommandEffect::Relocate))
            }
            AdminCommand::Unbind { .. } => {
                // 当前阶段 /unbind 暂不进行任何操作（channel 必须保持 bind 状态）
                Ok(("ℹ️ /unbind 暂不支持，channel 需保持绑定状态".to_string(), CommandEffect::None))
            }
            AdminCommand::Admin { messenger_id, user_id } => {
                config.add_admin(channel_id, &ChannelUser {
                    messenger_id: Arc::new(messenger_id.clone()),
                    user_id: Arc::new(user_id.clone()),
                }).await?;
                Ok((format!("✅ 已添加管理权限: {} / {}", messenger_id, user_id), CommandEffect::None))
            }
            AdminCommand::Unadmin { messenger_id, user_id } => {
                config.remove_admin(channel_id, messenger_id, user_id).await?;
                Ok((format!("✅ 已移除管理权限: {} / {}", messenger_id, user_id), CommandEffect::None))
            }
            AdminCommand::SetAgent { agent_id, role } => {
                let new_agent = agent_id.clone().unwrap_or_else(|| RESERVED_AGENT_ID.to_string());
                let new_role = role.clone().unwrap_or_else(|| RESERVED_ROLE_NAME.to_string());
                config.update_channel(channel_id, |c| {
                    c.agent_id = Arc::new(new_agent.clone());
                    c.role_name = Arc::new(new_role.clone());
                }).await?;
                Ok((format!("✅ 已设置 agent: {} / role: {}", new_agent, new_role), CommandEffect::Relocate))
            }
            AdminCommand::SetRole(role) => {
                let new_role = role.clone().unwrap_or_else(|| RESERVED_ROLE_NAME.to_string());
                config.update_channel(channel_id, |c| {
                    c.role_name = Arc::new(new_role.clone());
                }).await?;
                Ok((format!("✅ 已设置 role: {}", new_role), CommandEffect::Relocate))
            }
            AdminCommand::ModeEvent(event_id) => {
                let id = event_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                coordinator.set_channel_mode(channel_id, Mode::Event(id.clone())).await;
                Ok((format!("✅ 新事件 ID: {}", id), CommandEffect::Relocate))
            }
            AdminCommand::ModeRole => {
                coordinator.set_channel_mode(channel_id, Mode::Role).await;
                Ok(("✅ 已切换为角色模式".to_string(), CommandEffect::Relocate))
            }
            AdminCommand::Reenter(event_id) => {
                coordinator.set_channel_mode(channel_id, Mode::Event(event_id.clone())).await;
                Ok((format!("✅ 将重进事件: {}", event_id), CommandEffect::Relocate))
            }
            AdminCommand::SendChannel(on) => {
                coordinator.set_send_channel(channel_id, *on).await?;
                Ok((
                    if *on { "✅ 已设为发送 channel".to_string() } else { "✅ 已取消发送 channel".to_string() },
                    CommandEffect::None,
                ))
            }
            AdminCommand::Events => {
                let reply = coordinator.list_events(channel_id).await?;
                Ok((reply, CommandEffect::None))
            }
            AdminCommand::Reset => {
                Ok(("🔄 正在重置上下文...".to_string(), CommandEffect::ResetSession))
            }
            AdminCommand::Model(pm) => {
                coordinator.set_session_model(channel_id, pm.clone()).await?;
                Ok((format!("✅ 已切换模型为: {}/{}", pm.provider, pm.model), CommandEffect::None))
            }
        }
    }
}
```

- [ ] **Step 5: types.rs 删除旧 Agent 变体 + main.rs 移除 context_builder + 删除文件**

`kissbot-agent/src/types.rs` 中删除 `Agent(String)` 变体（现在无人引用）：

```rust
    /// 设置 channel 绑定的 agent 与 role（缺省用保留值 "0"）
    SetAgent { agent_id: Option<String>, role: Option<String> },
```

（删除其上方注释掉的旧 `Agent(String)` 行——实际是删除整行 `Agent(String),` 及其 doc 注释 `// 新增：/agent <id>`）

`kissbot-agent/src/main.rs` 中删除 `mod context_builder;`（保留 `mod session_manager;`）。

删除文件：`git rm kissbot-agent/src/context_builder.rs`

- [ ] **Step 6: memory_reader 事件编码改横线 + http_server 测试 JSON 适配**

`kissbot-agent/src/memory_reader.rs` 中，`read_history` 的事件分支改为横线拼接：

```rust
            Mode::Event(event_id) => {
                json!({
                    "agent_id": agent_id,
                    "role_name": format!("{}-{}", role_name, event_id),
                    "limit": MAX_RECENT_RECORDS,
                })
            }
```

`kissbot-agent/src/http_server.rs` 的测试 `config_endpoints_auth_and_crud` 中，添加 channel 的 JSON 改为：

```rust
        let (status, body) = send(app.clone(), "POST", "/config/channels", "admin-key-123",
            Some(serde_json::json!({
                "channel_id": "web-main", "ws_url": "ws://127.0.0.1:8201",
                "admins": [],
                "bind_user": { "messenger_id": "web", "user_id": "u1" },
                "agent_id": "0", "role_name": "0",
                "is_send_channel": false, "enabled": true
            }))).await;
```

- [ ] **Step 7: 全量测试 + 编译**

Run: `cd kissbot-agent && cargo build 2>&1 | tail -20 && cargo test 2>&1 | tail -30`
Expected: 编译通过、无 warning；全部测试 PASS（config_manager 新增 3 + session_manager 新增 4 + types 2 + 既有全部）

- [ ] **Step 8: 提交**

```bash
git add kissbot-agent/src/types.rs kissbot-agent/src/config_manager.rs kissbot-agent/src/coordinator.rs kissbot-agent/src/command_router.rs kissbot-agent/src/main.rs kissbot-agent/src/memory_reader.rs kissbot-agent/src/http_server.rs kissbot-agent/src/session_manager.rs
git rm kissbot-agent/src/context_builder.rs
git commit -m "feat(agent): 多会话集成——ChannelConfig 扁平化回写、Coordinator 按会话路由、CommandRouter 新命令语义、删除 context_builder"
```

---

### Task 4: 模板与 Playwright 集成测试适配 + 多会话用例

**Files:**
- Modify: `script/template/nexus.json`
- Modify: `test/workspace-template/agent-data/nexus.json`
- Modify: `test/tests/agent-config-api.spec.ts`（channel JSON 新结构）
- Modify: `test/tests/agent-commands.spec.ts`（新命令语义 + 多会话用例）

**Interfaces:**
- Consumes: Task 3 的可执行行为（agent 二进制）
- Produces: 验证多会话路由、发送 channel、命令回写与持久化的端到端测试

- [ ] **Step 1: 模板 nexus.json 新结构**

`script/template/nexus.json` 与 `test/workspace-template/agent-data/nexus.json` 中 channel 段改为：

```json
    "web-main": {
      "channel_id": "web-main",
      "ws_url": "ws://127.0.0.1:8201",
      "admins": [{ "messenger_id": "web", "user_id": "u2" }],
      "bind_user": { "messenger_id": "web", "user_id": "u1" },
      "agent_id": "0",
      "role_name": "0",
      "is_send_channel": true,
      "enabled": true
    }
```

（其余段不变；模板 agent_id 用 "0" 使启动即脱离态，正常消息不触发 LLM 调用）

- [ ] **Step 2: agent-config-api.spec.ts channel JSON 适配**

`test/tests/agent-config-api.spec.ts` 第 81 行附近的 channel 添加 JSON 改为：

```ts
      {
        channel_id: 'web-main', ws_url: 'ws://127.0.0.1:8201',
        admins: [], bind_user: { messenger_id: 'web', user_id: 'u1' },
        agent_id: '0', role_name: '0', is_send_channel: false, enabled: false,
      },
```

（enabled: false 保持该用例原意：添加但未启用）

- [ ] **Step 3: agent-commands.spec.ts 适配 + 多会话用例**

`test/tests/agent-commands.spec.ts` 全量替换（保持原有 /admin 权限链路用例，新增 /agent /role /mode /send-channel /unbind 用例；用例顺序依赖：先加 agent 才能 /model）：

```ts
import { test, expect } from '@playwright/test';
import { resetWorkspace, startBackend, stopBackend, startAgent, stopAgent, waitForPort } from './helpers/server';
import { spawnCli, type SpawnedCli } from './helpers/cli';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

let backend: ChildProcess;
let agent: ChildProcess;
let cliAdmin: SpawnedCli;   // u2：初始管理员
let cliUser: SpawnedCli;    // u3：admin/unadmin 测试对象

// 等待 cli 输出（返回 Promise，用于"不应出现"的断言配合超时）
function sleep(ms: number) { return new Promise(r => setTimeout(r, ms)); }

test.describe.serial('agent 管理命令测试（多会话路由，cli 经 channel-web 发送）', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    backend = startBackend(WORKSPACE);
    await waitForPort(8301, '127.0.0.1', 30000);
    agent = startAgent(WORKSPACE);
    await waitForPort(9090, '127.0.0.1', 30000);
    // 等待 agent 完成 channel 连接与绑定
    await sleep(2000);
    cliAdmin = spawnCli(['web', 'u2', 'g1', './downloads'], WORKSPACE);
    await cliAdmin.waitForOutput(/bound\./);
    cliUser = spawnCli(['web', 'u3', 'g1', './downloads'], WORKSPACE);
    await cliUser.waitForOutput(/bound\./);
  });

  test.afterAll(() => {
    if (cliAdmin) cliAdmin.proc.kill();
    if (cliUser) cliUser.proc.kill();
    stopAgent(agent);
    stopBackend(backend);
  });

  test('TC-01: 非管理员（u3）发送 /model 被忽略', async () => {
    const baseline = cliUser.getOutput();
    cliUser.stdin('/send /model deepseek deepseek-4-flash');
    await sleep(3000);
    const tail = cliUser.getOutput().slice(baseline.length);
    expect(tail).not.toMatch(/切换模型|模型调用失败|不存在|未关联/);
  });

  test('TC-02: 管理员（u2）发送 /agent a1 r1 设置 channel 的 agent 与 role', async () => {
    cliAdmin.stdin('/send /agent a1 r1');
    await cliAdmin.waitForOutput(/✅ 已设置 agent: a1 \/ role: r1/, 10000);
  });

  test('TC-03: 管理员（u2）发送 /admin web u3 添加管理权限', async () => {
    cliAdmin.stdin('/send /admin web u3');
    await cliAdmin.waitForOutput(/✅ 已添加管理权限: web \/ u3/, 10000);
  });

  test('TC-04: u3 成为管理员后发送 /model 调整会话模型', async () => {
    cliUser.stdin('/send /model deepseek deepseek-4-flash');
    await cliUser.waitForOutput(/✅ 已切换模型为: deepseek\/deepseek-4-flash/, 15000);
  });

  test('TC-05: 管理员（u2）发送 /role r2 修改 channel 角色（回写并重定位会话）', async () => {
    cliAdmin.stdin('/send /role r2');
    await cliAdmin.waitForOutput(/✅ 已设置 role: r2/, 10000);
  });

  test('TC-06: 管理员（u2）发送 /mode event 进入事件模式（自动生成事件 ID）', async () => {
    cliAdmin.stdin('/send /mode event');
    await cliAdmin.waitForOutput(/✅ 新事件 ID: [0-9a-f-]{36}/, 10000);
  });

  test('TC-07: 管理员（u2）发送 /send-channel on/off 切换发送 channel（回写）', async () => {
    cliAdmin.stdin('/send /send-channel on');
    await cliAdmin.waitForOutput(/✅ 已设为发送 channel/, 10000);
    cliAdmin.stdin('/send /send-channel off');
    await cliAdmin.waitForOutput(/✅ 已取消发送 channel/, 10000);
  });

  test('TC-08: /unbind 暂不操作，回复提示', async () => {
    cliAdmin.stdin('/send /unbind messenger web');
    await cliAdmin.waitForOutput(/ℹ️ \/unbind 暂不支持/, 10000);
  });

  test('TC-09: /agent 0 进入脱离态后普通消息被丢弃', async () => {
    cliAdmin.stdin('/send /agent 0');
    await cliAdmin.waitForOutput(/✅ 已设置 agent: 0 \/ role: 0/, 10000);
    // 脱离态后发送普通消息，不应有任何 agent 回复
    const baseline = cliAdmin.getOutput();
    cliAdmin.stdin('/send hello');
    await sleep(3000);
    const tail = cliAdmin.getOutput().slice(baseline.length);
    expect(tail).not.toMatch(/模型调用失败|hello/);
    // 脱离态仍可执行管理命令
    cliAdmin.stdin('/send /agent a1 r1');
    await cliAdmin.waitForOutput(/✅ 已设置 agent: a1 \/ role: r1/, 10000);
  });

  test('TC-10: 管理员（u2）发送 /unadmin web u3 移除权限', async () => {
    cliAdmin.stdin('/send /unadmin web u3');
    await cliAdmin.waitForOutput(/✅ 已移除管理权限: web \/ u3/, 10000);
  });

  test('TC-11: 移除权限后 u3 发送 /model 再被忽略', async () => {
    const baseline = cliUser.getOutput();
    cliUser.stdin('/send /model deepseek deepseek-4-flash');
    await sleep(3000);
    const tail = cliUser.getOutput().slice(baseline.length);
    expect(tail).not.toMatch(/切换模型/);
  });
});
```

- [ ] **Step 4: 运行集成测试**

Run:
```bash
cd test && npx playwright test tests/agent-config-api.spec.ts 2>&1 | tail -30
cd test && npx playwright test tests/agent-commands.spec.ts 2>&1 | tail -30
```
Expected: agent-config-api 全部 PASS；agent-commands 11 个用例全部 PASS

- [ ] **Step 5: 提交**

```bash
git add script/template/nexus.json test/workspace-template/agent-data/nexus.json test/tests/agent-config-api.spec.ts test/tests/agent-commands.spec.ts
git commit -m "test(agent): 多会话命令测试——agent/role/mode/send-channel/unbind 用例，模板 nexus 适配新 ChannelConfig 结构"
```

---

## 验证汇总

1. `cd kissbot-agent && cargo test` 全部 PASS
2. `cd test && npx playwright test tests/agent-config-api.spec.ts tests/agent-commands.spec.ts` 全部 PASS
3. 手工冒烟：`bash script/restart-all.sh` 后经 `script/start-cli.sh` 验证 /agent /role /mode /send-channel 命令回复与 nexus.json 回写
