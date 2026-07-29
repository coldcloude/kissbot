# Agent 重构实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 重构 kissbot-agent：扁平化目录、用 channel-client 替换手写 WS、将 channel 消息推记忆逻辑移植到 agent。

**Architecture:** Coordinator 实现 channel-client 的 `Terminal` trait 接收回调；新增 `MemoryStoreClient`（`FileObjectAppender` 批处理）推送 channel 消息记忆；所有 nexus 模块移至 `src/` 根目录。

**Tech Stack:** Rust, tokio, kai-file (FileObjectAppender), kissbot-channel-client, kissbot-api

## 全局约束

- 所有文件用 UTF-8 编码、`\n` 换行符
- 不删除代码中的注释
- 读写文件必须用 Read/Write/Edit 工具，禁止 sed/python 等命令修改文件

---

### Task 1: Cargo.toml 依赖变更 + 创建 memory_store_client.rs

**Files:**
- Modify: `kissbot-agent/Cargo.toml`
- Create: `kissbot-agent/src/memory_store_client.rs`

**Interfaces:**
- Consumes: 无（新文件）
- Produces: `MemoryStoreClient`（`push_channel_record()` 方法）、供 Task 3 Coordinator 使用

- [ ] **Step 1: 修改 Cargo.toml**

删除 `tokio-tungstenite` 和 `futures-util`，新增 `kissbot-channel-client` 和 `kai-file`：

```toml
[package]
name = "kissbot-agent"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
reqwest = { version = "0.12", features = ["json"] }
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.0", features = ["v4"] }
thiserror = "2.0"
async-trait = "0.1"
tracing = "0.1"
tracing-subscriber = "0.3"
dashmap = { version = "6.1", features = ["serde"] }
flume = "0.12"
kai-ws = { path = "../kai-rs/kai-ws" }
kai-file = { path = "../kai-rs/kai-file" }
kissbot-api = { path = "../kissbot-api" }
kissbot-security = { path = "../kissbot-security" }
kissbot-channel-client = { path = "../kissbot-channel-client" }
```

- [ ] **Step 2: 创建 memory_store_client.rs**

从 `kissbot-channel/src/memory_store_client.rs` 移植，调整为新架构：

```rust
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kai_file::{FileAppendWriter, FileObjectAppender, NoopErrorHandler, appender::FileAppendWriterContext};
use kissbot_api::memory::{ChannelRequest, ChannelRequests};
use kissbot_security::HEADER_API_KEY;
use kissbot_api::Content;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

// 批处理参数
const RECORD_QUEUE_SIZE: usize = 100;
const RECORD_MAX_DELAY: Duration = Duration::from_secs(1);
const FILE_KEY: &str = "0";

/// 单个 channel 记录（与 ChannelRequest 同构，用于 push_channel_record 接口）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub content: Content,
    pub time: Arc<String>,
}

pub struct MemoryStoreClient {
    appender: FileObjectAppender<String, ChannelRecord, MemoryStoreSender, MemoryStoreContext>,
}

impl MemoryStoreClient {
    pub fn new() -> Self {
        let sender = Arc::new(MemoryStoreSender::new());
        Self {
            appender: FileObjectAppender::new(sender, Arc::new(NoopErrorHandler {}), RECORD_MAX_DELAY, RECORD_QUEUE_SIZE),
        }
    }

    pub async fn push_channel_record(&self, record: ChannelRecord) {
        self.appender.append(FILE_KEY.to_string(), vec![record]).await;
    }
}

struct MemoryStoreContext {
    client: Client,
    base_url: String,
    api_key: Arc<String>,
}

pub struct MemoryStoreSender {
    context: Arc<Mutex<MemoryStoreContext>>,
}

impl MemoryStoreSender {
    pub fn new() -> Self {
        let api_config = kissbot_api::ApiConfig::get();
        let security = kissbot_security::SecurityConfig::get();
        let ctx = MemoryStoreContext {
            client: Client::new(),
            base_url: api_config.memory_store_url.clone(),
            api_key: security.api_key.clone(),
        };
        Self {
            context: Arc::new(Mutex::new(ctx)),
        }
    }
}

#[async_trait]
impl FileAppendWriter<String, ChannelRecord, MemoryStoreContext> for MemoryStoreSender {
    async fn get_lock(&self, _key: &String) -> Arc<Mutex<MemoryStoreContext>> {
        self.context.clone()
    }
    async fn remove_lock(&self, _key: &String) {}
}

#[async_trait]
impl FileAppendWriterContext<String, ChannelRecord> for MemoryStoreContext {
    async fn write(&mut self, _key: &String, records: Vec<ChannelRecord>) -> std::result::Result<(), kai_file::Error> {
        if self.base_url.is_empty() {
            return Ok(());
        }

        let requests: Vec<ChannelRequest> = records.into_iter().map(|r| ChannelRequest {
            agent_id: r.agent_id,
            role_name: r.role_name,
            messenger_id: r.messenger_id,
            user_id: r.user_id,
            group_id: r.group_id,
            is_self: r.is_self,
            content: r.content,
            time: r.time,
        }).collect();

        let req = ChannelRequests { requests, force: 1 };
        let url = format!("{}/store/channel", self.base_url.trim_end_matches('/'));
        let response = self.client.post(&url)
            .header(HEADER_API_KEY, self.api_key.as_str())
            .json(&req)
            .send()
            .await
            .map_err(|e| kai_file::Error::ExternalError(Box::new(e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let msg = response.text().await.unwrap_or_default();
            return Err(kai_file::Error::WriteError(format!("[{}] {}", status, msg)));
        }
        Ok(())
    }
}
```

---

### Task 2: 目录扁平化 — 移动所有 nexus/*.rs 到 src/

**Files:**
- Move: `kissbot-agent/src/nexus/config_manager.rs` → `kissbot-agent/src/config_manager.rs`
- Move: `kissbot-agent/src/nexus/coordinator.rs` → `kissbot-agent/src/coordinator.rs`
- Move: `kissbot-agent/src/nexus/memory_writer.rs` → `kissbot-agent/src/memory_writer.rs`
- Move: `kissbot-agent/src/nexus/memory_reader.rs` → `kissbot-agent/src/memory_reader.rs`
- Move: `kissbot-agent/src/nexus/llm_client.rs` → `kissbot-agent/src/llm_client.rs`
- Move: `kissbot-agent/src/nexus/context_builder.rs` → `kissbot-agent/src/context_builder.rs`
- Move: `kissbot-agent/src/nexus/command_router.rs` → `kissbot-agent/src/command_router.rs`
- Move: `kissbot-agent/src/nexus/mode_manager.rs` → `kissbot-agent/src/mode_manager.rs`
- Move: `kissbot-agent/src/nexus/station_router.rs` → `kissbot-agent/src/station_router.rs`
- Move: `kissbot-agent/src/nexus/station_client.rs` → `kissbot-agent/src/station_client.rs`
- Move: `kissbot-agent/src/nexus/http_server.rs` → `kissbot-agent/src/http_server.rs`
- Move: `kissbot-agent/src/nexus/types.rs` → `kissbot-agent/src/types.rs`

**Interfaces:** 纯路径变更，无接口变化

- [ ] **Step 1: 用 git mv 移动文件**

```bash
cd /home/admin/project/kissbot/kissbot-agent
for f in config_manager.rs coordinator.rs memory_writer.rs memory_reader.rs \
         llm_client.rs context_builder.rs command_router.rs mode_manager.rs \
         station_router.rs station_client.rs http_server.rs types.rs; do
  git mv src/nexus/$f src/$f
done
```

验证：`ls src/*.rs` 应看到 13 个文件（含 main.rs）。

- [ ] **Step 2: 逐个文件替换 import 路径**

每个从 `src/nexus/` 移出的文件中，将所有 `crate::nexus::` 替换为 `crate::`。

文件列表：config_manager.rs, coordinator.rs, memory_writer.rs, memory_reader.rs, llm_client.rs, context_builder.rs, command_router.rs, mode_manager.rs, station_router.rs, station_client.rs, http_server.rs, types.rs。

替换命令（每个文件执行）：

```bash
# 在每个文件中替换 import 路径
sed -i 's/crate::nexus::/crate::/g' src/config_manager.rs
sed -i 's/crate::nexus::/crate::/g' src/coordinator.rs
# ...（对每个移出的文件执行）
```

或者用 bash 一次性处理：

```bash
cd /home/admin/project/kissbot/kissbot-agent
for f in config_manager coordinator memory_writer memory_reader \
         llm_client context_builder command_router mode_manager \
         station_router station_client http_server types; do
  sed -i 's/crate::nexus::/crate::/g' src/$f.rs
done
```

- [ ] **Step 3: 手动检查 coordinator.rs 中 `super::` 的引用**

`coordinator.rs` 中可能有 `super::` 引用指向 `mod.rs` 中的模块。检查并替换为 `crate::`：

```bash
grep -n 'super::' src/coordinator.rs
```
如果有，改为 `crate::`。

- [ ] **Step 4: 更新 main.rs 中的 import**

```bash
sed -i 's/crate::nexus::/crate::/g' src/main.rs
```

- [ ] **Step 5: 删除 nexus/mod.rs**

```bash
rm src/nexus/mod.rs
```

检查 nexus 目录是否为空：`ls src/nexus/`。如果为空则删除目录：`rmdir src/nexus`。

---

### Task 3: 重写 Coordinator — 实现 Terminal trait

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`（完全重写）

**Interfaces:**
- Consumes: `MemoryStoreClient`（Task 1）、`ChannelClient`（channel-client）、`Terminal` trait（channel-client）
- Produces: `AgentCoordinator` 的 `new()` 和 `run()` 方法

- [ ] **Step 1: 更新 imports**

```rust
use std::sync::Arc;
use std::sync::Weak;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Local;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::types::{Mode, WriteTask, ContextMessage, AdminCommand, Result, Error};
use crate::config_manager::ConfigManager;
use crate::mode_manager::ModeManager;
use crate::command_router::CommandRouter;
use crate::llm_client::LlmClient;
use crate::context_builder::ContextBuilder;
use crate::memory_reader::MemoryReader;
use crate::memory_writer::MemoryWriter;
use crate::memory_store_client::{MemoryStoreClient, ChannelRecord};

use kissbot_api::channel::{IncomingMessage, OutgoingMessage, BindRequest};
use kissbot_api::message::{Content, AttachmentInfoResponse, GroupChangeNotification, UserRemoveNotification};
use kissbot_channel_client::{ChannelClient, Terminal};
```

- [ ] **Step 2: 重写 AgentCoordinator 结构体**

```rust
pub struct AgentCoordinator {
    config: Arc<ConfigManager>,
    mode_manager: Arc<ModeManager>,
    memory_reader: Arc<MemoryReader>,
    memory_writer: Arc<MemoryWriter>,
    memory_store_client: Arc<MemoryStoreClient>,
    context_builder: Arc<tokio::sync::Mutex<ContextBuilder>>,
    llm_client: Arc<tokio::sync::Mutex<LlmClient>>,
    channel_clients: Arc<DashMap<String, Arc<ChannelClient>>>,
}
```

- [ ] **Step 3: 实现 `new()` 方法**

在 `new()` 中：
1. 初始化 mode_manager、memory_reader、memory_writer、memory_store_client
2. 初始化 LLMClient、ContextBuilder（从 config 读取自我认知和历史）
3. 遍历 `config.channel_bindings()`，对每个 binding：
   - 创建 `ChannelClient::new(messenger_id, Arc::downgrade(self))`
   - 调用 `client.connect(ws_url, api_key).await`
   - 调用 `client.bind(BindRequest { messenger_id, user_id }).await`
   - 存入 `channel_clients`

```rust
impl AgentCoordinator {
    pub async fn new(
        config: Arc<ConfigManager>,
        memory_writer: MemoryWriter,
    ) -> Result<Arc<Self>> {
        let mode = config.current_mode().await;
        let mode_manager = Arc::new(ModeManager::new(mode.clone()));
        let memory_reader = Arc::new(MemoryReader::new());
        let memory_writer = Arc::new(memory_writer);
        let memory_store_client = Arc::new(MemoryStoreClient::new());

        let llm_config = config.llm_config().await;
        let llm_client = Arc::new(tokio::sync::Mutex::new(LlmClient::new(llm_config)));

        let mut context_builder = ContextBuilder::new();
        if let Ok(ego_info) = Self::load_ego_info(&config).await {
            context_builder.set_system_message(ego_info);
        }
        if let Ok(history) = memory_reader.read_history(&config, &mode).await {
            context_builder.load_history(history);
        }
        let _ = memory_reader.read_memory_struct_index(&config, &mode).await;

        let coordinator = Arc::new(Self {
            config,
            mode_manager,
            memory_reader,
            memory_writer,
            memory_store_client,
            context_builder: Arc::new(tokio::sync::Mutex::new(context_builder)),
            llm_client,
            channel_clients: Arc::new(DashMap::new()),
        });

        // 连接所有 channel
        coordinator.connect_all_channels().await;

        info!("AgentCoordinator 初始化完成");
        Ok(coordinator)
    }

    async fn connect_all_channels(self: &Arc<Self>) {
        let config = &self.config;
        let bindings = config.channel_bindings().await;
        // TODO: ws_url 和 api_key 需要从配置获取
        let ws_url = "ws://localhost:8080/ws".to_string();
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();

        for binding in &bindings {
            let messenger_id = binding.messenger_id.clone();
            let user_id = binding.user_id.clone();
            let client = ChannelClient::new(messenger_id.clone(), Arc::downgrade(self));
            let client_clone = client.clone();

            let ws_url = ws_url.clone();
            let api_key = api_key.clone();
            let coordinator = self.clone();
            self.channel_clients.insert(messenger_id.clone(), client);

            tokio::spawn(async move {
                loop {
                    match client_clone.connect(&ws_url, &api_key).await {
                        Ok(()) => {
                            info!("已连接 channel: {}", messenger_id);
                            // 绑定用户
                            let _ = client_clone.bind(BindRequest {
                                messenger_id: Arc::new(messenger_id.clone()),
                                user_id: Arc::new(user_id.clone()),
                            }).await;
                            // 连接会保持直到断开
                            // 断开后 closed() 回调触发，此处 pending 等待
                            std::future::pending::<()>().await;
                        }
                        Err(e) => {
                            warn!("连接 channel {} 失败: {:?}，5秒后重连", messenger_id, e);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            });
        }
    }
}
```

- [ ] **Step 4: 实现 Terminal trait**

注意：`closed()` 回调当前不做自动重连（重连逻辑在 `connect_all_channels` 的循环中），但可以记录日志。

```rust
#[async_trait]
impl Terminal for AgentCoordinator {
    async fn incoming_message(&self, _id: &str, message: Arc<IncomingMessage>) {
        // 1. 推上行消息到记忆
        let agent_id = Arc::new(self.config.agent_id().await);
        let role_name = Arc::new(self.config.current_role().await);
        self.memory_store_client.push_channel_record(ChannelRecord {
            agent_id,
            role_name,
            messenger_id: message.messenger_id.clone(),
            user_id: message.user_id.clone(),
            group_id: message.group_id.clone(),
            is_self: message.is_self,
            content: message.content.clone(),
            time: message.time.clone(),
        }).await;

        // 2. 处理消息
        self.handle_incoming(message).await;
    }

    async fn join_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) {
        info!("用户 {} 加入群组 {}", notification.user_id, notification.group_id);
    }

    async fn leave_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) {
        info!("用户 {} 离开群组 {}", notification.user_id, notification.group_id);
    }

    async fn user_removed(&self, _id: &str, notification: Arc<UserRemoveNotification>) {
        info!("用户 {} 被删除", notification.user_id);
    }

    async fn download_chunk(&self, _id: &str, _info: Arc<AttachmentInfoResponse>, _pos: u64, _data: Bytes) -> std::result::Result<(), kissbot_channel_client::Error> {
        // 当前未使用附件下载
        Ok(())
    }

    async fn closed(&self, id: &str) {
        info!("channel 连接关闭: {}，重连循环将自动恢复", id);
    }
}
```

- [ ] **Step 5: 实现 handle_incoming**

```rust
impl AgentCoordinator {
    async fn handle_incoming(&self, incoming: Arc<IncomingMessage>) {
        let messenger_id = incoming.messenger_id.to_string();
        let user_id = incoming.user_id.to_string();
        let group_id = incoming.group_id.to_string();
        let is_self = incoming.is_self;

        // 提取文本内容
        let content_text = match &incoming.content {
            Content::Text(t) => t.as_str().to_string(),
            Content::Multi(items) => items.iter()
                .filter_map(|c| match c { Content::Text(t) => Some(t.as_str().to_string()), _ => None })
                .collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        };

        // 检查群组是否在绑定范围内
        let bindings = self.config.channel_bindings().await;
        if !bindings.iter().any(|b| b.messenger_id == messenger_id) {
            return;
        }

        // is_self 检查
        if is_self == 1 {
            let ctx = self.context_builder.lock().await;
            if ctx.is_self_echo(&content_text) {
                return;
            }
            return;
        }

        // 管理命令检查
        if CommandRouter::is_command(&content_text) {
            if CommandRouter::check_admin(&self.config, &messenger_id, &user_id).await {
                self.handle_admin_command(&content_text, &messenger_id, &user_id, &group_id).await;
            }
            return;
        }

        // 普通消息 → agentic loop
        self.run_agentic_loop(incoming).await;
    }
}
```

- [ ] **Step 6: 实现 send_reply（使用 ChannelClient::send_message）**

```rust
impl AgentCoordinator {
    async fn send_reply(&self, messenger_id: &str, user_id: &str, group_id: &str, content: String) {
        let Some(client) = self.channel_clients.get(messenger_id) else {
            warn!("send_reply: 未找到 channel client: {}", messenger_id);
            return;
        };

        let msg = OutgoingMessage {
            messenger_id: Arc::new(messenger_id.to_string()),
            user_id: Arc::new(user_id.to_string()),
            group_id: Arc::new(group_id.to_string()),
            content: Content::Text(Arc::new(content.clone())),
        };

        match client.send_message(msg).await {
            Ok(response) => {
                // 下行成功后推记忆（is_self=1）
                let agent_id = Arc::new(self.config.agent_id().await);
                let role_name = Arc::new(self.config.current_role().await);
                self.memory_store_client.push_channel_record(ChannelRecord {
                    agent_id,
                    role_name,
                    messenger_id: Arc::new(messenger_id.to_string()),
                    user_id: Arc::new(user_id.to_string()),
                    group_id: Arc::new(group_id.to_string()),
                    is_self: 1,
                    content: response.content.clone(),
                    time: response.time.clone(),
                }).await;

                // 记录已发送内容（用于 is_self echo 检测）
                let mut ctx = self.context_builder.lock().await;
                ctx.record_sent_content(content);
            }
            Err(e) => {
                warn!("send_reply 失败: {:?}", e);
            }
        }
    }
}
```

- [ ] **Step 7: 保留原有 handle_admin_command / run_agentic_loop / reset_context / load_ego_info 等方法**

这些方法的核心逻辑不变，只调整：
- `run_agentic_loop` 的参数改为 `Arc<IncomingMessage>`，用 `incoming.content` 提取文本
- 移除原有的 `ws_client.send_reply()` 调用，改为 `self.send_reply()`
- 所有 `crate::nexus::` 改为 `crate::`

run_agentic_loop 的关键变化：

```rust
async fn run_agentic_loop(&self, incoming: Arc<IncomingMessage>) {
    let content_text = match &incoming.content {
        Content::Text(t) => t.as_str().to_string(),
        _ => String::new(),
    };
    let messenger_id = incoming.messenger_id.to_string();
    let user_id = incoming.user_id.to_string();
    let group_id = incoming.group_id.to_string();
    let time = incoming.time.to_string();

    // 原有逻辑与原来一致，使用 content_text 替代 incoming.content
    // ...（与原来相同）
}
```

- [ ] **Step 8: 实现 `run()` 方法**

由于 channel-client 通过回调工作，`run()` 不再从 flume channel 接收消息。改为一个保持进程不退出的循环：

```rust
impl AgentCoordinator {
    pub async fn run(&self) {
        info!("AgentCoordinator 启动，等待外部输入...");
        // channel-client 通过 Terminal 回调工作，此处保持进程运行
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
}
```

---

### Task 4: 更新 main.rs + 清理旧文件

**Files:**
- Modify: `kissbot-agent/src/main.rs`
- Delete: `kissbot-agent/src/ws_client.rs`（如果还存在）
- Verify: `kissbot-agent/src/nexus/mod.rs` 已删除

- [ ] **Step 1: 重写 main.rs**

```rust
use std::sync::Arc;
use tracing::info;

mod config_manager;
mod coordinator;
mod memory_writer;
mod memory_reader;
mod memory_store_client;
mod llm_client;
mod context_builder;
mod command_router;
mod mode_manager;
mod station_router;
mod station_client;
mod http_server;
mod types;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    info!("kissbot-agent 启动");

    let config_path = std::env::var("CONFIG_PATH")
        .unwrap_or_else(|_| "config.json".to_string());
    info!("加载配置: {}", config_path);
    let config = Arc::new(
        config_manager::ConfigManager::load(&config_path).await
            .expect("加载配置失败")
    );

    info!("Agent ID: {}", config.agent_id().await);

    // MemoryWriter（思考/工具调用推送，非 channel 消息）
    let memory_writer = memory_writer::MemoryWriter::start();

    // Coordinator（内部启动 ChannelClient 连接和 MemoryStoreClient）
    let coordinator = coordinator::AgentCoordinator::new(config.clone(), memory_writer)
        .await
        .expect("初始化 Coordinator 失败");

    // 管理 API 服务器（后台）
    let mgr_config = config.clone();
    tokio::spawn(async move {
        let server = http_server::HttpServer::new(mgr_config, 9090);
        if let Err(e) = server.start().await {
            tracing::error!("管理 API 服务器退出: {:?}", e);
        }
    });

    // 运行主循环
    info!("进入主循环");
    coordinator.run().await;
}
```

- [ ] **Step 2: 删除 ws_client.rs**

```bash
rm -f /home/admin/project/kissbot/kissbot-agent/src/nexus/ws_client.rs
# 如果仍存在 src/ws_client.rs
rm -f /home/admin/project/kissbot/kissbot-agent/src/ws_client.rs
```

- [ ] **Step 3: 检查 nexus 目录是否为空并删除**

```bash
rmdir /home/admin/project/kissbot/kissbot-agent/src/nexus 2>/dev/null; echo "done"
```

---

### Task 5: 编译与修复

**Files:** 无新增，修复编译错误

- [ ] **Step 1: 尝试编译**

```bash
cd /home/admin/project/kissbot/kissbot-agent && cargo build 2>&1
```

- [ ] **Step 2: 逐个修复编译错误**

预期的编译错误类型：
1. `crate::nexus::` 残留引用（在未列出的文件中）
2. `ExternalMessage` 类型未定义（已被移除）
3. `ws_client` 相关引用未清理
4. `IncomingMessage.content` 改为 `Content` 枚举导致 `to_string()` 调用失效
5. 缺少 `use Duration` 等 import

逐一修复：
- 搜索 `crate::nexus::`：`grep -rn 'crate::nexus::' src/`，全部改为 `crate::`
- 搜索 `ExternalMessage`：`grep -rn 'ExternalMessage' src/`，移除相关代码
- 搜索 `ws_client`：`grep -rn 'ws_client' src/`，确保无残留
- 检查所有 `incoming.content.to_string()` 调用，改为匹配 `Content` 枚举
- 检查缺少的 import（`use std::time::Duration;` 等）

- [ ] **Step 3: 编译通过后验证**

```bash
cd /home/admin/project/kissbot/kissbot-agent && cargo build
```

Expected: 编译成功，无 warning 遗留。
