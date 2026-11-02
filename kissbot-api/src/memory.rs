use std::sync::Arc;

use arcstr::ArcStr;
use kai_file::index::Record;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Content, ContentBlock};

// ========== 记忆后缀 ==========

/// 记忆后缀：角色 + 可选事件。memory 相关组件对外通信统一使用。
///
/// - 角色模式：`event_id` 为 `None`；
/// - 事件模式：`event_id` 为 `Some(事件标识)`。
///
/// 拼成文件/目录名（`{role_name}` 或 `{role_name}-{event_id}`）只在 memory 内部构造路径时进行。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RoleEvent {
    pub role_name: ArcStr,
    /// 事件标识；角色模式为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<ArcStr>,
}

impl RoleEvent {
    /// 角色模式（无事件）
    pub fn new(role_name: ArcStr) -> Self {
        Self { role_name, event_id: None }
    }

    /// 事件模式（角色 + 事件标识）
    pub fn with_event(role_name: ArcStr, event_id: ArcStr) -> Self {
        Self { role_name, event_id: Some(event_id) }
    }
}

// ========== 记录元信息 ==========

/// 摘要记忆信息：记录所属的 case。
///
/// 由摘要记忆的使用方（agent 侧的摘要记忆 pipeline）在写入记忆时填入：摘要记忆组件读到带该信息的
/// 记录时直接按 case 名称定位，不再走分组/逐层判定流程。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySummaryMeta {
    /// 这条记录所属的 case 名称
    pub case_name: ArcStr,
}

/// 记录元信息：各组件可选的附加信息（未识别的字段在反序列化时忽略）。
///
/// 只有通道记录携带 meta；思考/工具内容作为通道记录 `content` 的一部分随之归属，不需要单独记录。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryMeta {
    /// 摘要记忆信息
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_summary: Option<MemorySummaryMeta>,
}

impl MemoryMeta {
    /// 只带摘要记忆 case 名的元信息
    pub fn with_case_name(case_name: ArcStr) -> Self {
        Self {
            memory_summary: Some(MemorySummaryMeta { case_name }),
        }
    }

    /// 摘要记忆 case 名（没有该信息时为 None）
    pub fn case_name(&self) -> Option<&ArcStr> {
        self.memory_summary.as_ref().map(|meta| &meta.case_name)
    }
}

// ========== Request structures ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequest {
    /// 消息标识（来自 incoming_message / outgoing_message_response）
    #[serde(default)]
    pub msg_id: ArcStr,
    pub agent_id: ArcStr,
    pub role_event: Arc<RoleEvent>,
    pub messenger_id: ArcStr,
    pub user_id: ArcStr,
    /// 接收方身份（= channel 绑定的 user_id，即 agent 视角的 self；与 is_self 不同，其他人用绑定用户发消息时 user_id == self_user_id 但 is_self == 0）
    pub self_user_id: ArcStr,
    pub group_id: ArcStr,
    pub is_self: usize,
    pub messenger_name: ArcStr,
    pub user_name: ArcStr,
    pub group_name: ArcStr,
    pub content: Content,
    pub time: ArcStr,
    /// 可选的记录元信息（摘要记忆用它记录所属 case）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Arc<MemoryMeta>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequests {
    pub requests: Vec<ChannelRequest>,
    pub force: usize,
}

// ========== Query requests ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub agent_id: ArcStr,
    pub role_event: Arc<RoleEvent>,
    pub start_time: ArcStr,
    pub end_time: ArcStr,
}

/// 最近 N 条 channel 记录查询（无时间参数：取该 agent+role 最近 count 条，跨日期文件）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentQuery {
    pub agent_id: ArcStr,
    pub role_event: Arc<RoleEvent>,
    pub count: u32,
}

//===================== Record in file ======================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    /// 消息标识（来自 incoming_message / outgoing_message_response）。
    /// 消息与记录去重只按「通道三元组 + msg_id」判定，不比较内容。
    #[serde(default)]
    pub msg_id: ArcStr,
    /// 发送者身份（区分对话方向）
    pub user_id: ArcStr,
    /// agent 在 channel 绑定的用户（接收方身份 / agent 视角的 self）；
    /// 注意与 is_self 不同：其他人用绑定用户发消息时 user_id == self_user_id 但 is_self == 0
    pub self_user_id: ArcStr,
    pub messenger_id: ArcStr,
    pub group_id: ArcStr,
    pub is_self: usize,
    pub messenger_name: ArcStr,
    pub user_name: ArcStr,
    pub group_name: ArcStr,
    pub content: Content,
    pub time: ArcStr,
    pub sn: u64,
    /// 可选的记录元信息（摘要记忆用它记录所属 case）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Arc<MemoryMeta>>,
}

/// 思考内容：作为 [`Content::Think`] 内嵌在通道记录中，不单独写入记忆文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<ArcStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ArcStr>,
}

/// 工具调用：作为 [`Content::ToolCall`] 内嵌在通道记录中，不单独写入记忆文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: ArcStr,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tool_params: Arc<Value>,
}

/// 工具调用结果：作为 [`Content::ToolResult`] 内嵌在通道记录中，不单独写入记忆文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<Arc<Vec<ContentBlock>>>,
}

impl Record for ChannelRecord {
    fn time(&self) -> &str {
        self.time.as_str()
    }
}

//===================== Record file key ======================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RecordKey {
    pub agent_id: ArcStr,
    pub role_event: Arc<RoleEvent>,
    pub date: ArcStr,
}

//===================== Channel identity ======================

/// 通道标识：messenger + agent 在该通道绑定的用户 + 群组。
///
/// 同一会话的 incoming、self 两类通道记录由此三元组归入同一通道：
/// incoming 记录的 `self_user_id` 是接收方身份，self 记录取回复通道的绑定用户，二者一致。
/// 发送者 `user_id` 不参与：self 记录的 `user_id` 是绑定的用户身份，与对话对方的 `user_id` 不同。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChannelKey {
    pub messenger_id: ArcStr,
    pub self_user_id: ArcStr,
    pub group_id: ArcStr,
}

impl ChannelKey {
    pub fn new(messenger_id: ArcStr, self_user_id: ArcStr, group_id: ArcStr) -> Self {
        Self { messenger_id, self_user_id, group_id }
    }
}

impl From<&ChannelRecord> for ChannelKey {
    fn from(record: &ChannelRecord) -> Self {
        Self {
            messenger_id: record.messenger_id.clone(),
            self_user_id: record.self_user_id.clone(),
            group_id: record.group_id.clone(),
        }
    }
}

//===================== New data notification ======================

/// 新数据通知 payload_type：memory-store → memory-struct-*（0x3001xxxx）
pub const TYPE_NOTIFY_CHANNEL: u32 = 0x3001_0001;

/// 新数据通知 payload：写入完成后广播的位置信息（统一走 kai-ws 信封）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordNotification {
    pub agent_id: ArcStr,
    /// 记忆后缀（角色模式 = 角色名；事件模式 = 角色名 + 事件标识）
    pub role_event: Arc<RoleEvent>,
    pub time: ArcStr,
    /// 是否 agent 实际发送
    pub is_self: usize,
    /// 记录所属通道（平铺为 `messenger_id` / `self_user_id` / `group_id`）
    #[serde(flatten)]
    pub channel: ChannelKey,
}

//===================== 记录位置 ======================

/// 记录在文件内的位置（时间 + 序号）：水位与分页游标共用
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordPosition {
    pub time: ArcStr,
    pub sn: u64,
}

impl Default for RecordPosition {
    fn default() -> Self {
        Self {
            time: RECORD_ZERO_TIME.clone(),
            sn: 0,
        }
    }
}

/// 位置起点（早于任何真实记录）
pub const RECORD_ZERO_TIME: ArcStr = arcstr::literal!("2000-01-01 00:00:00");

impl RecordPosition {
    /// 某条通道记录的位置
    pub fn of(record: &ChannelRecord) -> Self {
        Self { time: record.time.clone(), sn: record.sn }
    }

    /// 记录是否位于该位置之后
    pub fn is_before(&self, time: &str, sn: u64) -> bool {
        (self.time.as_str(), self.sn) < (time, sn)
    }
}

/// 通道水位（watermark.json 的一条）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelWatermark {
    pub channel: ChannelKey,
    pub watermark: Arc<RecordPosition>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use super::*;

    #[test]
    fn test_role_event_fields_serde_and_arc_key() {
        use std::collections::HashSet;

        // 角色模式：event_id 缺省即 None，序列化省略
        let role = RoleEvent::new(ArcStr::from("dev"));
        assert_eq!(role.role_name.as_str(), "dev");
        assert!(role.event_id.is_none());
        assert_eq!(serde_json::to_value(&role).unwrap(), json!({"role_name": "dev"}));
        let back: RoleEvent = serde_json::from_value(json!({"role_name": "dev"})).unwrap();
        assert_eq!(back, role);

        // 事件模式
        let event = RoleEvent::with_event(ArcStr::from("dev"), ArcStr::from("e1"));
        assert_eq!(event.event_id.as_deref(), Some("e1"));
        assert_eq!(serde_json::to_value(&event).unwrap(), json!({"role_name": "dev", "event_id": "e1"}));
        let back: RoleEvent = serde_json::from_value(json!({"role_name": "dev", "event_id": "e1"})).unwrap();
        assert_eq!(back, event);

        // 值相等 + 可作 HashSet 键（Arc<RoleEvent> / Arc<MemoryKey> 依赖它）
        let mut set = HashSet::new();
        set.insert(RoleEvent::with_event(ArcStr::from("dev"), ArcStr::from("e1")));
        assert!(set.contains(&RoleEvent::with_event(ArcStr::from("dev"), ArcStr::from("e1"))));
        assert!(!set.contains(&RoleEvent::new(ArcStr::from("dev"))));
    }

    #[test]
    fn test_channel_key_serde_and_from_record() {
        let record = ChannelRecord {
            meta: None,
            user_id: ArcStr::from("alice"),
            self_user_id: ArcStr::from("self1"),
            messenger_id: ArcStr::from("web"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::from("alice"),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("你好")),
            time: ArcStr::from("2026-09-22 10:00:00"),
            sn: 1,
        };
        let key = ChannelKey::from(&record);
        assert_eq!(serde_json::to_value(&key).unwrap(), json!({
            "messenger_id": "web", "self_user_id": "self1", "group_id": "g1"
        }));
        // 发送者身份不参与通道键：同一会话的 self 与非 self 记录同属一个通道
        let mut self_record = record.clone();
        self_record.user_id = ArcStr::from("self1");
        self_record.is_self = 1;
        assert_eq!(ChannelKey::from(&self_record), key);
        assert!(key < ChannelKey::new(ArcStr::from("web"), ArcStr::from("self1"), ArcStr::from("g2")), "通道键可排序（BTreeMap 用）");
    }

    #[test]
    fn test_serde_channel_request() {
        let obj = ChannelRequest {
            meta: None,
            agent_id: ArcStr::from("agent1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("admin"))),
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::from("m-1"),
            messenger_name: ArcStr::from("M1Name"),
            user_name: ArcStr::from("U1Name"),
            group_name: ArcStr::from("G1Name"),
            content: Content::Text(ArcStr::from("Hello")),
            time: ArcStr::from("2026-01-01 00:00:00"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRequest = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(deserialized.agent_id, "agent1");
        assert_eq!(deserialized.msg_id, "m-1");
        assert_eq!(deserialized.self_user_id, "self1");
        assert_eq!(deserialized.messenger_name, "M1Name");
        assert_eq!(deserialized.user_name, "U1Name");
        assert_eq!(deserialized.group_name, "G1Name");
        assert!(matches!(deserialized.content, Content::Text(s) if s.as_str() == "Hello"));
        // 旧调用方不带 msg_id：反序列化为空串（不参与去重）
        let mut legacy = json;
        legacy.as_object_mut().unwrap().remove("msg_id");
        let legacy: ChannelRequest = serde_json::from_value(legacy).unwrap();
        assert!(legacy.msg_id.is_empty());
    }

    #[test]
    fn test_channel_meta_roundtrip_and_legacy_default() {
        // 摘要记忆信息写在 meta 里随记录透传；无 meta 的旧记录反序列化为 None
        let meta = Arc::new(MemoryMeta::with_case_name(ArcStr::from("role-weather")));
        let request = ChannelRequest {
            meta: Some(meta.clone()),
            agent_id: ArcStr::from("a1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::from("m-1"),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::from("alice"),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("你好")),
            time: ArcStr::from("2026-01-01 00:00:00"),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["meta"], serde_json::json!({"memory_summary": {"case_name": "role-weather"}}));
        assert_eq!(meta.case_name().map(|name| name.as_str()), Some("role-weather"));
        let back: ChannelRequest = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back.meta, Some(meta));

        // meta 缺省不序列化；旧记录（无 meta 字段）反序列化成功
        let no_meta = ChannelRequest { meta: None, ..request };
        assert!(serde_json::to_value(&no_meta).unwrap().get("meta").is_none());
        let mut legacy = json;
        legacy.as_object_mut().unwrap().remove("meta");
        let legacy: ChannelRequest = serde_json::from_value(legacy).unwrap();
        assert!(legacy.meta.is_none());
    }

    #[test]
    fn test_serde_recent_query() {
        let obj = RecentQuery {
            agent_id: ArcStr::from("a"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("r"))),
            count: 5,
        };
        let s = serde_json::to_string(&obj).unwrap();
        assert_eq!(s, r#"{"agent_id":"a","role_event":{"role_name":"r"},"count":5}"#);
        let back: RecentQuery = serde_json::from_str(&s).unwrap();
        assert_eq!(back.count, 5);
    }

    #[test]
    fn test_serde_channel_requests() {
        let req = ChannelRequest {
            meta: None,
            agent_id: ArcStr::from("a1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
            messenger_id: ArcStr::from("m1"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::from("M1Name"),
            user_name: ArcStr::from("U1Name"),
            group_name: ArcStr::from("G1Name"),
            content: Content::Text(ArcStr::from("Hello")),
            time: ArcStr::from("t1"),
        };
        let obj = ChannelRequests { requests: vec![req], force: 1 };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRequests = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.force, 1);
        assert_eq!(deserialized.requests.len(), 1);
    }

    #[test]
    fn test_serde_think_record_without_position() {
        let obj = ThinkRecord {
            reasoning_content: Some(ArcStr::from("推理")),
            thinking: Some(ArcStr::from("思考")),
        };
        let json = serde_json::to_value(&obj).unwrap();
        assert_eq!(json, serde_json::json!({"reasoning_content": "推理", "thinking": "思考"}));
        let back: ThinkRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back, obj);
        assert!(serde_json::to_value(ThinkRecord { reasoning_content: None, thinking: None }).unwrap().as_object().unwrap().is_empty());
    }

    #[test]
    fn test_serde_tool_call_record_without_position() {
        let obj = ToolCallRecord {
            tool_name: ArcStr::from("get_weather"),
            tool_params: Arc::new(json!({"city": "Beijing"})),
        };
        let json = serde_json::to_value(&obj).unwrap();
        assert_eq!(json, serde_json::json!({"tool_name": "get_weather", "tool_params": {"city": "Beijing"}}));
        let back: ToolCallRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back, obj);
    }

    #[test]
    fn test_serde_tool_result_record_without_position() {
        let obj = ToolResultRecord {
            tool_result: Some(Arc::new(vec![ContentBlock::text(ArcStr::from(r#"{"temp":25}"#))])),
        };
        let json = serde_json::to_value(&obj).unwrap();
        assert_eq!(json["tool_result"][0]["text"], r#"{"temp":25}"#);
        let back: ToolResultRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back, obj);
    }

    #[test]
    fn test_serde_query_request() {
        let obj = QueryRequest {
            agent_id: ArcStr::from("a1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("r1"))),
            start_time: ArcStr::from("2026-01-01"),
            end_time: ArcStr::from("2026-06-01"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: QueryRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_id, "a1");
    }

    // ========== Record trait ==========

    fn channel(time: &str, sn: u64) -> ChannelRecord {
        ChannelRecord {
            meta: None,
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            messenger_id: ArcStr::from("telegram"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::new(),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("hello")),
            time: ArcStr::from(time),
            sn,
        }
    }

    #[test]
    fn test_record_impl() {
        let channel = channel("2026-06-24 10:00:00", 5);
        assert_eq!(channel.sn, 5);
        assert_eq!(channel.time(), "2026-06-24 10:00:00");
    }

    // ========== Record serde ==========

    #[test]
    fn test_serde_channel_record() {
        let obj = ChannelRecord {
            meta: None,
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            messenger_id: ArcStr::from("telegram"),
            group_id: ArcStr::from("g1"),
            is_self: 0,
            msg_id: ArcStr::from("m-1"),
            messenger_name: ArcStr::from("M1Name"),
            user_name: ArcStr::from("U1Name"),
            group_name: ArcStr::from("G1Name"),
            content: Content::Text(ArcStr::from("hello")),
            time: ArcStr::from("2026-06-24 10:00:00"),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRecord = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.user_id, "u1");
        assert_eq!(deserialized.msg_id, "m-1");
        assert_eq!(deserialized.self_user_id, "self1");
        assert_eq!(deserialized.messenger_id, "telegram");
        assert_eq!(deserialized.group_id, "g1");
        assert_eq!(deserialized.messenger_name, "M1Name");
        assert_eq!(deserialized.user_name, "U1Name");
        assert_eq!(deserialized.group_name, "G1Name");
        assert!(matches!(deserialized.content, Content::Text(val) if val.as_str() == "hello"));
        assert_eq!(deserialized.sn, 1);
    }

    #[test]
    fn test_channel_record_content_embeds_detail_records() {
        // 思考/工具内容直接内嵌记录，序列化后随通道记录一起落盘
        let record = ChannelRecord {
            meta: None,
            content: Content::Multi(Arc::new(vec![
                Content::Think(Arc::new(ThinkRecord {
                    reasoning_content: Some(ArcStr::from("推理")),
                    thinking: None,
                })),
                Content::ToolCall(Arc::new(ToolCallRecord {
                    tool_name: ArcStr::from("search"),
                    tool_params: Arc::new(json!({"q": "weather"})),
                })),
                Content::ToolResult(Arc::new(ToolResultRecord { tool_result: None })),
                Content::Text(ArcStr::from("回复")),
            ])),
            ..channel("2026-06-24 10:00:00", 1)
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["content"]["msg_type"], "Multi");
        assert_eq!(json["content"]["data"][0]["msg_type"], "Think");
        assert_eq!(json["content"]["data"][0]["data"], json!({"reasoning_content": "推理"}));
        assert_eq!(json["content"]["data"][1]["data"]["tool_name"], "search");
        let back: ChannelRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back.content, record.content);
    }

    // ========== Key serde ==========

    #[test]
    fn test_serde_record_key() {
        let obj = RecordKey {
            agent_id: ArcStr::from("agent1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
            date: ArcStr::from("2026-06-24"),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RecordKey = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent_id, "agent1");
        assert_eq!(deserialized.role_event, Arc::new(RoleEvent::new(ArcStr::from("default"))));
    }

    // ========== 水位与通知 ==========

    #[test]
    fn test_watermark_serde_and_default() {
        let wm = RecordPosition::default();
        assert_eq!(wm.time, RECORD_ZERO_TIME);
        assert_eq!(wm.sn, 0);
        assert_eq!(serde_json::to_value(&wm).unwrap(), serde_json::json!({"time": "2000-01-01 00:00:00", "sn": 0}));
        let back: RecordPosition = serde_json::from_value(serde_json::json!({"time": "2026-09-22 13:48:37", "sn": 128})).unwrap();
        assert_eq!(back.time, "2026-09-22 13:48:37");
        assert_eq!(back.sn, 128);
        // 位置比较
        assert!(RecordPosition::default().is_before("2026-09-22 13:48:37", 1));
        let same = RecordPosition { time: ArcStr::from("2026-09-22 13:48:37"), sn: 1 };
        assert!(!same.is_before("2026-09-22 13:48:37", 1));
        assert!(same.is_before("2026-09-22 13:48:37", 2));
        assert!(same.is_before("2026-09-22 13:48:38", 1));
    }

    #[test]
    fn test_serde_record_notification() {
        let notification = RecordNotification {
            agent_id: ArcStr::from("0"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("dev"))),
            time: ArcStr::from("2026-09-22 13:48:37"),
            is_self: 0,
            channel: ChannelKey::new(ArcStr::from("web"), ArcStr::from("self1"), ArcStr::from("g1")),
        };
        let v = serde_json::to_value(&notification).unwrap();
        assert_eq!(v, serde_json::json!({
            "agent_id": "0", "role_event": {"role_name": "dev"}, "time": "2026-09-22 13:48:37", "is_self": 0,
            "messenger_id": "web", "self_user_id": "self1", "group_id": "g1"
        }));
        assert_eq!(serde_json::from_value::<RecordNotification>(v).unwrap(), notification);
        // 通道字段必填：缺通道的通知按格式错误处理
        assert!(serde_json::from_value::<RecordNotification>(serde_json::json!({
            "agent_id": "0", "role_event": {"role_name": "dev"}, "time": "2026-09-22 13:48:37", "is_self": 0
        })).is_err());
    }
}
