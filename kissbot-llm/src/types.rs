use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_mcp::ContentBlock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ========== 工具调用相关 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ArcStr,
    pub name: ArcStr,
    pub arguments: Value,
    pub result: Option<Vec<ContentBlock>>,
}

/// LLM 请求中的工具定义（OpenAI `tools[].function` 形状）。
/// 由工具执行侧（如 Station 的 MCP `tools/list`）转换而来，仅用于调用方与 Provider 侧；
/// 与 MCP 的转换留在调用方（本组件不依赖 MCP）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub name: ArcStr,
    pub description: ArcStr,
    pub parameters: Arc<Value>,
}

// ========== 模型相关 ==========

#[derive(Debug)]
pub struct ModelResponse {
    /// 返回内容
    pub raw_content: Value,
    pub content: Option<ArcStr>,
    pub thinking: Option<ArcStr>, // <think> 标签解析（去标签）
    /// 思考内容：API 字段（DeepSeek reasoning_content / anthropic thinking block）
    pub raw_reasoning_content: Value,
    pub reasoning_content: Option<ArcStr>,
    /// 工具调用内容
    pub raw_tool_calls: Value,
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 其他信息
    pub finish_reason: Option<ArcStr>,
    pub total_tokens: u64,
}

/// OpenAI 兼容上下文消息：role 即枚举变体（内部标签序列化，role 与其他字段平级）
/// 字段按编码规范用 ArcStr（Option 内同样用 ArcStr）；tool_calls 为 Vec 不包裹
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System { content: ArcStr },
    User { content: ArcStr },
    Assistant {
        #[serde(default, skip_serializing_if = "Value::is_null")]
        content: Value,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        reasoning_content: Value,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        tool_calls: Value,
    },
    Tool {
        tool_call_id: ArcStr,
        /// 调用结果
        content: ArcStr,
    },
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arcstr::ArcStr;
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn message_serialization_role_tag_same_level() {
        // 序列化：role 为平级标签字段（内部标签 tag=role + lowercase；键序为 serde 派生插入序，文本为探针确认结果）
        let cases: Vec<(Message, &str)> = vec![
            (Message::System { content: ArcStr::from("你是助手") }, r#"{"role":"system","content":"你是助手"}"#),
            (Message::User { content: ArcStr::from("你好") }, r#"{"role":"user","content":"你好"}"#),
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
            (Message::Tool { tool_call_id: ArcStr::from("call_1"), content: ArcStr::from("内容") }, r#"{"role":"tool","tool_call_id":"call_1","content":"内容"}"#),
        ];
        for (m, expected) in cases {
            assert_eq!(serde_json::to_string(&m).unwrap(), expected);
        }
    }

    /// assistant.tool_calls 的 wire 形状：与 API 返回 JSON 一致，由 raw_tool_calls 原样透传
    fn wire_tool_calls() -> Value {
        json!([{ "id": "call_1", "type": "function", "function": { "name": "read", "arguments": "{\"path\":\"/tmp/a.txt\"}" } }])
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
            id: ArcStr::from("call_1"),
            name: ArcStr::from("read"),
            arguments: json!({ "path": "/tmp/a.txt" }),
            result: Some(vec![ContentBlock::Text { text: ArcStr::from("内容"), annotations: None, meta: None }]),
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id.as_str(), "call_1");
        assert_eq!(back.name.as_str(), "read");
        assert_eq!(back.arguments["path"], "/tmp/a.txt", "arguments 保持 JSON 对象");
        assert!(matches!(back.result.unwrap().get(0).unwrap(), ContentBlock::Text { text, .. } if text.as_str() == "内容"));
    }
    #[test]
    fn message_assistant_optional_fields_omitted() {
        let m = Message::Assistant { content: Value::String("回答".into()), reasoning_content: Value::Null, tool_calls: Value::Null};
        let v = serde_json::to_value(&m).unwrap();
        assert!(v.get("reasoning_content").is_none(), "None 字段不应序列化");
        assert!(v.get("tool_calls").is_none(), "None 字段不应序列化");
    }

    #[test]
    fn tool_config_serializes_openai_function_shape() {
        // LLM 侧工具定义契约：OpenAI tools[].function 形状（name / description / parameters）
        let tc = ToolConfig {
            name: ArcStr::from("read"),
            description: ArcStr::from("读取文本文件"),
            parameters: Arc::new(json!({ "type": "object", "properties": { "path": { "type": "string" } } })),
        };
        let tj = serde_json::to_value(&tc).unwrap();
        assert_eq!(tj["name"], "read");
        assert_eq!(tj["parameters"]["properties"]["path"]["type"], "string");
    }
}
