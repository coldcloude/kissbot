# Channel 记录存储重构设计

日期：2026-08-07

## 背景与目标

当前 memory 中 channel 记录按 (messenger, user, group) 组合拆分文件：
`channel-{messenger_id}={user_id}={group_id}-records-{date}.jsonl`，
配套专用 key 类型 `ChannelRecordKey`、查询类型 `QueryChannelRequest`、
组合枚举 API `/store/query/combos`。agent 读取记忆需先枚举组合、再逐组合全史查询后合并。

本次重构目标：

1. channel 记录不再按 messenger/user/group 拆分，与 think / tool-call / tool-result 一样
   每 (agent, role, date) 一个文件
2. `ChannelRecord` 增加 `messenger_id`、`group_id`、`self_user_id` 字段
3. channel 消息使用公共的 `RecordKey` 和 `QueryRequest`，删除组合查询 API
4. agent 查询记忆过程简化

## 当前实现（现状）

- 文件：`channel-{m}={u}={g}-records-{date}.jsonl`，每组合一个文件
- 类型：`ChannelRecordKey{agent_id, role_name, messenger_id, user_id, group_id, date}`（专用 key）、
  `QueryChannelRequest`（专用查询）、`ChannelCombo{messenger_id, user_id, group_id}`（组合类型）
- 文件内 `ChannelRecord{user_id, is_self, messenger_name, user_name, group_name, content, time, sn}`——
  不含 messenger_id / group_id / self_user_id；user_id 为发送者
- 写入：`ChannelRequest.self_user_id`（接收方 = agent 绑定用户）放入 key.user_id 决定文件名，
  record.user_id 保留发送者身份
- 查询：`/store/query/channel`（精确组合）、`/store/query/combos`（枚举文件得到组合列表）
- agent 读取：`query_combos`（全史）→ 逐组合 `query_channel`（全史）→ 合并 → `recent_memory` 并集算法

## 设计

### §1 文件与记录类型

- 文件：`channel-records-{date}.jsonl`，每 (agent, role, date) 一个，与 think/tool-call/tool-result 并列；
  删除 `channel-{m}={u}={g}-records-{date}.jsonl` 结构
- `ChannelRecord` 增加三个字段（`Arc<String>`）：`messenger_id`、`group_id`、`self_user_id`；
  `user_id` 保留为发送者身份
- 删除类型：`ChannelRecordKey`、`QueryChannelRequest`、`ChannelCombo`（含 serde 测试）
- `ChannelParser` 改为：
  - `FilePathGenerator<RecordKey>` → 路径 `channel-records-{date}.jsonl`
  - `RequestParser<ChannelRequest, RecordKey, ChannelRecord>` → key = `RecordKey{agent_id, role_name, date}`；
    request 字段不变，`messenger_id` / `group_id` / `self_user_id` 直接拷入 record
  - `QueryParser<QueryRequest, RecordKey>` → 复用现有公共 `parse_query`
- `MemoryIndexer`：`channel_indices` 改为 `FileIndexContext<QueryRequest, RecordKey, ChannelRecord, ChannelParser>`；
  `mark_channel_obsolete` / `mark_channel_all_obsolete` 参数改为 `&RecordKey`
- `memory-store`：`ChannelFileIndexHook` 改 `FileHook<RecordKey>`，
  `ChannelAppender = RecordAppender<RecordKey, ChannelRecord, ChannelParser, ChannelFileIndexHook>`

### §2 查询 API

- `/store/query/channel` 端点保留，请求体改为 `QueryRequest`
  （agent_id + role_name + start_time + end_time），响应形状与其他三类一致：
  `Vec<(RecordKey, Vec<(u32, Arc<ChannelRecord>)>)>`
- 删除 `/store/query/combos` 路由及 `query_combos` handler（memory-store api.rs）
- `MemoryIndexer::query_channel_records` 参数改为 `QueryRequest`
- `query_combos`（index.rs 的目录枚举逻辑）及 `ChannelCombo` 相关测试全部删除

### §3 agent 查询简化

`MemoryReader`（kissbot-agent/src/memory_reader.rs）：

- 删除 `query_combos`；`query_channel` 去掉 `combo` 参数改为单次查询：
  POST `/store/query/channel`，body 为 `QueryRequest`
  （agent_id、role_name、start="2000-01-01 00:00:00"、end=now），全史一次取回
- `read_recent_for_context` 改为：单次全史查询 → `parse_channel_records` 解析 →
  `recent_memory(&msgs, count, window_start)` 并集算法（算法与行为不变，仅查询链路简化）
- `parse_channel_records` 解析逻辑不变（响应结构未变），
  `MemoryMsg` / `pack_memory_messages` / `extract_record_text` 不变
- 删除 `ChannelCombo` 导入

### §4 is_self 与 self_user_id 语义

写入端（coordinator 现有逻辑）不变，语义在 spec 中明确记录，防止未来消费者混用：

- `self_user_id`：agent 在 channel 绑定的用户（agent 视角的 self / 接收方身份）。
  上行消息取自 `event.recipient_user_id`；下行消息等于 agent 自己的 user_id
- `user_id`：消息发送者身份。同一 channel 双向消息归入同一文件后，
  靠 `user_id`（发送者）与 `self_user_id`（绑定用户）区分方向
- `is_self`：**只有 agent 实际发送的消息才是 1**（下行确认成功后写入；
  agent 经 msg_id 回显匹配 `is_self_echo_by_msg_id` 识别并过滤自身回声，不落库）。
  **不**由 `self_user_id == user_id` 推导
- 典型场景：其他人使用 agent 绑定的用户发送消息时，`user_id == self_user_id`，但 `is_self = 0`

### §5 测试与文档

测试更新：

- `kissbot-api/src/memory.rs`：删除 `QueryChannelRequest` / `ChannelCombo` serde 测试；
  所有 `ChannelRecord` 构造补 `messenger_id` / `group_id` / `self_user_id` 字段
- `kissbot-memory/src/data.rs`：文件名测试改为 `channel-records-{date}.jsonl`；
  `test_channel_request_parser` 断言 key 为 `RecordKey`（无 messenger/user/group）、record 含新字段
- `kissbot-memory/src/index.rs`：`test_mark_and_query_channel` 改为新文件名 + `RecordKey` + `QueryRequest`
  + 新字段 JSON；删除 `test_query_combos_*` 三个测试
- `kissbot-memory-store/src/record.rs`：channel 追加测试改为单文件结构
  （不同 messenger/user/group 的请求落入同一文件、sn 连续）

项目文档更新（`docs/spec/` 为主，与 superpowers 的 spec 目录区分）：

- `docs/spec/memory-directory.md`：目录结构、文件格式表
  （`channel-records-{date}.jsonl`、ChannelRecord 新字段、is_self 与 self_user_id 语义说明）
- `docs/spec/memory-index.md`：channel 改用公共 key/query，删除组合枚举描述
- `docs/design/components-design/kissbot-memory.md`：记录类型定义、查询能力描述同步
- `docs/design/components-design/kissbot-memory-store.md`：如涉及组合结构则同步
- `docs/plan/components-plan/` 相关计划文档：如提到组合文件结构则同步
- 其余 `docs/spec/*.md` 如无涉及组合结构则不改

## 不做的事（YAGNI）

- 不迁移旧数据：旧 `channel-{m}={u}={g}-records-{date}.jsonl` 直接弃用（项目早期，均为测试数据）
- 不统一四个 parser / 查询 handler 为泛型实现（本次仅把 channel 对齐到公共 key/query）
- 不保留 `ChannelRecordKey` 等兼容别名（项目内无其他消费者）

## 改动文件清单（代码）

- `kissbot-api/src/memory.rs`：删类型与测试、ChannelRecord 加字段
- `kissbot-memory/src/data.rs`：ChannelParser 改用 RecordKey/QueryRequest、文件名、测试
- `kissbot-memory/src/index.rs`：channel_indices 泛型参数、删 query_combos、测试
- `kissbot-memory-store/src/api.rs`：删 combos 路由/handler、channel 查询改 QueryRequest
- `kissbot-memory-store/src/record.rs`：ChannelFileIndexHook 改 RecordKey、测试
- `kissbot-agent/src/memory_reader.rs`：单次查询、删 combos/逐组合逻辑、测试
