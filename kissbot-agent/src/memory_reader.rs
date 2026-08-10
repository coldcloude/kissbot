use std::collections::HashSet;
use std::sync::Arc;

use serde_json::json;

use crate::config_manager::{ConfigManager, EffectiveContextConfig};
use crate::types::{Message, Mode, Result, Error};
use kissbot_api::memory::{ChannelRecord, QueryRequest, RecordKey};
use kissbot_api::{ApiResponse, Content};

/// query/channel 响应 data 类型：Vec<(RecordKey, Vec<(sn, ChannelRecord)>)>（组 → 记录列表）
type QueryChannelData = Vec<(RecordKey, Vec<(u32, ChannelRecord)>)>;

/// 记忆消息（channel record 的最小视图：name + content + is_self，id 类不保留）
#[derive(Debug, Clone)]
pub struct MemoryMsg {
    pub user_name: String,
    pub content: String,
    pub time: String,
    /// 是否 agent 自身消息（来自 ChannelRecord.is_self：1=self，0=他人）
    pub is_self: bool,
}

/// 打包记忆消息为交替的 User/Assistant 消息序列：
/// 找到第一条非 self（User）消息（之前的 self 消息丢弃，对话必须以 User 开头），
/// 连续同 is_self 的记录合并为一条消息，User/Assistant 交替；
/// User 段 content 逐行 "name: text"（name 为空只留 text），Assistant 段只保留 content（不含 name/time）；
/// 若以 User 结尾则补一条 Content 为空的 Assistant；空输入/无 User 返回空 Vec
pub fn pack_memory_messages(msgs: &[MemoryMsg]) -> Vec<Message> {
    // 对话必须以 User 开头：找到第一条非 self 消息
    let Some(first_user) = msgs.iter().position(|m| !m.is_self) else {
        return Vec::new();
    };
    let mut out: Vec<Message> = Vec::new();
    let mut user_buf: Vec<String> = Vec::new();
    let mut asst_buf: Vec<String> = Vec::new();
    let mut is_asst = msgs[first_user].is_self;  // 当前段类型（false=User 段）
    for m in &msgs[first_user..] {
        if m.is_self != is_asst {
            // 段类型切换：flush 上一段（连续同 is_self 已合并）
            if is_asst {
                out.push(Message::Assistant { content: Arc::new(asst_buf.join("\n")), reasoning_content: None, tool_calls: None });
            } else {
                out.push(Message::User { content: Arc::new(user_buf.join("\n")) });
            }
            user_buf.clear();
            asst_buf.clear();
            is_asst = m.is_self;
        }
        if m.is_self {
            asst_buf.push(m.content.clone());  // Assistant：只要 content，不带 name/time
        } else if m.user_name.is_empty() {
            user_buf.push(m.content.clone());
        } else {
            user_buf.push(format!("{}: {}", m.user_name, m.content));
        }
    }
    // flush 最后一段
    if is_asst {
        out.push(Message::Assistant { content: Arc::new(asst_buf.join("\n")), reasoning_content: None, tool_calls: None });
    } else {
        out.push(Message::User { content: Arc::new(user_buf.join("\n")) });
        // 以 User 结尾：补一条空 Assistant（模型对话需以待回答的 Assistant 结尾）
        out.push(Message::Assistant { content: Arc::new(String::new()), reasoning_content: None, tool_calls: None });
    }
    out
}

pub struct MemoryReader {
    client: reqwest::Client,
    /// memory-store 根地址（构造时从全局 ApiConfig 读取；测试可注入覆盖）
    store_url: String,
}

impl MemoryReader {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            store_url: kissbot_api::ApiConfig::get().memory_store_url.clone(),
        }
    }

    /// 从 memory-struct 读取顶层记忆索引（摘要列表）
    /// 在启动、切换模式、重置时作为初始上下文的一部分
    /// 注意：memory-struct 尚未实现，此方法当前调用无实际效果
    /// agent_id/role_name 为 coordinator 传入的当前运行状态快照
    #[allow(dead_code)]
    pub async fn read_memory_struct_index(
        &self,
        config: &ConfigManager,
        _agent_id: &str,
        _role_name: &str,
        _mode: &Mode,
    ) -> Result<Vec<String>> {
        let structs = config.memory_structs().await;
        if structs.is_empty() {
            return Ok(Vec::new());
        }
        // memory-struct 功能未实现，暂占位
        Ok(Vec::new())
    }

    /// 查询事件列表
    /// agent_id/role_name 为 coordinator 传入的当前运行状态快照
    #[allow(dead_code)]
    pub async fn list_events(
        &self,
        _config: &ConfigManager,
        agent_id: &str,
        role_name: &str,
    ) -> Result<Vec<String>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();

        let url = format!("{}/events", store_url.trim_end_matches('/'));

        let body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
        });

        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("查询事件失败: {}", e)))?;

        let data: serde_json::Value = resp.json().await?;
        let events = data["events"].as_array()
            .map(|arr| {
                arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
            })
            .unwrap_or_default();

        Ok(events)
    }

    /// role 模式记忆打包：单次全史查询 → 解析 → 并集算法 → 升序结果
    /// （channel 记录已合并为单文件，无需组合枚举；query_channel / parse_channel_records / recent_memory 均为单处引用，已内联于此）
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

        // ===== 单次全史查询（query_channel 内联）：POST {store}/store/query/channel（QueryRequest，所有 channel 记录同文件，取该时间全部） =====
        let url = format!("{}/store/query/channel", self.store_url.trim_end_matches('/'));
        let body = QueryRequest {
            agent_id: Arc::new(agent_id.to_string()),
            role_name: Arc::new(role_name.to_string()),
            start_time: Arc::new("2000-01-01 00:00:00".to_string()),  // 全史查询，范围覆盖即可
            end_time: Arc::new(now),
        };
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("记忆读取返回 {}", resp.status())));
        }
        // 响应反序列化为类型化 ApiResponse<QueryChannelData>（tuple 由 serde 解析，无需手拼索引）
        let resp: ApiResponse<QueryChannelData> = resp.json().await?;
        let groups = resp.data.unwrap_or_default();

        // ===== 解析（parse_channel_records 内联）：按 time 升序转 MemoryMsg =====
        // 先去重再转 MemoryMsg：以 (time, sn) 为唯一键过滤重复记录（sn 为同文件内唯一序号），
        // 保证后续所有分支（含不足 N 条直接返回）都不含重复记录
        let mut seen: HashSet<(String, u64)> = HashSet::new();
        let mut msgs: Vec<MemoryMsg> = Vec::new();
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
        msgs.sort_by(|a, b| a.time.cmp(&b.time));

        // ===== 并集算法（recent_memory 内联，修正设计 2.3）：(1) 取最后 N 条 → T_N；(2) M = max(cutoff, T_N)；
        // (3) [M, T_N]（含两端）取该时间全部；最终 = (1) ∪ (3)，时间正序
        // msgs 为解析阶段已按 (time, sn) 去重的全史升序结果；不足 N 条已全取，直接返回 =====
        let count = cfg.memory_count;
        if msgs.is_empty() || count == 0 {
            return Ok(Vec::new());
        }
        // 不足 N 条：已全部取出，直接返回（无需 T_N/M/(3)，避免空并集计算）
        if msgs.len() <= count {
            return Ok(msgs);
        }
        let start_idx = msgs.len() - count;
        let t_n = &msgs[start_idx].time;   // 最后 N 条最旧一条
        // 窗口起点晚于 T_N（M = start > T_N）：(3) 区间 [M, T_N] 为空 → 结果 = 最后 N 条
        if start.as_str() > t_n.as_str() {
            return Ok(msgs[start_idx..].to_vec());
        }
        // 窗口覆盖 T_N（M = T_N）：(3) 取全部 time == T_N 的记录；(3) ∪ (1) 等价于从第一条
        // time == T_N 的记录取到末尾（T_N 同时间组必横跨 start_idx，天然无重复，无需末尾排序去重）
        let head = msgs.iter().position(|m| m.time == *t_n).expect("T_N 记录必然存在（msgs[start_idx]）");  // 第一条 T_N 同时间记录
        Ok(msgs[head..].to_vec())
    }
}

/// 从 Content 枚举提取文本：Text 取内容；Multi 取其中 Text 子项拼接；其余（附件/系统通知/Think/ToolCall/ToolResult 等）返回空串（调用方跳过）
/// 注意：Content 用 #[serde(tag="msg_type", content="data")] 序列化，反序列化后为类型化枚举，直接匹配即可
fn extract_record_text(content: &Content) -> String {
    match content {
        Content::Text(text) => text.as_str().to_string(),
        Content::Multi(items) => items.iter()
            .filter_map(|c| match c {
                Content::Text(text) => Some(text.as_str().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;
    use axum::routing::post;
    use axum::{Json, Router};
    use tokio::net::TcpListener;

    // 测试用上下文配置（非记忆字段用默认值占位）
    fn ctx_config(memory_time_secs: u64, memory_count: usize) -> EffectiveContextConfig {
        EffectiveContextConfig {
            channel_batch_interval_secs: 0,
            memory_time_secs,
            memory_count,
            compress_prompt: String::new(),
            stations: Default::default(),
        }
    }

    // 相对 now 的时间字符串（与 read_recent_for_context 内部同格式）
    fn time_ago(secs: i64) -> String {
        chrono::Local::now()
            .checked_sub_signed(chrono::Duration::seconds(secs))
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    // 构造完整 ChannelRecord 的 JSON（与真实 store 序列化一致，typed 反序列化需要全部字段；sn 需调用方指定，channel_data 会按序号覆盖）
    fn channel_record_json(time: &str, user_name: &str, content: serde_json::Value, is_self: bool, sn: u64) -> serde_json::Value {
        json!({
            "user_id": "",
            "self_user_id": "",
            "messenger_id": "",
            "group_id": "",
            "is_self": if is_self { 1 } else { 0 },
            "messenger_name": "",
            "user_name": user_name,
            "group_name": "",
            "content": content,
            "time": time,
            "sn": sn,
        })
    }

    // 单条 Text ChannelRecord 的 JSON（is_self=0）
    fn record_json(time: &str, user_name: &str, content: &str) -> serde_json::Value {
        channel_record_json(time, user_name, json!({ "msg_type": "Text", "data": content }), false, 0)
    }

    // 带 is_self 的 Text ChannelRecord JSON（is_self=1 表示 agent 自身消息）
    fn record_json_self(time: &str, user_name: &str, content: &str, is_self: bool) -> serde_json::Value {
        channel_record_json(time, user_name, json!({ "msg_type": "Text", "data": content }), is_self, 0)
    }

    // query/channel 响应 data：Vec<(RecordKey, Vec<(sn, ChannelRecord)>)>；按序号注入 sn 到每条记录（同文件内唯一）
    fn channel_data(records: Vec<serde_json::Value>) -> serde_json::Value {
        let entries: Vec<serde_json::Value> = records.iter().enumerate()
            .map(|(sn, r)| {
                let mut rec = r.clone();
                rec["sn"] = json!(sn as u64);
                json!([sn, rec])
            })
            .collect();
        // group key 为 RecordKey（agent_id/role_name/date），与真实 store 返回一致
        json!([[{ "agent_id": "agent", "role_name": "role", "date": "2026-08-05" }, entries]])
    }

    // 启动本地 mock memory-store（POST /store/query/channel 固定返回该 data），返回根地址
    async fn start_mock_store(data: serde_json::Value) -> String {
        let app = Router::new().route("/store/query/channel", post(move || async move {
            Json(json!({ "success": true, "data": data, "error": null }))
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{}", addr)
    }

    // 测试构造：指定 memory-store 根地址（覆盖 new() 的 ApiConfig 读取，避免与 http_server 测试的全局配置冲突）
    fn reader_at(url: &str) -> MemoryReader {
        MemoryReader {
            client: reqwest::Client::new(),
            store_url: url.to_string(),
        }
    }

    #[tokio::test]
    async fn read_recent_dense_keeps_order_and_same_time_group() {
        // dense 场景：30 条全部落在窗口内（now-1800s ~ now-60s），最后 N=10
        let records: Vec<serde_json::Value> = (0..30)
            .map(|i| record_json(&time_ago(1800 - i * 60), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        let cfg = ctx_config(7200, 10);  // 窗口起点 now-7200s 早于所有记录 → M = T_N
        let out = reader_at(&url).read_recent_for_context("agent", "role", &cfg).await.unwrap();
        // 结果 = 最后 10 条（m20..m29），时间正序
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].content, "m20");
        assert!(out.windows(2).all(|w| w[0].time <= w[1].time), "时间正序");
    }

    #[tokio::test]
    async fn read_recent_sparse_extends_beyond_window() {
        // 稀疏分支：len > count（15 条）且全部记录早于窗口起点 cutoff=now-60s
        // → T_N < cutoff → M = cutoff → (3) 过滤 [cutoff, T_N] 为空 → 结果 = 最后 N 条（跨更早时间）
        let records: Vec<serde_json::Value> = (0..15)
            .map(|i| record_json(&time_ago(600 - i * 20), "u", &format!("m{}", i)))
            .collect();
        let url = start_mock_store(channel_data(records)).await;
        let cfg = ctx_config(60, 10);  // 窗口起点 now-60s 晚于全部记录
        let out = reader_at(&url).read_recent_for_context("agent", "role", &cfg).await.unwrap();
        assert_eq!(out.len(), 10, "稀疏场景结果 = 最后 10 条（跨更早时间）");
        assert_eq!(out[0].content, "m5", "起点 = 第 6 条（最后 10 条最旧一条）");
    }

    #[tokio::test]
    async fn read_recent_empty_and_less_than_n() {
        // 空数据 → 空结果
        let url = start_mock_store(channel_data(vec![])).await;
        let out = reader_at(&url).read_recent_for_context("agent", "role", &ctx_config(7200, 10)).await.unwrap();
        assert!(out.is_empty(), "空数据返回空");

        // 不足 N 条取全部
        let records = vec![
            record_json(&time_ago(60), "u", "a"),
            record_json(&time_ago(120), "u", "b"),
        ];
        let url = start_mock_store(channel_data(records)).await;
        let out = reader_at(&url).read_recent_for_context("agent", "role", &ctx_config(7200, 10)).await.unwrap();
        assert_eq!(out.len(), 2, "不足 N 条取全部");

        // count = 0：不参与并集，直接返回空（防 start_idx = len 空切片 panic）
        let url = start_mock_store(channel_data(vec![
            record_json(&time_ago(60), "u", "a"),
            record_json(&time_ago(120), "u", "b"),
        ])).await;
        let out = reader_at(&url).read_recent_for_context("agent", "role", &ctx_config(7200, 0)).await.unwrap();
        assert!(out.is_empty(), "count=0 返回空");
    }

    #[tokio::test]
    async fn read_recent_includes_same_time_group_beyond_n() {
        // 同时间组横跨 N 边界：索引 4,5,6 同在 T=now-200s，count=10 → start_idx=5 →
        // 最后 10 条 = 索引 5..14，T_N = now-200s；索引 4 在 start_idx 之前但同在 T_N → 应被 (3) 取回
        let t0 = time_ago(300);
        let t1 = time_ago(200);
        let t2 = time_ago(100);
        let mut records = Vec::new();
        for i in 0..4 { records.push(record_json(&t0, "u", &format!("m{}", i))); }
        for i in 4..7 { records.push(record_json(&t1, "u", &format!("m{}", i))); }
        for i in 7..15 { records.push(record_json(&t2, "u", &format!("m{}", i))); }
        let url = start_mock_store(channel_data(records)).await;
        let cfg = ctx_config(7200, 10);  // cutoff=now-7200s ≤ T_N → M = T_N
        let out = reader_at(&url).read_recent_for_context("agent", "role", &cfg).await.unwrap();
        // 结果 = 最后 10 条（索引 5..14）∪ 同 T_N 的索引 4 = 11 条，升序
        assert_eq!(out.len(), 11, "T_N 同时间组横跨 N 边界时完整保留（含 start_idx 之前同时间记录）");
        assert_eq!(out[0].time, t1, "起点 = T_N 同时间组");
        assert_eq!(out[0].content, "m4", "start_idx 之前的同时间记录被 (3) 取回");
        assert!(out.windows(2).all(|w| w[0].time <= w[1].time), "时间正序");
    }

    #[tokio::test]
    async fn read_recent_skips_non_text_records_and_joins_multi() {
        // Multi 内容取其中 Text 子项拼接；非文本记录跳过（parse_channel_records 内联行为）
        let t = time_ago(60);
        // 非 Text 变体（ToolCall 等）内容为空 → 跳过；Multi 取其中 Text 子项拼接
        // 注意：旧测试数据用的 "Image" 不是真实 Content 变体，typed 反序列化会失败
        let data = json!([[{ "agent_id": "agent", "role_name": "role", "date": "2026-08-05" }, [
            [0, channel_record_json(&t, "u1", json!({ "msg_type": "Text", "data": "你好" }), false, 0)],
            [1, channel_record_json(&t, "u2", json!({ "msg_type": "Multi", "data": [
                { "msg_type": "Text", "data": "a" }, { "msg_type": "ToolCall", "data": "key1" }, { "msg_type": "Text", "data": "b" }
            ] }), false, 1)],
            [2, channel_record_json(&t, "u3", json!({ "msg_type": "ToolCall", "data": "key2" }), false, 2)],
        ]]]);
        let url = start_mock_store(data).await;
        let out = reader_at(&url).read_recent_for_context("agent", "role", &ctx_config(7200, 10)).await.unwrap();
        assert_eq!(out.len(), 2, "非文本记录跳过");
        assert_eq!(out[0].content, "你好");
        assert_eq!(out[1].content, "a\nb", "Multi 内容取 Text 子项拼接");
    }

    #[tokio::test]
    async fn read_recent_dedups_duplicate_records_by_time_sn_at_parse() {
        // store 响应含重复记录（同 time+sn）：解析阶段先去重（而非并集末尾），
        // 不足 N 条直接返回的路径也不泄漏重复；不同 sn 的同内容记录不被误合并
        let t = time_ago(60);
        let data = json!([[{ "agent_id": "agent", "role_name": "role", "date": "2026-08-05" }, [
            [0, channel_record_json(&time_ago(120), "u", json!({ "msg_type": "Text", "data": "a" }), false, 0)],
            [1, channel_record_json(&t, "u", json!({ "msg_type": "Text", "data": "hello" }), false, 1)],
            [2, channel_record_json(&t, "u", json!({ "msg_type": "Text", "data": "hello" }), false, 1)],  // 同 time+sn 重复 → 去重
            [3, channel_record_json(&t, "u", json!({ "msg_type": "Text", "data": "hello" }), false, 2)],  // 不同 sn → 保留
        ]]]);
        let url = start_mock_store(data).await;
        // 不足 N 条（4 → count=10）：直接返回 msgs，重复记录也不得泄漏
        let out = reader_at(&url).read_recent_for_context("agent", "role", &ctx_config(7200, 10)).await.unwrap();
        assert_eq!(out.len(), 3, "同 (time, sn) 重复只保留一条，不同 sn 的同内容记录保留");
        assert_eq!(out.iter().filter(|m| m.content == "hello").count(), 2, "同秒同内容但 sn 不同 → 两条都保留");
    }

    #[tokio::test]
    async fn read_recent_extracts_is_self_and_packs_alternating() {
        // 全链路：mock store 返回 [self, self, user, user, self, user] → 解析提取 is_self → 打包交替 User/Assistant
        let records = vec![
            record_json_self(&time_ago(300), "agent", "a0", true),
            record_json_self(&time_ago(280), "agent", "a1", true),
            record_json_self(&time_ago(260), "u1", "m0", false),
            record_json_self(&time_ago(240), "u2", "m1", false),
            record_json_self(&time_ago(220), "agent", "a2", true),
            record_json_self(&time_ago(200), "u3", "m2", false),
        ];
        let url = start_mock_store(channel_data(records)).await;
        let msgs = reader_at(&url).read_recent_for_context("agent", "role", &ctx_config(7200, 100)).await.unwrap();
        let out = pack_memory_messages(&msgs);
        // 期望：[User("u1: m0\nu2: m1"), Assistant("a2"), User("u3: m2"), Assistant("")]
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u1: m0\nu2: m1"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.as_str() == "a2"));
        assert!(matches!(&out[2], Message::User { content } if content.as_str() == "u3: m2"));
        assert!(matches!(&out[3], Message::Assistant { content, .. } if content.is_empty()));
    }

    // 构造测试用 MemoryMsg
    fn msg(name: &str, content: &str, is_self: bool) -> MemoryMsg {
        MemoryMsg { user_name: name.into(), content: content.into(), time: "t".into(), is_self }
    }

    #[test]
    fn pack_memory_messages_alternates_merges_and_appends_empty_assistant() {
        let msgs = vec![
            msg("agent", "a0", true),   // 开头 self → 丢弃（对话必须以 User 开头）
            msg("u1", "m0", false),
            msg("", "m1", false),       // 空 name → 只留 content
            msg("agent", "line1", true),
            msg("agent", "line2", true),
            msg("u3", "m2", false),
        ];
        let out = pack_memory_messages(&msgs);
        // 期望：[User("u1: m0\nm1"), Assistant("line1\nline2"), User("u3: m2"), Assistant("")]
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u1: m0\nm1"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.as_str() == "line1\nline2"));
        assert!(matches!(&out[2], Message::User { content } if content.as_str() == "u3: m2"));
        assert!(matches!(&out[3], Message::Assistant { content, .. } if content.is_empty()));
    }

    #[test]
    fn pack_memory_messages_empty_or_all_self_returns_empty() {
        assert!(pack_memory_messages(&[]).is_empty());
        // 全 self（无 User 开头）→ 无可打包
        assert!(pack_memory_messages(&[msg("agent", "a", true), msg("agent", "b", true)]).is_empty());
    }

    #[test]
    fn pack_memory_messages_single_user_appends_empty_assistant() {
        let out = pack_memory_messages(&[msg("u", "hi", false)]);
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u: hi"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.is_empty()));
    }
}
