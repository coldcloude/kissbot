# Channel 记录存储重构实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** channel 记忆记录从按 (messenger, user, group) 组合拆文件改为每 (agent, role, date) 一个文件，ChannelRecord 增加 messenger_id/group_id/self_user_id，channel 使用公共 RecordKey/QueryRequest，删除组合查询 API，agent 记忆查询简化为单次全史查询。

**Architecture:** kissbot-api 定义类型（ChannelRecord 加字段、删除 ChannelRecordKey/QueryChannelRequest/ChannelCombo）→ kissbot-memory 的 ChannelParser 与索引改公共 key/query、删除 query_combos → kissbot-memory-store 的 API 与 FileHook 适配 → kissbot-agent 的 MemoryReader 单次查询简化 → 文档更新。

**Tech Stack:** Rust + cargo（workspace 多 crate）、kai-file（FilePathGenerator/QueryParser/FileIndexContext）、axum（memory-store API）、serde/tokio。

## Global Constraints

- 项目原则：不要删除代码中的注释；旧注释表述与新结构矛盾时更新其内容（必要修改），不删除仍有价值的语义注释
- 提交 comment 用中文，应包含该提交涉及的全部改动内容
- 所有文本文件 UTF-8 编码、`\n` 换行
- 禁止用 sed/python 等命令修改文件（只能使用 Read/Write/Edit 工具）
- 旧数据不迁移：`channel-{m}={u}={g}-records-{date}.jsonl` 直接弃用（项目早期，均为测试数据）
- 不统一四个 parser/查询 handler 为泛型实现；不保留 ChannelRecordKey 等兼容别名
- **跨 crate 重构期间的编译状态**：从 Task 1 起整个 workspace 编译会阶段性失败，属预期。每个任务的验证门禁是 `cargo test -p <本任务 crate>`（只编译该 crate 及其依赖）。Task 1 之后 kissbot-agent / kissbot-memory / kissbot-memory-store 编译失败，按顺序由 Task 2/3/4 逐一恢复；全部恢复后才能 `cargo build --workspace`。

---

### Task 1: kissbot-api 类型变更（ChannelRecord 加字段、删除专用类型）

**Files:**
- Modify: `kissbot-api/src/memory.rs`（ChannelRecord 结构体、删除 QueryChannelRequest / ChannelCombo / ChannelRecordKey、更新测试）

**Interfaces:**
- Consumes: 无
- Produces: `ChannelRecord` 增加 `messenger_id: Arc<String>`、`group_id: Arc<String>`、`self_user_id: Arc<String>` 三个字段（`user_id` 仍为发送者）；删除 `ChannelRecordKey`、`QueryChannelRequest`、`ChannelCombo` 三个类型

- [ ] **Step 1: 修改 ChannelRecord 结构体，增加三个字段**

把 `kissbot-api/src/memory.rs` 中的：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub user_id: Arc<String>,
    pub is_self: usize,
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
    pub group_name: Arc<String>,
    pub content: Content,
    pub time: Arc<String>,
    pub sn: u64,
}
```

改为（user_id 保留为发送者；新增 self_user_id=绑定用户、messenger_id、group_id）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    /// 发送者身份（区分对话方向）
    pub user_id: Arc<String>,
    /// agent 在 channel 绑定的用户（接收方身份 / agent 视角的 self）；
    /// 注意与 is_self 不同：其他人用绑定用户发消息时 user_id == self_user_id 但 is_self == 0
    pub self_user_id: Arc<String>,
    pub messenger_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
    pub group_name: Arc<String>,
    pub content: Content,
    pub time: Arc<String>,
    pub sn: u64,
}
```

- [ ] **Step 2: 删除专用类型**

删除以下三个结构体（含其上方注释）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryChannelRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
}
```

```rust
/// channel 记录文件组合（messenger + user + group），用于按组合精确查询
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ChannelCombo {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ChannelRecordKey {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub date: Arc<String>,
}
```

- [ ] **Step 3: 删除对应的三个 serde 测试**

删除 `test_serde_query_channel_request`、`test_serde_channel_combo`、`test_serde_channel_record_key` 三个测试函数。

- [ ] **Step 4: 更新现有 ChannelRecord 构造**

`test_serde_channel_record`（Record serde 区）改为：

```rust
    #[test]
    fn test_serde_channel_record() {
        let obj = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new("M1Name".to_string()),
            user_name: Arc::new("U1Name".to_string()),
            group_name: Arc::new("G1Name".to_string()),
            content: Content::Text(Arc::new("hello".to_string())),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.user_id, "u1");
        assert_eq!(*deserialized.self_user_id, "self1");
        assert_eq!(*deserialized.messenger_id, "telegram");
        assert_eq!(*deserialized.group_id, "g1");
        assert_eq!(*deserialized.messenger_name, "M1Name");
        assert_eq!(*deserialized.user_name, "U1Name");
        assert_eq!(*deserialized.group_name, "G1Name");
        assert!(matches!(deserialized.content, Content::Text(val) if val.as_str() == "hello"));
        assert_eq!(deserialized.sn, 1);
    }
```

`test_record_impl` 中的 `ChannelRecord` 构造补充三个字段（在 `user_id` 之后插入）：

```rust
        let mut channel = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("hello".to_string())),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 5,
        };
```

`test_record_cmp_time` 中的 `r1` 与 `r2` 构造同样补充三字段（r1 中 `user_id` 后插入；r2 用 `..r1.clone()` 的不用改，其余两个完整构造补上）：

```rust
        let r1 = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("hello".to_string())),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let r2 = ChannelRecord {
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("world".to_string())),
            time: Arc::new("2026-06-24 10:00:01".to_string()),
            sn: 1,
        };
```

- [ ] **Step 5: 运行 kissbot-api 测试**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-api`
Expected: PASS（全部测试通过；本 crate 独立编译成功）

- [ ] **Step 6: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-api/src/memory.rs && git commit -m "refactor(api): ChannelRecord 增加 messenger_id/group_id/self_user_id 字段；删除 ChannelRecordKey/QueryChannelRequest/ChannelCombo 专用类型及 serde 测试（channel 记忆将改用公共 RecordKey/QueryRequest）"
```

---

### Task 2: kissbot-agent MemoryReader 查询简化

**Files:**
- Modify: `kissbot-agent/src/memory_reader.rs`

**Interfaces:**
- Consumes: Task 1 已删除 `ChannelCombo`；本任务移除对它的全部引用，使 kissbot-agent 恢复编译
- Produces: `query_channel(&self, agent_id: &str, role_name: &str, start_time: &str, end_time: &str) -> Result<Vec<MemoryMsg>>`（单次全史查询，POST `/store/query/channel`，body 为 `{agent_id, role_name, start_time, end_time}`）；`read_recent_for_context` 不再依赖 combos

- [ ] **Step 1: 删除 ChannelCombo 导入**

删除 `kissbot-agent/src/memory_reader.rs` 第 3 行的：

```rust
use kissbot_api::memory::ChannelCombo;
```

（`serde_json::json` 仍在使用，保留。）

- [ ] **Step 2: 删除 query_combos 方法**

删除整个 `query_combos` 方法（含注释）：

```rust
    /// 组合查询：POST {store}/store/query/combos，返回 (agent, role) 时间范围内出现的 channel 组合
    async fn query_combos(&self, agent_id: &str, role_name: &str, start: &str, end: &str) -> Result<Vec<ChannelCombo>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();
        let url = format!("{}/store/query/combos", store_url.trim_end_matches('/'));
        let body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
            "start_time": start,
            "end_time": end,
        });
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("组合查询失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("组合查询返回 {}", resp.status())));
        }
        let data: serde_json::Value = resp.json().await?;
        // 响应 data 非预期结构时记日志（降级为空组合，避免静默丢组合）
        match serde_json::from_value(data["data"].clone()) {
            Ok(combos) => Ok(combos),
            Err(e) => {
                warn!("组合查询响应解析失败: {}", e);
                Ok(Vec::new())
            }
        }
    }
```

- [ ] **Step 3: query_channel 去掉 combo 参数，改为单次全史查询**

把：

```rust
    /// 组合内精确查询：POST {store}/store/query/channel（messenger/user/group 精确 key，取该时间全部）
    async fn query_channel(
        &self,
        agent_id: &str,
        role_name: &str,
        combo: &ChannelCombo,
        start_time: &str,
        end_time: &str,
    ) -> Result<Vec<MemoryMsg>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();
        let url = format!("{}/store/query/channel", store_url.trim_end_matches('/'));
        let body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
            "messenger_id": combo.messenger_id,
            "user_id": combo.user_id,
            "group_id": combo.group_id,
            "start_time": start_time,
            "end_time": end_time,
        });
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("记忆读取返回 {}", resp.status())));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(self.parse_channel_records(&data["data"]))
    }
```

改为：

```rust
    /// 单次全史查询：POST {store}/store/query/channel（QueryRequest，所有 channel 记录同文件，取该时间全部）
    async fn query_channel(
        &self,
        agent_id: &str,
        role_name: &str,
        start_time: &str,
        end_time: &str,
    ) -> Result<Vec<MemoryMsg>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();
        let url = format!("{}/store/query/channel", store_url.trim_end_matches('/'));
        let body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
            "start_time": start_time,
            "end_time": end_time,
        });
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("记忆读取返回 {}", resp.status())));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(self.parse_channel_records(&data["data"]))
    }
```

- [ ] **Step 4: read_recent_for_context 改为单次查询**

把：

```rust
    /// role 模式记忆打包：组合查询 → 每组合全史查询合并 → 并集算法 → 升序结果
    pub async fn read_recent_for_context(
        &self,
        agent_id: &str,
        role_name: &str,
        cfg: &EffectiveContextConfig,
    ) -> Result<Vec<MemoryMsg>> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let start = chrono::Local::now()
            .checked_sub_signed(chrono::Duration::seconds(cfg.memory_time_secs as i64))
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "2000-01-01 00:00:00".to_string());

        // 组合：按全史范围取（组合由文件枚举，范围覆盖即可）
        let combos = self.query_combos(agent_id, role_name, "2000-01-01 00:00:00", &now).await?;

        // (1) 每组合全史查询，合并升序
        let mut merged: Vec<MemoryMsg> = Vec::new();
        for combo in &combos {
            match self.query_channel(agent_id, role_name, combo, "2000-01-01 00:00:00", &now).await {
                Ok(msgs) => merged.extend(msgs),
                Err(e) => warn!("组合查询失败 messenger={} user={} group={}: {}",
                    combo.messenger_id, combo.user_id, combo.group_id, e),
            }
        }
        merged.sort_by(|a, b| a.time.cmp(&b.time));

        Ok(recent_memory(&merged, cfg.memory_count, &start))
    }
```

改为：

```rust
    /// role 模式记忆打包：单次全史查询 → 并集算法 → 升序结果（channel 记录已合并为单文件，无需组合枚举）
    pub async fn read_recent_for_context(
        &self,
        agent_id: &str,
        role_name: &str,
        cfg: &EffectiveContextConfig,
    ) -> Result<Vec<MemoryMsg>> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let start = chrono::Local::now()
            .checked_sub_signed(chrono::Duration::seconds(cfg.memory_time_secs as i64))
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "2000-01-01 00:00:00".to_string());

        // 单次全史查询（所有 channel 记录同文件，范围覆盖即可）
        let msgs = self.query_channel(agent_id, role_name, "2000-01-01 00:00:00", &now).await?;

        Ok(recent_memory(&msgs, cfg.memory_count, &start))
    }
```

- [ ] **Step 5: 运行 kissbot-agent 测试**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-agent`
Expected: PASS（recent_memory / pack_memory_messages / extract 等测试不受影响；本 crate 恢复编译）

- [ ] **Step 6: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-agent/src/memory_reader.rs && git commit -m "refactor(agent): MemoryReader 记忆查询简化——删除 query_combos 与逐组合查询，query_channel 改为单次全史查询（QueryRequest body），read_recent_for_context 单次查询后复用并集算法，行为不变"
```

---

### Task 3: kissbot-memory ChannelParser 与索引改公共 key/query

**Files:**
- Modify: `kissbot-memory/src/data.rs`（ChannelParser 三个 trait impl + 测试）
- Modify: `kissbot-memory/src/index.rs`（channel_indices 泛型、mark 签名、query_channel_records 签名、删除 query_combos、测试）

**Interfaces:**
- Consumes: Task 1 的类型（ChannelRecord 新字段；ChannelRecordKey/QueryChannelRequest 已删）
- Produces: `ChannelParser` 实现 `FilePathGenerator<RecordKey>`（文件名 `channel-records-{date}.jsonl`）、`RequestParser<ChannelRequest, RecordKey, ChannelRecord>`、`QueryParser<QueryRequest, RecordKey>`（复用公共 `parse_query`）；`MemoryIndexer::query_channel_records(query: QueryRequest) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)>>`；`mark_channel_obsolete` / `mark_channel_all_obsolete(&self, key: &RecordKey)`；`query_combos` 已删除

- [ ] **Step 1: ChannelParser 改 FilePathGenerator<RecordKey>**

把 `kissbot-memory/src/data.rs` 中的：

```rust
#[async_trait]
impl FilePathGenerator<ChannelRecordKey> for ChannelParser {
    async fn get_path(&self, key: &ChannelRecordKey) -> std::result::Result<PathBuf,kai_file::Error> {
        let year_role_dir = ensure_year_role_dir(key.agent_id.as_str(), key.role_name.as_str(), key.date.as_str()).await?;
        let file_name = format!("channel-{}={}={}-records-{}.jsonl", key.messenger_id.as_str(), key.user_id.as_str(), key.group_id.as_str(), key.date.as_str());
        Ok(year_role_dir.join(file_name))
    }
}
```

改为：

```rust
#[async_trait]
impl FilePathGenerator<RecordKey> for ChannelParser {
    async fn get_path(&self, key: &RecordKey) -> std::result::Result<PathBuf,kai_file::Error> {
        let year_role_dir = ensure_year_role_dir(key.agent_id.as_str(), key.role_name.as_str(), key.date.as_str()).await?;
        let file_name = format!("channel-records-{}.jsonl", key.date);
        Ok(year_role_dir.join(file_name))
    }
}
```

- [ ] **Step 2: ChannelParser 改 RequestParser<ChannelRequest, RecordKey, ChannelRecord>**

把：

```rust
impl RequestParser<ChannelRequest, ChannelRecordKey, ChannelRecord> for ChannelParser {
    fn parse_request(&self, request: ChannelRequest) -> (ChannelRecordKey, ChannelRecord) {
        // 文件名（key.user_id）按接收方 self_user_id 分文件（同一 channel 双向消息归入绑定用户文件）；
        // record.user_id 保留发送者身份（区分对话方向）
        let user_id = request.user_id.clone();
        let key = ChannelRecordKey {
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            messenger_id: request.messenger_id.clone(),
            user_id: request.self_user_id.clone(),
            group_id: request.group_id.clone(),
            date: Arc::new(as_date(&request.time).to_string()),
        };
        let record = ChannelRecord {
            user_id: user_id,
            is_self: request.is_self,
            messenger_name: request.messenger_name.clone(),
            user_name: request.user_name.clone(),
            group_name: request.group_name.clone(),
            content: request.content.clone(),
            time: request.time.clone(),
            sn: 0,
        };
        (key, record)
    }
}
```

改为：

```rust
impl RequestParser<ChannelRequest, RecordKey, ChannelRecord> for ChannelParser {
    fn parse_request(&self, request: ChannelRequest) -> (RecordKey, ChannelRecord) {
        // 所有 channel 记录归入同一文件（每 agent+role+date）；
        // record 保存完整身份：user_id=发送者、self_user_id=agent 绑定用户（接收方）、messenger_id/group_id
        let key = RecordKey {
            agent_id: request.agent_id.clone(),
            role_name: request.role_name.clone(),
            date: Arc::new(as_date(&request.time).to_string()),
        };
        let record = ChannelRecord {
            user_id: request.user_id.clone(),
            self_user_id: request.self_user_id.clone(),
            messenger_id: request.messenger_id.clone(),
            group_id: request.group_id.clone(),
            is_self: request.is_self,
            messenger_name: request.messenger_name.clone(),
            user_name: request.user_name.clone(),
            group_name: request.group_name.clone(),
            content: request.content.clone(),
            time: request.time.clone(),
            sn: 0,
        };
        (key, record)
    }
}
```

- [ ] **Step 3: ChannelParser 改 QueryParser<QueryRequest, RecordKey>**

把：

```rust
impl QueryParser<QueryChannelRequest, ChannelRecordKey> for ChannelParser {
    fn parse_query(&self, query: QueryChannelRequest) -> Vec<(ChannelRecordKey, (String, String))> {
        let agent_id = query.agent_id.clone();
        let role_name = query.role_name.clone();
        let messenger_id = query.messenger_id.clone();
        let user_id = query.user_id.clone();
        let group_id = query.group_id.clone();
        let mut results = Vec::new();
        if let Ok(date_times) = get_date_time_segments(&query.start_time, &query.end_time) {
            for time in date_times {
                let date = as_date(&time.0);
                results.push((ChannelRecordKey {
                    agent_id: agent_id.clone(),
                    role_name: role_name.clone(),
                    messenger_id: messenger_id.clone(),
                    user_id: user_id.clone(),
                    group_id: group_id.clone(),
                    date: Arc::new(date.to_string()),
                }, time));
            }
        }
        results
    }
}
```

改为：

```rust
impl QueryParser<QueryRequest, RecordKey> for ChannelParser {
    fn parse_query(&self, query: QueryRequest) -> Vec<(RecordKey, (String, String))> {
        parse_query(query)
    }
}
```

- [ ] **Step 4: 更新 data.rs 测试**

`test_channel_file_name` 改为：

```rust
    #[tokio::test]
    async fn test_channel_file_name() {
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ChannelParser;
        let path = parser.get_path(&key).await.unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(file_name, "channel-records-2026-06-24.jsonl");
    }
```

`test_channel_file_dir_empty_role` 改为：

```rust
    #[tokio::test]
    async fn test_channel_file_dir_empty_role() {
        // 空 role_name 目录应为 `2026-`（非 `2026`）
        let key = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let parser = ChannelParser;
        let path = parser.get_path(&key).await.unwrap();
        let dir_name = path.parent().unwrap().file_name().unwrap().to_str().unwrap();
        assert_eq!(dir_name, "2026-");
    }
```

`test_channel_request_parser` 改为：

```rust
    #[test]
    fn test_channel_request_parser() {
        let request = ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 1,
            messenger_name: Arc::new("TelegramName".to_string()),
            user_name: Arc::new("U1Name".to_string()),
            group_name: Arc::new("G1Name".to_string()),
            content: Content::Text(Arc::new("hello".to_string())),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
        };
        let parser = ChannelParser;
        let (key, record) = parser.parse_request(request);
        // key 为公共 RecordKey（无 messenger/user/group 字段）
        assert_eq!(*key.agent_id, "agent1");
        assert_eq!(*key.role_name, "default");
        assert_eq!(*key.date, "2026-06-24");
        // record 保存完整身份：user_id=发送者、self_user_id=绑定用户、messenger_id/group_id
        assert_eq!(*record.user_id, "u1");
        assert_eq!(*record.self_user_id, "self1");
        assert_eq!(*record.messenger_id, "telegram");
        assert_eq!(*record.group_id, "g1");
        assert_eq!(record.is_self, 1);
        assert!(matches!(record.content, Content::Text(v) if v.as_str() == "hello"));
        assert_eq!(record.sn, 0);
    }
```

- [ ] **Step 5: index.rs 改 channel_indices 泛型与签名**

把 `kissbot-memory/src/index.rs` 顶部：

```rust
use kissbot_api::{QueryChannelRequest, QueryRequest};
```

改为：

```rust
use kissbot_api::QueryRequest;
```

把：

```rust
pub struct MemoryIndexer {
    channel_indices: FileIndexContext<QueryChannelRequest, ChannelRecordKey, ChannelRecord, ChannelParser>,
```

改为：

```rust
pub struct MemoryIndexer {
    channel_indices: FileIndexContext<QueryRequest, RecordKey, ChannelRecord, ChannelParser>,
```

把：

```rust
    pub fn mark_channel_obsolete(&self, key: &ChannelRecordKey) {
        self.channel_indices.mark_obsolete(key);
    }

    pub fn mark_channel_all_obsolete(&self, key: &ChannelRecordKey) {
        self.channel_indices.mark_all_obsolete(key);
    }
```

改为：

```rust
    pub fn mark_channel_obsolete(&self, key: &RecordKey) {
        self.channel_indices.mark_obsolete(key);
    }

    pub fn mark_channel_all_obsolete(&self, key: &RecordKey) {
        self.channel_indices.mark_all_obsolete(key);
    }
```

把：

```rust
    pub async fn query_channel_records(&self, query: QueryChannelRequest) -> Result<Vec<(ChannelRecordKey, Vec<(u32, Arc<ChannelRecord>)>)>> {
        Ok(self.channel_indices.query_all(query).await?)
    }
```

改为：

```rust
    pub async fn query_channel_records(&self, query: QueryRequest) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)>> {
        Ok(self.channel_indices.query_all(query).await?)
    }
```

- [ ] **Step 6: 删除 index.rs 的 query_combos**

删除整个 `query_combos` 方法（含上方 doc 注释）：

```rust
    /// 枚举 <root>/<agent_id>/memory-store/<year>-<role_name>/channel-*.jsonl，
    /// 解析文件名中的 (messenger, user, group) 组合（按文件日期过滤在时间范围内），去重返回
    /// 供 agent 先取组合、再对每个组合用精确 key 时间区间查询（记忆打包流程）
    pub async fn query_combos(&self, query: QueryRequest) -> Result<Vec<ChannelCombo>> {
        use kai_date::as_date;
        // 时间戳短于日期长度时 as_date 切片会 panic——视为无有效范围，直接返回（防御客户端短输入，短路不触文件系统）；
        // 另防多字节输入：长度足够但第 10 字节处于字符中间时切片仍会 panic，同样返回空
        if query.start_time.len() < 10 || query.end_time.len() < 10
            || !query.start_time.is_char_boundary(10) || !query.end_time.is_char_boundary(10) {
            return Ok(Vec::new());
        }
        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(query.agent_id.as_str()).await?;
        let mut combos: Vec<ChannelCombo> = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&store_dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if let Some((_, role)) = dir_name.split_once('-') {
                if role == query.role_name.as_str() && entry.path().is_dir() {
                    let mut year_dir = tokio::fs::read_dir(entry.path()).await?;
                    while let Some(f) = year_dir.next_entry().await? {
                        let fname = f.file_name().to_string_lossy().to_string();
                        if !fname.starts_with("channel-") || !fname.ends_with(".jsonl") { continue; }
                        // 文件名：channel-{m}={u}={g}-records-{yyyy-mm-dd}.jsonl
                        let body = fname.trim_start_matches("channel-");
                        let Some((prefix, date)) = body.rsplit_once("-records-") else { continue; };
                        let date = date.trim_end_matches(".jsonl");
                        if date < as_date(query.start_time.as_str()) || date > as_date(query.end_time.as_str()) {
                            continue;  // 文件日期不在时间范围内
                        }
                        let mut parts = prefix.splitn(3, '=');
                        let (Some(m), Some(u), Some(g)) = (parts.next(), parts.next(), parts.next()) else { continue; };
                        combos.push(ChannelCombo {
                            messenger_id: Arc::new(m.to_string()),
                            user_id: Arc::new(u.to_string()),
                            group_id: Arc::new(g.to_string()),
                        });
                    }
                }
            }
        }
        // 去重（按组合三元组排序后相邻去重）
        combos.sort_by(|a, b| {
            a.messenger_id.as_str().cmp(b.messenger_id.as_str())
                .then(a.user_id.as_str().cmp(b.user_id.as_str()))
                .then(a.group_id.as_str().cmp(b.group_id.as_str()))
        });
        combos.dedup_by(|a, b| {
            a.messenger_id == b.messenger_id && a.user_id == b.user_id && a.group_id == b.group_id
        });
        Ok(combos)
    }
```

同时把主代码顶部 `use std::sync::{Arc, OnceLock};` 改为 `use std::sync::OnceLock;`（query_combos 删除后 Arc 不再被主代码使用；若编译器提示，一并处理）。

- [ ] **Step 7: 更新 index.rs 测试**

`test_mark_and_query_channel` 改为（文件名、key、query 均用公共类型，JSON 记录带新字段）：

```rust
    #[tokio::test]
    async fn test_mark_and_query_channel() {
        let agent_id = "agent1";
        let role_name = "default";
        let date = "2026-06-24";
        let dir = format!("{}-{}", &date[..4], role_name);
        let filename = format!("channel-records-{}.jsonl", date);

        let key = RecordKey {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            date: Arc::new(date.to_string()),
        };
        let query_range = |s: &str, e: &str| QueryRequest {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            start_time: Arc::new(format!("{} {}", date, s)),
            end_time: Arc::new(format!("{} {}", date, e)),
        };

        // timeline: 00:00:00 < A(08:00) < start(09:00) < B(10:00) < C(11:00) < end(13:00) < F(14:00)
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"A"},"time":"2026-06-24 08:00:00","sn":1}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"B"},"time":"2026-06-24 10:00:00","sn":2}"#).await;

        let indexer = MemoryIndexer::new();
        // query range excludes A, includes B
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 1);
        assert!(matches!(results[0].1[0].1.content.clone(), Content::Text(v) if v.as_str() == "B"));

        // write C (in range) after MemoryIndexer created
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"C"},"time":"2026-06-24 11:00:00","sn":3}"#).await;

        // mark + query — incremental load picks up C
        indexer.mark_channel_obsolete(&key);
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 2);
        assert!(matches!(results[0].1[0].1.content.clone(), Content::Text(v) if v.as_str() == "B"));
        assert!(matches!(results[0].1[1].1.content.clone(), Content::Text(v) if v.as_str() == "C"));

        // delete file, write D(before start), E(in range), F(after end)
        let store_dir = crate::DirectoryManager::get().ensure_agent_store_dir(agent_id).await.unwrap();
        let file_path = store_dir.join(&dir).join(&filename);
        tokio::fs::remove_file(&file_path).await.unwrap();
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"D"},"time":"2026-06-24 08:30:00","sn":4}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"E"},"time":"2026-06-24 10:30:00","sn":5}"#).await;
        append_jsonl(agent_id, role_name, &filename, date,
            r#"{"user_id":"u1","self_user_id":"self1","messenger_id":"telegram","group_id":"g1","is_self":0,"messenger_name":"","user_name":"","group_name":"","content":{"msg_type":"Text","data":"F"},"time":"2026-06-24 14:00:00","sn":6}"#).await;

        // mark_all — full rebuild, only E in range
        indexer.mark_channel_all_obsolete(&key);
        let results = indexer.query_channel_records(query_range("09:00:00", "13:00:00")).await.unwrap();
        assert_eq!(results[0].1.len(), 1);
        assert!(matches!(results[0].1[0].1.content.clone(), Content::Text(v) if v.as_str() == "E"));
    }
```

删除以下三个测试：`test_query_combos_enumerates_channel_files`、`test_query_combos_short_time_input_returns_empty`、`test_query_combos_multibyte_time_input_returns_empty`（含"组合枚举"注释块）。

- [ ] **Step 8: 运行 kissbot-memory 测试**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-memory`
Expected: PASS（本 crate 独立编译成功；kissbot-memory-store 仍编译失败属预期，Task 4 恢复）

- [ ] **Step 9: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-memory/src/data.rs kissbot-memory/src/index.rs && git commit -m "refactor(memory): ChannelParser 与索引改用公共 RecordKey/QueryRequest——channel 记录合并为单文件 channel-records-{date}.jsonl，record 保存 messenger_id/group_id/self_user_id 完整身份，删除 query_combos 组合枚举及测试"
```

---

### Task 4: kissbot-memory-store API 与 FileHook 适配

**Files:**
- Modify: `kissbot-memory-store/src/api.rs`（channel 查询改 QueryRequest、删 combos 路由/handler）
- Modify: `kissbot-memory-store/src/record.rs`（ChannelFileIndexHook 改 FileHook<RecordKey>、ChannelAppender 泛型、测试文件名与 key 类型）

**Interfaces:**
- Consumes: Task 3 的 `MemoryIndexer::query_channel_records(QueryRequest)` 与 `mark_channel_obsolete(&RecordKey)` 签名
- Produces: `/store/query/channel` 接受 `QueryRequest`；`/store/query/combos` 路由删除；`ChannelAppender = RecordAppender<RecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>`。本任务完成后 `cargo build --workspace` 恢复绿色

- [ ] **Step 1: api.rs 的 channel 查询 handler 改 QueryRequest**

把 `kissbot-memory-store/src/api.rs` 中的：

```rust
async fn query_channel_records(Json(req): Json<memory::QueryChannelRequest>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_channel_records(req).await;
    match records {
        Ok(records) => (StatusCode::OK, Json(ApiResponse::success(records))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
```

改为：

```rust
async fn query_channel_records(Json(req): Json<memory::QueryRequest>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_channel_records(req).await;
    match records {
        Ok(records) => (StatusCode::OK, Json(ApiResponse::success(records))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
```

- [ ] **Step 2: api.rs 删除 combos 路由与 handler**

删除路由行：

```rust
        .route("/store/query/combos", post(query_combos))
```

删除 handler（含注释）：

```rust
/// 查询 (agent, role, 时间范围) 对应的 channel 记录组合（messenger + user + group），
/// agent 先取组合、再对每个组合用 /store/query/channel 精确查询（记忆打包流程）
async fn query_combos(Json(req): Json<memory::QueryRequest>) -> impl IntoResponse {
    let combos = MemoryIndexer::get().query_combos(req).await;
    match combos {
        Ok(combos) => (StatusCode::OK, Json(ApiResponse::success(combos))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
```

- [ ] **Step 3: record.rs 的 ChannelFileIndexHook 改 RecordKey**

把 `kissbot-memory-store/src/record.rs` 中的：

```rust
#[derive(Clone)]
struct ChannelFileIndexHook;

impl FileHook<ChannelRecordKey> for ChannelFileIndexHook {
    fn on_append(&self, key: &ChannelRecordKey) {
        MemoryIndexer::get().mark_channel_obsolete(key);
    }

    fn on_force_append(&self, key: &ChannelRecordKey) {
        MemoryIndexer::get().mark_channel_all_obsolete(key);
    }
}
```

改为：

```rust
#[derive(Clone)]
struct ChannelFileIndexHook;

impl FileHook<RecordKey> for ChannelFileIndexHook {
    fn on_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_channel_obsolete(key);
    }

    fn on_force_append(&self, key: &RecordKey) {
        MemoryIndexer::get().mark_channel_all_obsolete(key);
    }
}
```

把：

```rust
type ChannelAppender = RecordAppender<ChannelRecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>;
```

改为：

```rust
type ChannelAppender = RecordAppender<RecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>;
```

- [ ] **Step 4: record.rs 测试改 key 类型与文件名**

以下测试中所有 `HashMap<ChannelRecordKey, Vec<ChannelRecord>>` 改为 `HashMap<RecordKey, Vec<ChannelRecord>>`：
`test_append_new_file`、`test_append_multiple_records`、`test_append_sequential`（两处）、`test_append_multiple_keys`。

以下测试中所有期望路径的文件名 `channel-telegram=self1=g1-records-2026-06-25.jsonl` 改为 `channel-records-2026-06-25.jsonl`：
`test_append_new_file`、`test_append_multiple_records`、`test_append_sequential`、`test_append_force_out_of_order`、`test_append_force_with_existing_data`。

`test_append_multiple_keys` 中两个期望路径改为：

```rust
        // 验证两个文件都存在（不同 agent_id → 不同 RecordKey → 不同目录），sn 各自从 1 开始
        let path1 = root
            .join("test_mk_a")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");
        let path2 = root
            .join("test_mk_b")
            .join("memory-store")
            .join("2026-default")
            .join("channel-records-2026-06-25.jsonl");
```

- [ ] **Step 5: 运行测试与 workspace 构建**

Run: `cd /home/admin/project/kissbot && cargo test -p kissbot-memory-store`
Expected: PASS

Run: `cd /home/admin/project/kissbot && cargo build --workspace`
Expected: 编译成功（所有 crate 恢复绿色；若出现未使用导入等 warning 按编译器提示清理）

- [ ] **Step 6: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-memory-store/src/api.rs kissbot-memory-store/src/record.rs && git commit -m "refactor(memory-store): channel 查询改用公共 QueryRequest，删除 /store/query/combos 路由与 handler；ChannelFileIndexHook/ChannelAppender 改用 RecordKey；测试改单文件结构（channel-records-{date}.jsonl）"
```

---

### Task 5: 文档更新

**Files:**
- Modify: `docs/spec/memory-directory.md`（目录结构、文件格式表、ChannelRecord 字段与 is_self/self_user_id 语义）
- Verify（如无组合/专用类型引用则不改）：`docs/spec/memory-index.md`、`docs/spec/memory-store.md`、`docs/design/components-design/kissbot-memory.md`、`docs/design/components-design/kissbot-memory-store.md`、`docs/plan/components-plan/kissbot-memory-store.md`、`docs/spec/channel-message.md`

**Interfaces:**
- Consumes: 前 4 个任务已落地的新行为
- Produces: 项目文档与实现一致；`docs/spec/memory-directory.md` 明确记录 is_self 与 self_user_id 的不同语义

- [ ] **Step 1: 更新 memory-directory.md 目录结构**

把 `docs/spec/memory-directory.md` 中：

```
│   │       ├── channel-{messenger_id}={user_id}={group_id}-records-{date}.jsonl
```

改为：

```
│   │       ├── channel-records-{date}.jsonl
```

- [ ] **Step 2: 更新 memory-directory.md 文件格式表**

把：

```
| channel-{messenger_id}={user_id}={group_id}-records-{date}.jsonl | 消息通道的文本记录（按 messenger、user、group 和时间组织） |
```

改为：

```
| channel-records-{date}.jsonl | 消息通道的文本记录（同 agent、角色、日期下全部通道消息；记录内携带 messenger/group/self 身份） |
```

- [ ] **Step 3: 在 memory-directory.md 增加 ChannelRecord 字段与身份语义小节**

在"文件格式"表格之后追加：

```markdown
## ChannelRecord 身份字段与语义

channel 记录包含完整身份字段：

| 字段 | 含义 |
|------|------|
| user_id | 消息发送者身份 |
| self_user_id | agent 在 channel 绑定的用户（接收方身份 / agent 视角的 self） |
| messenger_id | 消息来源通道标识 |
| group_id | 群组标识（单聊可为空） |
| is_self | 是否 agent 实际发送（1 / 0） |

> **is_self 与 self_user_id 不同**：is_self 只在 agent 实际发送消息时为 1（agent 经 msg_id 回显匹配识别并过滤自身回声后确定），不由 `user_id == self_user_id` 推导。其他人使用 agent 绑定的用户发送消息时，`user_id == self_user_id`，但 `is_self == 0`。
```

- [ ] **Step 4: 检查其余文档是否需同步**

Run: `cd /home/admin/project/kissbot && grep -rn "channel-{\|combos\|ChannelCombo\|QueryChannelRequest\|ChannelRecordKey" docs/spec docs/design/components-design docs/plan/components-plan`
Expected: 无输出（说明其余文档无组合/专用类型引用，无需改动）；如有命中，把命中处改为对应的新表述（组合文件 → `channel-records-{date}.jsonl`、组合枚举 → 单次全史查询）。

- [ ] **Step 5: 全量测试与提交**

Run: `cd /home/admin/project/kissbot && cargo test --workspace`
Expected: PASS（全部 crate 测试通过）

```bash
cd /home/admin/project/kissbot && git add docs/spec/memory-directory.md && git commit -m "docs(memory): 目录结构与文件格式表更新——channel 记录合并为 channel-records-{date}.jsonl，新增 ChannelRecord 身份字段说明（user_id/self_user_id/messenger_id/group_id）及 is_self 与 self_user_id 语义（is_self 仅 agent 实际发送为 1，经 msg_id 回显匹配，不由身份相等推导）"
```

---

## Self-Review（计划自审）

**Spec 覆盖对照：**
- §1 文件与记录类型 → Task 1（api 类型）+ Task 3（data.rs/index.rs）
- §2 查询 API → Task 4（api.rs）
- §3 agent 查询简化 → Task 2（memory_reader.rs）
- §4 is_self 与 self_user_id 语义 → Task 1（结构体注释）+ Task 5（文档）
- §5 测试与文档 → 各任务内置测试步骤 + Task 5
- YAGNI 声明（不迁移、不统一泛型、无别名）→ Global Constraints

**占位符扫描：** 无 TBD/TODO；每个代码步骤含完整代码。

**类型一致性：** `RecordKey`/`QueryRequest`/`ChannelRecord`（含新字段）在 Task 1 定义、Task 2/3/4 消费，签名一致；`query_channel_records(QueryRequest)` 在 Task 3 定义、Task 4 调用；`mark_channel_obsolete(&RecordKey)` 在 Task 3 定义、Task 4 调用；文件名统一为 `channel-records-{date}.jsonl`。
