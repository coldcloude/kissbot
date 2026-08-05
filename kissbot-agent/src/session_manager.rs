use std::collections::HashSet;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use arc_swap::ArcSwap;
use dashmap::DashMap;

use crate::config_manager::ProviderModel;
use crate::types::{Message, Mode, SessionKey};

/// 会话上下文：纯内存消息序列 + system 消息（缓存/历史持久化由 coordinator 负责）
pub struct SessionContext {
    messages: Vec<Message>,
    system_message: Option<String>,
}

impl SessionContext {
    pub fn new() -> Self {
        Self { messages: Vec::new(), system_message: None }
    }

    /// 设置系统消息（会话创建或重置时）
    pub fn set_system_message(&mut self, content: String) {
        self.system_message = Some(content);
    }

    /// 取系统消息（压缩/恢复用；当前调用方经 build() 内部读取，保留供后续使用）
    #[allow(dead_code)]
    pub fn system_message(&self) -> Option<&str> {
        self.system_message.as_deref()
    }

    /// 从缓存/记忆加载历史消息重建上下文（system 之外的部分）
    pub fn load_messages(&mut self, messages: Vec<Message>) {
        self.messages.clear();
        self.messages = messages;
    }

    /// 追加一条消息
    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// 构建模型消息列表（system 在最前）
    pub fn build(&self) -> Vec<Message> {
        let mut items = Vec::new();
        if let Some(system) = &self.system_message {
            items.push(Message::System { content: Arc::new(system.clone()) });
        }
        items.extend(self.messages.iter().cloned());
        items
    }

    /// 消息条数（不含 system；is_overflow 内部直接算，保留供调用方读取）
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// 检查是否超长（threshold 来自模型 effective 配置的 max_context_messages）
    pub fn is_overflow(&self, max: usize) -> bool {
        self.messages.len() >= max
    }

    /// 清空上下文（重置时调用；system 保留）
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

/// 单个会话：独立上下文、模型与模式状态
pub struct Session {
    pub agent_name: Arc<String>,    // 运行态：从 key 复制（context 配置查找用）
    pub role_name: Arc<String>,     // 运行态：从 key 复制（身份读取源；SessionKey 仅作去重，不存于 Session）
    pub mode: Arc<Mode>,            // 运行态：从 key 复制
    pub context: tokio::sync::Mutex<SessionContext>,
    /// 待合批缓冲（合批超时后打包进上下文）
    pub batch: tokio::sync::Mutex<crate::batching::BatchBuffer>,
    /// 合批代数：重置时递增使旧计时任务失效
    pub batch_gen: Arc<AtomicU64>,
    /// 会话级模型（创建时取 default_model，/model 调整）；None = 无模型（普通消息静默忽略）
    pub model: ArcSwap<Option<ProviderModel>>,
    /// 会话状态保存的 agent_id（UUID；创建时取自触发 channel 的运行态绑定，之后不变）
    /// 取记忆/ego 一律用本字段（agent_name 仅作 context 配置查找，不参与记忆/ego 定位）
    pub agent_id: Arc<String>,
}

impl Session {
    pub fn new(key: &SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> Self {
        Self {
            agent_name: Arc::new(key.agent_name.clone()),
            role_name: Arc::new(key.role_name.clone()),
            mode: Arc::new(key.mode.clone()),
            context: tokio::sync::Mutex::new(SessionContext::new()),
            batch: tokio::sync::Mutex::new(crate::batching::BatchBuffer::new()),
            batch_gen: Arc::new(AtomicU64::new(0)),
            model: ArcSwap::from_pointee(model),
            agent_id,
        }
    }
}

/// 会话管理器：汇总所有绑定 channel 的 (agent_name, role_name, mode) 去重维护会话集合
/// （session_key 仅用于去重；agent_id 解析结果由各 channel 运行态绑定保存，不在此提取）
pub struct SessionManager {
    sessions: DashMap<SessionKey, Arc<Session>>,
}

impl SessionManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
        })
    }

    /// 按 key 取会话
    pub fn get(&self, key: &SessionKey) -> Option<Arc<Session>> {
        self.sessions.get(key).map(|e| e.value().clone())
    }

    /// 定位会话，不存在则创建（model 为初始模型，None = 无模型；agent_id 为会话状态保存的解析结果）；返回 (会话, 是否新建)
    /// 双重锁定：先 get 快速路径（命中直接返回），未命中再走 entry API 原子创建（并发下仅一个创建成功）
    pub fn get_or_create(&self, key: &SessionKey, model: Option<ProviderModel>, agent_id: Arc<String>) -> (Arc<Session>, bool) {
        if let Some(s) = self.get(key) {
            return (s, false);
        }
        match self.sessions.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(e) => (e.get().clone(), false),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                let session = Arc::new(Session::new(key, model, agent_id));
                e.insert(session.clone());
                (session, true)
            }
        }
    }

    /// 只保留仍在绑定集合中的会话（绑定信息变化后清理无绑定会话）
    pub fn retain(&self, keys: &HashSet<SessionKey>) {
        self.sessions.retain(|k, _| keys.contains(k));
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
    fn get_or_create_with_none_model() {
        let mgr = SessionManager::new();
        let key = SessionKey { agent_name: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let (s, created) = mgr.get_or_create(&key, None, Arc::new("a".into()));
        assert!(created);
        assert!(s.model.load().is_none());
    }

    #[test]
    fn session_copies_role_name_and_mode_from_key() {
        let key = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let model = Some(ProviderModel { provider: "p".into(), model: "m".into() });
        let agent_id = Arc::new("uuid".to_string());
        let session = Session::new(&key, model, agent_id);
        assert_eq!(session.role_name.as_str(), "r1");
        assert_eq!(*session.mode, Mode::Event("e1".into()));
    }
}
