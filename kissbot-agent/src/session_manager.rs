use std::collections::HashSet;
use std::sync::Arc;

use dashmap::DashMap;
use kissbot_llm::Session;

use crate::types::SessionKey;

/// 会话管理器：汇总所有绑定 channel 的 (agent_id, role_event) 去重维护会话集合
/// （session_key 仅用于去重；agent_id 直接取 key 的 agent_id 字段）。
/// 会话上下文本体（内存 + 本地缓存 + 历史归档）由 kissbot-llm 的 Session 承担，
/// 本结构只负责「按会话标识去重持有」与「无绑定会话清理」。
pub struct SessionManager {
    sessions: DashMap<SessionKey, Arc<Session>>,
    /// 数据目录（作为会话上下文缓存/历史归档的路径基准，随 session_id 传给 Session）
    data_dir: String,
}

impl SessionManager {
    pub fn new(data_dir: &str) -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
            data_dir: data_dir.to_string(),
        })
    }

    /// 定位会话，不存在则创建（返回 (会话, 是否新建)）；
    /// 新建时完成上下文恢复（recover_from_cache）；pipeline 同步与系统提示词初始化由调用方在拿到会话后执行
    pub async fn get_or_create(&self, key: &SessionKey) -> (Arc<Session>, bool) {
        if let Some(s) = self.sessions.get(key) {
            return (s.clone(), false);
        }
        match self.sessions.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(e) => {
                (e.get().clone(), false)
            },
            dashmap::mapref::entry::Entry::Vacant(e) => {
                // session_id 编码（决定 context/<session_id>.jsonl 文件名）由会话标识提供
                let session = Arc::new(Session::new(&self.data_dir, &key.encode()));
                // 新建会话初始化：从本地缓存恢复上下文（文件不存在视为空上下文）
                let _ = session.recover_from_cache().await;
                // 保存 session
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

    use kissbot_api::RoleEvent;

    fn key(agent: &str, role: &str) -> SessionKey {
        SessionKey { agent_id: Arc::new(agent.into()), role_event: Arc::new(RoleEvent::new(role)) }
    }

    /// 测试用 SessionManager（data_dir 指向临时目录；会话持久化路径仅构造不落盘）
    fn mgr() -> Arc<SessionManager> {
        let dir = tempfile::tempdir().unwrap();
        SessionManager::new(dir.path().to_str().unwrap())
    }

    #[tokio::test]
    async fn get_or_create_dedupes() {
        let mgr = mgr();
        let k = key("a1", "r1");
        // 会话创建只取 key：模型与参数由 (agent, role) + session 配置在消息发送时解析
        let (s1, new1) = mgr.get_or_create(&k).await;
        assert!(new1, "首次创建应报告新建");
        let (s2, new2) = mgr.get_or_create(&k).await;
        assert!(!new2, "已存在不应报告新建");
        assert!(Arc::ptr_eq(&s1, &s2), "同 key 应返回同一 Session");
        // 不同事件是不同会话
        let k_event = SessionKey { agent_id: Arc::new("a1".into()), role_event: Arc::new(RoleEvent::with_event("r1", "e1")) };
        let (s3, _) = mgr.get_or_create(&k_event).await;
        assert!(!Arc::ptr_eq(&s1, &s3), "不同事件应返回不同 Session");
    }

    #[tokio::test]
    async fn retain_keeps_only_bound_sessions() {
        let mgr = mgr();
        let k1 = key("a1", "r1");
        let k2 = key("a2", "r2");
        let (s1, _) = mgr.get_or_create(&k1).await;
        let (s2, _) = mgr.get_or_create(&k2).await;
        // 绑定信息变化后只保留仍在绑定集合中的会话
        mgr.retain(&HashSet::from([k1.clone()]));
        let (again, _) = mgr.get_or_create(&k1).await;
        assert!(Arc::ptr_eq(&s1, &again), "保留的会话应复用");
        let (recreated, _) = mgr.get_or_create(&k2).await;
        assert!(!Arc::ptr_eq(&s2, &recreated), "被清理的会话应重建");
    }
}
