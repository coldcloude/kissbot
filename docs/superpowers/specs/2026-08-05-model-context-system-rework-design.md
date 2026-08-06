# 模型上下文系统重构修正设计（Rework）

## 目标

基于 2026-08-05 主重构设计（已实现）的四项修正：①合批重置语义改为「重置等待+统一合并」；②撤销 memory-store 的 limit/聚合扩展，改为「组合查询 API + 每组合时间查询 + 并集算法 + key 关联」；③补 ToolCall/ToolResult Content 变体与 channel 占位记录，ToolCall/ToolResult 同 key；④DashMap 读锁不跨 await。

## 1. 合批重置语义（修正防竞态方案）

原实现用 `batch_gen`（代数）在重置时使延时任务失效并清空缓冲。修正后：

- **删除** `Session.batch_gen` 与 enqueue_batch 中的代数校验、reset_context 中的 `fetch_add` 与 `batch.clear()`
- **新增** `Session.resetting: Arc<AtomicBool>`：`reset_context` 开始时置 true、结束置 false
- 延时任务超时醒来：若 `resetting` 为 true → 继续等待（循环 sleep 轮询）直至重置完成；重置完成后立即打包一次
- 语义：重置期间**不触发超时打包**，期间到达的消息**不清空**，统一并入重置后的一次打包（不丢消息、不串话）
- 打包入口不变：`take()` 全部 → `pack_batch` → `run_agentic_loop`

## 2. memory-store 修正与记忆打包流程

### 2.1 撤销原扩展

- `kissbot-api/src/memory.rs`：删除 `QueryChannelRequest.limit` / `QueryRequest.limit`（复原）
- `kissbot-memory/src/index.rs`：删除 `query_channel_aggregate`、`take_recent`，恢复原 `query_channel_records`（精确 key 时间区间查询）
- 删除 Task 8 新增的目录聚合测试；`kissbot-memory-store` 不动

### 2.2 新增组合查询 API

memory-store 新增端点：**查询 (agent, role, 时间范围) 对应的 (messenger, user, group) 组合列表**。

- 实现：枚举 `<root>/<agent_id>/memory-store/<year>-<role_name>/channel-*.jsonl` 文件名，解析出 `(messenger, user, group)` 组合（不读取记录内容）
- 请求体：`{agent_id, role_name, start_time, end_time}`；响应：组合列表（去重）
- 用途：agent 先拿组合，再对每个组合用现有精确 key 时间区间 API 查询

### 2.3 记忆打包流程（role 模式，修正后）

```
输入：agent_id, role_name, memory_time_secs, memory_count, 当前时间 now
cutoff = now - memory_time_secs

(1) 取最后 N 条（N = memory_count）：
    对每个组合查询 [2000-01-01 00:00:00, now]（取该时间全部，无 limit），
    跨组合合并升序，客户端截取最后 N 条；
    T_N = 这 N 条中最旧一条的时间（不足 N 条时取全局最旧一条）
(2) M = max(cutoff, T_N)
(3) 对每个组合查询 [M, T_N]（含两端，取该时间全部）
最终结果 = (1) ∪ (3) 的并集，按时间正序排列
```

要点：
- 每组合的查询用现有精确 key（messenger/user/group）时间区间 API，**无 limit**，取该时间全部
- 无 limit 意味着「最后 N 条」由客户端在合并结果上截取（(1) 的查询范围取全史，保证稀疏场景也能取到 N 条）
- **(3) 的终点是 T_N（含）**：当倒数第 N 条时间点有多条同时间记录（如倒数 9～13 条都在 T_N）时，[M, T_N] 会取回 N 之外的同时间记录（11～13 条），与 (1) 并集后不按计数截断拆散同一时间组
- 稀疏场景（窗口不足 N 条）：T_N < cutoff → M = cutoff → (3) = [cutoff, T_N] 为空（start > end）→ 结果 = 最后 N 条跨更早时间；窗口更大场景：M = T_N → (3) = [T_N, T_N] 仅同时间组 → 结果 = 窗口全量（含同时间组）
- 结果仅保留 channel 记录的 name+content（与主设计一致）

### 2.4 think/tool-call/tool-result 的 key 关联取回

- agent 先从 channel 记录中收集 key（`Content::Think(key)`、`Content::ToolCall(key)`、`Content::ToolResult(key)` 占位记录携带）
- think/tool-call/tool-result 详情的取回：按「key 出现在上述 channel 记录集合中」过滤
- 本轮记忆打包的 user 消息仍只含 channel 的 name+content；key 关联机制为详情还原打基础

## 3. ToolCall/ToolResult Content 变体与占位记录

- `kissbot-api` 的 `Content` 枚举**新增变体** `ToolCall(Arc<String>)` 与 `ToolResult(Arc<String>)`（key 参数），仿 `Think(Arc<String>)`；同步所有匹配点（channel-web 渲染、extract_text、memory-store 等）
- agentic loop 中每个 tool call：生成 UUID key → 写 channel 占位记录（`Content::ToolCall(key)`，仿 think 的 ChannelRecord 流程）→ `ToolCallRequest.key = key`；工具结果 → `ToolResultRequest.key = 同一 key`（ToolResult 用 ToolCall 的 key）
- 这样 channel 时间线可见工具调用锚点，think/tool 详情经 key 关联

## 4. DashMap 读锁不跨 await

- `StationRuntime::call_tool`：`local_tools.get(name).map(|r| r.value().clone())`——从 Ref 克隆出 `Arc<dyn Tool>` 立即释放读锁，再 `tool.call(params).await`
- `execute_tool_call`：先把 `station_runtimes` 迭代结果收集为 `Vec<(String, Arc<StationRuntime>)>`（克隆后释放全局读锁），再逐项 `call_tool().await`

## 5. 受影响文件

- `kissbot-agent/src/batching.rs`、`session_manager.rs`、`coordinator.rs`（合批重置 + loop 工具 key + execute_tool_call + reset_context）
- `kissbot-agent/src/memory_reader.rs`（记忆打包流程重写：组合查询 + 并集算法 + key 关联）
- `kissbot-agent/src/station.rs`（call_tool 克隆释放读锁）
- `kissbot-agent/src/context_config.rs`（memory_count 保留复用；算法改动不涉及配置结构）
- `kissbot-api/src/memory.rs`（撤销 limit；新增组合查询请求/响应结构）
- `kissbot-api/src/message.rs`（Content 新增 ToolCall/ToolResult 变体）
- `kissbot-memory/src/index.rs`（撤销聚合/limit；新增组合枚举）
- `kissbot-memory-store/src/api.rs`（新增组合端点路由）
- `kissbot-channel-web` 等 Content 匹配点同步
- 相关测试、组件设计文档、`script/README.md` 手动验证清单同步更新

## 6. 测试范围

- 合批：resetting 等待语义（重置期间消息并入重置后打包）
- 记忆打包：并集算法（含 T_N 与 cutoff 关系的工作示例）、组合查询、key 关联过滤
- Content 新变体 serde roundtrip；loop 工具占位记录 + ToolCall/ToolResult 同 key 断言
- DashMap 克隆释放（编译层 + 现有 station 测试适配）
- memory-store 组合 API 单测（枚举文件名解析组合）
