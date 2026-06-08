use std::path::PathBuf;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// 附件元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub att_id: Arc<String>,
    pub filename: Arc<String>,
    pub mime_type: Arc<String>,
    pub size_bytes: u64,
    pub has_thumbnail: bool,
}

/// 附件存储：本地文件系统
pub struct AttachmentStore {
    base_path: PathBuf,
}

impl AttachmentStore {
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: PathBuf::from(base_path),
        }
    }

    /// 保存附件，返回附件索引 key
    pub fn save_attachment(
        &self,
        group_id: &str,
        msg_id: &str,
        filename: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<AttachmentMeta> {
        let dir = self.base_path.join(group_id).join(msg_id);
        std::fs::create_dir_all(&dir)?;

        let file_path = dir.join(filename);
        std::fs::write(&file_path, data)?;

        let is_image = mime_type.starts_with("image/");
        let has_thumbnail = if is_image {
            // 生成缩略图
            let thumb_path = dir.join(format!("thumb_{}", filename));
            if let Ok(img) = image::load_from_memory(data) {
                let thumb = img.thumbnail(200, 200);
                thumb.save(&thumb_path)?;
                true
            } else {
                false
            }
        } else {
            false
        };

        Ok(AttachmentMeta {
            att_id: Arc::new(format!("{}/{}/{}", group_id, msg_id, filename)),
            filename: Arc::new(filename.to_string()),
            mime_type: Arc::new(mime_type.to_string()),
            size_bytes: data.len() as u64,
            has_thumbnail,
        })
    }

    /// 获取附件数据
    pub fn get_attachment_data(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<Vec<u8>> {
        let file_path = self.base_path.join(group_id).join(msg_id).join(filename);
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(format!("{}/{}/{}", group_id, msg_id, filename)));
        }
        Ok(std::fs::read(&file_path)?)
    }

    /// 获取缩略图数据
    pub fn get_thumbnail(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<Vec<u8>> {
        let thumb_path = self.base_path.join(group_id).join(msg_id).join(format!("thumb_{}", filename));
        if !thumb_path.exists() {
            return Err(Error::AttachmentNotFound(format!("thumb_{}/{}/{}", group_id, msg_id, filename)));
        }
        Ok(std::fs::read(&thumb_path)?)
    }

    /// 根据 key 解析路径并获取附件
    /// key 格式: "{group_id}/{msg_id}/{filename}"
    pub fn get_attachment_by_key(&self, key: &str) -> Result<Vec<u8>> {
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() < 3 {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let group_id = parts[0];
        let msg_id = parts[1];
        let filename = parts[2..].join("/");
        self.get_attachment_data(group_id, msg_id, &filename)
    }

    /// 根据 key 获取缩略图
    pub fn get_thumbnail_by_key(&self, key: &str) -> Result<Vec<u8>> {
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() < 3 {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let group_id = parts[0];
        let msg_id = parts[1];
        let filename = parts[2..].join("/");
        self.get_thumbnail(group_id, msg_id, &filename)
    }

    /// 解析附件 key 获取元数据
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
            att_id: Arc::new(key.to_string()),
            filename: Arc::new(filename),
            mime_type: Arc::new(mime_type.to_string()),
            size_bytes: metadata.len(),
            has_thumbnail: thumb_path.exists(),
        })
    }
}
