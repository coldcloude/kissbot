// ========== 会话上下文本地缓存 ==========
// 缓存文件：<data_dir>/context/<session_key编码>.jsonl，每行一条 Message（JSON）
// 存储时不截断（tokio::fs 追加）；读取时全量回读（ReverseLineReader 从尾读再反转）

use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use crate::types::{Error, Message, Mode, Result, SessionKey};

/// session_key → 文件名安全编码（十六进制，避免路径/非法字符；agent|role|mode 含 event id）
pub fn encode_session_key(key: &SessionKey) -> String {
    let mode = match &key.mode {
        Mode::Role => "role".to_string(),
        Mode::Event(e) => format!("event:{}", e),
    };
    let raw = format!("{}|{}|{}", key.agent_name, key.role_name, mode);
    raw.as_bytes().iter().map(|b| format!("{:02x}", b)).collect()
}

/// 会话上下文本地缓存：<data_dir>/context/<session_key编码>.jsonl
pub struct ContextCache {
    dir: PathBuf,
}

impl ContextCache {
    pub fn new(data_dir: &str) -> Self {
        Self { dir: PathBuf::from(data_dir).join("context") }
    }

    pub fn path_for(&self, key: &SessionKey) -> PathBuf {
        self.dir.join(format!("{}.jsonl", encode_session_key(key)))
    }

    /// 追加消息（每行一条 Message JSON；不截断）
    pub async fn append(&self, key: &SessionKey, messages: &[Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| Error::IoError(e.to_string()))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true).append(true).open(&path).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        for m in messages {
            let line = serde_json::to_string(m)?;
            file.write_all(line.as_bytes()).await
                .map_err(|e| Error::IoError(e.to_string()))?;
            file.write_all(b"\n").await
                .map_err(|e| Error::IoError(e.to_string()))?;
        }
        Ok(())
    }

    /// 全量回读（按时间顺序）；文件不存在返回空
    pub async fn read_all(&self, key: &SessionKey) -> Result<Vec<Message>> {
        let path = self.path_for(key);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut reader = kai_file::ReverseLineReader::new(&path, None, None).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        let mut msgs = Vec::new();
        while let Some(line) = reader.next_line().await
            .map_err(|e| Error::IoError(e.to_string()))?
        {
            let s = line.line.trim();
            if s.is_empty() { continue; }
            if let Ok(m) = serde_json::from_str::<Message>(s) {
                msgs.push(m);
            }
        }
        msgs.reverse();
        // 崩溃一致性清理：恢复应从完整轮次开始。
        // 1) 若末尾是带 tool_calls 的 assistant（崩溃发生在追加 assistant(tool_calls) 后、
        //    工具响应写入前），丢弃这条悬挂的 assistant——否则恢复后 tool_calls 悬空无 Tool 响应。
        // 2) 丢弃开头的 Tool 消息（恢复起点之前的残留）。
        if let Some(Message::Assistant { tool_calls: Some(_), .. }) = msgs.last() {
            msgs.pop();
        }
        while matches!(msgs.first(), Some(Message::Tool { .. })) {
            msgs.remove(0);
        }
        Ok(msgs)
    }

    /// 清空缓存（重建/重置时调用；文件不存在幂等）
    pub async fn clear(&self, key: &SessionKey) -> Result<()> {
        let path = self.path_for(key);
        match tokio::fs::remove_file(&path).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::IoError(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::types::{Message, Mode, SessionKey};

    fn key() -> SessionKey {
        SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) }
    }

    fn sample_msgs() -> Vec<Message> {
        vec![
            Message::User { content: Arc::new("你好".into()) },
            Message::Assistant { content: Arc::new("在的".into()), reasoning_content: Some(Arc::new("思考".into())), tool_calls: None },
        ]
    }

    #[test]
    fn encode_session_key_distinguishes() {
        let k1 = key();
        let k2 = SessionKey { agent_name: "a1".into(), role_name: "r2".into(), mode: Mode::Role };
        let k3 = SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e2".into()) };
        assert_ne!(encode_session_key(&k1), encode_session_key(&k2), "不同 role 不同编码");
        assert_ne!(encode_session_key(&k1), encode_session_key(&k3), "不同 event 不同编码");
        // 编码不含分隔符原始字符（文件名安全）
        let enc = encode_session_key(&key());
        assert!(!enc.contains('|') && !enc.contains('/'), "编码应文件名安全");
    }

    #[tokio::test]
    async fn append_then_read_all_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContextCache::new(dir.path().to_str().unwrap());
        let k = key();
        assert!(cache.read_all(&k).await.unwrap().is_empty(), "初始为空");
        cache.append(&k, &sample_msgs()).await.unwrap();
        let back = cache.read_all(&k).await.unwrap();
        assert_eq!(back.len(), 2);
        assert!(matches!(&back[0], Message::User { content } if content.as_str() == "你好"));
        assert!(matches!(&back[1], Message::Assistant { reasoning_content: Some(r), .. } if r.as_str() == "思考"), "reasoning_content 应保留");
    }

    #[tokio::test]
    async fn append_twice_accumulates_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContextCache::new(dir.path().to_str().unwrap());
        let k = key();
        cache.append(&k, &sample_msgs()).await.unwrap();
        cache.append(&k, &[Message::User { content: Arc::new("再问".into()) }]).await.unwrap();
        assert_eq!(cache.read_all(&k).await.unwrap().len(), 3, "追加不截断");
        cache.clear(&k).await.unwrap();
        assert!(cache.read_all(&k).await.unwrap().is_empty(), "clear 后为空");
        // 文件不存在时 clear 幂等
        cache.clear(&k).await.unwrap();
    }

    #[tokio::test]
    async fn read_all_sanitizes_dangling_tool_turn() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContextCache::new(dir.path().to_str().unwrap());
        // 用例 A：完整轮次 user → assistant(tool_calls) → tool，末尾再追一条悬挂 assistant(tool_calls)
        // （崩溃发生在追加 assistant(tool_calls) 后、Tool 响应写入前）→ 回读丢弃悬挂尾巴，保留完整轮次
        let k_a = key();
        cache.append(&k_a, &[
            Message::User { content: Arc::new("查一下".into()) },
            Message::Assistant { content: Arc::new(String::new()), reasoning_content: None, tool_calls: Some(vec![]) },
            Message::Tool { tool_call_id: Arc::new("c1".into()), name: Arc::new("read".into()), content: Arc::new("内容".into()) },
        ]).await.unwrap();
        cache.append(&k_a, &[Message::Assistant { content: Arc::new(String::new()), reasoning_content: None, tool_calls: Some(vec![]) }]).await.unwrap();
        let back = cache.read_all(&k_a).await.unwrap();
        assert_eq!(back.len(), 3, "悬挂的 assistant(tool_calls) 应被丢弃，保留完整轮次");
        assert!(matches!(&back[1], Message::Assistant { tool_calls: Some(_), .. }), "完整轮次的 assistant(tool_calls) 保留");
        assert!(matches!(&back[2], Message::Tool { .. }), "tool 响应保留");

        // 用例 B：仅一条悬挂 assistant(tool_calls) → 回读为空
        let k_b = SessionKey { agent_name: "a1".into(), role_name: "r2".into(), mode: Mode::Role };
        cache.append(&k_b, &[Message::Assistant { content: Arc::new(String::new()), reasoning_content: None, tool_calls: Some(vec![]) }]).await.unwrap();
        assert!(cache.read_all(&k_b).await.unwrap().is_empty(), "仅悬挂 assistant 时回读为空");

        // 用例 C：开头的 Tool 残留（恢复起点之前的半条轮次）被丢弃
        let k_c = SessionKey { agent_name: "a1".into(), role_name: "r3".into(), mode: Mode::Role };
        cache.append(&k_c, &[
            Message::Tool { tool_call_id: Arc::new("c9".into()), name: Arc::new("read".into()), content: Arc::new("残留".into()) },
            Message::User { content: Arc::new("继续".into()) },
        ]).await.unwrap();
        let back_c = cache.read_all(&k_c).await.unwrap();
        assert_eq!(back_c.len(), 1, "开头 Tool 残留被丢弃，保留后续完整消息");
        assert!(matches!(&back_c[0], Message::User { content } if content.as_str() == "继续"));
    }
}
