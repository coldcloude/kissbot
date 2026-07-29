# kissbot-agent 重构设计：适配 channel/memory、复用 channel-client、扁平化目录

## 概述

对 kissbot-agent 组件进行重构：
1. 用 `kissbot-channel-client` 替换手写 WS 客户端
2. 将 channel 消息推记忆逻辑移植到 agent
3. 扁平化目录结构（删除 `nexus/` 子目录）
4. 适配 `kissbot-api` 中修改后的 `Content` 枚举类型

## 目录扁平化

| 原路径 | 新路径 |
|--------|--------|
| `src/nexus/mod.rs` | 删除 |
| `src/nexus/*.rs` | `src/*.rs` |
| `src/nexus/ws_client.rs` | 删除（被 channel-client 替代） |
| `src/main.rs` | 不变 |

import 路径替换：所有 `crate::nexus::xxx` → `crate::xxx`。

## 依赖变更

```toml
# Cargo.toml 新增
kissbot-channel-client = { path = "../kissbot-channel-client" }
kai-file = { path = "../kai-rs/kai-file" }

# 删除
tokio-tungstenite = "0.26"
futures-util = "0.3"
```

## Coordinator 重构

### 结构变化

```
原结构                               新结构
AgentCoordinator                    AgentCoordinator
  config: Arc<ConfigManager>          config: Arc<ConfigManager>
  mode_manager: Arc<ModeManager>      mode_manager: Arc<ModeManager>
  memory_reader: Arc<MemoryReader>    memory_reader: Arc<MemoryReader>
  memory_writer: Arc<MemoryWriter>    memory_writer: Arc<MemoryWriter>
  context_builder: Mutex<...>        memory_store_client: Arc<MemoryStoreClient>  ← 新增
  llm_client: Mutex<LlmClient>       context_builder: Mutex<...>
  external_rx: Receiver<ExtMsg>      llm_client: Mutex<LlmClient>
  ws_client: Arc<WSClient>           channel_clients: Arc<DashMap<String, Arc<ChannelClient>>>
```

不再有 `external_rx` 和 `ws_client`。Coordinator 实现 `Terminal` trait（来自 channel-client），直接接收回调。

### 初始化流程

`Coordinator::new()` 内部：

1. 加载配置（与原来一致）
2. 创建 MemoryWriter（与原来一致）
3. 创建 MemoryStoreClient（从 channel 移植）
4. 遍历 `config.channel_bindings()`，对每个 binding：
   - 创建 `ChannelClient::new(messenger_id, Arc::downgrade(self))`
   - 调用 `client.connect(ws_url, api_key).await`
   - 存入 `channel_clients` map
5. 初始化 context_builder/llm_client（与原来一致）

### Terminal 实现：incoming_message

```rust
async fn incoming_message(&self, id: &str, message: Arc<IncomingMessage>) {
    // 1. 推上行消息到记忆
    let record = ChannelRecord {
        agent_id: Arc::new(self.config.agent_id().await),
        role_name: Arc::new(self.config.current_role().await),
        messenger_id: message.messenger_id.clone(),
        user_id: message.user_id.clone(),
        group_id: message.group_id.clone(),
        is_self: message.is_self,
        content: message.content.clone(),
        time: message.time.clone(),
    };
    self.memory_store_client.push_channel_record(record).await;

    // 2. 传入 Arc 进入处理（不深拷贝）
    self.handle_incoming(message).await;
}
```

`handle_incoming` 改为接收 `Arc<IncomingMessage>`：

```rust
async fn handle_incoming(&self, incoming: Arc<IncomingMessage>) {
    // 通过 Content 枚举提取文本
    let content_text = match &incoming.content {
        Content::Text(t) => t.as_str().to_string(),
        _ => incoming.content.to_string(),
    };
    // ... 原有逻辑（is_self 检查、管理命令、agentic loop）
    // 注意 agentic loop 中传入的是 incoming 的 Arc
}
```

### Terminal 实现：closed

重连逻辑：在 `closed()` 中延迟后重新创建 `ChannelClient` 并连接。

### 下行消息发送

```rust
async fn send_reply(&self, messenger_id: &str, user_id: &str, group_id: &str, content: String) {
    let Some(client) = self.channel_clients.get(messenger_id) else { return };

    let msg = OutgoingMessage {
        messenger_id: Arc::new(messenger_id.to_string()),
        user_id: Arc::new(user_id.to_string()),
        group_id: Arc::new(group_id.to_string()),
        content: Content::Text(Arc::new(content.clone())),
    };

    if let Ok(response) = client.send_message(msg).await {
        // 下行成功后推记忆（is_self=1，使用返回的 content）
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

        // 记录已发送内容（is_self echo 检测）
        let mut ctx = self.context_builder.lock().await;
        ctx.record_sent_content(content);
    }
}
```

### 其他 Terminal 回调

`join_group` / `leave_group` / `user_removed` / `download_chunk`：

- `join_group/leave_group`: 推一条 `Content::GroupJoin`/`Content::GroupLeave` 类型的 channel 记忆，然后可以产生系统提示
- `user_removed`: 清理功能（当前可以留空，后续扩展）
- `download_chunk`: 当前不做处理（附件下载尚未用到）

## MemoryStoreClient（从 channel 移植）

从 `kissbot-channel/src/memory_store_client.rs` 移植，使用 `FileObjectAppender` 实现批处理。

### 接口

```rust
pub struct MemoryStoreClient { ... }

impl MemoryStoreClient {
    pub fn new() -> Self;                      // 使用 ApiConfig::get().memory_store_url
    pub async fn push_channel_record(&self, record: ChannelRecord) -> Result<()>;
}
```

### ChannelRecord 结构

复用 `kissbot_api::memory::ChannelRequest`（包含 agent_id, role_name, messenger_id, user_id, group_id, is_self, content, time）。

内部通过 `FileAppendWriter` 在内存中按 key 聚合批处理（最大延迟 1s/最大 100 条），然后 POST `/store/channel`。

## 类型适配

### IncomingMessage.content

`IncomingMessage` 的 `content` 字段现在是 `Content` 枚举（而非旧版的 `msg_type` + `content: String` + `attachment_map`）。

代码中涉及 `incoming.content` 读取的地方改为通过 `Content` 枚举提取：

```rust
// 提取文本
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

### OutgoingMessage

由旧版 `msg_type` + `content: String` + `attachment_map` 改为 `content: Content`。

## 主入口 main.rs 变更

```rust
// 简化后的 main.rs
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let config_path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.json".to_string());
    let config = Arc::new(ConfigManager::load(&config_path).await.expect("加载配置失败"));

    // MemoryWriter（思考/工具调用推送）
    let memory_writer = MemoryWriter::start();

    // Coordinator（内部启动 ChannelClient 连接、MemoryStoreClient）
    let coordinator = AgentCoordinator::new(config.clone(), memory_writer).await.expect("初始化失败");

    // 管理 API
    tokio::spawn(async move {
        let server = HttpServer::new(config, 9090);
        if let Err(e) = server.start().await {
            tracing::error!("管理 API 服务器退出: {:?}", e);
        }
    });

    coordinator.run().await;
}
```

Coordinator 的 `run()` 不再从 flume channel 接收消息。改为常规的后台守护（当前是空循环或保持连接），因为 channel-client 的 WS 连接自己维护，Terminal 回调会直接调用 Coordinator 的方法。

## 未改动部分

以下模块的**内部逻辑保持不变**，只调整 import 路径（`crate::nexus::` → `crate::`）：

- `ConfigManager`（config_manager.rs）
- `MemoryWriter`（memory_writer.rs）
- `MemoryReader`（memory_reader.rs）
- `LlmClient`（llm_client.rs）
- `ContextBuilder`（context_builder.rs）
- `CommandRouter`（command_router.rs）
- `ModeManager`（mode_manager.rs）
- `StationRouter`（station_router.rs）
- `StationClient`（station_client.rs）
- `HttpServer`（http_server.rs）
- `types.rs` 中的 Error、Mode、WriteTask、ContextMessage、AdminCommand 等定义

## 风险与注意事项

1. **channel-client 的 `Terminal` 回调运行在 kai-ws 协程中**：`push_channel_record` 使用 `FileObjectAppender` 异步批处理，不阻塞回调；`handle_incoming` 中的 LLM 调用可能耗时较长，但这是 agentic loop 的正常行为（回调返回后 WS 协程继续处理其他消息）
2. **ChannelClient reconnect**：当前 `channel-client` 不提供自动重连。`closed()` 回调中需要实现手动重连逻辑
3. **Content::Multi 的处理**：`run_agentic_loop` 中传入 LLM 的 content 如果是 Multi 类型，需要拼接为文本
