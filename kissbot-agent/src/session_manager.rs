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
}

impl SessionContext {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            system_message: None,
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
    #[allow(dead_code)]
    pub fn push_tool_call(&mut self, tool_name: String, parameters: serde_json::Value, time: String) {
        self.messages.push_back(ContextMessage::ToolCall { tool_name, parameters, time });
    }

    /// 追加 tool result
    #[allow(dead_code)]
    pub fn push_tool_result(&mut self, tool_name: String, result: serde_json::Value, time: String) {
        self.messages.push_back(ContextMessage::ToolResult { tool_name, result, time });
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
    /// 会话级模型（创建时取 default_model，/model 调整）；None = 无模型（普通消息静默忽略）
    pub model: ArcSwap<Option<ProviderModel>>,
    /// 会话状态保存的 agent_id（UUID；创建时取自触发 channel 的运行态绑定，之后不变）
    /// session_key 仅作去重：取记忆/ego 一律用本字段，不再从 key 提取 agent_name 解析
    pub agent_id: Arc<String>,
}

impl Session {
    pub fn new(key: SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> Self {
        Self {
            key,
            context: tokio::sync::Mutex::new(SessionContext::new()),
            model: ArcSwap::from_pointee(model),
            agent_id,
        }
    }
}

/// 会话管理器：汇总所有绑定 channel 的 (agent_name, role_name, mode) 去重维护会话集合
/// （session_key 仅用于去重；agent_id 解析结果由各 channel 运行态绑定保存，不在此提取）
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

    /// 定位会话，不存在则创建（model 为初始模型，None = 无模型；agent_id 为会话状态保存的解析结果）；返回 (会话, 是否新建)
    pub fn get_or_create(&self, key: &SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> (Arc<Session>, bool) {
        if let Some(s) = self.get(key) {
            return (s, false);
        }
        let session = Arc::new(Session::new(key.clone(), model, agent_id));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(agent: &str, role: &str) -> SessionKey {
        SessionKey { agent_name: agent.into(), role_name: role.into(), mode: Mode::Role }
    }

    #[test]
    fn get_or_create_dedupes() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k = key("a1", "r1");
        let (s1, created1) = mgr.get_or_create(&k, Some(model.clone()), Arc::new("a1".into()));
        assert!(created1, "首次创建");
        let (s2, created2) = mgr.get_or_create(&k, Some(model.clone()), Arc::new("a1".into()));
        assert!(!created2, "同 key 复用");
        assert!(Arc::ptr_eq(&s1, &s2), "同 key 应返回同一 Session");
        // 不同 mode 是不同会话
        let k_event = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let (_s3, created3) = mgr.get_or_create(&k_event, Some(model), Arc::new("a1".into()));
        assert!(created3, "事件模式是独立会话");
    }

    #[test]
    fn retain_prunes_unbound() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k1 = key("a1", "r1");
        let k2 = key("a2", "r2");
        mgr.get_or_create(&k1, Some(model.clone()), Arc::new("a1".into()));
        mgr.get_or_create(&k2, Some(model), Arc::new("a2".into()));
        let mut keep = HashSet::new();
        keep.insert(k1.clone());
        mgr.retain(&keep);
        assert!(mgr.get(&k1).is_some(), "仍在绑定集合的会话保留");
        assert!(mgr.get(&k2).is_none(), "无绑定会话销毁");
    }

    #[test]
    fn channel_mode_default_role_and_set() {
        let mgr = SessionManager::new();
        assert_eq!(mgr.channel_mode("c1"), Mode::Role, "缺省角色模式");
        mgr.set_channel_mode("c1", Mode::Event("e9".into()));
        assert_eq!(mgr.channel_mode("c1"), Mode::Event("e9".into()));
        assert_eq!(mgr.channel_mode("c2"), Mode::Role, "未设置仍为角色模式");
    }

    #[test]
    fn get_or_create_with_none_model() {
        let mgr = SessionManager::new();
        let key = SessionKey { agent_name: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let (s, created) = mgr.get_or_create(&key, None, Arc::new("a".into()));
        assert!(created);
        assert!(s.model.load().is_none());
    }
}
