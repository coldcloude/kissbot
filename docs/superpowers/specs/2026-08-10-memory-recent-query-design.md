# memory-store 最近 N 条查询 + agent 两次查询并集设计

日期：2026-08-10

## 背景与目标

### 现状问题

1. **memory-store 缺少"查询最后 N 条"能力**：`QueryRequest` 只有 `{agent_id, role_name, start_time, end_time}`，`/store/query/channel` 只能按时间范围全量返回。`kai_file::FileIndexContext::query_last(key, n)` 底层原语已存在（单文件尾部读取、组内升序），但 memory 组件未接线。
2. **agent 全史拉取代价高**：`read_recent_for_context` 用 `start_time="2000-01-01"` 全史查询，把整个时间范围所有记录拉到内存再做客户端并集。历史越长，网络/反序列化/内存开销越大。

### 本次目标

1. memory-store 增加 ChannelRecord 的"最近 N 条"查询；**Think/ToolCall/ToolResult 不加**，其他限制条数的功能**不加**。跨日期文件处理参考 `kissbot-channel-web/src/message_store.rs` 的 `get_recent`。
2. `read_recent_for_context` 修正为设计方案的两次查询：**先取最后 N 条 → 再取时间段 → 取并集**。

## 设计约定（与前序讨论的差异修正）

- **按时间查询不带 limit**：`QueryRequest` 保持 `{agent_id, role_name, start_time, end_time}` 不变（此前规划中"误加 limit"的做法去掉）。
- **最近 N 条查询不带时间参数**：独立 `RecentQuery { agent_id, role_name, count }`，取该 agent+role 最近 count 条，**无时间过滤**。
- **并集公式**：`结果 = 最后 N 条 ∪ [M, ln] 时间段全部记录`，其中 `M = min(时间窗起点, ln)`，`ln` = 最后 N 条中最旧一条的时间。两次查询结果取真并集（按 (time, sn) 去重后按 time 升序）。
  - 注意：设计文档（kissbot-agent-nexus.md）此前写的是 `M = max(时间窗起点, ln)`，**本次修正为 min**；并同步修正文档。
  - `M = min` 统一处理两分支、无需单独讨论：cutoff ≤ ln 时 Query2 为 `[cutoff, ln]`（窗口内全部）；cutoff > ln 时 `M = ln`，Query2 退化为单一时间点 `[ln, ln]`（取 ln 同时间组）。`get_date_time_segments` 对 start == end 返回单段，单点区间可行。

## 一、Store API（kissbot-api）

### kissbot-api/src/memory.rs

- **`QueryRequest` 不变**。
- 新增：

```rust
/// 最近 N 条 channel 记录查询（无时间参数：取该 agent+role 最近 count 条，跨日期文件）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentQuery {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub count: u32,
}
```

### kissbot-memory-store/src/api.rs

- 新增端点 `POST /store/query/channel/recent`：

```rust
.route("/store/query/channel/recent", post(query_channel_recent_records))
```

- handler 透传 `Json<memory::RecentQuery>` → `MemoryIndexer::get().query_channel_recent(&req.agent_id, &req.role_name, req.count)`，响应形状与 `query/channel` 一致（`ApiResponse<Vec<(RecordKey, Vec<(u32, ChannelRecord)>)>>`）。

## 二、Store 实现（kissbot-memory）

### date_sets 缓存（懒加载）

`MemoryIndexer` 增加字段：

```rust
channel_date_sets: DashMap<(String, String), BTreeSet<String>>,  // (agent_id, role_name) → 已存在的 channel 日期
channel_dates_loaded: tokio::sync::OnceCell<()>,                  // 懒加载守卫（async 扫描只执行一次）
```

- **懒加载**：`query_channel_recent` 首次调用前 `get_or_init(扫描)` 一次——枚举 `<root>/<agent_id>/memory-store/<year>-<role_name>/channel-records-<date>.jsonl`，把 (agent, role) → date 填入集合（用 `DirectoryManager` 的 root_dir）。
- **增量更新**：`mark_channel_obsolete(key)` / `mark_channel_all_obsolete(key)` 里插入 `key.date`——这两个方法已被 `ChannelFileIndexHook::on_append / on_force_append` 调用，append 后日期自然进缓存，无需改 record.rs。懒加载扫描与增量插入幂等（BTreeSet 插入）。
- 不做 main.rs 启动扫描（懒加载即可）。

### query_channel_recent

```rust
pub async fn query_channel_recent(
    &self, agent_id: &str, role_name: &str, count: u32,
) -> Result<Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)>>
```

算法（参考 channel-web `get_recent` 的跨文件处理）：

1. count == 0 → 返回空。
2. 懒加载守卫：`self.channel_dates_loaded.get_or_init(扫描).await`。
3. 取 `channel_date_sets` 中 (agent_id, role_name) 的日期集合，**倒序遍历**（最新日期在前）：
   - `key = RecordKey { agent_id, role_name, date }`
   - `channel_indices.query_last(&key, remaining)`（kai-file 现成：ensure_updated + 分页尾部读取 + 组内升序）
   - 追加到结果；`remaining -= 取到条数`；`remaining == 0` 停止。
4. 结果组按日期升序返回（收集时倒序 → reverse）；组内记录已升序。
5. **无时间过滤**。

## 三、Agent 流程（kissbot-agent/src/memory_reader.rs）

`read_recent_for_context` 改为两次查询：

```
1. Query1 = POST {store}/store/query/channel/recent
   body: RecentQuery { agent_id, role_name, count = memory_count }
   → 最近 N 条（≤ N），按 time 升序
2. 解析 Query1（(time, sn) 去重）
   - 空 或 count == 0 → 返回空
   - 不足 N 条 → 直接返回（全史都在 Query1 里，无需 Query2）
3. ln = Query1 最旧一条的 time
   M = min(时间窗起点 start, ln)     // start 由 memory_time_secs 计算，字符串比较
4. Query2 = POST {store}/store/query/channel   // 恒发起（唯一跳过条件：步骤 2 直接返回）
   body: QueryRequest { agent_id, role_name, start_time = M, end_time = ln }   // 无 limit
   → [M, ln] 时间段全部记录（M == ln 时退化为单点，即 ln 同时间组）
5. 并集：Query2 结果用**同一个 (time, sn) seen 集**解析（Query1 已填过的记录自动跳过）
   → 合并后按 time 升序返回
```

`M = min(cutoff, ln)` 统一覆盖两分支，无需单独讨论 cutoff 与 ln 的大小关系：

| M 的取值 | Query2 窗口 | 结果 |
|---|---|---|
| M = cutoff（cutoff ≤ ln） | [cutoff, ln] | 最后 N ∪ 窗口内 [cutoff, ln] 全部 = **窗口内全部记录** |
| M = ln（cutoff > ln） | [ln, ln] 单点 | 最后 N ∪ ln 同时间组（ln 组在窗口外也包含，不拆散） |

### 与现有代码行为的差异（有意修正）

- 现有：单次全史查询 + `M = max`，cutoff > ln 时只返回最后 N（不含 ln 组）；cutoff ≤ ln 时只补 time == ln 组。
- 新：两次查询真并集，Query2 恒为 `[M, ln]`（M = min），cutoff ≤ ln 时取窗口内全部，cutoff > ln 时退化为单点补 ln 组。

## 四、测试

### kissbot-memory

- 跨多日期文件取最近 N：写 2~3 个日期文件（直接写盘或走 append），`query_channel_recent` 断言跨文件取满、组升序。
- count 超总量 → 返回全部；count == 0 → 空。
- 懒加载：文件预先存在（未经过 append hook）→ 首次 recent 查询能扫到。
- 增量：append（mark_channel_obsolete 路径）后新日期可查。
- 现有 `query_all`（时间范围）测试不变（QueryRequest 未动）。

### kissbot-agent

- mock store 增加 `/store/query/channel/recent` 语义（按 (time, sn) 排序取后 count 条）；`/store/query/channel` 保留范围查询语义。
- 重写 recent_memory 相关测试覆盖 `M = min` 的两种取值：cutoff ≤ ln（窗口内全部）、cutoff > ln（退化为 [ln, ln] 补 ln 组），以及不足 N、count 0、去重、(time,sn) 解析。
- `pack_memory_messages` / `is_self` 相关测试保持。

## 五、文档更新

- `docs/design/components-design/kissbot-agent-nexus.md`：第一级"单次全史查询"→"两次查询（最近 N + 时间段 [M, ln]）"，`M = max` → `M = min`，结果 = 并集。
- `docs/spec/kissbot-agent-modules.md`：memory_reader 组合查询描述、序列图同步。
- 旧规划 `docs/superpowers/plans/2026-08-05-model-context-system-refactor.md` Task 8（`QueryRequest.limit` + 每 messenger 文件聚合，基于已废弃设计）不再适用，本次以本设计为准。

## 涉及文件

- `kissbot-api/src/memory.rs`（RecentQuery）
- `kissbot-memory-store/src/api.rs`（端点）
- `kissbot-memory/src/index.rs`（date_sets + query_channel_recent + mark 钩子插入）
- `kissbot-memory/src/data.rs`（目录枚举辅助）
- `kissbot-agent/src/memory_reader.rs`（两次查询 + 并集）
- 文档两处
