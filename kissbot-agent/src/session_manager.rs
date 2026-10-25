use std::collections::HashSet;
use std::sync::Arc;

use arcstr::ArcStr;
use dashmap::{DashMap, Entry};
use kissbot_llm::Session;

/// 会话存储：按 session_id 去重持有会话。
///
/// session 的数量与 session_id 由 pipeline 决定：pipeline 需要会话时用自己生成的 id 调
/// `get_or_create`，本结构只负责创建/恢复/保存与按 ids 清理。
/// 会话上下文本体（内存 + 本地缓存 + 历史归档）由 kissbot-llm 的 Session 承担。
pub struct SessionManager {
    sessions: DashMap<ArcStr, Arc<Session>>,
    /// 数据目录（作为会话上下文缓存/历史归档的路径基准，随 session_id 传给 Session）
    data_dir: ArcStr,
}

impl SessionManager {
    pub fn new(data_dir: &str) -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
            data_dir: ArcStr::from(data_dir),
        })
    }

    /// 按 session_id 定位会话，不存在则创建（返回 (会话, 是否新建)）；
    /// 新建时完成上下文恢复（recover_from_cache）
    pub async fn get_or_create(&self, session_id: ArcStr) -> (Arc<Session>, bool) {
        if let Some(s) = self.sessions.get(&session_id) {
            return (s.clone(), false);
        }
        match self.sessions.entry(session_id.clone()) {
            Entry::Occupied(e) => {
                (e.get().clone(), false)
            },
            Entry::Vacant(e) => {
                let session = Arc::new(Session::new(self.data_dir.as_str(), session_id));
                // 新建会话初始化：从本地缓存恢复上下文（文件不存在视为空上下文）
                let _ = session.recover_from_cache().await;
                e.insert(session.clone());
                (session, true)
            }
        }
    }

    /// 只保留给定 session_id 集合中的会话（pipeline 清理时按各 pipeline 报告的在用 id 调用）
    pub fn retain(&self, session_ids: &HashSet<ArcStr>) {
        self.sessions.retain(|k, _| session_ids.contains(k));
    }

    /// 单测用：会话是否仍在存储中（生产代码不依赖该查询）
    #[cfg(test)]
    pub fn contains(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 SessionManager（data_dir 指向临时目录；会话持久化路径仅构造不落盘）
    fn mgr() -> (tempfile::TempDir, Arc<SessionManager>) {
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_str().unwrap());
        (dir, mgr)
    }

    #[tokio::test]
    async fn get_or_create_dedupes_by_session_id() {
        let (_dir, mgr) = mgr();
        let (s1, new1) = mgr.get_or_create(ArcStr::from("a1-r1")).await;
        assert!(new1, "首次创建应报告新建");
        let (s2, new2) = mgr.get_or_create(ArcStr::from("a1-r1")).await;
        assert!(!new2, "已存在不应报告新建");
        assert!(Arc::ptr_eq(&s1, &s2), "同 session_id 应返回同一 Session");
        let (s3, new3) = mgr.get_or_create(ArcStr::from("a1-r1-e1")).await;
        assert!(new3);
        assert!(!Arc::ptr_eq(&s1, &s3), "不同 session_id 应返回不同 Session");
    }

    #[tokio::test]
    async fn retain_keeps_only_given_session_ids() {
        let (_dir, mgr) = mgr();
        let (s1, _) = mgr.get_or_create(ArcStr::from("a1-r1")).await;
        let (s2, _) = mgr.get_or_create(ArcStr::from("a2-r2")).await;
        mgr.retain(&HashSet::from([ArcStr::from("a1-r1")]));
        assert!(mgr.contains("a1-r1"));
        assert!(!mgr.contains("a2-r2"));
        let (again, new) = mgr.get_or_create(ArcStr::from("a1-r1")).await;
        assert!(!new);
        assert!(Arc::ptr_eq(&s1, &again), "保留的会话应复用");
        let (recreated, new) = mgr.get_or_create(ArcStr::from("a2-r2")).await;
        assert!(new, "被清理的会话应重建");
        assert!(!Arc::ptr_eq(&s2, &recreated));
    }
}
