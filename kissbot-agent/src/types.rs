pub mod error;
pub mod session_api;
pub mod channel_api;

pub use error::*;
pub use session_api::*;
pub use channel_api::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_hash_eq_by_value() {
        use std::collections::HashSet;
        let a = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let b = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let c = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 SessionKey 应命中 HashSet");
        assert!(!set.contains(&c), "不同 mode 不应命中");
    }

    #[test]
    fn role_mode_encodes_event_only() {
        assert_eq!(role_mode("dev", &Mode::Role), "dev");
        assert_eq!(role_mode("dev", &Mode::Event("e1".into())), "dev-e1");
    }

    #[test]
    fn session_key_encode_matches_context_file_layout() {
        // session_id 编码决定 <data_dir>/context/<session_id>.jsonl 的文件名段
        let role = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        assert_eq!(role.encode(), "a1-r1", "角色模式不带事件段");
        let event = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        assert_eq!(event.encode(), "a1-r1-e1", "事件模式拼事件 id");
    }
}
