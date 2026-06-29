use std::path::{Path, PathBuf};
use std::io::{Read, Write};
use std::sync::Arc;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// 附件元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub att_id: Arc<String>,
    pub file_name: Arc<String>,
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
        data: Bytes,
        mime_type: &str,
    ) -> Result<AttachmentMeta> {
        let dir = self.base_path.join(group_id).join(msg_id);
        std::fs::create_dir_all(&dir)?;

        let file_path = dir.join(filename);
        std::fs::write(&file_path, &data)?;

        let is_image = mime_type.starts_with("image/");
        let has_thumbnail = if is_image {
            // 生成缩略图
            let thumb_path = dir.join(format!("thumb_{}", filename));
            if let Ok(img) = image::load_from_memory(&data) {
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
            file_name: Arc::new(filename.to_string()),
            mime_type: Arc::new(mime_type.to_string()),
            size_bytes: data.len() as u64,
            has_thumbnail,
        })
    }

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

    /// 获取附件数据
    pub fn get_attachment_data(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<Bytes> {
        let file_path = self.base_path.join(group_id).join(msg_id).join(filename);
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(format!("{}/{}/{}", group_id, msg_id, filename)));
        }
        Ok(Bytes::from(std::fs::read(&file_path)?))
    }

    /// 获取缩略图数据，如果缩略图不存在则按需生成
    pub fn get_thumbnail(&self, group_id: &str, msg_id: &str, filename: &str) -> Result<Bytes> {
        let dir = self.base_path.join(group_id).join(msg_id);
        let thumb_path = dir.join(format!("thumb_{}", filename));

        // 如果缩略图已存在则直接返回
        if thumb_path.exists() {
            return Ok(Bytes::from(std::fs::read(&thumb_path)?));
        }

        // 延迟生成缩略图
        let file_path = dir.join(filename);
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(format!("{}/{}/{}", group_id, msg_id, filename)));
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

    /// 根据 key 解析路径并获取附件
    /// key 格式: "{group_id}/{msg_id}/{filename}"
    pub fn get_attachment_by_key(&self, key: &str) -> Result<Bytes> {
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
    pub fn get_thumbnail_by_key(&self, key: &str) -> Result<Bytes> {
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
            file_name: Arc::new(filename),
            mime_type: Arc::new(mime_type.to_string()),
            size_bytes: metadata.len(),
            has_thumbnail: thumb_path.exists(),
        })
    }

    /// 根据 key 打开附件文件，返回 (File, 文件长度)
    pub fn open_file(&self, key: &str) -> Result<(std::fs::File, u64)> {
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() < 3 {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let file_path = self.base_path.join(parts[0]).join(parts[1]).join(parts[2..].join("/"));
        if !file_path.exists() {
            return Err(Error::AttachmentNotFound(key.to_string()));
        }
        let metadata = std::fs::metadata(&file_path)?;
        let file = std::fs::File::open(&file_path)?;
        Ok((file, metadata.len()))
    }
}
