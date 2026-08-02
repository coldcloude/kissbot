use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use arc_swap::ArcSwap;
use dashmap::DashMap;

use crate::config_manager::{ChannelConfig, ProviderModel};
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
}

impl Session {
    pub fn new(key: SessionKey, model: Option<ProviderModel>) -> Self {
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

    /// 定位会话，不存在则创建（model 为初始模型，None = 无模型）；返回 (会话, 是否新建)
    pub fn get_or_create(&self, key: &SessionKey, model: Option<ProviderModel>) -> (Arc<Session>, bool) {
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

    /// 从绑定该会话的多个 channel 中选定发送 channel：
    /// is_send_channel=true 优先，否则选首个绑定；无匹配返回 None
    pub fn resolve_send_channel(
        &self,
        key: &SessionKey,
        channels: Vec<(String, Arc<ChannelConfig>)>,
    ) -> Option<String> {
        let mut first = None;
        for (cid, ch) in channels {
            if ch.agent_name.as_str() != key.agent_name || ch.role_name.as_str() != key.role_name {
                continue;
            }
            if self.channel_mode(&cid) != key.mode {
                continue;
            }
            if !ch.enabled {
                continue;
            }
            if first.is_none() {
                first = Some(cid.clone());
            }
            if ch.is_send_channel {
                return Some(cid);
            }
        }
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_manager::ChannelUser;

    fn sample_channel(id: &str, agent: &str, role: &str, is_send: bool) -> ChannelConfig {
        ChannelConfig {
            channel_id: Arc::new(id.into()),
            ws_url: Arc::new("ws://127.0.0.1:8201".into()),
            admins: Arc::new(HashSet::new()),
            bind_user: ChannelUser { messenger_id: Arc::new("web".into()), user_id: Arc::new("u1".into()) },
            agent_name: Arc::new(agent.into()),
            role_name: Arc::new(role.into()),
            is_send_channel: is_send,
            enabled: true,
        }
    }

    fn key(agent: &str, role: &str) -> SessionKey {
        SessionKey { agent_name: agent.into(), role_name: role.into(), mode: Mode::Role }
    }

    #[test]
    fn get_or_create_dedupes() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k = key("a1", "r1");
        let (s1, created1) = mgr.get_or_create(&k, Some(model.clone()));
        assert!(created1, "首次创建");
        let (s2, created2) = mgr.get_or_create(&k, Some(model.clone()));
        assert!(!created2, "同 key 复用");
        assert!(Arc::ptr_eq(&s1, &s2), "同 key 应返回同一 Session");
        // 不同 mode 是不同会话
        let k_event = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let (_s3, created3) = mgr.get_or_create(&k_event, Some(model));
        assert!(created3, "事件模式是独立会话");
    }

    #[test]
    fn retain_prunes_unbound() {
        let mgr = SessionManager::new();
        let model = ProviderModel { provider: "deepseek".into(), model: "deepseek-4-flash".into() };
        let k1 = key("a1", "r1");
        let k2 = key("a2", "r2");
        mgr.get_or_create(&k1, Some(model.clone()));
        mgr.get_or_create(&k2, Some(model));
        let mut keep = HashSet::new();
        keep.insert(k1.clone());
        mgr.retain(&keep);
        assert!(mgr.get(&k1).is_some(), "仍在绑定集合的会话保留");
        assert!(mgr.get(&k2).is_none(), "无绑定会话销毁");
    }

    #[test]
    fn resolve_send_channel_flag_then_first() {
        let mgr = SessionManager::new();
        let k = key("a1", "r1");
        let channels = vec![
            ("c1".to_string(), Arc::new(sample_channel("c1", "a1", "r1", false))),
            ("c2".to_string(), Arc::new(sample_channel("c2", "a1", "r1", true))),
            ("c3".to_string(), Arc::new(sample_channel("c3", "a1", "r1", false))),
        ];
        assert_eq!(mgr.resolve_send_channel(&k, channels.clone()).as_deref(), Some("c2"), "is_send_channel 优先");

        // 全 false → 首个绑定
        let channels_all_false = vec![
            ("c1".to_string(), Arc::new(sample_channel("c1", "a1", "r1", false))),
            ("c3".to_string(), Arc::new(sample_channel("c3", "a1", "r1", false))),
        ];
        assert_eq!(mgr.resolve_send_channel(&k, channels_all_false).as_deref(), Some("c1"));

        // 不同三元组 → None
        let other = vec![("c9".to_string(), Arc::new(sample_channel("c9", "a9", "r9", true)))];
        assert_eq!(mgr.resolve_send_channel(&k, other), None);
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
        let (s, created) = mgr.get_or_create(&key, None);
        assert!(created);
        assert!(s.model.load().is_none());
    }
}
