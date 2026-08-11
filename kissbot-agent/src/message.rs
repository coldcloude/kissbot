use std::sync::Arc;

use kissbot_api::channel::IncomingMessageEvent;
use kissbot_api::message::Content;

use crate::types::Message;

/// 消息内容（channel record 的最小视图：name + 文本段 + is_self；id 类不保留；时间由排序键承担，不保留）
#[derive(Debug, Clone)]
pub struct MessageContent {
    pub user_name: Arc<String>,
    /// 文本段列表：每个元素为 Content::Text 的 Arc 克隆（Multi 拆多个元素；打包时以 \n 拼接）
    pub content: Vec<Arc<String>>,
    /// 是否 agent 自身消息（is_self：1=self，0=他人）
    pub is_self: bool,
}

/// parse_channel_groups 与 pack_batch 共用的 MessageContent 构造：
/// user_name 克隆 Arc；content 递归提取 Text 段（collect_text_parts）；is_self 按 >0 转 bool
pub fn extract_content(user_name: &Arc<String>, is_self: usize, content: &Content) -> MessageContent {
    let mut parts = Vec::new();
    collect_text_parts(content, &mut parts);
    MessageContent { user_name: user_name.clone(), content: parts, is_self: is_self > 0 }
}

/// 递归收集 Content 中的全部 Text 段到 parts：Text 直接 clone 其 Arc<String>；Multi 递归遍历（可嵌套任意深度），
/// 各 Text 子项各占一个元素；其余变体（附件/系统通知/Think/ToolCall/ToolResult 等）不产生段（调用方跳过空结果）
/// 注意：Content 用 #[serde(tag="msg_type", content="data")] 序列化，反序列化后为类型化枚举，直接匹配即可
fn collect_text_parts(content: &Content, parts: &mut Vec<Arc<String>>) {
    match content {
        Content::Text(text) => parts.push(text.clone()),
        Content::Multi(items) => {
            for item in items {
                collect_text_parts(item, parts);
            }
        }
        _ => {}
    }
}

/// is_self=0 的单行格式（pack_memory_messages 的 User 分支与 pack_batch 共用）：
/// name 为空只留 text，否则 "name: text"
fn user_line(user_name: &str, text: &str) -> String {
    if user_name.is_empty() {
        text.to_string()
    } else {
        format!("{}: {}", user_name, text)
    }
}

/// 打包记忆消息为交替的 User/Assistant 消息序列：
/// content 为空的记录（非文本）跳过；
/// 找到第一条非 self（User）消息（之前的 self 消息丢弃，对话必须以 User 开头），
/// 连续同 is_self 的记录合并为一条消息，User/Assistant 交替；
/// User 段 content 逐行 "name: text"（name 为空只留 text），Assistant 段只保留 content（不含 name/time）；
/// 若以 User 结尾则补一条 Content 为空的 Assistant；空输入/无 User 返回空 Vec
pub fn pack_memory_messages(msgs: &[MessageContent]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    let mut user_buf: Vec<String> = Vec::new();
    let mut asst_buf: Vec<String> = Vec::new();
    let mut is_asst = false;  // 当前段类型（false=User 段）
    let mut started = false;  // 是否已开始对话（找到第一条非 self 消息）；此前 self 全部丢弃
    for m in msgs {
        if m.content.is_empty() {
            continue;  // 非文本记录（空 content）跳过
        }
        if !started {
            // 对话必须以 User 开头：丢弃前导 self
            if m.is_self {
                continue;
            }
            started = true;
            is_asst = false;
        }
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
        // 拼接文本提前算一遍（is_self 与 user_line 两分支共用；元素为 Content::Text 的 Arc 克隆）
        let text = m.content.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
        if m.is_self {
            asst_buf.push(text);  // Assistant：只要 content，不带 name/time
        } else {
            user_buf.push(user_line(m.user_name.as_str(), &text));  // User：name 空只留 content，否则 "name: text"
        }
    }
    // flush 最后一段（仅当已开始对话）
    if started {
        if is_asst {
            out.push(Message::Assistant { content: Arc::new(asst_buf.join("\n")), reasoning_content: None, tool_calls: None });
        } else {
            out.push(Message::User { content: Arc::new(user_buf.join("\n")) });
            // 以 User 结尾：补一条空 Assistant（模型对话需以待回答的 Assistant 结尾）
            out.push(Message::Assistant { content: Arc::new(String::new()), reasoning_content: None, tool_calls: None });
        }
    }
    out
}

/// 将一批 IncomingMessageEvent 打包为一条 User Message（替换 session_manager BatchConsumer 的内联拼接）：
/// 每 event 经 extract_content 构造（is_self=0），空 content（非文本）跳过，逐行 user_line 拼接（\n 连接）；
/// 全部跳过时返回空 content 的 User（try_flush 已在 items 为空时提前返回，此处输入必非空）
pub fn pack_batch(events: &[Arc<IncomingMessageEvent>]) -> Message {
    let mut lines: Vec<String> = Vec::new();
    for e in events {
        let mc = extract_content(&e.incoming_message.user_name, 0, &e.incoming_message.content);
        if mc.content.is_empty() {
            continue;  // 非文本事件跳过（与 pack_memory_messages 的 is_self=0 处理一致）
        }
        let text = mc.content.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
        lines.push(user_line(mc.user_name.as_str(), &text));
    }
    Message::User { content: Arc::new(lines.join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_api::channel::IncomingMessage;
    use kissbot_api::message::Content;

    // 构造测试用 MessageContent
    fn msg(name: &str, content: &str, is_self: bool) -> MessageContent {
        MessageContent { user_name: Arc::new(name.to_string()), content: vec![Arc::new(content.to_string())], is_self }
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
    fn pack_memory_messages_skips_empty_content_records() {
        // 非文本记录（content 为空 Vec）→ 跳过：不产生消息、不触发段切换、不阻止后续合并
        let empty = || MessageContent { user_name: Arc::new("agent".to_string()), content: vec![], is_self: true };
        let msgs = vec![
            msg("u1", "hi", false),
            empty(),                       // 空 content → 跳过（夹在两条 User 之间也不切段）
            msg("u2", "there", false),
            msg("agent", "ok", true),
            empty(),                       // 结尾空 content → 跳过（不影响末尾 Assistant flush）
        ];
        let out = pack_memory_messages(&msgs);
        // 期望：[User("u1: hi\nu2: there"), Assistant("ok")]
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u1: hi\nu2: there"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.as_str() == "ok"));
    }

    #[test]
    fn pack_memory_messages_single_user_appends_empty_assistant() {
        let out = pack_memory_messages(&[msg("u", "hi", false)]);
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Message::User { content } if content.as_str() == "u: hi"));
        assert!(matches!(&out[1], Message::Assistant { content, .. } if content.is_empty()));
    }

    #[test]
    fn extract_content_collects_text_parts_and_is_self_flag() {
        // Text → 单段；is_self > 0 → true
        let mc = extract_content(&Arc::new("u".to_string()), 1, &Content::Text(Arc::new("hi".into())));
        assert_eq!(mc.content, vec![Arc::new("hi".to_string())]);
        assert!(mc.is_self, "is_self>0 → true");
        // Multi 递归收集 Text 段；非文本子项跳过
        let multi = Content::Multi(vec![
            Content::Text(Arc::new("a".into())),
            Content::ToolCall(Arc::new("k".into())),
            Content::Multi(vec![Content::Text(Arc::new("b".into()))]),
        ]);
        let mc2 = extract_content(&Arc::new("u".to_string()), 0, &multi);
        assert_eq!(mc2.content, vec![Arc::new("a".to_string()), Arc::new("b".to_string())]);
        assert!(!mc2.is_self, "is_self=0 → false");
        // 非文本 → 空 content
        let mc3 = extract_content(&Arc::new("u".to_string()), 0, &Content::ToolCall(Arc::new("k".into())));
        assert!(mc3.content.is_empty());
    }

    // 构造测试用 IncomingMessageEvent（content 由调用方指定）
    fn ev(name: &str, content: Content) -> Arc<IncomingMessageEvent> {
        Arc::new(IncomingMessageEvent {
            recipient_user_id: Arc::new("self".into()),
            incoming_message: Arc::new(IncomingMessage {
                msg_id: Arc::new("m".into()),
                messenger_id: Arc::new("web".into()),
                user_id: Arc::new("u".into()),
                group_id: Arc::new("g".into()),
                messenger_name: Arc::new("".into()),
                user_name: Arc::new(name.into()),
                group_name: Arc::new("".into()),
                content,
                time: Arc::new("2026-08-07 10:00:00".into()),
            }),
        })
    }

    #[test]
    fn pack_batch_joins_user_lines_and_skips_non_text() {
        let events = vec![
            ev("u1", Content::Text(Arc::new("a".into()))),
            ev("", Content::Text(Arc::new("b".into()))),
            ev("u3", Content::ToolCall(Arc::new("k".into()))),  // 非文本 → 跳过
            ev("u4", Content::Text(Arc::new("c".into()))),
        ];
        let msg = pack_batch(&events);
        assert!(matches!(&msg, Message::User { content } if content.as_str() == "u1: a\nb\nu4: c"), "非文本事件跳过，空 name 只留 text");

        // 全非文本 → 空 content 的 User
        let all_empty = vec![ev("u", Content::ToolCall(Arc::new("k".into())))];
        let msg2 = pack_batch(&all_empty);
        assert!(matches!(&msg2, Message::User { content } if content.is_empty()));
    }
}
