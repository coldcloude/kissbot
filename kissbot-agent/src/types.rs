pub mod error;
pub mod llm_api;
pub mod session_api;
pub mod channel_api;
pub mod station_api;

pub use error::*;
pub use llm_api::*;
pub use session_api::*;
pub use channel_api::*;
pub use station_api::*;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};

use super::*;

    #[test]
    fn session_key_hash_eq_by_value() {
        use std::collections::HashSet;
        let a = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let b = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let c = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 SessionKey 应命中 HashSet");
        assert!(!set.contains(&c), "不同 mode 不应命中");
    }

    #[test]
    fn role_mode_encodes_event_only() {
        assert_eq!(role_mode("dev", &Mode::Role), "dev");
        assert_eq!(role_mode("dev", &Mode::Event("e1".into())), "dev-e1");
    }

    /// assistant.tool_calls 的 wire 形状：与 API 返回 JSON 一致，由 raw_tool_calls 原样透传
    /// （ToolCall 派生序列化为 {id, name, data}，不是 wire 形状，故这里直接用 wire JSON）
    fn wire_tool_calls() -> Value {
        json!([{ "id": "call_1", "type": "function", "function": { "name": "read", "arguments": "{\"path\":\"/tmp/a.txt\"}" } }])
    }

    #[test]
    fn message_serialization_role_tag_same_level() {
        // 序列化：role 为平级标签字段（内部标签 tag=role + lowercase；键序为 serde 派生插入序，文本为探针确认结果）
        let cases: Vec<(Message, &str)> = vec![
            (Message::System { content: Arc::new("你是助手".into()) }, r#"{"role":"system","content":"你是助手"}"#),
            (Message::User { content: Arc::new("你好".into()) }, r#"{"role":"user","content":"你好"}"#),
            (Message::Assistant {
                content: Value::String(String::new()),
                reasoning_content: Value::String("思考".into()),
                tool_calls: Value::Null,
            }, r#"{"role":"assistant","content":"","reasoning_content":"思考"}"#),
            (Message::Assistant {
                content: Value::String(String::new()),
                reasoning_content: Value::Null,
                tool_calls: Value::Null,
            }, r#"{"role":"assistant","content":""}"#),
            // Tool 消息已无 name 字段（工具名由 assistant.tool_calls 承载）
            (Message::Tool { tool_call_id: Arc::new("call_1".into()), content: Arc::new("内容".into()) }, r#"{"role":"tool","tool_call_id":"call_1","content":"内容"}"#),
        ];
        for (m, expected) in cases {
            assert_eq!(serde_json::to_string(&m).unwrap(), expected);
        }
    }

    #[test]
    fn message_serialization_passes_tool_calls_through_unchanged() {
        // tool_calls 为 raw_tool_calls（API 原始 JSON）原样透传：内部键序由 serde_json Map 决定，故按结构断言
        let m = Message::Assistant {
            content: Value::String(String::new()),
            reasoning_content: Value::Null,
            tool_calls: wire_tool_calls(),
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["tool_calls"][0]["id"], "call_1", "wire 形状原样携带");
        assert_eq!(v["tool_calls"][0]["function"]["name"], "read");
        assert_eq!(v["tool_calls"][0]["function"]["arguments"], r#"{"path":"/tmp/a.txt"}"#, "arguments 保持 JSON 字符串");
    }

    #[test]
    fn message_deserialization_role_tag_same_level() {
        // 反序列化：role 标签定位变体，无值字段缺省为 Null
        let sys: Message = serde_json::from_str(r#"{"role":"system","content":"你是助手"}"#).unwrap();
        assert!(matches!(sys, Message::System { content } if content.as_str() == "你是助手"));

        let user: Message = serde_json::from_str(r#"{"role":"user","content":"你好"}"#).unwrap();
        assert!(matches!(user, Message::User { content } if content.as_str() == "你好"));

        let asst: Message = serde_json::from_str(r#"{"role":"assistant","content":"","reasoning_content":"思考"}"#).unwrap();
        assert!(matches!(asst, Message::Assistant { reasoning_content: Value::String(r), tool_calls: Value::Null, .. } if r.as_str() == "思考"));

        let asst2: Message = serde_json::from_str(r#"{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/tmp/a.txt\"}"}}]}"#).unwrap();
        assert!(matches!(&asst2, Message::Assistant { reasoning_content: Value::Null, tool_calls: tcs, .. }
            if tcs[0]["id"].as_str().unwrap() == "call_1"
                && tcs[0]["function"]["name"].as_str().unwrap() == "read"
                && tcs[0]["function"]["arguments"].as_str().unwrap() == r#"{"path":"/tmp/a.txt"}"#));

        let tool: Message = serde_json::from_str(r#"{"role":"tool","tool_call_id":"call_1","content":"内容"}"#).unwrap();
        assert!(matches!(tool, Message::Tool { tool_call_id, content }
            if tool_call_id.as_str() == "call_1" && content.as_str() == "内容"));
    }

    #[test]
    fn tool_call_derived_serde_roundtrip() {
        // ToolCall 为派生序列化（{id, name, data}），wire 形状由 raw_tool_calls 透传承担（见上）
        let tc = ToolCall {
            id: Arc::new("call_1".into()),
            name: Arc::new("read".into()),
            data: ToolData {
                arguments: json!({ "path": "/tmp/a.txt" }),
                result: json!("内容"),
                error: Value::Null,
            },
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id.as_str(), "call_1");
        assert_eq!(back.name.as_str(), "read");
        assert_eq!(back.data.arguments["path"], "/tmp/a.txt", "arguments 保持 JSON 对象");
        assert_eq!(back.data.result, json!("内容"));
        assert!(back.data.error.is_null());
    }

    #[test]
    fn message_assistant_optional_fields_omitted() {
        let m = Message::Assistant { content: Value::String("回答".into()), reasoning_content: Value::Null, tool_calls: Value::Null};
        let v = serde_json::to_value(&m).unwrap();
        assert!(v.get("reasoning_content").is_none(), "None 字段不应序列化");
        assert!(v.get("tool_calls").is_none(), "None 字段不应序列化");
    }
}
