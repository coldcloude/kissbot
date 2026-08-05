use std::{cmp::Ordering, sync::Arc};

use kai_file::index::Record;
use serde::{Deserialize, Serialize};

use crate::Content;

// ========== Request structures ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    /// 接收方身份（= channel 绑定的 user_id，即 agent 视角的 self）；文件名按此值分文件
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
    pub reasoning_content: Arc<String>,   // 原 content 拆分：API 字段解析的内容
    pub thinking: Arc<String>,            // 原 content 拆分：<think> 标签解析的内容（去标签）
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
    pub tool_params: Arc<serde_json::Value>,
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
    pub tool_result: Arc<serde_json::Value>,
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
pub struct QueryChannelRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
    /// 可选：返回该范围内最近 N 条（合并排序后截取）；None 返回全部
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
    /// 可选：返回该范围内最近 N 条；None 返回全部
    #[serde(default)]
    pub limit: Option<usize>,
}


//===================== Record in file ======================

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRecord {
    pub reasoning_content: Arc<String>,
    pub thinking: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: Arc<String>,
    pub tool_params: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub tool_result: Arc<serde_json::Value>,
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
pub struct ChannelRecordKey {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub date: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RecordKey {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub date: Arc<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
            reasoning_content: Arc::new("推理".to_string()),
            thinking: Arc::new("思考".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let back: ThinkRequest = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(*back.reasoning_content, "推理");
        assert_eq!(*back.thinking, "思考");
        assert!(json.get("content").is_none(), "content 字段已拆分");
    }

    #[test]
    fn test_serde_think_record_dual_fields() {
        let obj = ThinkRecord {
            reasoning_content: Arc::new("推理".to_string()),
            thinking: Arc::new("".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let back: ThinkRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*back.reasoning_content, "推理");
        assert_eq!(back.sn, 1);
    }

    #[test]
    fn test_serde_think_request() {
        let obj = ThinkRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("admin".to_string()),
            reasoning_content: Arc::new("thinking...".to_string()),
            thinking: Arc::new(String::new()),
            key: Arc::new("key1".to_string()),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ThinkRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.reasoning_content, "thinking...");
        assert_eq!(*deserialized.thinking, "");
    }

    #[test]
    fn test_serde_think_requests() {
        let req = ThinkRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            reasoning_content: Arc::new("thinking...".to_string()),
            thinking: Arc::new(String::new()),
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
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
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
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
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
    fn test_serde_query_channel_request() {
        let obj = QueryChannelRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            start_time: Arc::new("2026-01-01".to_string()),
            end_time: Arc::new("2026-06-01".to_string()),
            limit: None,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: QueryChannelRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.start_time, "2026-01-01");
    }

    #[test]
    fn test_serde_query_request() {
        let obj = QueryRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            start_time: Arc::new("2026-01-01".to_string()),
            end_time: Arc::new("2026-06-01".to_string()),
            limit: None,
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
            reasoning_content: Arc::new("think".to_string()),
            thinking: Arc::new(String::new()),
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
        assert_eq!(*deserialized.messenger_name, "M1Name");
        assert_eq!(*deserialized.user_name, "U1Name");
        assert_eq!(*deserialized.group_name, "G1Name");
        assert!(matches!(deserialized.content, Content::Text(val) if val.as_str() == "hello"));
        assert_eq!(deserialized.sn, 1);
    }

    #[test]
    fn test_serde_think_record() {
        let obj = ThinkRecord {
            reasoning_content: Arc::new("think content".to_string()),
            thinking: Arc::new(String::new()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("2026-06-24 10:00:00".to_string()),
            sn: 1,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ThinkRecord = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.reasoning_content, "think content");
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
    fn test_serde_channel_record_key() {
        let obj = ChannelRecordKey {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("default".to_string()),
            messenger_id: Arc::new("telegram".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            date: Arc::new("2026-06-24".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRecordKey = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "agent1");
        assert_eq!(*deserialized.messenger_id, "telegram");
        assert_eq!(*deserialized.date, "2026-06-24");
    }

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
}
