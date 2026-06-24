use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ========== Request structures ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
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
    pub content: Arc<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
}

// ========== Records ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub messenger_id: Arc<String>,
    pub user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub is_self: usize,
    pub msg_type: Arc<String>,
    pub content: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub content: Arc<String>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub tool_name: Arc<String>,
    pub tool_params: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub agent_id: Arc<String>,
    pub role_name: Arc<String>,
    pub tool_result: Arc<serde_json::Value>,
    pub key: Arc<String>,
    pub time: Arc<String>,
    pub sn: u64,
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
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("Hello".to_string()),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "agent1");
        assert_eq!(*deserialized.content, "Hello");
    }

    #[test]
    fn test_serde_channel_requests() {
        let req = ChannelRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("Hello".to_string()),
            time: Arc::new("t1".to_string()),
        };
        let obj = ChannelRequests { requests: vec![req], force: 1 };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRequests = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.force, 1);
        assert_eq!(deserialized.requests.len(), 1);
    }

    #[test]
    fn test_serde_think_request() {
        let obj = ThinkRequest {
            agent_id: Arc::new("agent1".to_string()),
            role_name: Arc::new("admin".to_string()),
            content: Arc::new("thinking...".to_string()),
            key: Arc::new("key1".to_string()),
            time: Arc::new("2026-01-01 00:00:00".to_string()),
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ThinkRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.content, "thinking...");
    }

    #[test]
    fn test_serde_think_requests() {
        let req = ThinkRequest {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            content: Arc::new("thinking...".to_string()),
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
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: QueryRequest = serde_json::from_value(json).unwrap();
        assert_eq!(*deserialized.agent_id, "a1");
    }

    #[test]
    fn test_serde_channel_record() {
        let obj = ChannelRecord {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            messenger_id: Arc::new("m1".to_string()),
            user_id: Arc::new("u1".to_string()),
            group_id: Arc::new("g1".to_string()),
            is_self: 0,
            msg_type: Arc::new("text".to_string()),
            content: Arc::new("Hello".to_string()),
            time: Arc::new("t1".to_string()),
            sn: 100,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ChannelRecord = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.sn, 100);
        assert_eq!(*deserialized.content, "Hello");
    }

    #[test]
    fn test_serde_think_record() {
        let obj = ThinkRecord {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            content: Arc::new("thinking...".to_string()),
            key: Arc::new("k1".to_string()),
            time: Arc::new("t1".to_string()),
            sn: 200,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ThinkRecord = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.sn, 200);
    }

    #[test]
    fn test_serde_tool_call_record() {
        let obj = ToolCallRecord {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            tool_name: Arc::new("get_weather".to_string()),
            tool_params: Arc::new(serde_json::json!({"city": "Beijing"})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("t1".to_string()),
            sn: 300,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolCallRecord = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.sn, 300);
    }

    #[test]
    fn test_serde_tool_result_record() {
        let obj = ToolResultRecord {
            agent_id: Arc::new("a1".to_string()),
            role_name: Arc::new("r1".to_string()),
            tool_result: Arc::new(serde_json::json!({"temp": 25})),
            key: Arc::new("k1".to_string()),
            time: Arc::new("t1".to_string()),
            sn: 400,
        };
        let json = serde_json::to_value(&obj).unwrap();
        let deserialized: ToolResultRecord = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.sn, 400);
    }
}
