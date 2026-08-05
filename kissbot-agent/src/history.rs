// ========== 历史上下文归档 ==========
// 归档 = 直接复制当前缓存文件到历史目录并加上时间戳文件名（无包装格式），本轮只写不读

use std::path::{Path, PathBuf};

use chrono::Local;

use crate::context_cache::encode_session_key;
use crate::types::{Error, Result, SessionKey};

/// 历史上下文归档：<data_dir>/context-history/<session_key编码>-<时间戳>.jsonl
/// 归档 = 直接复制当前缓存文件（无包装格式），本轮只写不读
pub struct HistoryArchive {
    dir: PathBuf,
}

impl HistoryArchive {
    pub fn new(data_dir: &str) -> Self {
        Self { dir: PathBuf::from(data_dir).join("context-history") }
    }

    /// 复制缓存文件到历史目录（文件名带时间戳）；返回目标路径
    pub async fn archive(&self, key: &SessionKey, source: &Path) -> Result<PathBuf> {
        if !source.exists() {
            return Err(Error::IoError(format!("缓存文件不存在: {}", source.display())));
        }
        tokio::fs::create_dir_all(&self.dir).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        let ts = Local::now().format("%Y-%m-%d-%H%M%S").to_string();
        let dest = self.dir.join(format!("{}-{}.jsonl", encode_session_key(key), ts));
        tokio::fs::copy(source, &dest).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        Ok(dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::context_cache::{ContextCache, encode_session_key};
    use crate::types::{Message, Mode, SessionKey};

    fn key() -> SessionKey {
        SessionKey { agent_name: "a1".into(), role_name: "r1".into(), mode: Mode::Role }
    }

    #[tokio::test]
    async fn archive_copies_cache_file_with_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContextCache::new(dir.path().to_str().unwrap());
        let history = HistoryArchive::new(dir.path().to_str().unwrap());
        let k = key();
        cache.append(&k, &[Message::User { content: Arc::new("你好".into()) }]).await.unwrap();
        let source = cache.path_for(&k);
        let dest = history.archive(&k, &source).await.unwrap();
        // 目标文件名 = <key编码>-<时间戳>.jsonl
        let fname = dest.file_name().unwrap().to_str().unwrap().to_string();
        assert!(fname.starts_with(&encode_session_key(&k)), "文件名以 key 编码开头: {}", fname);
        assert!(fname.ends_with(".jsonl"));
        // 内容与源一致
        assert_eq!(tokio::fs::read_to_string(&dest).await.unwrap(),
                   tokio::fs::read_to_string(&source).await.unwrap());
        // 原文件保留（压缩后仍要重写）
        assert!(source.exists());
    }

    #[tokio::test]
    async fn archive_missing_source_errors() {
        let dir = tempfile::tempdir().unwrap();
        let history = HistoryArchive::new(dir.path().to_str().unwrap());
        let missing = dir.path().join("nonexistent.jsonl");
        assert!(history.archive(&key(), &missing).await.is_err(), "源不存在应报错");
    }
}
