use std::collections::VecDeque;

use crate::types::ContextMessage;
use crate::model_client::MessageItem;

/// 最大上下文消息数量，超过时触发重置
const MAX_CONTEXT_MESSAGES: usize = 100;

pub struct ContextBuilder {
    messages: VecDeque<ContextMessage>,
    system_message: Option<String>,
    /// 保存已发送的消息 content，用于 is_self=1 对比
    sent_contents: VecDeque<String>,
}

impl ContextBuilder {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            system_message: None,
            sent_contents: VecDeque::with_capacity(64),
        }
    }

    /// 设置系统消息（启动时或角色切换时）
    pub fn set_system_message(&mut self, content: String) {
        self.system_message = Some(content);
    }

    /// 从 MemoryReader 加载历史记录重建上下文
    pub fn load_history(&mut self, messages: Vec<ContextMessage>) {
        self.messages.clear();
        for msg in messages {
            self.messages.push_back(msg);
        }
    }

    /// 追加用户消息
    pub fn push_user_message(&mut self, msg: ContextMessage) {
        self.messages.push_back(msg);
    }

    /// 追加 assistant 回复
    pub fn push_assistant(&mut self, content: String, time: String) {
        self.messages.push_back(ContextMessage::Assistant { content, time });
    }

    /// 追加 tool call
    #[allow(dead_code)]
    pub fn push_tool_call(&mut self, tool_name: String, parameters: serde_json::Value, time: String) {
        self.messages.push_back(ContextMessage::ToolCall { tool_name, parameters, time });
    }

    /// 追加 tool result
    #[allow(dead_code)]
    pub fn push_tool_result(&mut self, tool_name: String, result: serde_json::Value, time: String) {
        self.messages.push_back(ContextMessage::ToolResult { tool_name, result, time });
    }

    /// 记录已发送的消息内容（用于 is_self=1 识别）
    pub fn record_sent_content(&mut self, content: String) {
        if self.sent_contents.len() >= 64 {
            self.sent_contents.pop_front();
        }
        self.sent_contents.push_back(content);
    }

    /// 检查内容是否为最近发出的消息回显
    pub fn is_self_echo(&self, content: &str) -> bool {
        self.sent_contents.iter().any(|s| s == content)
    }

    /// 构建模型消息列表
    pub fn build(&self) -> Vec<MessageItem> {
        let mut items = Vec::new();

        if let Some(system) = &self.system_message {
            items.push(MessageItem {
                role: "system".to_string(),
                content: system.clone(),
            });
        }

        for msg in &self.messages {
            match msg {
                ContextMessage::User { content, .. } => {
                    items.push(MessageItem {
                        role: "user".to_string(),
                        content: content.clone(),
                    });
                }
                ContextMessage::Assistant { content, .. } => {
                    items.push(MessageItem {
                        role: "assistant".to_string(),
                        content: content.clone(),
                    });
                }
                ContextMessage::ToolCall { tool_name, parameters, .. } => {
                    items.push(MessageItem {
                        role: "assistant".to_string(),
                        content: format!("工具调用: {} ({})", tool_name, parameters),
                    });
                }
                ContextMessage::ToolResult { tool_name, result, .. } => {
                    items.push(MessageItem {
                        role: "user".to_string(),
                        content: format!("工具 {} 返回: {}", tool_name, result),
                    });
                }
            }
        }

        items
    }

    /// 检查上下文是否超长
    pub fn is_overflow(&self) -> bool {
        self.messages.len() >= MAX_CONTEXT_MESSAGES
    }

    /// 清空上下文（重置时调用）
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}
