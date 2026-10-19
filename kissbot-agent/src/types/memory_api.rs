use std::sync::Arc;

use arcstr::ArcStr;
use chrono::Local;
use kissbot_api::channel::{IncomingMessageEvent, OutgoingMessageResponse};
use kissbot_api::{ChannelRequest, MemoryMeta, ThinkRequest, ToolCallRequest, ToolResultRequest};
use kissbot_llm::ToolCall;

use crate::configs::OutChannel;
use crate::types::MemoryKey;

fn now() -> ArcStr {
    ArcStr::from(Local::now().format("%Y-%m-%d %H:%M:%S").to_string())
}

/// 上行消息 → 记忆写入请求（纯函数：身份全部取自消息本身）。
///
/// 接收方身份 = `event.recipient_user_id`（agent 在 channel 绑定的用户，即 agent 视角的 self）；
/// `meta` 由调用方决定（摘要记忆 pipeline 在 locate 之后填入所属 case，其余 pipeline 传 None）。
pub fn channel_request_from_incoming(
    memory_key: &MemoryKey,
    event: &IncomingMessageEvent,
    meta: Option<Arc<MemoryMeta>>,
) -> ChannelRequest {
    ChannelRequest {
        agent_id: memory_key.agent_id.clone(),
        role_event: memory_key.role_event.clone(),
        messenger_id: event.incoming_message.messenger_id.clone(),
        user_id: event.incoming_message.user_id.clone(),
        self_user_id: event.recipient_user_id.clone(),
        group_id: event.incoming_message.group_id.clone(),
        is_self: 0,
        msg_id: event.incoming_message.msg_id.clone(),
        messenger_name: event.incoming_message.messenger_name.clone(),
        user_name: event.incoming_message.user_name.clone(),
        group_name: event.incoming_message.group_name.clone(),
        content: event.incoming_message.content.clone(),
        time: event.incoming_message.time.clone(),
        meta,
    }
}

/// 下行回复 → 记忆写入请求（纯函数：身份取回复通道，消息内容取发送结果）。
///
/// `meta` 由调用方决定（摘要记忆 pipeline 写入当前触发 case，其余 pipeline 传 None）。
pub fn channel_request_from_outgoing(
    memory_key: &MemoryKey,
    out_channel: &OutChannel,
    response: &OutgoingMessageResponse,
    meta: Option<Arc<MemoryMeta>>,
) -> ChannelRequest {
    ChannelRequest {
        agent_id: memory_key.agent_id.clone(),
        role_event: memory_key.role_event.clone(),
        messenger_id: out_channel.user.messenger_id.clone(),
        user_id: out_channel.user.user_id.clone(),
        self_user_id: out_channel.user.user_id.clone(),
        group_id: out_channel.group_id.clone(),
        is_self: 1,
        msg_id: response.msg_id.clone(),
        messenger_name: response.messenger_name.clone(),
        user_name: response.user_name.clone(),
        group_name: response.group_name.clone(),
        content: response.content.clone(),
        time: response.time.clone(),
        meta,
    }
}

pub fn build_memory_think(memory_key: &MemoryKey, key: ArcStr, reasoning_content: Option<ArcStr>, thinking: Option<ArcStr>) -> ThinkRequest {
    let time = now();
    ThinkRequest {
        agent_id: memory_key.agent_id.clone(),
        role_event: memory_key.role_event.clone(),
        reasoning_content,
        thinking,
        key,
        time,
    }
}

pub fn build_memory_tool_call(memory_key: &MemoryKey, key: ArcStr, tool_call: &ToolCall) -> ToolCallRequest {
    let time = now();
    let tool_params = match tool_call.result_arguments.as_ref() {
        Some(params) => params.clone(),
        None => Arc::new(tool_call.arguments.clone()),
    };
    ToolCallRequest {
        agent_id: memory_key.agent_id.clone(),
        role_event: memory_key.role_event.clone(),
        tool_name: tool_call.name.clone(),
        tool_params,
        key,
        time,
    }
}

pub fn build_memory_tool_result(memory_key: &MemoryKey, key: ArcStr, tool_call: &ToolCall) -> ToolResultRequest {
    let time = now();
    ToolResultRequest {
        agent_id: memory_key.agent_id.clone(),
        role_event: memory_key.role_event.clone(),
        tool_result: tool_call.result.clone(),
        key,
        time,
    }
}
#[cfg(test)]
mod tests {
    use kissbot_api::channel::{ChannelUser, IncomingMessage, OutgoingMessageResponse};
    use kissbot_api::message::Content;
    use kissbot_api::RoleEvent;

    use super::*;

    fn memory_key() -> MemoryKey {
        MemoryKey {
            agent_id: ArcStr::from("a1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("dev"))),
        }
    }

    fn incoming() -> IncomingMessageEvent {
        IncomingMessageEvent {
            recipient_user_id: ArcStr::from("self1"),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: ArcStr::from("m1"),
                messenger_id: ArcStr::from("web"),
                user_id: ArcStr::from("u1"),
                group_id: ArcStr::from("g1"),
                messenger_name: ArcStr::from("Web"),
                user_name: ArcStr::from("alice"),
                group_name: ArcStr::from("群一"),
                content: Content::Text(ArcStr::from("你好")),
                time: ArcStr::from("2026-09-23 10:00:00"),
            }),
        }
    }

    #[test]
    fn incoming_request_takes_identity_from_message_and_keeps_meta() {
        let meta = Arc::new(MemoryMeta::with_case_name(ArcStr::from("role-weather")));
        let request = channel_request_from_incoming(&memory_key(), &incoming(), Some(meta.clone()));
        assert_eq!(request.agent_id.as_str(), "a1");
        assert_eq!(request.role_event.as_ref(), &RoleEvent::new(ArcStr::from("dev")));
        assert_eq!(request.messenger_id.as_str(), "web");
        assert_eq!(request.user_id.as_str(), "u1");
        assert_eq!(request.self_user_id.as_str(), "self1", "接收方身份 = recipient_user_id");
        assert_eq!(request.group_id.as_str(), "g1");
        assert_eq!(request.is_self, 0);
        assert_eq!(request.msg_id.as_str(), "m1");
        assert_eq!(request.user_name.as_str(), "alice");
        assert_eq!(request.time.as_str(), "2026-09-23 10:00:00");
        assert_eq!(request.meta, Some(meta), "摘要记忆 pipeline 在 locate 后填入 case");
        assert!(matches!(request.content, Content::Text(ref text) if text.as_str() == "你好"));
    }

    #[test]
    fn outgoing_request_uses_reply_channel_and_response() {
        let out_channel = OutChannel {
            channel_id: ArcStr::from("web-1"),
            user: Arc::new(ChannelUser {
                messenger_id: ArcStr::from("web"),
                user_id: ArcStr::from("self1"),
            }),
            group_id: ArcStr::from("g1"),
        };
        let response = OutgoingMessageResponse {
            msg_id: ArcStr::from("m2"),
            time: ArcStr::from("2026-09-23 10:00:01"),
            content: Content::Text(ArcStr::from("回复")),
            messenger_name: ArcStr::from("Web"),
            user_name: ArcStr::from("self1"),
            group_name: ArcStr::from("群一"),
        };
        let meta = Arc::new(MemoryMeta::with_case_name(ArcStr::from("role-weather")));
        let request = channel_request_from_outgoing(&memory_key(), &out_channel, &response, Some(meta.clone()));
        assert_eq!(request.messenger_id.as_str(), "web");
        assert_eq!(request.user_id.as_str(), "self1");
        assert_eq!(request.self_user_id.as_str(), "self1");
        assert_eq!(request.group_id.as_str(), "g1");
        assert_eq!(request.is_self, 1);
        assert_eq!(request.msg_id.as_str(), "m2");
        assert_eq!(request.time.as_str(), "2026-09-23 10:00:01");
        assert_eq!(request.meta, Some(meta));
        assert!(matches!(request.content, Content::Text(ref text) if text.as_str() == "回复"));

        // 不带 meta：普通记忆 pipeline
        assert!(channel_request_from_outgoing(&memory_key(), &out_channel, &response, None).meta.is_none());
    }
}
