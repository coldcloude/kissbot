// ========== Channel 合批 ==========

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

    /// 清空（会话重置时）
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
}
