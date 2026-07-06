use std::path::{Path, PathBuf};
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use bytes::Bytes;
use lru::LruCache;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// 附件元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub file_name: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
    pub has_thumbnail: bool,
}

/// 附件存储：本地文件系统
///
/// 文件结构：{base_path}/{group_id}/{uuid}（本体），{uuid}.metadata（元数据）
/// key 格式：{group_id}/{uuid}
/// metadata 缓存：LRU 策略
pub struct AttachmentStore {
    base_path: PathBuf,
    meta_cache: Mutex<LruCache<String, Arc<AttachmentMeta>>>,
}

impl AttachmentStore {
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: PathBuf::from(base_path),
            meta_cache: Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1024).unwrap())),
        }
    }

    /// 解析 key 为 (group_id, uuid)
    fn parse_key(key: &str) -> Result<(&str, &str)> {
        let parts: Vec<&str> = key.splitn(2, '/').collect();
        if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        Ok((parts[0], parts[1]))
    }

    /// 获取 metadata（走 LRU 缓存，缓存未命中时从文件读取）
    pub fn get_meta(&self, key: &str) -> Result<Arc<AttachmentMeta>> {
        let (group_id, uuid) = Self::parse_key(key)?;

        // 检查 LRU 缓存
        {
            let mut cache = self.meta_cache.lock().unwrap();
            if let Some(meta) = cache.get(uuid) {
                return Ok(Arc::clone(meta));
            }
        }

        // 从 metadata 文件读取
        let meta_path = self.base_path.join(group_id).join(format!("{}.metadata", uuid));
        if !meta_path.exists() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let content = std::fs::read_to_string(&meta_path)?;
        let meta: AttachmentMeta = serde_json::from_str(&content)?;
        let meta = Arc::new(meta);

        // 插入 LRU 缓存
        {
            let mut cache = self.meta_cache.lock().unwrap();
            cache.put(uuid.to_string(), Arc::clone(&meta));
        }

        Ok(meta)
    }

    /// 打开附件文件（下载用）
    pub fn open_file(&self, key: &str) -> Result<(std::fs::File, u64)> {
        let (group_id, uuid) = Self::parse_key(key)?;
        let file_path = self.base_path.join(group_id).join(uuid);
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let metadata = std::fs::metadata(&file_path)?;
        let file = std::fs::File::open(&file_path)?;
        Ok((file, metadata.len()))
    }

    /// 获取缩略图数据
    pub fn get_thumbnail(&self, key: &str) -> Result<Bytes> {
        let (group_id, uuid) = Self::parse_key(key)?;
        let dir = self.base_path.join(group_id);
        let thumb_path = dir.join(format!("thumb_{}", uuid));

        // 如果缩略图已存在则直接返回
        if thumb_path.exists() {
            return Ok(Bytes::from(std::fs::read(&thumb_path)?));
        }

        // 按需生成缩略图
        let file_path = dir.join(uuid);
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }

        let mime_type = mime_guess::from_path(&file_path).first_or_octet_stream();
        if mime_type.type_() == mime_guess::mime::IMAGE {
            if let Ok(data) = std::fs::read(&file_path) {
                if let Ok(img) = image::load_from_memory(&data) {
                    let thumb = img.thumbnail(200, 200);
                    if thumb.save(&thumb_path).is_ok() {
                        return Ok(Bytes::from(std::fs::read(&thumb_path)?));
                    }
                }
            }
        }

        Err(Error::InternalError("not an image or failed to generate thumbnail".to_string()))
    }

    // ===== 以下方法为旧版兼容，使用 {group_id}/{msg_id}/{filename} 路径格式 =====

    /// 创建临时文件，返回 (临时文件路径, 目标文件路径)
    pub fn create_temp_file(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<(PathBuf, PathBuf)> {
        let dir = self.base_path.join(group_id).join(msg_id);
        std::fs::create_dir_all(&dir)?;
        let temp_path = dir.join(format!(".{}.uploading", filename));
        let target_path = dir.join(filename);
        // 创建空临时文件
        std::fs::write(&temp_path, &[])?;
        Ok((temp_path, target_path))
    }

    /// 追加 payload 数据到临时文件
    pub fn append_to_temp(&self, temp_path: &Path, data: &Bytes) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(temp_path)?;
        file.write_all(data)?;
        Ok(())
    }

    /// 将临时文件重命名为正式文件
    pub fn finalize_upload(temp_path: &Path, target_path: &Path) -> Result<()> {
        std::fs::rename(temp_path, target_path)?;
        Ok(())
    }

    /// 根据 key（{group_id}/{msg_id}/{filename}）获取元数据
    pub fn get_meta_by_key(&self, key: &str) -> Result<AttachmentMeta> {
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() < 3 {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let group_id = parts[0];
        let msg_id = parts[1];
        let filename = parts[2..].join("/");
        let file_path = self.base_path.join(group_id).join(msg_id).join(&filename);
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let metadata = std::fs::metadata(&file_path)?;
        let mime_type = mime_guess::from_path(&filename).first_or_octet_stream();
        let thumb_path = self.base_path.join(group_id).join(msg_id).join(format!("thumb_{}", filename));

        Ok(AttachmentMeta {
            file_name: Arc::new(filename),
            mime_type: Arc::new(mime_type.to_string()),
            size_bytes: metadata.len(),
            has_thumbnail: thumb_path.exists(),
        })
    }

    /// 根据 key（{group_id}/{msg_id}/{filename}）获取缩略图
    pub fn get_thumbnail_by_key(&self, key: &str) -> Result<Bytes> {
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() < 3 {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let group_id = parts[0];
        let msg_id = parts[1];
        let filename = parts[2..].join("/");
        let dir = self.base_path.join(group_id).join(msg_id);
        let thumb_path = dir.join(format!("thumb_{}", filename));

        // 如果缩略图已存在则直接返回
        if thumb_path.exists() {
            return Ok(Bytes::from(std::fs::read(&thumb_path)?));
        }

        // 延迟生成缩略图
        let file_path = dir.join(&filename);
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }

        let mime_type = mime_guess::from_path(&filename).first_or_octet_stream();
        if mime_type.type_() == mime_guess::mime::IMAGE {
            if let Ok(data) = std::fs::read(&file_path) {
                if let Ok(img) = image::load_from_memory(&data) {
                    let thumb = img.thumbnail(200, 200);
                    if thumb.save(&thumb_path).is_ok() {
                        return Ok(Bytes::from(std::fs::read(&thumb_path)?));
                    }
                }
            }
        }

        Err(Error::InternalError("not an image or failed to generate thumbnail".to_string()))
    }
}
