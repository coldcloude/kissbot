pub mod error;
pub mod session_api;
pub mod channel_api;

pub use error::*;
pub use session_api::*;
pub use channel_api::*;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arcstr::ArcStr;
    use kissbot_api::RoleEvent;

    use super::*;

    fn key(agent_id: &str, role: &str, event: Option<&str>) -> SessionKey {
        SessionKey {
            agent_id: ArcStr::from(agent_id),
            role_event: Arc::new(match event {
                Some(event) => RoleEvent::with_event(role, event),
                None => RoleEvent::new(role),
            }),
        }
    }

    #[test]
    fn session_key_hash_eq_by_value() {
        use std::collections::HashSet;
        let a = key("a1", "r1", None);
        let b = key("a1", "r1", None);
        let c = key("a1", "r1", Some("e1"));
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 SessionKey 应命中 HashSet");
        assert!(!set.contains(&c), "不同事件不应命中");
    }

    #[test]
    fn arc_session_key_works_as_set_key() {
        // 内部字段用 ArcStr / Arc<RoleEvent>，结构体实现 PartialEq + Hash，
        // 因此 Arc<SessionKey> 可直接作 Set / Map 键
        use std::collections::HashSet;
        let a = Arc::new(key("a1", "r1", None));
        let b = Arc::new(key("a1", "r1", None));
        let c = Arc::new(key("a1", "r1", Some("e1")));
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b), "等值 Arc<SessionKey> 应命中 HashSet");
        assert!(!set.contains(&c), "不同事件不应命中");
    }

    #[test]
    fn session_key_encode_matches_context_file_layout() {
        // session_id 编码决定 <data_dir>/context/<session_id>.jsonl 的文件名段
        assert_eq!(key("a1", "r1", None).encode(), "a1-r1", "角色模式不带事件段");
        assert_eq!(key("a1", "r1", Some("e1")).encode(), "a1-r1-e1", "事件模式拼事件 id");
    }
}
