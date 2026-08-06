// ========== Channel 合批 ==========

use crate::session_manager::Session;
use std::sync::Arc;
use std::time::Duration;

/// 每会话待合批缓冲：消息先入缓冲，超时（channel_batch_interval_secs）无新消息才打包为一条 user 消息
#[derive(Default)]
pub struct BatchBuffer {
    items: Vec<(String, String)>,  // (user_name, 文本)
}

impl BatchBuffer {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn push(&mut self, name: &str, text: &str) {
        self.items.push((name.to_string(), text.to_string()));
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 取出全部并清空（打包用）
    pub fn take(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.items)
    }

    /// 清空缓冲（保留 API：当前重置流程不清空——重置期间消息统一并入重置后打包，见 flush_after_reset）
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// 打包为一条 user 消息的 content：逐行 "name: text"（name 为空只留 text）
pub fn pack_batch(items: &[(String, String)]) -> String {
    items.iter().map(|(name, text)| {
        if name.is_empty() { text.clone() } else { format!("{}: {}", name, text) }
    }).collect::<Vec<_>>().join("\n")
}

/// 延时打包：等待 interval 后，若会话正在重置则继续等待（重置期间不触发超时），
/// 重置完成后立即打包一次；期间到达的消息统一合并（缓冲不清空）。缓冲为空返回 None
pub async fn flush_after_reset(session: &Arc<Session>, interval: Duration) -> Option<String> {
    tokio::time::sleep(interval).await;
    // 重置期间等待（轮询，重置通常毫秒级）
    while session.resetting.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut b = session.batch.lock().await;
    if b.is_empty() {
        return None;
    }
    let items = b.take();
    drop(b);
    Some(pack_batch(&items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_batch_formats_name_content_lines() {
        let items = vec![
            ("u1".to_string(), "你好".to_string()),
            ("u2".to_string(), "在吗".to_string()),
            (String::new(), "无名字".to_string()),
        ];
        assert_eq!(pack_batch(&items), "u1: 你好\nu2: 在吗\n无名字");
    }

    #[test]
    fn batch_buffer_push_take_clear() {
        let mut b = BatchBuffer::new();
        assert!(b.is_empty());
        b.push("u1", "a");
        b.push("u2", "b");
        assert!(!b.is_empty());
        let items = b.take();
        assert_eq!(items.len(), 2);
        assert!(b.is_empty(), "take 后清空");
        b.push("u1", "c");
        b.clear();
        assert!(b.is_empty());
        assert!(b.take().is_empty());
    }

    #[tokio::test]
    async fn flush_after_reset_waits_then_packs() {
        use crate::session_manager::Session;
        use crate::types::{Mode, SessionKey};
        let key = SessionKey { agent_name: "a".into(), role_name: "r".into(), mode: Mode::Role };
        let session = Arc::new(Session::new(&key, None, Arc::new("aid".into())));
        session.batch.lock().await.push("u1", "你好");
        // 重置期间：resetting=true，flush 不应打包
        session.resetting.store(true, std::sync::atomic::Ordering::SeqCst);
        let session2 = session.clone();
        let task = tokio::spawn(async move {
            flush_after_reset(&session2, Duration::from_millis(20)).await
        });
        // 重置期间（20ms interval 已过）：缓冲仍保留消息（未被打包）
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!session.batch.lock().await.is_empty(), "重置期间不应打包，缓冲应保留消息");
        // 重置期间到达的新消息并入缓冲
        session.batch.lock().await.push("u2", "在吗");
        // 重置完成：置 false，flush 立即打包（统一合并）
        session.resetting.store(false, std::sync::atomic::Ordering::SeqCst);
        let content = task.await.unwrap().expect("应打包");
        assert_eq!(content, "u1: 你好\nu2: 在吗", "重置期间消息统一合并");
        assert!(session.batch.lock().await.is_empty(), "打包后缓冲清空");
    }
}
