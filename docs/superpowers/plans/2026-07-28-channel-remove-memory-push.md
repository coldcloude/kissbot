# 从 channel-manager 移除记忆推送 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development (recommended) or executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 从 channel 组件中去除推送记忆到 memory-store 的全部逻辑，使 channel 不再感知 agent_id、role_name 等 agent 相关信息，绑定只记录 connect_id

**Architecture:** 改动涉及 5 个 crate：kissbot-api（BindRequest 协议）、kissbot-channel（核心逻辑排除）、kissbot-channel-client-cli（协议适配）、kissbot-channel-client/tests（协议适配）。memory_store_client.rs 从编译中排除但文件保留磁盘，flume/reqwest/kai-file 依赖同步清理。

**Tech Stack:** Rust, tokio, serde

## Global Constraints

- BindRequest 删除 agent_id、role_name 字段，只保留 messenger_id、user_id
- kissbot-agent 本次不动（后续统一改）
- memory_store_client.rs 文件保留在 kissbot-channel/src/ 下，但不参与编译
- flume、reqwest、kai-file 依赖从 kissbot-channel/Cargo.toml 移除

---
### Task 1: kissbot-api — BindRequest 协议删除 agent_id/role_name

**Files:**
- Modify: `kissbot-api/src/channel.rs:143-145`
- Test: `kissbot-api/src/channel.rs:355-368`（内联测试）

**Interfaces:**
- Consumes: 无
- Produces: `BindRequest { messenger_id: Arc<String>, user_id: Arc<String> }`

- [ ] **Step 1: 编辑 BindRequest 结构体，删除 agent_id 和 role_name 字段**

在原文件 `kissbot-api/src/channel.rs` 中找到：

```rust
pub struct BindRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
}
```

改为：

```rust
pub struct BindRequest {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
}
```

- [ ] **Step 2: 编辑内联测试，删除 agent_id/role_name 构造和断言**

找到 `test_serde_bind_request` 测试函数：

```rust
#[test]
fn test_serde_bind_request() {
    let obj = BindRequest {
        agent_id: Arc::new("agent1".to_string()),
        role_name: Arc::new("admin".to_string()),
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
    };
    let json = serde_json::to_value(&obj).unwrap();
    let deserialized: BindRequest = serde_json::from_value(json).unwrap();
    assert_eq!(*deserialized.agent_id, "agent1");
    assert_eq!(*deserialized.role_name, "admin");
}
```

改为：

```rust
#[test]
fn test_serde_bind_request() {
    let obj = BindRequest {
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
    };
    let json = serde_json::to_value(&obj).unwrap();
    let deserialized: BindRequest = serde_json::from_value(json).unwrap();
    assert_eq!(*deserialized.messenger_id, "m1");
    assert_eq!(*deserialized.user_id, "u1");
}
```

- [ ] **Step 3: 编译验证**

```bash
cd kissbot-api && cargo build && cargo test
```

Expected: 无警告无错误

- [ ] **Step 4: 提交**

```bash
git add kissbot-api/src/channel.rs
git commit -m "feat(api): BindRequest 删除 agent_id 和 role_name 字段"
```

---

### Task 2: 下游 BindRequest 调用方适配（cli + mock）

**Files:**
- Modify: `kissbot-channel-client-cli/src/main.rs:36-41`
- Modify: `kissbot-channel-client/tests/mock.rs:327-331`

**Interfaces:**
- Consumes: `BindRequest { messenger_id, user_id }`
- Produces: 无新接口

- [ ] **Step 1: 编辑 kissbot-channel-client-cli 的 bind 方法**

在 `kissbot-channel-client-cli/src/main.rs` 中找到：

```rust
client.bind(BindRequest {
    agent_id: Arc::new("cli".to_string()),
    role_name: Arc::new("cli".to_string()),
    messenger_id: Arc::new(self.messenger_id.clone()),
    user_id: Arc::new(self.user_id.clone()),
}).await
```

改为：

```rust
client.bind(BindRequest {
    messenger_id: Arc::new(self.messenger_id.clone()),
    user_id: Arc::new(self.user_id.clone()),
}).await
```

- [ ] **Step 2: 编辑 mock.rs 的 make_bind_request 函数**

在 `kissbot-channel-client/tests/mock.rs` 中找到：

```rust
pub fn make_bind_request(messenger_id: &str, user_id: &str) -> BindRequest {
    BindRequest {
        agent_id: Arc::new("test-agent".to_string()),
        role_name: Arc::new("test-role".to_string()),
        messenger_id: Arc::new(messenger_id.to_string()),
        user_id: Arc::new(user_id.to_string()),
    }
}
```

改为：

```rust
pub fn make_bind_request(messenger_id: &str, user_id: &str) -> BindRequest {
    BindRequest {
        messenger_id: Arc::new(messenger_id.to_string()),
        user_id: Arc::new(user_id.to_string()),
    }
}
```

- [ ] **Step 3: 编译验证**

```bash
cd kissbot-channel-client-cli && cargo build
cd kissbot-channel-client && cargo build
```

Expected: 无警告无错误

- [ ] **Step 4: 提交**

```bash
git add kissbot-channel-client-cli/src/main.rs kissbot-channel-client/tests/mock.rs
git commit -m "refactor(cli,mock): BindRequest 调用适配新协议"
```

---

### Task 3: kissbot-channel lib.rs + error.rs — 从编译中排除 memory_store_client，清理相关错误变体

**Files:**
- Modify: `kissbot-channel/src/lib.rs:5,12`
- Modify: `kissbot-channel/src/error.rs:3,41-46`

**注意：** 此任务中 error.rs 暂不删除 `ReqwestError` 变体（reqwest 依赖仍在 Cargo.toml 中，编译没问题），Task 5 删 reqwest 依赖时会同步清理。

- [ ] **Step 1: lib.rs — 注释掉 memory_store_client 模块和 pub use**

在 `kissbot-channel/src/lib.rs` 中找到：

```rust
pub mod memory_store_client;
...
pub use memory_store_client::MemoryStoreClient;
```

改为：

```rust
// pub mod memory_store_client; // 保留文件到磁盘，后续移植到 agent
...
// pub use memory_store_client::MemoryStoreClient; // 保留文件到磁盘，后续移植到 agent
```

- [ ] **Step 2: error.rs — 删除 MessageRecord 导入和 flume 两个变体**

在 `kissbot-channel/src/error.rs` 中找到：

```rust
use crate::{memory_store_client::MessageRecord};
```

改为：

```rust
```

删除以下两个变体：

```rust
    #[error("Flume Send error: {0}")]
    SendError(#[from] flume::SendError<MessageRecord>),

    #[error("Flume Recv error: {0}")]
    RecvError(#[from] flume::RecvError),
```

- [ ] **Step 3: 编译验证**

```bash
cd kissbot-channel && cargo build
```

Expected: 无警告无错误（ReqwestError 等变体暂时保留，编译可过）

- [ ] **Step 4: 提交**

```bash
git add kissbot-channel/src/lib.rs kissbot-channel/src/error.rs
git commit -m "refactor(channel): 从编译中排除 memory_store_client，清理 flume 错误变体"
```

---

### Task 4: kissbot-channel channel_manager.rs — 移除记忆推送逻辑

**Files:**
- Modify: `kissbot-channel/src/channel_manager.rs:4,24-25,52,213-229,715-729,777-783`

- [ ] **Step 1: 删除 MemoryStoreClient 导入**

找到：

```rust
use crate::memory_store_client::MemoryStoreClient;
```

删除此行。

- [ ] **Step 2: BoundInfo 只保留 connect_id**

找到：

```rust
#[derive(Clone)]
struct BoundInfo {
    pub connect_id: u32,
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
}
```

改为：

```rust
#[derive(Clone)]
struct BoundInfo {
    pub connect_id: u32,
}
```

- [ ] **Step 3: ChannelManager 结构体删除 memory_store_client 字段**

找到：

```rust
pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
    messenger_map: DashMap<String, Arc<MessengerContext>>,
    memory_store_client: Arc<MemoryStoreClient>,
    // 上传方向：transfer_id → AttachmentReceiverContext
    attachment_receiver_map: DashMap<u32, AttachmentReceiverContext>,
    // 下载方向：transfer_id → AttachmentSenderContext
    attachment_sender_map: DashMap<u32, AttachmentSenderContext>,
}
```

改为：

```rust
pub struct ChannelManager {
    global_connect_id: AtomicU32,
    connect_map: DashMap<u32, Arc<ConnectContext>>,
    messenger_map: DashMap<String, Arc<MessengerContext>>,
    // 上传方向：transfer_id → AttachmentReceiverContext
    attachment_receiver_map: DashMap<u32, AttachmentReceiverContext>,
    // 下载方向：transfer_id → AttachmentSenderContext
    attachment_sender_map: DashMap<u32, AttachmentSenderContext>,
}
```

- [ ] **Step 4: new() 构造函数删除 memory_store_client 初始化**

找到：

```rust
    pub fn new() -> Self {
        Self {
            global_connect_id: AtomicU32::new(0),
            connect_map: DashMap::new(),
            messenger_map: DashMap::new(),
            memory_store_client: Arc::new(MemoryStoreClient::new()),
            attachment_receiver_map: DashMap::new(),
            attachment_sender_map: DashMap::new(),
        }
    }
```

改为：

```rust
    pub fn new() -> Self {
        Self {
            global_connect_id: AtomicU32::new(0),
            connect_map: DashMap::new(),
            messenger_map: DashMap::new(),
            attachment_receiver_map: DashMap::new(),
            attachment_sender_map: DashMap::new(),
        }
    }
```

- [ ] **Step 5: BindAgentUserProcessor 不再读取 agent_id/role_name**

找到 BindAgentUserProcessor 的 `raw_process_json` 方法中的以下部分：

```rust
        let agent_id = bind_request.agent_id;
        let role_name = bind_request.role_name;

        let messenger_context = manager.messenger_map.get(bind_request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(bind_request.messenger_id.to_string()))?;
        
        let messenger_info = messenger_context.messenger.get_info().await?;

        if !messenger_info.user_map.contains_key(bind_request.user_id.as_str()) {
            return Err(Error::UserNotFound(bind_request.user_id.to_string()));
        }

        //绑定用户
        let bound_info = messenger_context.bound_map.entry(bind_request.user_id.to_string()).or_insert_with(|| BoundInfo {
            connect_id: connect_context.connect_id,
            agent_id: agent_id.clone(),
            role_name: role_name.clone(),
        });
        
        if bound_info.connect_id != connect_context.connect_id {
            return Err(Error::UserAlreadyBound(bound_info.connect_id.to_string()));
        }
```

改为：

```rust
        let messenger_context = manager.messenger_map.get(bind_request.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(bind_request.messenger_id.to_string()))?;
        
        let messenger_info = messenger_context.messenger.get_info().await?;

        if !messenger_info.user_map.contains_key(bind_request.user_id.as_str()) {
            return Err(Error::UserNotFound(bind_request.user_id.to_string()));
        }

        //绑定用户
        let bound_info = messenger_context.bound_map.entry(bind_request.user_id.to_string()).or_insert_with(|| BoundInfo {
            connect_id: connect_context.connect_id,
        });
        
        if bound_info.connect_id != connect_context.connect_id {
            return Err(Error::UserAlreadyBound(bound_info.connect_id.to_string()));
        }
```

- [ ] **Step 6: 删除 send_to_memory_store 方法**

删除整个 `send_to_memory_store` 方法：

```rust
    async fn send_to_memory_store(&self, event: Arc<IncomingMessage>) -> Result<()>{
        //找到对应的agent和role
        let messenger_context = self.messenger_map.get(event.messenger_id.as_str())
        .ok_or_else(|| Error::MessengerNotFound(event.messenger_id.to_string()))?;
        
        let bound_info = messenger_context.bound_map.get(event.user_id.as_str())
        .ok_or_else(|| Error::UserNotFound(format!("User not bound: user_id {}", event.user_id)))?;

        self.memory_store_client.push_messages(bound_info.agent_id.clone(), bound_info.role_name.clone(), event).await?;
        Ok(())
    }
```

- [ ] **Step 7: handle_incoming_message 改为只推送 agent**

找到：

```rust
    pub async fn handle_incoming_message(&self, event: Arc<IncomingMessage>) {
        let span = span!(Level::INFO, "channel_manager handle incoming message");
        let _enter = span.enter();
        let results = tokio::join!(
            self.send_to_agent(event.clone()),
            self.send_to_memory_store(event.clone()),
        );
        for result in vec![results.0, results.1] {
            if let Err(e) = result {
                error!("Error processing incoming message: {:?}", e);
            }
        }
    }
```

改为：

```rust
    pub async fn handle_incoming_message(&self, event: Arc<IncomingMessage>) {
        let span = span!(Level::INFO, "channel_manager handle incoming message");
        let _enter = span.enter();
        if let Err(e) = self.send_to_agent(event).await {
            error!("Error processing incoming message: {:?}", e);
        }
    }
```

- [ ] **Step 8: 编译验证**

```bash
cd kissbot-channel && cargo build
```

Expected: 无警告无错误。注意文件开头可能还有 `use crate::memory_store_client::MemoryStoreClient;` 需要确认已删（Step 1）；确保 `use tokio::join` 不再是 unused import。如果 `tokio::join!` 不再使用但还在文件顶部 import 路径中（`use tokio::sync::oneshot::Sender` 等），检查是否有多余的 `use` 需要清理。

- [ ] **Step 9: 提交**

```bash
git add kissbot-channel/src/channel_manager.rs
git commit -m "refactor(channel): channel_manager 移除记忆推送逻辑，BoundInfo 只保留 connect_id"
```

---

### Task 5: kissbot-channel Cargo.toml — 移除不再使用的依赖

**Files:**
- Modify: `kissbot-channel/Cargo.toml:13,18-19,22`
- Modify: `kissbot-channel/src/error.rs:22`（删除 RegwestError 变体）

- [ ] **Step 1: 编辑 Cargo.toml，移除 flume、reqwest、kai-file**

找到：

```toml
flume = "0.12"
...
reqwest = { version = "0.12", features = ["json"] }
...
kai-file = { path = "../kai-rs/kai-file" }
```

删除以上三行。

- [ ] **Step 2: error.rs 删除 ReqwestError 变体**

找到：

```rust
    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),
```

删除此行。

- [ ] **Step 3: 编译验证**

```bash
cd kissbot-channel && cargo build
cd kissbot-channel-web && cargo build
cd kissbot-channel-client && cargo build
cd kissbot-channel-client-cli && cargo build
```

Expected: 全部通过

- [ ] **Step 4: 提交**

```bash
git add kissbot-channel/Cargo.toml kissbot-channel/src/error.rs
git commit -m "build(channel): 移除 flume、reqwest、kai-file 依赖"
```

---

### Task 6: 全量编译验证

- [ ] **Step 1: 全量 build + test**

```bash
cargo build -p kissbot-api -p kissbot-channel -p kissbot-channel-client -p kissbot-channel-client-cli -p kissbot-channel-web 2>&1
```

- [ ] **Step 2: 运行测试**

```bash
cargo test -p kissbot-api
cargo test -p kissbot-channel-client
cargo test -p kissbot-channel-web 2>&1 || echo "kissbot-channel-web 依赖环境，测试可跳过"
```

- [ ] **Step 3: 确认 memory_store_client.rs 文件仍在磁盘**

```bash
ls -la kissbot-channel/src/memory_store_client.rs
```

Expected: 文件存在，未删除

- [ ] **Step 4: 提交**

```bash
git add -A && git status
# 确认只有期望的文件变更后
git commit -m "chore: 全量构建验证通过"
```
