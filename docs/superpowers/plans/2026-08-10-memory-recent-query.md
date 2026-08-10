# memory-store 最近 N 条查询 + agent 两次查询并集 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** memory-store 增加 ChannelRecord 的"最近 N 条"查询（无时间参数），agent 的 `read_recent_for_context` 改为两次查询（最近 N + [M, ln] 时间段）取并集。

**Architecture:** kissbot-api 新增 `RecentQuery`；kissbot-memory 维护 channel 日期缓存（懒加载扫描 + append 钩子增量），`query_channel_recent` 跨日期文件倒序取尾部（复用 kai-file `query_last`，参考 channel-web `get_recent`）；kissbot-memory-store 新增 `/store/query/channel/recent` 端点；agent 先查 recent(count) 再查范围 [M, ln]（M = min(cutoff, ln)），两次结果共用 (time, sn) seen 集解析实现并集去重。

**Tech Stack:** Rust、tokio、axum 0.7、dashmap 6.1、kai-file（`FileIndexContext::query_last`）、reqwest。

## Global Constraints

- **不要删除代码中的注释**（项目 CLAUDE.md 铁律）——改注释用更新内容，不删行。
- 读写文件必须使用 Read/Write/Edit 工具，禁止 sed/python 修改文件。
- 文本文件 UTF-8、`\n` 换行。
- git commit comment 用中文，包含该次提交所有改动。
- TDD：每个功能任务先写失败测试，确认失败后再实现。
- `QueryRequest` 保持 `{agent_id, role_name, start_time, end_time}` 不变，**不加 limit**（此前规划误加，去掉）。
- `RecentQuery` 无时间参数；Think/ToolCall/ToolResult 及其他 limit 功能**一律不加**。
- 并集公式：`结果 = 最近 N 条 ∪ [M, ln] 时间段全部记录`，`M = min(时间窗起点, ln)`；两次查询结果共用 (time, sn) seen 集解析（自动去重）后按 time 升序。
- date_sets 懒加载（首次 recent 查询前扫描一次），不做启动扫描。
- 每个 crate 独立（无根 workspace）：`cd kissbot-api && cargo test`、`cd kissbot-memory && cargo test`、`cd kissbot-memory-store && cargo test`、`cd kissbot-agent && cargo test`。

---

### Task 1: kissbot-api 新增 RecentQuery

**Files:**
- Modify: `kissbot-api/src/memory.rs`（QueryRequest 定义后新增 RecentQuery）
- Test: `kissbot-api/src/memory.rs` 的 `#[cfg(test)] mod tests` 模块

**Interfaces:**
- Produces: `kissbot_api::memory::RecentQuery { agent_id: Arc<String>, role_name: Arc<String>, count: u32 }`（Serialize/Deserialize/Clone/Debug）——Task 3（store 端点）、Task 4（agent 请求体）依赖。

- [ ] **Step 1: 写失败测试**

在 `kissbot-api/src/memory.rs` 的 `mod tests` 里（`test_serde_channel_request` 旁边）追加：

```rust
    #[test]
    fn test_serde_recent_query() {
        let obj = RecentQuery {
            agent_id: Arc::new("a".into()),
            role_name: Arc::new("r".into()),
            count: 5,
        };
        let s = serde_json::to_string(&obj).unwrap();
        assert_eq!(s, r#"{"agent_id":"a","role_name":"r","count":5}"#);
        let back: RecentQuery = serde_json::from_str(&s).unwrap();
        assert_eq!(back.count, 5);
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-api && cargo test test_serde_recent_query`
Expected: 编译失败——`cannot find type RecentQuery in this scope`

- [ ] **Step 3: 实现 RecentQuery**

在 `kissbot-api/src/memory.rs` 的 `QueryRequest` 结构体（第 83-88 行）之后追加：

```rust
/// 最近 N 条 channel 记录查询（无时间参数：取该 agent+role 最近 count 条，跨日期文件）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentQuery {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub count: u32,
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cd kissbot-api && cargo test`
Expected: PASS（含新测试与原有测试）

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-api/src/memory.rs
git commit -m "feat(api): 新增 RecentQuery（最近 N 条 channel 查询，无时间参数）+ serde 测试"
```

---

### Task 2: kissbot-memory 最近 N 条查询核心

**Files:**
- Modify: `kissbot-memory/src/directory.rs`（新增 `root_dir()` 访问器）
- Modify: `kissbot-memory/src/index.rs`（date_sets 字段、懒加载扫描、query_channel_recent、mark 钩子插入日期）
- Test: `kissbot-memory/src/index.rs` 的 `#[cfg(test)] mod tests` 模块

**Interfaces:**
- Consumes: `kai_file::FileIndexContext::query_last(&key: &RecordKey, n: u32) -> Result<Vec<(u32, Arc<ChannelRecord>)>>`（组内升序）；`kissbot_api::memory::RecordKey`。
- Produces:
  - `MemoryIndexer::query_channel_recent(&self, agent_id: &str, role_name: &str, count: u32) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)>>`（组按日期升序、组内升序）——Task 3 端点调用。
  - `MemoryIndexer::mark_channel_obsolete` / `mark_channel_all_obsolete` 行为扩展：同时把 `key.date` 插入 date_sets（Task 3 的 append 钩子依赖）。
  - `DirectoryManager::root_dir(&self) -> &Path`。

- [ ] **Step 1: 写失败测试**

在 `kissbot-memory/src/index.rs` 的 `mod tests` 里追加两个测试与一个辅助函数（tests 模块已有 `use super::*;`、`use std::sync::{Arc, Once, OnceLock};`、`use kissbot_api::Content;`、`append_jsonl` 辅助）：

```rust
    // 提取 ChannelRecord 的 Text 内容（测试断言用）
    fn text_of(r: &Arc<ChannelRecord>) -> String {
        match &r.content {
            Content::Text(t) => t.as_str().to_string(),
            _ => String::new(),
        }
    }

    #[tokio::test]
    async fn test_query_channel_recent_cross_files() {
        init_test_config();
        // 用独立 agent id 避免与其他测试共享全局 temp root 时相互污染
        let agent_id = "recent_agent";
        let role_name = "r1";
        let rec = |time: &str, text: &str| format!(
            r#"{{"user_id":"u1","self_user_id":"self1","messenger_id":"web","group_id":"g1","is_self":0,"messenger_name":"","user_name":"u","group_name":"","content":{{"msg_type":"Text","data":"{}"}},"time":"{}","sn":1}}"#,
            text, time);
        append_jsonl(agent_id, role_name, "channel-records-2026-08-01.jsonl", "2026-08-01", &rec("2026-08-01 09:00:00", "a")).await;
        append_jsonl(agent_id, role_name, "channel-records-2026-08-01.jsonl", "2026-08-01", &rec("2026-08-01 10:00:00", "b")).await;
        append_jsonl(agent_id, role_name, "channel-records-2026-08-02.jsonl", "2026-08-02", &rec("2026-08-02 09:00:00", "c")).await;
        append_jsonl(agent_id, role_name, "channel-records-2026-08-03.jsonl", "2026-08-03", &rec("2026-08-03 09:00:00", "d")).await;
        append_jsonl(agent_id, role_name, "channel-records-2026-08-03.jsonl", "2026-08-03", &rec("2026-08-03 10:00:00", "e")).await;

        let indexer = MemoryIndexer::new();
        // 懒加载扫描 → 跨文件取最近 3 条（c, d, e），按时间升序
        let results = indexer.query_channel_recent(agent_id, role_name, 3).await.unwrap();
        let flat: Vec<String> = results.iter().flat_map(|(_, v)| v.iter()).map(|(_, r)| text_of(r)).collect();
        assert_eq!(flat, vec!["c", "d", "e"], "跨日期文件取最近 3 条");

        // count 超总量 → 全部（按时间升序）
        let results = indexer.query_channel_recent(agent_id, role_name, 100).await.unwrap();
        let flat: Vec<String> = results.iter().flat_map(|(_, v)| v.iter()).map(|(_, r)| text_of(r)).collect();
        assert_eq!(flat, vec!["a", "b", "c", "d", "e"], "count 超总量返回全部");

        // count == 0 → 空
        assert!(indexer.query_channel_recent(agent_id, role_name, 0).await.unwrap().is_empty(), "count=0 返回空");
    }

    #[tokio::test]
    async fn test_query_channel_recent_incremental_after_obsolete() {
        init_test_config();
        let agent_id = "recent_agent2";
        let role_name = "r1";
        let rec = |time: &str, text: &str| format!(
            r#"{{"user_id":"u1","self_user_id":"self1","messenger_id":"web","group_id":"g1","is_self":0,"messenger_name":"","user_name":"u","group_name":"","content":{{"msg_type":"Text","data":"{}"}},"time":"{}","sn":1}}"#,
            text, time);
        append_jsonl(agent_id, role_name, "channel-records-2026-08-01.jsonl", "2026-08-01", &rec("2026-08-01 09:00:00", "a")).await;

        let indexer = MemoryIndexer::new();
        let out = indexer.query_channel_recent(agent_id, role_name, 10).await.unwrap();
        assert_eq!(out.iter().map(|(_, v)| v.len()).sum::<usize>(), 1);

        // 新日期文件 + mark_channel_obsolete（append 钩子路径）→ date_sets 增量补入
        append_jsonl(agent_id, role_name, "channel-records-2026-08-02.jsonl", "2026-08-02", &rec("2026-08-02 09:00:00", "b")).await;
        let key = RecordKey {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            date: Arc::new("2026-08-02".to_string()),
        };
        indexer.mark_channel_obsolete(&key);
        let out = indexer.query_channel_recent(agent_id, role_name, 10).await.unwrap();
        let flat: Vec<String> = out.iter().flat_map(|(_, v)| v.iter()).map(|(_, r)| text_of(r)).collect();
        assert_eq!(flat, vec!["a", "b"], "mark_channel_obsolete 后新日期可查");
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-memory && cargo test test_query_channel_recent`
Expected: 编译失败——`no method named query_channel_recent found` / `no method named root_dir found`

- [ ] **Step 3: 实现**

**(a) `kissbot-memory/src/directory.rs`**——在 `DirectoryManager` impl 里（`get()` 附近）加访问器：

```rust
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }
```

**(b) `kissbot-memory/src/index.rs`**——修改 imports（顶部）：

```rust
use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use kissbot_api::QueryRequest;
use tokio::sync::OnceCell;

use crate::data::{ChannelParser, ThinkParser, ToolCallParser, ToolResultParser};
use crate::error::Result;
use crate::DirectoryManager;
use kai_file::FileIndexContext;

use kissbot_api::memory::*;
```

（把现有 `use kissbot_api::QueryRequest;` / `use std::sync::{Arc, OnceLock};` 整理为上述顺序，新增 BTreeSet/DashMap/OnceCell/DirectoryManager。）

**(c) `MemoryIndexer` 结构体加字段**：

```rust
pub struct MemoryIndexer {
    channel_indices: FileIndexContext<QueryRequest, RecordKey, ChannelRecord, ChannelParser>,
    think_indices: FileIndexContext<QueryRequest, RecordKey, ThinkRecord, ThinkParser>,
    tool_call_indices: FileIndexContext<QueryRequest, RecordKey, ToolCallRecord, ToolCallParser>,
    tool_result_indices: FileIndexContext<QueryRequest, RecordKey, ToolResultRecord, ToolResultParser>,
    /// channel 文件日期缓存：(agent_id, role_name) → 已存在的日期（懒加载扫描 + append 钩子增量维护）
    channel_date_sets: DashMap<(String, String), BTreeSet<String>>,
    /// date_sets 懒加载守卫（首次 recent 查询前扫描一次存量文件）
    channel_dates_loaded: OnceCell<()>,
}
```

**(d) `new()` 初始化新字段**：

```rust
    pub fn new() -> Self {
        Self {
            channel_indices: FileIndexContext::new(ChannelParser {}),
            think_indices: FileIndexContext::new(ThinkParser {}),
            tool_call_indices: FileIndexContext::new(ToolCallParser {}),
            tool_result_indices: FileIndexContext::new(ToolResultParser {}),
            channel_date_sets: DashMap::new(),
            channel_dates_loaded: OnceCell::new(),
        }
    }
```

**(e) `mark_channel_obsolete` / `mark_channel_all_obsolete` 插入日期**（append/全量重写后该日期的 channel 文件必然存在）：

```rust
    pub fn mark_channel_obsolete(&self, key: &RecordKey) {
        self.channel_indices.mark_obsolete(key);
        // date_sets 增量维护：append 后该日期的 channel 文件必然存在（与懒加载扫描幂等）
        self.channel_date_sets.entry((key.agent_id.as_str().to_string(), key.role_name.as_str().to_string()))
            .or_default().insert(key.date.as_str().to_string());
    }

    pub fn mark_channel_all_obsolete(&self, key: &RecordKey) {
        self.channel_indices.mark_all_obsolete(key);
        // 全量重写后文件仍存在（日期不变），同样补入 date_sets
        self.channel_date_sets.entry((key.agent_id.as_str().to_string(), key.role_name.as_str().to_string()))
            .or_default().insert(key.date.as_str().to_string());
    }
```

**(f) 新增 `query_channel_recent` 与 `scan_channel_dates`**（放在 `query_channel_records` 方法后面）：

```rust
    /// 最近 N 条 channel 记录（跨日期文件，参考 channel-web message_store::get_recent）：
    /// date_sets 日期倒序逐个 query_last(remaining)，取满即停；组按日期升序返回、组内升序；无时间过滤
    pub async fn query_channel_recent(&self, agent_id: &str, role_name: &str, count: u32) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        // 懒加载：首次 recent 查询前扫描一次存量 channel 文件（此后由 mark_channel_obsolete/all_obsolete 增量维护）
        // 扫描为尽力而为：失败仅影响存量发现，增量 append 仍可用，故忽略结果
        let _ = self.channel_dates_loaded.get_or_init(|| async {
            let _ = self.scan_channel_dates().await;
        }).await;

        let mut remaining = count;
        let mut results: Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)> = Vec::new();
        if let Some(dates) = self.channel_date_sets.get(&(agent_id.to_string(), role_name.to_string())) {
            for date in dates.iter().rev() {  // 最新日期在前
                if remaining == 0 { break; }
                let key = RecordKey {
                    agent_id: Arc::new(agent_id.to_string()),
                    role_name: Arc::new(role_name.to_string()),
                    date: Arc::new(date.clone()),
                };
                let msgs = self.channel_indices.query_last(&key, remaining).await?;
                if !msgs.is_empty() {
                    remaining -= msgs.len() as u32;
                    results.push((key, msgs));
                }
            }
        }
        results.reverse();  // 日期倒序收集 → 升序返回（与 query_all 一致）
        Ok(results)
    }

    /// 扫描存量 channel 文件填充 date_sets：枚举 <root>/<agent_id>/memory-store/<year>-<role_name>/channel-records-<date>.jsonl
    async fn scan_channel_dates(&self) -> Result<()> {
        let root = DirectoryManager::get().root_dir().to_path_buf();
        let mut agent_entries = tokio::fs::read_dir(&root).await?;
        while let Some(agent_entry) = agent_entries.next_entry().await? {
            if !agent_entry.path().is_dir() { continue; }
            let Some(agent_id) = agent_entry.file_name().to_str().map(String::from) else { continue; };
            // 仅统计有 uuid 文件的真实 agent（与 DirectoryManager::list_agents 一致）
            if !agent_entry.path().join(format!("agent-{}", agent_id)).exists() { continue; }
            let store_dir = agent_entry.path().join("memory-store");
            let mut year_entries = match tokio::fs::read_dir(&store_dir).await {
                Ok(d) => d,
                Err(_) => continue,  // 无 store 目录 → 跳过该 agent
            };
            while let Some(year_entry) = year_entries.next_entry().await? {
                if !year_entry.path().is_dir() { continue; }
                // year-role 目录形如 "2026-default"；role = 去掉 "YYYY-" 前缀
                let year_name = year_entry.file_name().to_string_lossy().to_string();
                let Some(role_name) = year_name.get(5..).map(String::from) else { continue; };
                let mut file_entries = tokio::fs::read_dir(year_entry.path()).await?;
                while let Some(file_entry) = file_entries.next_entry().await? {
                    let name = file_entry.file_name().to_string_lossy().to_string();
                    if let Some(date) = name.strip_prefix("channel-records-").and_then(|n| n.strip_suffix(".jsonl")) {
                        self.channel_date_sets.entry((agent_id.clone(), role_name.clone()))
                            .or_default().insert(date.to_string());
                    }
                }
            }
        }
        Ok(())
    }
```

- [ ] **Step 4: 运行确认通过**

Run: `cd kissbot-memory && cargo test`
Expected: PASS（新测试 + 原有 test_mark_and_query_channel/think/tool_call/tool_result 全部通过）

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-memory/src/directory.rs kissbot-memory/src/index.rs
git commit -m "feat(memory): query_channel_recent 最近 N 条跨日期查询——date_sets 懒加载扫描 + append 钩子增量 + query_last 尾部读取"
```

---

### Task 3: kissbot-memory-store 新增 /store/query/channel/recent 端点

**Files:**
- Modify: `kissbot-memory-store/src/api.rs`

**Interfaces:**
- Consumes: `kissbot_api::memory::RecentQuery`（Task 1）、`MemoryIndexer::query_channel_recent`（Task 2）。
- Produces: `POST /store/query/channel/recent`，body `RecentQuery`，响应 `ApiResponse<Vec<(RecordKey, Vec<(u32, ChannelRecord)>)>>`（与 `/store/query/channel` 同构）——Task 4 agent 调用。

- [ ] **Step 1: 加端点**

在 `kissbot-memory-store/src/api.rs`：

路由注册（`create_router` 里 `/store/query/channel` 后面加一行）：

```rust
        .route("/store/query/channel/recent", post(query_channel_recent_records))
```

handler（放在 `query_channel_records` 函数后面）：

```rust
async fn query_channel_recent_records(Json(req): Json<memory::RecentQuery>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_channel_recent(&req.agent_id, &req.role_name, req.count).await;
    match records {
        Ok(records) => (StatusCode::OK, Json(ApiResponse::success(records))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
```

- [ ] **Step 2: 编译确认**

Run: `cd kissbot-memory-store && cargo check && cargo test`
Expected: 编译通过、测试通过（本 crate 无 api 端点测试；逻辑由 Task 2 的 kissbot-memory 单测覆盖，端点线格式由 Task 4 的 agent mock 集成测试验证——薄透传层，不重复造服务器测试）

- [ ] **Step 3: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-memory-store/src/api.rs
git commit -m "feat(memory-store): 新增 /store/query/channel/recent 端点（RecentQuery → query_channel_recent 透传）"
```

---

### Task 4: kissbot-agent read_recent_for_context 两次查询并集

**Files:**
- Modify: `kissbot-agent/src/memory_reader.rs`（read_recent_for_context 重写 + parse_channel_groups 辅助 + imports + 测试模块 mock 与用例重写）

**Interfaces:**
- Consumes: `kissbot_api::memory::RecentQuery`（Task 1）、`/store/query/channel/recent` 与 `/store/query/channel`（Task 3）、现有 `QueryRequest`。
- Produces: `read_recent_for_context` 保持签名 `(&self, agent_id: &str, role_name: &str, cfg: &EffectiveContextConfig) -> Result<Vec<MemoryMsg>>`，行为改为两次查询并集（coordinator 调用点不变）。

**算法（务必按此实现）：**

```
Query1 = recent(count) → 解析（(time, sn) 去重）→ 升序
  空 或 count==0 → 空；recent_raw < count（Query1 返回的原始记录数，含非文本）→ 直接返回
ln = Query1 原始记录最旧一条的 time（非文本记录也算边界！）
M = min(时间窗起点 start, ln)
Query2 = 范围 [M, ln]（无 limit）→ 用同一 seen 集解析（并集去重）→ 升序
```

- [ ] **Step 1: 重写测试（预期先失败）**

**(a) 重写 `start_mock_store` 为双端点**（替换现有函数）：

```rust
    // 启动本地 mock memory-store：
    // /store/query/channel/recent —— 按 (time, sn) 排序取后 count 条（无时间过滤，与真实 store 一致）
    // /store/query/channel —— 按 [start_time, end_time] 过滤（无 limit，与真实 store 一致）
    async fn start_mock_store(data: serde_json::Value) -> String {
        // 展平所有组为 [(sn, record)]，模拟 store 内部记录流
        let all: Vec<(u64, serde_json::Value)> = data.as_array().unwrap().iter()
            .flat_map(|group| group[1].as_array().unwrap().iter()
                .map(|entry| (entry[0].as_u64().unwrap(), entry[1].clone())))
            .collect();
        let recent_all = all.clone();
        let range_all = all.clone();
        let app = Router::new()
            .route("/store/query/channel/recent", post(move |Json(req): Json<RecentQuery>| async move {
                let mut v = recent_all.clone();
                v.sort_by(|a, b| a.1["time"].as_str().cmp(&b.1["time"].as_str()).then(a.0.cmp(&b.0)));
                let tail: Vec<_> = v.into_iter().rev().take(req.count as usize).collect();
                let entries: Vec<_> = tail.into_iter().rev().map(|(sn, rec)| json!([sn, rec])).collect();
                Json(json!({ "success": true, "data": [[{ "agent_id": req.agent_id, "role_name": req.role_name, "date": "2026-08-05" }, entries]], "error": null }))
            }))
            .route("/store/query/channel", post(move |Json(req): Json<QueryRequest>| async move {
                let entries: Vec<_> = range_all.iter()
                    .filter(|(_, r)| {
                        let t = r["time"].as_str().unwrap();
                        t >= req.start_time.as_str() && t <= req.end_time.as_str()
                    })
                    .map(|(sn, rec)| json!([sn, rec.clone()]))
                    .collect();
                Json(json!({ "success": true, "data": [[{ "agent_id": req.agent_id, "role_name": req.role_name, "date": "2026-08-05" }, entries]], "error": null }))
            }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{}", addr)
    }
```

**(b) 重写 `read_recent_dense_keeps_order_and_same_time_group`**（新语义：cutoff ≤ ln → 窗口内全部）：

```rust
    #[tokio::test]
    async fn read_recent_window_covers_ln_returns_all_window_records() {
        // cutoff ≤ ln（窗口覆盖最后 N 边界）：M = cutoff → Query2 = [cutoff, ln] 整段 → 并集 = 窗口内全部记录
        // 30 条全部落在窗口内（now-1800 ~ now-60），count=10 → 结果 = 全部 30 条（m0..m29），时间正序
        let records: Vec<serde_json::Value> = (0..30)
            .map(|i| record_json(&time_ago(1800 - i * 60), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        let cfg = ctx_config(7200, 10);  // cutoff=now-7200 早于 ln=now-600 → M = cutoff
        let out = reader_at(&url).read_recent_for_context("agent", "role", &cfg).await.unwrap();
        assert_eq!(out.len(), 30, "窗口覆盖 ln 时结果 = 窗口内全部记录");
        assert_eq!(out[0].content, "m0");
        assert_eq!(out[29].content, "m29");
        assert!(out.windows(2).all(|w| w[0].time <= w[1].time), "时间正序");
    }
```

**(c) 更新 `read_recent_sparse_extends_beyond_window` 注释与断言文案**（断言不变：结果仍是 10 条 m5..m14；Query2 = [ln, ln] 单点只取回 m5，并集后仍 10 条）：

```rust
    #[tokio::test]
    async fn read_recent_sparse_extends_beyond_window() {
        // cutoff > ln（最后 N 条全在窗口内，ln 早于窗口起点）：M = min(cutoff, ln) = ln →
        // Query2 退化为单点 [ln, ln]（ln 同时间组），并集后仍为最后 N 条（跨更早时间）
        let records: Vec<serde_json::Value> = (0..15)
            .map(|i| record_json(&time_ago(600 - i * 20), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        let cfg = ctx_config(60, 10);  // 窗口起点 now-60s 晚于全部记录 → M = ln
        let out = reader_at(&url).read_recent_for_context("agent", "role", &cfg).await.unwrap();
        assert_eq!(out.len(), 10, "稀疏场景结果 = 最后 10 条（跨更早时间）");
        assert_eq!(out[0].content, "m5", "起点 = 第 6 条（最后 10 条最旧一条）");
    }
```

**(d) 更新 `read_recent_includes_same_time_group_beyond_n`**（cutoff 7200 → 250，使测试落在"窗口覆盖但未全覆盖"分支，断言不变）：

```rust
    #[tokio::test]
    async fn read_recent_includes_same_time_group_beyond_n() {
        // 同时间组横跨 N 边界：索引 4,5,6 同在 t1=now-200s，count=10 → Query1 = 索引 5..14，ln = t1；
        // cutoff=now-250 介于 t0 与 t1 之间 → M = cutoff → Query2 = [now-250, t1] 整段（含完整 t1 组）
        // → 并集 = 索引 4..14 共 11 条（m4 在 start_idx 之前但同 ln 时间，被取回）
        let t0 = time_ago(300);
        let t1 = time_ago(200);
        let t2 = time_ago(100);
        let mut records = Vec::new();
        for i in 0..4 { records.push(record_json(&t0, "u", &format!("m{}", i))); }
        for i in 4..7 { records.push(record_json(&t1, "u", &format!("m{}", i))); }
        for i in 7..15 { records.push(record_json(&t2, "u", &format!("m{}", i))); }
        let url = start_mock_store(channel_data(records)).await;
        let cfg = ctx_config(250, 10);  // cutoff=now-250 介于 t0 与 t1 之间 → M = cutoff
        let out = reader_at(&url).read_recent_for_context("agent", "role", &cfg).await.unwrap();
        assert_eq!(out.len(), 11, "ln 同时间组完整保留（含 start_idx 之前同时间记录）");
        assert_eq!(out[0].time, t1, "起点 = ln 同时间组");
        assert_eq!(out[0].content, "m4", "start_idx 之前的同时间记录被并集取回");
        assert!(out.windows(2).all(|w| w[0].time <= w[1].time), "时间正序");
    }
```

**(e) 新增并集去重测试**（追加在 dedup 测试后面）：

```rust
    #[tokio::test]
    async fn read_recent_unions_both_queries_and_dedups() {
        // 两次查询并集去重：Query2 区间 [M, ln] 与 Query1 尾部在 ln 处重叠（m2 两查询都有）
        // 12 条 m0..m11 时间 now-1200+i*60，count=10 → Query1 = m2..m11，ln = m2 的时间 now-1080；
        // cutoff=now-1800 ≤ ln → M = cutoff → Query2 = [now-1800, now-1080] → m0,m1,m2 → 共用 seen 去重 → 12 条
        let records: Vec<serde_json::Value> = (0..12)
            .map(|i| record_json(&time_ago(1200 - i * 60), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        let cfg = ctx_config(1800, 10);
        let out = reader_at(&url).read_recent_for_context("agent", "role", &cfg).await.unwrap();
        assert_eq!(out.len(), 12, "并集去重后 = 全部 12 条（m2 只保留一条）");
        assert_eq!(out[0].content, "m0");
        assert_eq!(out[11].content, "m11");
        assert!(out.windows(2).all(|w| w[0].time <= w[1].time), "时间正序");
    }
```

其余测试（`read_recent_empty_and_less_than_n`、`read_recent_skips_non_text_records_and_joins_multi`、`read_recent_dedups_duplicate_records_by_time_sn_at_parse`、`read_recent_extracts_is_self_and_packs_alternating`）**不改**——count 均 ≥ 记录数，走 recent 端点返回全量，行为不变。

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test memory_reader`
Expected: 编译失败（`RecentQuery` 未导入/`parse_channel_groups` 未定义）或部分用例断言失败（`read_recent_window_covers_ln...` 期望 30 实际 10、`read_recent_includes_same_time_group...` 期望 11 实际 10）——旧代码是新语义不满足

- [ ] **Step 3: 实现**

**(a) imports**（顶部）：

```rust
use kissbot_api::memory::{ChannelRecord, QueryRequest, RecentQuery, RecordKey};
```

**(b) 重写 `read_recent_for_context`**（替换整个函数体）：

```rust
    /// role 模式记忆打包：两次查询并集——① RecentQuery 最近 N 条（无时间参数）→ ln = 最旧一条 time；
    /// ② QueryRequest 时间范围 [M, ln]（M = min(时间窗起点, ln)，无 limit，M == ln 时退化为单点取 ln 同时间组）；
    /// 结果 = ① ∪ ②（两次解析共用 (time, sn) seen 集 → 自动去重），时间正序
    /// （query_channel_recent / query_channel / parse_channel_records 均为单处引用，已内联于此）
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

        let count = cfg.memory_count;

        // ===== Query1（query_channel_recent 内联）：POST {store}/store/query/channel/recent（RecentQuery，无时间参数） =====
        let url = format!("{}/store/query/channel/recent", self.store_url.trim_end_matches('/'));
        let body = RecentQuery {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            count: count as u32,
        };
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("记忆读取返回 {}", resp.status())));
        }
        let resp: ApiResponse<QueryChannelData> = resp.json().await?;
        let groups = resp.data.unwrap_or_default();
        // Query1 返回的原始记录数（含非文本）："不足 N 条直接返回"须基于原始记录而非解析后文本
        let recent_raw = groups.iter().map(|(_, v)| v.len()).sum::<usize>();

        // ===== 解析（parse_channel_records 内联）：以 (time, sn) 为唯一键去重（两次查询共用同一 seen 集 → 并集自动去重） =====
        let mut seen: HashSet<(String, u64)> = HashSet::new();
        let mut msgs: Vec<MemoryMsg> = Vec::new();
        parse_channel_groups(groups, &mut seen, &mut msgs);
        msgs.sort_by(|a, b| a.time.cmp(&b.time));

        if msgs.is_empty() || count == 0 {
            return Ok(Vec::new());
        }
        // 不足 N 条：Query1 已含全部记录，直接返回（无需 Query2）
        if recent_raw < count {
            return Ok(msgs);
        }
        // ln = Query1 最旧一条（原始记录）的 time；M = min(时间窗起点 start, ln)
        let ln = groups.iter().flat_map(|(_, v)| v.iter())
            .map(|(_, r)| r.time.as_str())
            .min()
            .expect("Query1 非空（recent_raw >= count >= 1）")
            .to_string();
        let m = if start.as_str() < ln.as_str() { start } else { ln.clone() };  // M = min(cutoff, ln)

        // ===== Query2（query_channel 内联）：POST {store}/store/query/channel（QueryRequest 时间范围 [M, ln]，无 limit） =====
        // M == ln 时退化为单点 [ln, ln]（取 ln 同时间组）；与 Query1 并集（共用 seen 集去重）
        let url = format!("{}/store/query/channel", self.store_url.trim_end_matches('/'));
        let body = QueryRequest {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            start_time: Arc::new(m),
            end_time: Arc::new(ln),
        };
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("记忆读取返回 {}", resp.status())));
        }
        let resp: ApiResponse<QueryChannelData> = resp.json().await?;
        parse_channel_groups(resp.data.unwrap_or_default(), &mut seen, &mut msgs);
        msgs.sort_by(|a, b| a.time.cmp(&b.time));
        Ok(msgs)
    }
```

**(c) 新增 `parse_channel_groups` 辅助**（放在 `read_recent_for_context` 之后、`extract_record_text` 之前）：

```rust
/// 解析 query 响应分组 → 去重追加到 msgs：(time, sn) 为唯一键（sn 为同文件内唯一序号）；
/// 两次查询共用同一 seen 集 → 并集自动去重（Query2 的 [M, ln] 区间与 Query1 尾部在 ln 处重叠）
fn parse_channel_groups(groups: QueryChannelData, seen: &mut HashSet<(String, u64)>, msgs: &mut Vec<MemoryMsg>) {
    for (_, records) in groups {
        for (_, rec) in records {
            let time = rec.time.as_str().to_string();
            let user_name = rec.user_name.as_str().to_string();
            let is_self = rec.is_self != 0;
            let sn = rec.sn;
            let content = extract_record_text(&rec.content);
            if content.is_empty() {
                continue;  // 非文本记录跳过
            }
            if !seen.insert((time.clone(), sn)) {
                continue;  // 同 (time, sn) 的重复记录只保留一条
            }
            msgs.push(MemoryMsg { user_name, content, time, is_self });
        }
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cd kissbot-agent && cargo test memory_reader && cargo test`
Expected: 全部 PASS（111+ 用例）

Run: `cd kissbot-agent && cargo clippy --all-targets`
Expected: 无新增 warning（仅保留既有 coordinator.rs 的 pre-existing warning）

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot
git add kissbot-agent/src/memory_reader.rs
git commit -m "refactor(agent): read_recent_for_context 改两次查询并集——recent(count) + [M, ln] 时间段（M=min(cutoff,ln)），共用 (time,sn) seen 去重，mock store 加 /channel/recent 端点"
```

---

### Task 5: 文档同步

**Files:**
- Modify: `docs/design/components-design/kissbot-agent-nexus.md`（第 95 行、第 259 行）
- Modify: `docs/spec/kissbot-agent-modules.md`（第 16 行、第 203 行）

- [ ] **Step 1: nexus.md 第 95 行**

old:
```
  - 第一级：从 memory-store 读取最近对话消息——单次全史查询（QueryRequest：agent_id + role_name + 时间范围，所有 channel 记录同文件，无需组合枚举）取该时间全部；按并集算法：取最后 N 条（`memory_count`，不足 N 条直接返回全部），T_N = 最旧一条时间，M = max(时间窗起点, T_N)，结果 = 最后 N 条 ∪ [M, T_N] 同时间组（同时间记录不拆散），时间正序
```
new:
```
  - 第一级：从 memory-store 读取最近对话消息——两次查询并集：① RecentQuery（agent_id + role_name + count，无时间参数）取最近 N 条（不足 N 条直接返回全部），T_N = 最旧一条时间；② QueryRequest 时间范围 [M, T_N]（M = min(时间窗起点, T_N)，无 limit，M == T_N 时退化为单点取同时间组）取该段全部；结果 = ① ∪ ②（按 (time, sn) 去重后时间正序）
```

- [ ] **Step 2: nexus.md 第 259 行**

old:
```
- 会话创建或重置时按模式构建：event 模式从本地缓存全量恢复（缓存生命周期 = 当前上下文，超长即压缩，天然有界）；role 模式从 memory-store 单次全史查询 + 并集算法读取最近消息（最后 N 条 ∪ [M, T_N] 同时间组，窗口内早于 T_N 的记录不含）打包为一条 user 消息
```
new:
```
- 会话创建或重置时按模式构建：event 模式从本地缓存全量恢复（缓存生命周期 = 当前上下文，超长即压缩，天然有界）；role 模式从 memory-store 两次查询并集读取最近消息（最近 N 条 ∪ [M, T_N] 时间段，M = min(时间窗起点, T_N)）打包为一条 user 消息
```

- [ ] **Step 3: agent-modules.md 第 16 行**

old:
```
| memory_reader | MemoryReader | 从 memory-store 读记忆构建上下文（组合查询 + 并集打包、事件列表、记忆索引） | 被 coordinator 调用；依赖 config + memory-store |
```
new:
```
| memory_reader | MemoryReader | 从 memory-store 读记忆构建上下文（最近 N + 时间段两次查询并集打包、事件列表、记忆索引） | 被 coordinator 调用；依赖 config + memory-store |
```

- [ ] **Step 4: agent-modules.md 第 203 行**

old:
```
        CO->>MR: read_recent_for_context（组合查询 + 并集打包为一条 user 消息）
```
new:
```
        CO->>MR: read_recent_for_context（最近 N + 时间段两次查询并集打包为一条 user 消息）
```

- [ ] **Step 5: Commit**

```bash
cd /home/admin/project/kissbot
git add docs/design/components-design/kissbot-agent-nexus.md docs/spec/kissbot-agent-modules.md
git commit -m "docs: 记忆上下文读取改为两次查询并集（RecentQuery 最近 N + [M, T_N] 时间段，M=min）"
```
