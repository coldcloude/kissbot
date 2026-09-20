use std::sync::Arc;

use serde_json::Value;

use crate::{config_manager::ConfigManager, nexus::Nexus, pipeline::AgentOutputProcessor, types::{Message, ModelResponse, Result, SessionKey, role_mode}};
use async_trait::async_trait;

/// Agentic Loop 工具调用轮次上限（防死循环）
const MAX_TOOL_ROUNDS: usize = 10;

/// 单个内容块渲染为提示词文本：文本块取原文，其余块给紧凑占位（不把 base64 塞进上下文）
fn render_content_block(block: &Value) -> String {
    let field = |name: &str| block.get(name).and_then(Value::as_str).unwrap_or("");
    match block.get("type").and_then(Value::as_str) {
        Some("text") => field("text").to_string(),
        Some("image") => format!("[image {}]", field("mimeType")),
        Some("audio") => format!("[audio {}]", field("mimeType")),
        Some("resource_link") => format!("[resource_link {} {}]", field("name"), field("uri")),
        Some("resource") => match block.pointer("/resource/text").and_then(Value::as_str) {
            Some(text) => text.to_string(),
            None => format!(
                "[resource {}]",
                block.pointer("/resource/uri").and_then(Value::as_str).unwrap_or("")
            ),
        },
        _ => block.to_string(),
    }
}

/// 工具结果渲染：MCP content 数组（内容块数组）逐块渲染，字符串原样输出，其余值按 JSON 输出
pub(crate) fn render_tool_result(result: &Value) -> String {
    if let Some(text) = result.as_str() {
        return text.to_string();
    }
    if let Some(blocks) = result.as_array() {
        if !blocks.is_empty() && blocks.iter().all(|b| b.get("type").and_then(Value::as_str).is_some()) {
            return blocks.iter().map(render_content_block).collect::<Vec<_>>().join("\n");
        }
    }
    result.to_string()
}

/// 工具消息体：成功取 `result`，失败取 `error`（两者都是 MCP content 转成的内容块数组）
pub(crate) fn tool_message_body(result: &Value, error: &Value) -> String {
    if result.is_null() {
        render_tool_result(error)
    } else {
        render_tool_result(result)
    }
}

pub struct ChannelOutputProcessor {
    session_key: Arc<SessionKey>,
}

impl ChannelOutputProcessor {
    pub fn new(session_key: Arc<SessionKey>) -> Self {
        Self { session_key }
    }
}

#[async_trait]
impl AgentOutputProcessor for ChannelOutputProcessor {
    async fn accept(&self, round: usize, response: Result<ModelResponse>) -> (bool, Vec<Message>) {
        if let Ok(mut resp) = response {
            let mut next = false;
            let mut new_messages = Vec::new();
            new_messages.push(Message::Assistant {
                content: resp.raw_content.take(),
                reasoning_content: resp.raw_reasoning_content.take(),
                tool_calls: resp.raw_tool_calls.take(),
            });
            if let Some(mut tool_calls) = resp.tool_calls {
                for tool_call in tool_calls.drain(..) {
                    let content = tool_message_body(&tool_call.data.result, &tool_call.data.error);
                    new_messages.push(Message::Tool {
                        tool_call_id: tool_call.id,
                        content: Arc::new(content),
                    });
                    next = true;
                }
            }
            if let Some(content) = resp.content.as_ref() {
                let oc_cfg = ConfigManager::get().out_channel_config(self.session_key.agent_id.as_str(), self.session_key.role_name.as_str()).await;
                if let Some(out_channel) = oc_cfg.out_channel.as_ref() {
                    let agent_id = self.session_key.agent_id.as_str();
                    let role_name = self.session_key.role_name.as_str();
                    let role_event = role_mode(role_name, &self.session_key.mode);
                    Nexus::get().send_outgoing(agent_id, role_name, &role_event, out_channel.as_ref(), content.clone()).await;
                }
            }
            if round >= MAX_TOOL_ROUNDS {
                next = false;
            }
            (next, new_messages)
        } else {
            (false, Vec::new())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_blocks_render_as_plain_text() {
        let result = json!([{ "type": "text", "text": "文件内容" }]);
        assert_eq!(render_tool_result(&result), "文件内容");
    }

    #[test]
    fn multiple_text_blocks_join_by_newline() {
        let result = json!([
            { "type": "text", "text": "第一行" },
            { "type": "text", "text": "第二行" }
        ]);
        assert_eq!(render_tool_result(&result), "第一行\n第二行");
    }

    #[test]
    fn non_text_blocks_render_as_compact_placeholders() {
        let result = json!([
            { "type": "text", "text": "说明" },
            { "type": "image", "data": "eHl6", "mimeType": "image/png" },
            { "type": "audio", "data": "eA==", "mimeType": "audio/wav" },
            { "type": "resource_link", "uri": "file:///a.txt", "name": "a.txt" },
            { "type": "resource", "resource": { "uri": "file:///b.txt", "text": "内嵌" } },
            { "type": "resource", "resource": { "uri": "file:///c.png", "blob": "eHl6" } }
        ]);
        let rendered = render_tool_result(&result);
        assert_eq!(
            rendered,
            "说明\n[image image/png]\n[audio audio/wav]\n[resource_link a.txt file:///a.txt]\n内嵌\n[resource file:///c.png]"
        );
        assert!(!rendered.contains("eHl6"), "base64 不应进入提示词");
    }

    #[test]
    fn non_content_values_render_verbatim() {
        assert_eq!(render_tool_result(&json!("纯文本")), "纯文本");
        assert_eq!(render_tool_result(&json!({ "ok": true })), "{\"ok\":true}");
        assert_eq!(render_tool_result(&json!([])), "[]");
    }

    #[test]
    fn tool_message_body_picks_error_when_result_empty() {
        let error = json!([{ "type": "text", "text": "工具不存在: x" }]);
        assert_eq!(tool_message_body(&Value::Null, &error), "工具不存在: x");
        let result = json!([{ "type": "text", "text": "文件内容" }]);
        assert_eq!(tool_message_body(&result, &Value::Null), "文件内容");
        // 失败结果的图片块同样给占位、不塞 base64
        let error_with_image = json!([
            { "type": "text", "text": "失败" },
            { "type": "image", "data": "eHl6", "mimeType": "image/png" }
        ]);
        assert_eq!(tool_message_body(&Value::Null, &error_with_image), "失败\n[image image/png]");
    }
}
