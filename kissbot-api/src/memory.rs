use std::{cmp::Ordering, sync::Arc};

use kai_file::index::Record;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Content;

// ========== Request structures ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    /// 接收方身份（= channel 绑定的 user_id，即 agent 视角的 self；与 is_self 不同，其他人用绑定用户发消息时 user_id == self_user_id 但 is_self == 0）
    pub self_user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub messenger_name: Arc<String>,
    pub user_name: Arc<String>,
    pub group_name: Arc<String>,
    pub content: Content,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequests {
    pub requests: Vec<ChannelRequest>,
    pub force: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<Arc<String>>,   // API 字段解析的内容
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Arc<String>>,            // <think> 标签解析的内容（去标签）
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRequests {
    pub requests: Vec<ThinkRequest>,
    pub force: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub tool_name: Arc<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tool_params: Arc<Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequests {
    pub requests: Vec<ToolCallRequest>,
    pub force: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tool_result: Arc<Value>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tool_error: Arc<Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRequests {
    pub requests: Vec<ToolResultRequest>,
    pub force: usize,
}

// ========== Query requests ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
}

/// 最近 N 条 channel 记录查询（无时间参数：取该 agent+role 最近 count 条，跨日期文件）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentQuery {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub count: u32,
}

//===================== Record in file ======================

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<Arc<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Arc<String>>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: Arc<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tool_params: Arc<Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tool_result: Arc<Value>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tool_error: Arc<Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

pub trait MemoryRecord: Record {
    fn sn(&self) -> u64;
    fn set_sn(&mut self, sn: u64);
    fn time_string(&self) -> Arc<String>;
    fn cmp(&self, other: &Self) -> Ordering {
        let sign = self.time().cmp(other.time());
        if sign == Ordering::Equal {
            self.sn().cmp(&other.sn())
        } else {
            sign
        }
    }
}

macro_rules! impl_record {
    ($($t:ty),*) => {
        $(impl Record for $t {
            fn time(&self) -> &str {
                self.time.as_str()
            }
        })*
        $(impl MemoryRecord for $t {
            fn sn(&self) -> u64 {
                self.sn
            }
            fn set_sn(&mut self, sn: u64) {
                self.sn = sn;
            }
            fn time_string(&self) -> Arc<String> {
                self.time.clone()
            }
        })*
    };
}

impl_record!(
    ChannelRecord,
    ThinkRecord,
    ToolCallRecord,
    ToolResultRecord
);

//===================== Record file key ======================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RecordKey {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub date: Arc<String>,
}

//===================== Channel identity ======================

/// 通道标识：messenger + agent 在该通道绑定的用户 + 群组。
///
/// 同一会话的 incoming、self、思考/工具占位三类通道记录由此三元组归入同一通道：
/// incoming 记录的 `self_user_id` 是接收方身份，self 与占位记录取回复通道的绑定用户，二者一致。
/// 发送者 `user_id` 不参与：self 记录的 `user_id` 是绑定的用户身份，与对话对方的 `user_id` 不同。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChannelKey {
    pub messenger_id: Arc<String>,
    pub self_user_id: Arc<String>,
    pub group_id: Arc<String>,
}

impl ChannelKey {
    pub fn new(messenger_id: impl Into<String>, self_user_id: impl Into<String>, group_id: impl Into<String>) -> Self {
        Self {
            messenger_id: Arc::new(messenger_id.into()),
            self_user_id: Arc::new(self_user_id.into()),
            group_id: Arc::new(group_id.into()),
        }
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

/// 记录类型：四类原始记录的公共分类（通知、读取与文件命名共用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordKind {
    Channel,
    Think,
    ToolCall,
    ToolResult,
}

impl RecordKind {
    pub const ALL: [RecordKind; 4] = [
        RecordKind::Channel,
        RecordKind::Think,
        RecordKind::ToolCall,
        RecordKind::ToolResult,
    ];

    /// 记录文件名的类型段（`{stem}-records-{date}.jsonl`）
    pub fn file_stem(self) -> &'static str {
        match self {
            RecordKind::Channel => "channel",
            RecordKind::Think => "think",
            RecordKind::ToolCall => "tool-call",
            RecordKind::ToolResult => "tool-result",
        }
    }

    /// 通知帧的 payload_type（kai-ws 信封；记录类型由 payload_type 承载）
    pub fn payload_type(self) -> u32 {
        match self {
            RecordKind::Channel => TYPE_NOTIFY_CHANNEL,
            RecordKind::Think => TYPE_NOTIFY_THINK,
            RecordKind::ToolCall => TYPE_NOTIFY_TOOL_CALL,
            RecordKind::ToolResult => TYPE_NOTIFY_TOOL_RESULT,
        }
    }

    pub fn from_payload_type(payload_type: u32) -> Option<Self> {
        match payload_type {
            TYPE_NOTIFY_CHANNEL => Some(RecordKind::Channel),
            TYPE_NOTIFY_THINK => Some(RecordKind::Think),
            TYPE_NOTIFY_TOOL_CALL => Some(RecordKind::ToolCall),
            TYPE_NOTIFY_TOOL_RESULT => Some(RecordKind::ToolResult),
            _ => None,
        }
    }
}

/// 新数据通知 payload_type：memory-store → memory-struct-*（0x3001xxxx）
pub const TYPE_NOTIFY_CHANNEL: u32 = 0x3001_0001;
pub const TYPE_NOTIFY_THINK: u32 = 0x3001_0002;
pub const TYPE_NOTIFY_TOOL_CALL: u32 = 0x3001_0003;
pub const TYPE_NOTIFY_TOOL_RESULT: u32 = 0x3001_0004;

/// 新数据通知 payload：写入完成后广播的位置信息（记录类型见 payload_type，统一走 kai-ws 信封）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordNotification {
    pub agent_id: Arc<String>,
    /// 记忆后缀：角色模式为角色名，事件模式为 `角色名-事件id`
    pub role: Arc<String>,
    pub time: Arc<String>,
    /// 仅通道消息携带：是否 agent 实际发送
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_self: Option<usize>,
    /// 仅通道消息携带：记录所属通道（平铺为 `messenger_id` / `self_user_id` / `group_id`）。
    /// 收到通知后只评估该通道；思考/工具记录只有 key、写入侧无法确定通道，因此不带。
    #[serde(default, flatten)]
    pub channel: Option<ChannelKey>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use super::*;

    #[test]
    fn test_channel_key_serde_and_from_record() {
        let record = ChannelRecord {
            user_id: Arc::new("alice".into()),
            self_user_id: Arc::new("self1".into()),
            messenger_id: Arc::new("web".into()),
            group_id: Arc::new("g1".into()),
            is_self: 0,
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new("alice".into()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("你好".into())),
            time: Arc::new("2026-09-22 10:00:00".into()),
            sn: 1,
        };
        let key = ChannelKey::from(&record);
        assert_eq!(serde_json::to_value(&key).unwrap(), json!({
            "messenger_id": "web", "self_user_id": "self1", "group_id": "g1"
        }));
        // 发送者身份不参与通道键：同一会话的 self 与非 self 记录同属一个通道
        let mut self_record = record.clone();
        self_record.user_id = Arc::new("self1".into());
        self_record.is_self = 1;
        assert_eq!(ChannelKey::from(&self_record), key);
        assert!(key < ChannelKey::new("web", "self1", "g2"), "通道键可排序（BTreeMap 用）");
    }

    #[test]
    fn test_serde_channel_request() {
        let obj = ChannelRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("admin".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new("M1Name".to_string()),
            user_name: Arc::new("U1Name".to_string()),
            group_name: Arc::new("G1Name".to_string()),
            content: Content::Text(Arc::new("Hello".to_string())),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "agent1");
        assert_eq!(*deserialized.self_user_id, "self1");
        assert_eq!(*deserialized.messenger_name, "M1Name");
        assert_eq!(*deserialized.user_name, "U1Name");
        assert_eq!(*deserialized.group_name, "G1Name");
        assert!(matches!(deserialized.content, Content::Text(s) if s.as_str() == "Hello"));
    }

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

    #[test]
    fn test_serde_channel_requests() {
        let req = ChannelRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            self_user_id: Arc::new("self1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            messenger_name: Arc::new("M1Name".to_string()),
            user_name: Arc::new("U1Name".to_string()),
            group_name: Arc::new("G1Name".to_string()),
            content: Content::Text(Arc::new("Hello".to_string())),
            time: Arc::new("t1".to_string()),
        };
        let obj = ChannelRequests { requests: vec![req], force: 1 };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRequests = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.force, 1);
        assert_eq!(deserialized.requests.len(), 1);
    }

    #[test]
    fn test_serde_think_request_dual_fields() {
        let obj = ThinkRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            reasoning_content: Some(Arc::new("推理".to_string())),
            thinking: Some(Arc::new("思考".to_string())),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let back: ThinkRequest = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(*back.reasoning_content.unwrap(), "推理");
        assert_eq!(*back.thinking.unwrap(), "思考");
        assert!(json.get("content").is_none(), "content 字段已拆分");
    }

    #[test]
    fn test_serde_think_record_dual_fields() {
        let obj = ThinkRecord {
            reasoning_content: Some(Arc::new("推理".to_string())),
            thinking: Some(Arc::new("思考".to_string())),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let back: ThinkRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*back.reasoning_content.unwrap(), "推理");
        assert_eq!(*back.thinking.unwrap(), "思考");
        assert_eq!(back.sn, 1);
    }

    #[test]
    fn test_serde_think_request() {
        let obj = ThinkRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("admin".to_string()),
            reasoning_content: Some(Arc::new("thinking...".to_string())),
            thinking: None,
            key: Arc::new("key1".to_string()),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ThinkRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.reasoning_content.unwrap(), "thinking...");
        assert_eq!(deserialized.thinking, None);
    }

    #[test]
    fn test_serde_think_requests() {
        let req = ThinkRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            reasoning_content: Some(Arc::new("thinking...".to_string())),
            thinking: None,
            key: Arc::new("k1".to_string()),
            time: Arc::new("t1".to_string()),
        };
        let obj = ThinkRequests { requests: vec![req], force: 0 };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ThinkRequests = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.requests.len(), 1);
        assert_eq!(deserialized.force, 0);
    }

    #[test]
    fn test_serde_tool_call_request() {
        let obj = ToolCallRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            tool_name: Arc::new("get_weather".to_string()),            tool_params: Arc::new(json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("t1".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolCallRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.tool_name, "get_weather");
    }

    #[test]
    fn test_serde_tool_call_requests() {
        let obj = ToolCallRequests { requests: vec![], force: 1 };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolCallRequests = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.requests.len(), 0);
        assert_eq!(deserialized.force, 1);
    }

    #[test]
    fn test_serde_tool_result_request() {
        let obj = ToolResultRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            tool_result: Arc::new(json!({"temp": 25})),
            tool_error: Arc::new(Value::Null),
            key: Arc::new("k1".to_string()),
            time: Arc::new("t1".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolResultRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.key, "k1");
    }

    #[test]
    fn test_serde_tool_result_requests() {
        let obj = ToolResultRequests { requests: vec![], force: 0 };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolResultRequests = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.requests.len(), 0);
    }

    #[test]
    fn test_serde_query_request() {
        let obj = QueryRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            start_time: Arc::new("2026-01-01".to_string()),
            end_time: Arc::new("2026-06-01".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: QueryRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "a1");
    }

    // ========== Record trait ==========

    #[test]
    fn test_record_impl() {
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
        assert_eq!(channel.sn(), 5);
        assert_eq!(channel.time(), "2026-06-24 10:00:00");
        channel.set_sn(10);
        assert_eq!(channel.sn(), 10);

        let think = ThinkRecord {
            reasoning_content: Some(Arc::new("think".to_string())),
            thinking: None,
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:01".to_string()),
            sn: 1,
        };
        assert_eq!(think.sn(), 1);
        assert_eq!(think.time(), "2026-06-24 10:00:01");
    }

    #[test]
    fn test_record_cmp_time() {
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
        assert_eq!(r1.cmp(&r2), std::cmp::Ordering::Less);

        // same time, different sn
        let r3 = ChannelRecord {
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 2,
            ..r1.clone()
        };
        assert_eq!(r1.cmp(&r3), std::cmp::Ordering::Less);
    }

    // ========== Record serde ==========

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

    #[test]
    fn test_serde_think_record() {
        let obj = ThinkRecord {
            reasoning_content: Some(Arc::new("think content".to_string())),
            thinking: None,
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ThinkRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.reasoning_content.unwrap(), "think content");
        assert_eq!(*deserialized.key, "k1");
    }

    #[test]
    fn test_serde_tool_call_record() {
        let obj = ToolCallRecord {
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolCallRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.tool_name, "get_weather");
        assert_eq!(deserialized.tool_params["city"], "Beijing");
    }

    #[test]
    fn test_serde_tool_result_record() {
        let obj = ToolResultRecord {
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
            tool_error: Arc::new(serde_json::Value::Null),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolResultRecord = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.tool_result["temp"], 25);
    }

    // ========== Key serde ==========

    #[test]
    fn test_serde_record_key() {
        let obj = RecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: RecordKey = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "agent1");
        assert_eq!(*deserialized.role_name, "default");
    }

    // ========== New data notification ==========

    #[test]
    fn test_record_kind_file_stem_and_payload_type() {
        assert_eq!(RecordKind::Channel.file_stem(), "channel");
        assert_eq!(RecordKind::ToolCall.file_stem(), "tool-call");
        for kind in RecordKind::ALL {
            assert_eq!(RecordKind::from_payload_type(kind.payload_type()), Some(kind), "payload_type 往返");
        }
        assert_eq!(RecordKind::from_payload_type(0x9999_9999), None);
        // 四类 payload_type 互不相同
        let mut types: Vec<u32> = RecordKind::ALL.iter().map(|k| k.payload_type()).collect();
        types.sort_unstable();
        types.dedup();
        assert_eq!(types.len(), 4);
    }

    #[test]
    fn test_record_kind_serde_kebab_case() {
        assert_eq!(serde_json::to_string(&RecordKind::ToolCall).unwrap(), r#""tool-call""#);
        assert_eq!(serde_json::from_str::<RecordKind>(r#""tool-result""#).unwrap(), RecordKind::ToolResult);
    }

    #[test]
    fn test_serde_record_notification_channel_and_think() {
        // 通道消息通知带 is_self 与通道三元组（平铺在 payload 顶层）
        let channel = RecordNotification {
            agent_id: Arc::new("0".into()),
            role: Arc::new("dev".into()),
            time: Arc::new("2026-09-22 13:48:37".into()),
            is_self: Some(0),
            channel: Some(ChannelKey::new("web", "self1", "g1")),
        };
        let v = serde_json::to_value(&channel).unwrap();
        assert_eq!(v, json!({
            "agent_id": "0", "role": "dev", "time": "2026-09-22 13:48:37", "is_self": 0,
            "messenger_id": "web", "self_user_id": "self1", "group_id": "g1"
        }));
        assert_eq!(serde_json::from_value::<RecordNotification>(v).unwrap(), channel);

        // 其余类型不带 is_self 与通道（缺省为 None，可省略）
        let think = RecordNotification {
            agent_id: Arc::new("0".into()),
            role: Arc::new("dev".into()),
            time: Arc::new("2026-09-22 13:48:37".into()),
            is_self: None,
            channel: None,
        };
        let v = serde_json::to_value(&think).unwrap();
        assert!(v.get("is_self").is_none(), "None 不序列化");
        assert!(v.get("messenger_id").is_none() && v.get("group_id").is_none(), "通道缺省不序列化");
        assert_eq!(serde_json::from_value::<RecordNotification>(v).unwrap(), think);
    }
}
