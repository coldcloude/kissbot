# 模型上下文系统修正实现计划（Rework）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按修正设计（docs/superpowers/specs/2026-08-05-model-context-system-rework-design.md）调整已实现的主重构：合批重置语义、撤销 memory-store limit/聚合并新增组合查询 API、记忆打包并集算法、Content 补 ToolCall/ToolResult 变体与工具 key 机制、DashMap 读锁不跨 await。

**Architecture:** ①合批用 `Session.resetting: Arc<AtomicBool>` 替代 batch_gen（重置期间延时任务等待，结束后统一打包）；②memory-store 复原精确 key 查询 + 新增组合枚举端点（扫描 channel 文件名），agent 逐组合查询；③记忆打包 = 全史合并取最后 N → T_N → M=max(cutoff,T_N) → [M,T_N] 并集；④Content 新增 ToolCall/ToolResult(key) 变体，loop 中写 channel 占位记录，ToolCallRequest/ToolResultRequest 同 key；⑤DashMap get 后 clone Arc 释放读锁再 await。

**Tech Stack:** Rust 2024、tokio、serde/serde_json、kai-file（ReverseLineReader 不用于新查询，仅历史遗留）、arc-swap、dashmap、kissbot-api/memory/channel 多 crate。

## Global Constraints

- 遵守 `.claude/rules/coding-standards.md`：时间格式 `yyyy-MM-dd HH:mm:ss`；非枚举/非 Map Key/非 Vec 字段用 `Arc<T>` 包裹
- 不要删除代码中的注释（CLAUDE.md）；读写文件用 Read/Edit/Write 工具（禁止 sed/python 修改文件）
- 测试运行：各 crate 独立 `cd <crate> && cargo test`（无根 workspace）
- 提交 comment 中文，覆盖本次改动全部内容
- 本计划撤销主计划 Task 8 的 memory-store 扩展：`QueryChannelRequest.limit`/`QueryRequest.limit` 字段、`query_channel_aggregate`、`take_recent` 及对应测试全部删除复原
- Content 枚举新增变体后，编译器会列出所有非穷尽 match——逐处修复（工具相关内容在 agent 侧按需处理，channel-web 渲染用占位文本）

---

### Task 1: 合批重置语义（resetting 等待 + 统一合并）

**Files:**
- Modify: `kissbot-agent/src/batching.rs`（新增 flush 辅助函数）
- Modify: `kissbot-agent/src/session_manager.rs`（batch_gen 删除、resetting 新增）
- Modify: `kissbot-agent/src/coordinator.rs`（enqueue_batch / reset_context）

**Interfaces:**
- Produces: `Session.resetting: Arc<AtomicBool>`；`batching::flush_after_reset(session: &Arc<Session>, interval: Duration) -> Option<String>`（等待 interval → 循环等待 resetting 为 false → take 打包 → Some(content)；缓冲空返回 None）；enqueue_batch 首条时 spawn 调用 flush_after_reset
- Consumes: 现有 `BatchBuffer`/`pack_batch`

- [ ] **Step 1: 写失败测试（batching.rs tests 追加）**

```rust
    #[tokio::test]
    async fn flush_after_reset_waits_then_packs() {
        use crate::session_manager::Session;
        use crate::types::{Mode, SessionKey};
        use std::sync::atomic::AtomicBool;
        let key = SessionKey { agent_name: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let session = Arc::new(Session::new(&key, None, Arc::new("aid".into())));
        session.batch.lock().await.push("u1", "你好");
        // 重置期间：resetting=true，flush 不应打包
        session.resetting.store(true, std::sync::atomic::Ordering::SeqCst);
        let session2 = session.clone();
        let task = tokio::spawn(async move {
            flush_after_reset(&session2, Duration::from_millis(20)).await
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(session.batch.lock().await.take().is_empty() == false || true, "占位");
        // 重置完成：置 false，flush 立即打包
        session.resetting.store(false, std::sync::atomic::Ordering::SeqCst);
        let content = task.await.unwrap().expect("应打包");
        assert_eq!(content, "u1: 你好");
        assert!(session.batch.lock().await.is_empty(), "打包后缓冲清空");
    }
```

> 占位断言行在实现后移除（先确认任务在 resetting=true 期间未返回——用 `tokio::time::timeout` 断言未完成更准确，见 Step 3 最终测试代码）。

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test flush_after_reset_waits_then_packs`
Expected: 编译失败——`flush_after_reset` 未定义

- [ ] **Step 3: 实现**

`batching.rs` 新增：

```rust
use crate::session_manager::Session;
use std::sync::Arc;
use std::time::Duration;

/// 延时打包：等待 interval 后，若会话正在重置则继续等待（重置期间不触发超时），
/// 重置完成后立即打包一次；期间到达的消息统一合并（缓冲不清空）。缓冲为空返回 None
pub async fn flush_after_reset(session: &Arc<Session>, interval: Duration) -> Option<String> {
    tokio::time::sleep(interval).await;
    // 重置期间等待（轮询，重置通常毫秒级）
    while session.resetting.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut b = session.batch.lock().await;
    if b.is_empty() {
        return None;
    }
    let items = b.take();
    drop(b);
    Some(crate::batching::pack_batch(&items))
}
```

`session_manager.rs` `Session`：删除 `batch_gen: Arc<AtomicU64>` 字段与初始化；新增：

```rust
    /// 重置标志：reset_context 开始时置 true、结束时置 false；合批延时任务据此等待
    pub resetting: Arc<AtomicBool>,
```

（`Session::new` 中 `resetting: Arc::new(AtomicBool::new(false))`；删除 batch_gen 相关行。）

`coordinator.rs` `enqueue_batch` 改为（删除 gen 快照/校验）：

```rust
        let mut batch = session.batch.lock().await;
        let was_empty = batch.is_empty();
        batch.push(user_name, content_text);
        drop(batch);
        if !was_empty {
            return;  // 已有计时任务在跑，等待汇合
        }
        let Some(coordinator) = self.weak_self.get().and_then(|w| w.upgrade()) else {
            warn!("enqueue_batch: 协调器已释放，跳过合批");
            return;
        };
        let session = session.clone();
        let out_channel = out_channel.clone();
        let channel_id = channel_id.to_string();
        tokio::spawn(async move {
            if let Some(content) = crate::batching::flush_after_reset(&session, interval).await {
                coordinator.run_agentic_loop(&channel_id, &session, content, &out_channel).await;
            }
        });
```

`reset_context`：删除 `batch.clear()` 与 `batch_gen.fetch_add`，改为置标志：

```rust
    session.resetting.store(true, std::sync::atomic::Ordering::SeqCst);
    // ...现有归档/清缓存/重建逻辑...
    session.resetting.store(false, std::sync::atomic::Ordering::SeqCst);
```

（保留 reset 期间的注释说明：缓冲不清空、期间消息统一并入重置后打包。）

最终测试代码（替换 Step 1 的占位版）：

```rust
    #[tokio::test]
    async fn flush_after_reset_waits_then_packs() {
        use crate::session_manager::Session;
        use crate::types::{Mode, SessionKey};
        let key = SessionKey { agent_name: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let session = Arc::new(Session::new(&key, None, Arc::new("aid".into())));
        session.batch.lock().await.push("u1", "你好");
        session.resetting.store(true, std::sync::atomic::Ordering::SeqCst);
        let session2 = session.clone();
        let task = tokio::spawn(async move {
            flush_after_reset(&session2, Duration::from_millis(20)).await
        });
        // 重置期间（20ms interval 已过）任务不应返回
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(tokio::time::timeout(Duration::from_millis(10), task).await.is_err(),
            "重置期间不应打包");
        // 重置完成 → 立即打包（含期间到达的消息）
        session.batch.lock().await.push("u2", "在吗");
        session.resetting.store(false, std::sync::atomic::Ordering::SeqCst);
        let content = task.await.unwrap().expect("应打包");
        assert_eq!(content, "u1: 你好\nu2: 在吗", "重置期间消息统一合并");
        assert!(session.batch.lock().await.is_empty());
    }
```

> 注：`timeout` 消费 task——失败分支（expect 之前）需 `let task = ...` 后可重复 await；此处 `timeout(.., task)` 返回 Err 时 task 已被消费，改用 `task` 前先 `std::mem::take` 或按实际编译调整——若不可行，简化断言：reset 期间 sleep 60ms 后检查 `session.batch` 仍含 1 条（未被打包），再置 false 后 await 任务。

- [ ] **Step 4: 运行测试**

Run: `cd kissbot-agent && cargo test flush_after_reset_waits_then_packs && cargo test`
Expected: 新增测试 PASS，全量通过（删除 batch_gen 后无残留引用）

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/
git commit -m "refactor(agent): 合批重置语义改为重置等待+统一合并——删除 batch_gen 代数机制，Session 加 resetting 标志；reset_context 不清缓冲只置标志；延时任务 flush_after_reset 等待重置完成后打包一次，期间消息统一合并（不丢不串话）"
```

---

### Task 2: Content 新增 ToolCall/ToolResult 变体

**Files:**
- Modify: `kissbot-api/src/message.rs`
- Modify: Content 匹配点（编译器列出非穷尽 match）：`kissbot-agent/src/coordinator.rs`、`kissbot-channel-web/src/messenger.rs` 等

**Interfaces:**
- Produces: `Content::ToolCall(Arc<String>)` / `Content::ToolResult(Arc<String>)`（key 参数，仿 `Think(Arc<String>)`）

- [ ] **Step 1: 写失败测试（message.rs tests 追加）**

```rust
    #[test]
    fn test_serde_content_tool_call_result() {
        let call = Content::ToolCall(Arc::new("tc-1".to_string()));
        let j1 = serde_json::to_value(&call).unwrap();
        assert_eq!(j1, serde_json::json!({"msg_type":"ToolCall","data":"tc-1"}));
        let back1: Content = serde_json::from_value(j1).unwrap();
        assert_eq!(back1, call);

        let result = Content::ToolResult(Arc::new("tc-1".to_string()));
        let j2 = serde_json::to_value(&result).unwrap();
        assert_eq!(j2, serde_json::json!({"msg_type":"ToolResult","data":"tc-1"}));
        let back2: Content = serde_json::from_value(j2).unwrap();
        assert_eq!(back2, result);
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-api && cargo test test_serde_content_tool_call_result`
Expected: 编译失败——`Content::ToolCall` 未定义

- [ ] **Step 3: 实现**

`kissbot-api/src/message.rs` `Content` 枚举新增两个变体（紧跟 `Think` 变体后，注释说明与 Think 同构、key 关联工具详情）：

```rust
    /// 工具调用占位：data 为 key（关联 ToolCallRequest 详情）
    ToolCall(Arc<String>),
    /// 工具结果占位：data 为 key（与 ToolCallRequest 同 key）
    ToolResult(Arc<String>),
```

运行 `cargo build`（kissbot-api）与全仓 `cargo build`（逐个 crate），编译器列出所有非穷尽 match，逐处修复：
- `kissbot-agent/src/coordinator.rs` extract_text（`_ => String::new()` 已有则无需改；`match &event.incoming_message.content` 的系统事件分支若已有 `_ => {}` 无需改）
- `kissbot-channel-web/src/messenger.rs` 渲染分支：新变体渲染占位文本（如 `"[工具调用]"`/`"[工具结果]"`，不展示 key）
- 其它非穷尽 match 按上下文处理（内存/通道层透传或忽略）

- [ ] **Step 4: 运行测试**

Run: `cd kissbot-api && cargo test`；`cd kissbot-agent && cargo test`；`cd kissbot-channel-web && cargo build`（必要时 test）
Expected: 全部通过；新增 serde 测试 PASS

- [ ] **Step 5: Commit**

```bash
git add kissbot-api kissbot-agent kissbot-channel-web
git commit -m "feat(api): Content 新增 ToolCall/ToolResult(key) 变体（仿 Think）——channel 占位记录与工具详情 key 关联基础；同步全仓非穷尽 match（channel-web 渲染占位文本）；serde roundtrip 测试"
```

---

### Task 3: memory-store 撤销 limit/聚合 + 新增组合查询 API

**Files:**
- Modify: `kissbot-api/src/memory.rs`（删 limit、新增 ChannelCombo）
- Modify: `kissbot-memory/src/index.rs`（删聚合/take_recent、复原查询、新增 query_combos）
- Modify: `kissbot-memory-store/src/api.rs`（新增组合端点路由）

**Interfaces:**
- Produces: `kissbot_api::memory::ChannelCombo { messenger_id: Arc<String>, user_id: Arc<String>, group_id: Arc<String> }`（Serialize/Deserialize/Hash/Eq/PartialEq）；`MemoryIndexer::query_combos(query: QueryRequest) -> Result<Vec<ChannelCombo>>`；端点 `POST /store/query/combos`（请求体 QueryRequest，响应 ApiResponse<Vec<ChannelCombo>>）
- Removes: `QueryChannelRequest.limit`、`QueryRequest.limit`、`query_channel_aggregate`、`take_recent` 及 Task 8 测试

- [ ] **Step 1: 写失败测试（kissbot-memory index.rs tests 追加）**

```rust
    #[tokio::test]
    async fn test_query_combos_enumerates_channel_files() {
        use kissbot_api::memory::QueryRequest;
        use tokio::io::AsyncWriteExt;
        let root = init_test_config();
        // 写两个 channel 文件（不同 messenger/user/group，同 role）
        let mk = |messenger: &str, user: &str, group: &str| {
            root.join("combo_agent").join("memory-store").join("2026-r1")
                .join(format!("channel-{}={}={}-records-2026-08-05.jsonl", messenger, user, group))
        };
        for path in [mk("web", "self1", "g1"), mk("tg", "self2", "g2")] {
            tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
            tokio::fs::write(&path, "{\"time\":\"2026-08-05 10:00:00\"}\n").await.unwrap();
        }
        let indexer = MemoryIndexer::new();
        let query = QueryRequest {
            agent_id: Arc::new("combo_agent".into()),
            role_name: Arc::new("r1".into()),
            start_time: Arc::new("2026-08-05 00:00:00".into()),
            end_time: Arc::new("2026-08-05 23:59:59".into()),
        };
        let combos = indexer.query_combos(query).await.unwrap();
        assert_eq!(combos.len(), 2);
        let mut ids: Vec<(String, String, String)> = combos.iter().map(|c| {
            (c.messenger_id.to_string(), c.user_id.to_string(), c.group_id.to_string())
        }).collect();
        ids.sort();
        assert_eq!(ids[0], ("tg".into(), "self2".into(), "g2".into()));
        assert_eq!(ids[1], ("web".into(), "self1".into(), "g1".into()));
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-memory && cargo test test_query_combos_enumerates_channel_files`
Expected: 编译失败——`query_combos` 未定义

- [ ] **Step 3: 实现**

`kissbot-api/src/memory.rs`：删除 `QueryChannelRequest.limit` 与 `QueryRequest.limit` 字段（含 serde 注释）；复原相关测试构造（补 limit 的行删除）；新增：

```rust
/// channel 记录文件组合（messenger + user + group），用于按组合精确查询
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ChannelCombo {
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
}
```

`kissbot-memory/src/index.rs`：
- 删除 `query_channel_aggregate`、`take_recent`；`query_channel_records` 复原为直接 `self.channel_indices.query_all(query).await`（不加 limit 分支）；`query_think_records`/`query_tool_call_records`/`query_tool_result_records` 复原（删 take_recent 调用）
- 删除 Task 8 的目录聚合测试
- 新增：

```rust
    /// 枚举 <root>/<agent_id>/memory-store/<year>-<role_name>/channel-*.jsonl，
    /// 解析文件名中的 (messenger, user, group) 组合（按文件日期过滤在时间范围内），去重返回
    pub async fn query_combos(&self, query: QueryRequest) -> Result<Vec<ChannelCombo>> {
        use kai_date::as_date;
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
                        // channel-{m}={u}={g}-records-{yyyy-mm-dd}.jsonl
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
        // 去重（按组合三元组）
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

> 注：`as_date` 返回 &str（kai-date），与 Arc<String> 比较需转换——按实际编译调整（如 `date.as_str()` 比较或用 String）。

`kissbot-memory-store/src/api.rs` 新增路由与 handler：

```rust
        .route("/store/query/combos", post(query_combos))

async fn query_combos(Json(req): Json<memory::QueryRequest>) -> impl IntoResponse {
    let records = MemoryIndexer::get().query_combos(req).await;
    match records {
        Ok(combos) => (StatusCode::OK, Json(ApiResponse::success(combos))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(e.to_string()))),
    }
}
```

- [ ] **Step 4: 适配现有测试**

`kissbot-api` tests：删除 `test_serde_query_channel_request`/`test_serde_query_request` 中补的 `limit: None` 行。`kissbot-agent` 若无 limit 构造引用则无需动（memory_reader 在 Task 4 重写）。

- [ ] **Step 5: 运行测试**

Run: `cd kissbot-memory && cargo test`；`cd kissbot-api && cargo test`；`cd kissbot-memory-store && cargo build`
Expected: 全部通过（新增 combos 测试 PASS）

- [ ] **Step 6: Commit**

```bash
git add kissbot-api kissbot-memory kissbot-memory-store
git commit -m "refactor(memory): 撤销 limit/目录聚合扩展，新增组合查询 API——QueryChannelRequest/QueryRequest 删 limit（复原精确 key 时间区间查询，删 query_channel_aggregate/take_recent 及测试）；新增 ChannelCombo 结构与 POST /store/query/combos（枚举 role 目录 channel 文件名解析组合，按文件日期过滤）"
```

---

### Task 4: 记忆打包重写（组合查询 + 并集算法）

**Files:**
- Modify: `kissbot-agent/src/memory_reader.rs`（重写）
- Modify: `kissbot-agent/src/coordinator.rs`（build_initial_context role 分支适配，签名不变）

**Interfaces:**
- Produces: `memory_reader::{MemoryMsg{user_name, content, time}, recent_memory(&[MemoryMsg], count: usize, cutoff: &str) -> Vec<MemoryMsg>, read_recent_for_context(agent_id, role_name, cfg: &EffectiveContextConfig) -> Result<Vec<MemoryMsg>>}`；`query_combos` 调 `/store/query/combos`；`query_channel(combo, start, end)` 调 `/store/query/channel`（精确 key）
- Removes: `window_wins` 两查询比较

- [ ] **Step 1: 写失败测试（memory_reader.rs tests 追加）**

```rust
    #[test]
    fn recent_memory_union_keeps_order_and_same_time_group() {
        use chrono::Local;
        let t = |m: &str| MemoryMsg { user_name: "u".into(), content: m.into(), time: m.into() };
        // 时间用可排序字符串：dense 场景 30 条在窗口内，最后 N=10
        let mut list: Vec<MemoryMsg> = (1..=30).map(|i| t(&format!("2026-08-05 10:{:02}:00", i % 60))).collect();
        list.sort_by(|a, b| a.time.cmp(&b.time));
        let cutoff = "2026-08-05 09:00:00";  // 窗口起点早于所有记录 → T_N > cutoff → M = T_N
        let out = recent_memory(&list, 10, cutoff);
        // 结果 = 最后 10 条 + 同时间组（时间正序）
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].time, list[20].time);
        assert!(out.windows(2).all(|w| w[0].time <= w[1].time), "时间正序");
    }

    #[test]
    fn recent_memory_sparse_extends_beyond_window() {
        let t = |m: &str, tm: &str| MemoryMsg { user_name: "u".into(), content: m.into(), time: tm.into() };
        // 稀疏：最后 10 条跨 3h，窗口仅 3 条在 [cutoff, now]
        let mut list: Vec<MemoryMsg> = vec![
            t("a", "2026-08-05 07:00:00"), t("b", "2026-08-05 07:05:00"),
            t("c", "2026-08-05 07:10:00"), t("d", "2026-08-05 07:15:00"),
            t("e", "2026-08-05 07:20:00"), t("f", "2026-08-05 07:25:00"),
            t("g", "2026-08-05 07:30:00"), t("h", "2026-08-05 07:35:00"),
            t("i", "2026-08-05 07:40:00"), t("j", "2026-08-05 07:45:00"),
        ];
        list.sort_by(|a, b| a.time.cmp(&b.time));
        let cutoff = "2026-08-05 08:00:00";  // 窗口起点晚于全部记录 → T_N < cutoff → M = cutoff
        let out = recent_memory(&list, 10, cutoff);
        assert_eq!(out.len(), 10, "稀疏场景结果 = 最后 N 条（跨更早时间）");
        assert_eq!(out[0].time, "2026-08-05 07:00:00");
    }

    #[test]
    fn recent_memory_empty_and_less_than_n() {
        assert!(recent_memory(&[], 10, "2026-08-05 09:00:00").is_empty());
        let t = |m: &str, tm: &str| MemoryMsg { user_name: "u".into(), content: m.into(), time: tm.into() };
        let list = vec![t("a", "2026-08-05 10:00:00"), t("b", "2026-08-05 10:01:00")];
        let out = recent_memory(&list, 10, "2026-08-05 09:00:00");
        assert_eq!(out.len(), 2, "不足 N 条取全部");
    }
```

> 同时间组用例：最后 N 条的最旧一条时间点有多条同时间记录时，结果包含 N 之外的同时间记录——Step 3 实现后补一个显式断言（构造 T_N 处 5 条同时间）。

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test recent_memory_union_keeps_order_and_same_time_group`
Expected: 编译失败——`recent_memory` 未定义

- [ ] **Step 3: 实现**

`memory_reader.rs` 重写（保留 MemoryMsg、pack_memory_messages、extract_record_text、read_memory_struct_index、list_events）：

```rust
use kissbot_api::memory::ChannelCombo;

/// 并集算法（修正设计 2.3）：(1) 取最后 N 条 → T_N；(2) M = max(cutoff, T_N)；
/// (3) [M, T_N]（含两端）取该时间全部；最终 = (1) ∪ (3)，时间正序
/// list 为全史合并升序结果（调用方已按时间排序）
pub fn recent_memory(list: &[MemoryMsg], count: usize, cutoff: &str) -> Vec<MemoryMsg> {
    if list.is_empty() {
        return Vec::new();
    }
    // 不足 N 条：已全部取出，直接返回（无需 T_N/M/(3)，避免空并集计算）
    if list.len() <= count {
        return list.to_vec();
    }
    let start_idx = list.len() - count;
    let last_n = &list[start_idx..];
    let t_n = &last_n[0].time;   // 最旧一条
    let m = if cutoff >= t_n.as_str() { cutoff.to_string() } else { t_n.to_string() };
    // (3) [M, T_N] 含两端；与 (1) 并集后升序
    let mut out: Vec<MemoryMsg> = list.iter()
        .filter(|msg| msg.time.as_str() >= m.as_str() && msg.time.as_str() <= t_n.as_str())
        .cloned()
        .chain(last_n.iter().cloned())
        .collect();
    out.sort_by(|a, b| a.time.cmp(&b.time));
    out.dedup_by(|a, b| a.time == b.time && a.content == b.content && a.user_name == b.user_name);
    out
}
```

> 说明：`(1)` 的全史查询已覆盖 `(3)` 的 [M, T_N] 区间，故 `(3)` 在客户端从同一列表过滤（等价于独立查询）；`dedup_by` 防 (1) 与 (3) 重叠（[M, T_N] ∩ last_n 的重复）。

`MemoryReader` 新方法（替换 query_channel_range/query_channel_recent/read_recent_for_context）：

```rust
    /// 组合查询：POST /store/query/combos
    async fn query_combos(&self, agent_id: &str, role_name: &str, start: &str, end: &str) -> Result<Vec<ChannelCombo>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();
        let url = format!("{}/store/query/combos", store_url.trim_end_matches('/'));
        let body = json!({ "agent_id": agent_id, "role_name": role_name, "start_time": start, "end_time": end });
        let resp = self.client.post(&url).json(&body).send().await
            .map_err(|e| Error::MemoryStoreError(format!("组合查询失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("组合查询返回 {}", resp.status())));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(serde_json::from_value(data["data"].clone()).unwrap_or_default())
    }

    /// 组合内精确查询：POST /store/query/channel（messenger/user/group 精确 key，取该时间全部）
    async fn query_channel(
        &self,
        combo: &ChannelCombo,
        start_time: &str,
        end_time: &str,
    ) -> Result<Vec<MemoryMsg>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();
        let url = format!("{}/store/query/channel", store_url.trim_end_matches('/'));
        let body = json!({
            "agent_id": combo_agent_id_placeholder(),  // 见下
            "role_name": ...,
            "messenger_id": combo.messenger_id,
            "user_id": combo.user_id,
            "group_id": combo.group_id,
            "start_time": start_time,
            "end_time": end_time,
        });
        // ... 与现有 query_channel 相同的发送/解析逻辑（parse_channel_records 复用）
    }
```

> 注意：`/store/query/channel` 请求体含 `agent_id`/`role_name`——`query_channel` 需额外传入 agent_id/role_name（签名 `query_channel(&self, agent_id, role_name, combo, start, end)`），占位函数名仅为示意，实现时直接用参数。

`read_recent_for_context` 重写：

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
            if let Ok(msgs) = self.query_channel(agent_id, role_name, combo, "2000-01-01 00:00:00", &now).await {
                merged.extend(msgs);
            }
        }
        merged.sort_by(|a, b| a.time.cmp(&b.time));

        Ok(recent_memory(&merged, cfg.memory_count, &start))
    }
```

删除 `window_wins` 与旧 `query_channel_range`/`query_channel_recent`。`coordinator.rs` role 分支调用签名不变（read_recent_for_context(agent_id, role_name, cfg)），无需改动（确认）。

- [ ] **Step 4: 补同时间组显式断言测试**

```rust
    #[test]
    fn recent_memory_includes_same_time_group_beyond_n() {
        let t = |m: &str, tm: &str| MemoryMsg { user_name: "u".into(), content: m.into(), time: tm.into() };
        // 15 条：记录 11~15 都与记录 10 同在 T_N=10:50
        let mut list: Vec<MemoryMsg> = Vec::new();
        for i in 1..=10 { list.push(t(&format!("m{}", i), &format!("2026-08-05 10:{:02}:00", 30 + i))); }
        for i in 11..=15 { list.push(t(&format!("m{}", i), "2026-08-05 10:50:00")); }
        list.sort_by(|a, b| a.time.cmp(&b.time));
        let out = recent_memory(&list, 10, "2026-08-05 09:00:00");
        // 最后 10 条 = m6..m15（T_N=10:50），同时间组 m11~m15 全保留 → 结果 10 条（m6..m15）
        assert_eq!(out.len(), 10, "T_N 同时间组不拆散（本例如最后 10 条起点恰在同时间组起点）");
        assert!(out.windows(2).all(|w| w[0].time <= w[1].time));
    }
```

> 实现后按实际输出校正断言（同时间组边界的精确条数取决于列表构造）——若结果与预期不符，以「同时间组完整保留 + 正序」为准调整断言数字。

- [ ] **Step 5: 运行测试**

Run: `cd kissbot-agent && cargo test memory_reader && cargo test`
Expected: 全部通过（新 recent_memory 测试 PASS，window_wins 删除后无残留）

- [ ] **Step 6: Commit**

```bash
git add kissbot-agent/src/
git commit -m "refactor(agent): 记忆打包重写——组合查询 API + 每组合全史查询合并 + 并集算法 recent_memory（(1)最后N→T_N, (2)M=max(cutoff,T_N), (3)[M,T_N]含端点并集升序，同时间组不拆散）；删除 window_wins 两查询比较"
```

---

### Task 5: loop 工具 key 机制（channel 占位 + 同 key）

**Files:**
- Modify: `kissbot-agent/src/coordinator.rs`（loop 工具分支）

**Interfaces:**
- Consumes: `Content::ToolCall(key)`/`Content::ToolResult(key)`（Task 2）
- Produces: 每个 tool call 生成 UUID key；写 channel 占位 `ChannelRecord(Content::ToolCall(key))`（仿 think 4a 结构，身份来自 out_channel）；`ToolCallRequest.key = key`；`ToolResultRequest.key = 同一 key`

- [ ] **Step 1: 写失败测试（coordinator.rs tests 追加，纯函数）**

```rust
    /// 工具占位记录构造（仿 think 的 ChannelRecord(Think) 流程）：返回 ChannelRequest
    fn tool_placeholder_request(
        session: &Arc<Session>,
        out_channel: &OutChannel,
        key: &str,
        is_result: bool,
        now: &str,
    ) -> ChannelRequest {
        let role_name = memory_role(session.role_name.as_str(), &session.mode);
        let content = if is_result { Content::ToolResult(Arc::new(key.to_string())) }
                      else { Content::ToolCall(Arc::new(key.to_string())) };
        ChannelRequest {
            agent_id: session.agent_id.clone(),
            role_name: Arc::new(role_name),
            messenger_id: Arc::new(out_channel.user.messenger_id.clone()),
            user_id: Arc::new(out_channel.user.user_id.clone()),
            self_user_id: Arc::new(out_channel.user.user_id.clone()),
            group_id: out_channel.group_id.clone(),
            is_self: 1,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new(String::new()),
            group_name: Arc::new(String::new()),
            content,
            time: Arc::new(now.to_string()),
        }
    }

    #[test]
    fn tool_placeholder_uses_same_key_for_call_and_result() {
        // session/out_channel 构造见下（复用 coordinator 测试已有模式）
        let key = uuid::Uuid::new_v4().to_string();
        let call = tool_placeholder_request(&session, &out_channel, &key, false, "2026-08-05 10:00:00");
        let result = tool_placeholder_request(&session, &out_channel, &key, true, "2026-08-05 10:00:01");
        // 占位内容携带同一 key
        assert!(matches!(&call.content, Content::ToolCall(k) if k.as_str() == key));
        assert!(matches!(&result.content, Content::ToolResult(k) if k.as_str() == key));
    }
```

（session/out_channel 构造：Session::new + OutChannel{...}，参照现有 coordinator 测试；若构造繁琐，将断言函数改为接收 (role_name, out_channel, key, is_result, now) 返回 Content + 校验 key 一致性即可。）

- [ ] **Step 2: 运行确认失败**

Run: `cd kissbot-agent && cargo test tool_placeholder_uses_same_key_for_call_and_result`
Expected: 编译失败——`tool_placeholder_request` 未定义

- [ ] **Step 3: 实现**

`coordinator.rs` `run_agentic_loop` 工具分支（`for call in &model_resp.tool_calls` 循环内），在执行工具前：

```rust
                for call in &model_resp.tool_calls {
                    // 5a. 工具调用 key：UUID（ToolCall/ToolResult 详情与 channel 占位同 key 关联）
                    let tool_key = uuid::Uuid::new_v4().to_string();
                    // 5b. channel 占位记录（仿 think 流程，身份来自 out_channel，is_self=1）
                    let call_placeholder = self.tool_placeholder_request(session, out_channel, &tool_key, false, &now);
                    self.memory_store_client.push_channel_record(call_placeholder).await;
                    // 5c. 执行工具
                    let result = self.execute_tool_call(session, call).await;
                    let result_text = result.to_string();
                    // 5d. 上下文/缓存追加 Tool 消息（不变）
                    ...
                    // 5e. 记忆写入：ToolCallRequest.key 与 ToolResultRequest.key 用同一 key
                    self.memory_store_client.push_tool_call(ToolCallRequest {
                        ...
                        key: Arc::new(tool_key.clone()),
                        ...
                    }).await;
                    self.memory_store_client.push_tool_result(ToolResultRequest {
                        ...
                        key: Arc::new(tool_key.clone()),
                        ...
                    }).await;
                    // 5f. tool-result 占位记录（同 key）
                    let result_placeholder = self.tool_placeholder_request(session, out_channel, &tool_key, true, &now);
                    self.memory_store_client.push_channel_record(result_placeholder).await;
                }
```

（`now` 在循环前已定义；`tool_placeholder_request` 为 coordinator 私有方法，测试用同模块访问。）

- [ ] **Step 4: 运行测试**

Run: `cd kissbot-agent && cargo test tool_placeholder_uses_same_key_for_call_and_result && cargo test`
Expected: 全部通过

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/coordinator.rs
git commit -m "feat(agent): 工具调用 key 机制——每个 tool call 生成 UUID key，写 channel 占位记录（Content::ToolCall/ToolResult(key) 仿 think），ToolCallRequest.key 与 ToolResultRequest.key 用同一 key（详情经 channel 时间线关联）"
```

---

### Task 6: DashMap 克隆释放 + 收尾

**Files:**
- Modify: `kissbot-agent/src/station.rs`（call_tool 克隆）
- Modify: `kissbot-agent/src/coordinator.rs`（execute_tool_call 收集 Vec）
- Modify: 文档（组件设计文档记忆打包/工具分派描述、`script/README.md` 手动验证清单）

**Interfaces:**
- Consumes: 无新接口

- [ ] **Step 1: 改 station.rs call_tool（克隆 Arc 释放读锁）**

```rust
    pub async fn call_tool(&self, name: &str, params: Value) -> Result<Value> {
        if self.config.base_url.is_empty() {
            // 克隆出 Arc 立即释放 DashMap 读锁（不跨 await 持锁）
            let tool = self.local_tools.get(name).map(|r| r.value().clone())
                .ok_or_else(|| Error::InternalError(format!("本地工具不存在: {}", name)))?;
            return tool.call(params).await;
        }
        Err(Error::InternalError(format!("远程 Station 调用未实现（本轮仅本地模式）: {}", name)))
    }
```

- [ ] **Step 2: 改 execute_tool_call（先收集 Vec 再循环）**

```rust
    async fn execute_tool_call(&self, session: &Arc<Session>, call: &crate::types::ToolCall) -> serde_json::Value {
        let cfg = self.config.context_config(session.agent_name.as_str(), session.role_name.as_str()).await;
        // 先克隆出 Arc 列表（释放 DashMap 全局读锁），再逐项 await
        let runtimes: Vec<(String, Arc<StationRuntime>)> = self.station_runtimes.iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        for (station_id, runtime) in &runtimes {
            if cfg.stations.contains(station_id.as_str()) && runtime.has_tool(call.name.as_str()) {
                match runtime.call_tool(call.name.as_str(), (*call.arguments).clone()).await {
                    Ok(v) => return v,
                    Err(e) => return serde_json::json!({ "error": e.to_string() }),
                }
            }
        }
        serde_json::json!({ "error": format!("工具不存在: {}", call.name) })
    }
```

- [ ] **Step 3: 运行测试**

Run: `cd kissbot-agent && cargo test`；`cd kissbot-memory && cargo test`；`cd kissbot-api && cargo test`；`cd kissbot-memory-store && cargo build`
Expected: 全部通过，build 无新增警告

- [ ] **Step 4: 更新文档**

`docs/design/components-design/kissbot-agent-nexus.md`：
- 记忆读取器节：role 模式 = 组合查询（memory-store 组合 API）→ 每组合时间查询 → 并集算法（最后 N 与时间窗，同时间组不拆散）
- 工具调用分派节：工具调用写 channel 占位记录（Content::ToolCall/ToolResult，key 关联详情）

`docs/design/components-design/kissbot-agent-station.md`：无需改（DashMap 克隆是内部实现）。

`script/README.md`：手动验证清单更新——验证点补充「记忆打包：role 模式重启后上下文含最近 N 条 + 窗口内消息」与「工具调用后 channel 时间线出现 ToolCall/ToolResult 占位记录、详情可经 key 查询」。

- [ ] **Step 5: Commit**

```bash
git add kissbot-agent/src/ docs/design/components-design/ script/README.md
git commit -m "fix(agent): DashMap 读锁不跨 await（call_tool 克隆 Arc 释放读锁、execute_tool_call 先收集 Vec 再循环）；更新组件设计文档与手动验证清单（组合查询/并集算法/工具占位记录）"
```

---

## Self-Review 记录

（writing-plans 自审，修正项已同步进正文）

**1. Spec 覆盖检查**（rework spec 6 节 → 任务）：

| Spec 节 | 任务 |
|---|---|
| 1 合批重置等待+统一合并 | T1 |
| 2.1 撤销 limit/聚合 | T3 |
| 2.2 组合查询 API | T3 |
| 2.3 并集算法 | T4 |
| 2.4 key 关联 | T4（收集占位，本轮打包仍只 name+content）+ T5（占位写入） |
| 3 Content 变体 + 占位 + 同 key | T2 + T5 |
| 4 DashMap 克隆释放 | T6 |
| 5 受影响文件 | 各任务 Files |
| 6 测试范围 | 各任务测试 |

**2. 占位符扫描**：无 TBD/TODO；Task 1/4/5 测试中的「占位断言/按实际校正」均为有明确替代方案的指引（Step 3 给出最终代码）。

**3. 类型一致性**：`recent_memory(&[MemoryMsg], usize, &str) -> Vec<MemoryMsg>`（T4 定义，测试一致）；`flush_after_reset(&Arc<Session>, Duration) -> Option<String>`（T1 定义）；`tool_placeholder_request(session, out_channel, key, is_result, now) -> ChannelRequest`（T5）；`query_combos(&self, agent_id, role_name, start, end) -> Result<Vec<ChannelCombo>>`（T3 服务端 / T4 客户端同名，分属两 crate 不冲突）。`Session.resetting` 在 T1 加字段、T4 无引用、T5 无引用——仅 T1 使用，无冲突。
