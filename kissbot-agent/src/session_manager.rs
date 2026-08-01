use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use arc_swap::ArcSwap;
use dashmap::DashMap;

use crate::config_manager::ProviderModel;
use crate::types::{ContextMessage, MessageItem, Mode, SessionKey};

/// 最大上下文消息数量，超过时触发重置
const MAX_CONTEXT_MESSAGES: usize = 100;

/// 会话上下文（原 ContextBuilder 逻辑，按会话持有）
pub struct SessionContext {
    messages: VecDeque<ContextMessage>,
    system_message: Option<String>,
    /// 保存已发送的消息 content，用于 is_self=1 对比
    sent_contents: VecDeque<String>,
}

impl SessionContext {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            system_message: None,
            sent_contents: VecDeque::with_capacity(64),
        }
    }

    /// 设置系统消息（会话创建或重置时）
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
    pub fn push_tool_call(&mut self, tool_name: String, parameters: serde_json::Value, time: String) {
        self.messages.push_back(ContextMessage::ToolCall { tool_name, parameters, time });
    }

    /// 追加 tool result
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

/// 单个会话：独立上下文、模型与模式状态
pub struct Session {
    pub key: SessionKey,
    pub context: tokio::sync::Mutex<SessionContext>,
    /// 会话级模型（创建时取 default_model，/model 调整）
    pub model: ArcSwap<ProviderModel>,
}

impl Session {
    pub fn new(key: SessionKey, model: ProviderModel) -> Self {
        Self {
            key,
            context: tokio::sync::Mutex::new(SessionContext::new()),
            model: ArcSwap::from_pointee(model),
        }
    }
}

/// 会话管理器：汇总所有绑定 channel 的 (agent_id, role_name, mode) 去重维护会话集合
pub struct SessionManager {
    sessions: DashMap<SessionKey, Arc<Session>>,
    /// 运行态 per-channel mode（不回写，重启回 Role）
    channel_modes: DashMap<String, Mode>,
}

impl SessionManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
            channel_modes: DashMap::new(),
        })
    }

    /// 按 key 取会话
    pub fn get(&self, key: &SessionKey) -> Option<Arc<Session>> {
        self.sessions.get(key).map(|e| e.value().clone())
    }

    /// 定位会话，不存在则创建（model 为初始模型）；返回 (会话, 是否新建)
    pub fn get_or_create(&self, key: &SessionKey, model: ProviderModel) -> (Arc<Session>, bool) {
        if let Some(s) = self.get(key) {
            return (s, false);
        }
        let session = Arc::new(Session::new(key.clone(), model));
        self.sessions.insert(key.clone(), session.clone());
        (session, true)
    }

    /// 只保留仍在绑定集合中的会话（绑定信息变化后清理无绑定会话）
    pub fn retain(&self, keys: &HashSet<SessionKey>) {
        self.sessions.retain(|k, _| keys.contains(k));
    }

    /// 设置来源 channel 的运行态模式（不回写）
    pub fn set_channel_mode(&self, channel_id: &str, mode: Mode) {
        self.channel_modes.insert(channel_id.to_string(), mode);
    }

    /// 读取来源 channel 的运行态模式（缺省角色模式）
    pub fn channel_mode(&self, channel_id: &str) -> Mode {
        self.channel_modes.get(channel_id).map(|m| m.value().clone()).unwrap_or(Mode::Role)
    }
}
