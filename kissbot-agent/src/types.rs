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
                tool_calls: json!(vec![
                    Arc::new(ToolCall {
                        id: Arc::new("call_1".into()),
                        name: Arc::new("read".into()),
                        data: ToolData {
                            arguments: json!({"path": "/tmp/a.txt"}),
                            error: Value::Null,
                            result: Value::Null,
                        },
                    }),
                ]),
            }, r#"{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/tmp/a.txt\"}"}}]}"#),
            (Message::Tool { tool_call_id: Arc::new("call_1".into()), content: Arc::new("内容".into()) }, r#"{"role":"tool","tool_call_id":"call_1","name":"read","content":"内容"}"#),
        ];
        for (m, expected) in cases {
            assert_eq!(serde_json::to_string(&m).unwrap(), expected);
        }
    }

    #[test]
    fn message_deserialization_role_tag_same_level() {
        // 反序列化：role 标签定位变体，None 字段缺省
        let sys: Message = serde_json::from_str(r#"{"role":"system","content":"你是助手"}"#).unwrap();
        assert!(matches!(sys, Message::System { content } if content.as_str() == "你是助手"));

        let user: Message = serde_json::from_str(r#"{"role":"user","content":"你好"}"#).unwrap();
        assert!(matches!(user, Message::User { content } if content.as_str() == "你好"));

        let asst: Message = serde_json::from_str(r#"{"role":"assistant","content":"","reasoning_content":"思考"}"#).unwrap();
        assert!(matches!(asst, Message::Assistant { reasoning_content: Value::String(r), tool_calls: Value::Null, .. } if r.as_str() == "思考"));

        let asst2: Message = serde_json::from_str(r#"{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/tmp/a.txt\"}"}}]}"#).unwrap();
        assert!(matches!(&asst2, Message::Assistant { reasoning_content: Value::Null, tool_calls: tcs, .. }
            if tcs[0]["id"].as_str().unwrap() == "call_1" && tcs[0]["name"].as_str().unwrap() == "read" && tcs[0]["arguments"]["path"] == "/tmp/a.txt"));

        let tool: Message = serde_json::from_str(r#"{"role":"tool","tool_call_id":"call_1","name":"read","content":"内容"}"#).unwrap();
        assert!(matches!(tool, Message::Tool { tool_call_id, content }
            if tool_call_id.as_str() == "call_1" && content.as_str() == "内容"));
    }

    #[test]
    fn message_assistant_optional_fields_omitted() {
        let m = Message::Assistant { content: Value::String("回答".into()), reasoning_content: Value::Null, tool_calls: Value::Null};
        let v = serde_json::to_value(&m).unwrap();
        assert!(v.get("reasoning_content").is_none(), "None 字段不应序列化");
        assert!(v.get("tool_calls").is_none(), "None 字段不应序列化");
    }

    // #[test]
    // fn tool_call_wire_serde_roundtrip_and_tolerance() {
    //     // 序列化：wire 形状 {id, type:"function", function:{name, arguments(JSON 字符串)}}
    //     let tc = ToolCall { id: Arc::new("call_1".into()), name: Arc::new("read".into()), arguments: json!({"path": "/tmp/a.txt"}) };
    //     let json = serde_json::to_string(&tc).unwrap();
    //     assert_eq!(json, r#"{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/tmp/a.txt\"}"}}"#, "arguments 序列化为 JSON 字符串");
    //     // 反序列化：wire 形状还原（arguments 解析回对象）
    //     let back: ToolCall = serde_json::from_str(&json).unwrap();
    //     assert_eq!(back.id.as_str(), "call_1");
    //     assert_eq!(back.name.as_str(), "read");
    //     assert_eq!(back.data.arguments["path"], "/tmp/a.txt", "arguments 字符串解析回 JSON 对象");
    //     // 容错：arguments 为对象直接用；缺失回退 Null；type 忽略
    //     let obj_arg: ToolCall = serde_json::from_str(r#"{"id":"c","function":{"name":"n","arguments":{"a":1}}}"#).unwrap();
    //     assert_eq!(obj_arg.data.arguments["a"], 1);
    //     let no_arg: ToolCall = serde_json::from_str(r#"{"id":"c","function":{"name":"n"}}"#).unwrap();
    //     assert!(no_arg.data.arguments.is_null(), "缺失 arguments 回退 Null");
    //     // 缺 id / 缺 function.name 报错（parse_openai_response 整体回退空）
    //     assert!(serde_json::from_str::<ToolCall>(r#"{"type":"function"}"#).is_err());
    //     assert!(serde_json::from_str::<ToolCall>(r#"{"id":"c"}"#).is_err());
    // }
}
