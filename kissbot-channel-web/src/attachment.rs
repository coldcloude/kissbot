use std::path::{Path, PathBuf};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use kissbot_api::message::{AttachmentInfo, AttachmentInfoResponse};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::error::{Error, Result};

/// 上传队列命令
pub struct UploadCommand {
    size: u32,
	pos: u64,
    data: Bytes,
    res: oneshot::Sender<Result<u64>>,
}

type UploadChannel = flume::Sender<UploadCommand>;

/// 附件元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub key: Arc<String>,
    pub info: Arc<AttachmentInfo>,
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
    /// 上传队列：transfer_id → UploadChannel
    upload_channels: DashMap<u32, UploadChannel>,
    /// transfer_id → key 映射（上传和下载通用）
    transfer_key_map: DashMap<u32, Arc<String>>,
    transfer_id_seq: AtomicU32,
}

impl AttachmentStore {
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: PathBuf::from(base_path),
            meta_cache: Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1024).unwrap())),
            upload_channels: DashMap::new(),
            transfer_key_map: DashMap::new(),
            transfer_id_seq: AtomicU32::new(0),
        }
    }

    pub(crate) fn next_transfer_id(&self) -> u32 {
        self.transfer_id_seq.fetch_add(1, Ordering::SeqCst)
    }

    /// 生成 transfer_id 并存储 transfer_id → key 映射
    pub fn next_transfer_id_for(&self, key: Arc<String>) -> u32 {
        let id = self.next_transfer_id();
        self.transfer_key_map.insert(id, key);
        id
    }

    /// 解析 key 为 (group_id, uuid)
    pub(crate) fn parse_key(key: &str) -> Result<(&str, &str)> {
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

    // ===== 上传队列 =====

    /// 异步写入 chunk（通过 flume 队列串行处理）
    pub async fn write_chunk(&self, transfer_id: u32, pos: u64, size: u32, data: Bytes) -> Result<u64> {
        let sender = self.upload_channels.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?;
        let (res_tx, res_rx) = oneshot::channel();

        sender.send(UploadCommand {
            size,
            pos,
            data,
            res: res_tx,
        })?;

        res_rx.await?
    }

    /// 内部处理上传写入（同步执行，在 flume 异步任务中调用）
    fn process_upload_write_inner(
        base_path: &Path,
        key: &str,
        current_pos: u64,
        size: u32,
        pos: u64,
        data: &Bytes,
    ) -> Result<u64> {
        if pos < current_pos {
            return Ok(current_pos); // 已写入，幂等
        }
        if pos > current_pos {
            return Err(Error::AttachmentPositionOutOfOrder(key.to_string(), current_pos, pos));
        }
        let clamp_data = if data.len() != size as usize {
            data.get(0..size as usize)
            .ok_or_else(|| Error::InvalidMessage(format!("data length = {}, size = {}",data.len(),size).to_string()))?
        }
        else {
            data
        };
        let clamp_size = clamp_data.len() as u64;

        let (group_id, uuid) = AttachmentStore::parse_key(key)?;
        let dir = base_path.join(group_id);
        let temp_path = dir.join(format!(".{}.uploading", uuid));

        // 追加写入临时文件
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&temp_path)?;
        file.write_all(clamp_data)?;

        let result_pos = current_pos + clamp_size;

        Ok(result_pos)
    }

    // ===== 下载 =====

    /// 发送下载 payload（内部按 CHUNK_SIZE 分块读取并调用 sender）
    pub async fn send_download_payload(&self, transfer_id: u32, sender: &dyn kissbot_channel::AttachmentDownloadPayloadSender) -> Result<()> {
        use kissbot_api::channel::OFFSET_ATT_DATA;
        const CHUNK_SIZE: u64 = 65536;

        let key = self.transfer_key_map.get(&transfer_id)
            .ok_or_else(|| Error::AttachmentNotFound(transfer_id.to_string()))?
            .clone();
        let (group_id, uuid) = Self::parse_key(key.as_str())?;
        let file_path = self.base_path.join(group_id).join(uuid);
        let file_len = std::fs::metadata(&file_path)?.len();
        let mut file = std::fs::File::open(&file_path)?;

        let mut pos = 0u64;
        let mut ok = true;
        while pos < file_len && ok {
            let end = std::cmp::min(pos + CHUNK_SIZE, file_len);
            let chunk_size = (end - pos) as usize;
            let (sn, mut buf) = sender.prepare_send(transfer_id, chunk_size as u32, pos)?;
            // 读取到 payload 偏移处
            if let Err(e) = (&mut file).read_exact(&mut buf[OFFSET_ATT_DATA..OFFSET_ATT_DATA + chunk_size]) {
                return Err(Error::InternalError(format!("Failed to read file chunk: {}", e)));
            }
            ok = sender.send(sn, transfer_id, chunk_size as u32, pos, buf).await.is_ok();
            pos = end;
        }
        // 下载完成，清理 transfer_key_map
        self.transfer_key_map.remove(&transfer_id);

        Ok(())
    }

    /// 根据 key 和范围读取附件数据
    /// 内部 parse_key → open file → seek → read_exact
    pub fn read_attachment_range(&self, key: &str, start: u64, length: u64) -> Result<Bytes> {
        let (group_id, uuid) = Self::parse_key(key)?;
        let file_path = self.base_path.join(group_id).join(uuid);
        let mut file = std::fs::File::open(&file_path)?;
        file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; length as usize];
        file.read_exact(&mut buf)?;
        Ok(Bytes::from(buf))
    }

}

// ===== AttachmentRegistry 实现 =====

#[async_trait]
impl kissbot_channel::AttachmentRegistry for AttachmentStore {
    async fn register(&self, _messenger_id: &str, _user_id: &str, group_id: &str, info: Arc<AttachmentInfo>) -> std::result::Result<Arc<AttachmentInfoResponse>, kissbot_channel::Error> {
        let uuid = Arc::new(uuid::Uuid::new_v4().to_string());
        let key = Arc::new(format!("{}/{}", group_id, uuid));
        let transfer_id = self.next_transfer_id();

        // 创建 group 目录
        let dir = self.base_path.join(group_id);
        std::fs::create_dir_all(&dir)?;

        // 创建空临时文件
        let temp_path = dir.join(format!(".{}.uploading", uuid));
        std::fs::write(&temp_path, &[])?;

        // 写 metadata 文件
        let meta = Arc::new(AttachmentMeta {
            key: key.clone(),
            info: info.clone(),
            has_thumbnail: false,
        });
        let meta_path = dir.join(format!("{}.metadata", uuid));
        let meta_json = serde_json::to_string(meta.as_ref())?;
        std::fs::write(&meta_path, &meta_json)?;

        // 插入 LRU 缓存
        {
            let mut cache = self.meta_cache.lock().unwrap();
            cache.put(key.as_str().to_string(), meta.clone());
        }

        // 创建上传队列并注册到 upload_channels
        let (tx, rx) = flume::unbounded::<UploadCommand>();
        self.upload_channels.insert(transfer_id, tx);

        let channels = self.upload_channels.clone();
        let base_path = self.base_path.clone();
        let uuid_ch = uuid.clone();
        tokio::spawn(async move {
            // 保存上传进度
            let mut current_pos = 0u64;
            // 从上传队列中读取数据
            while let Ok(cmd) = rx.recv_async().await {
                // 写入 chunk
                let result = Self::process_upload_write_inner(
                    &base_path, meta.key.as_str(), current_pos, cmd.size, cmd.pos, &cmd.data,
                );
                
                // 更新写入结果
                match result.as_ref() {
                    Ok(pos) => current_pos = *pos,
                    Err(e) => {
                        match e {
                            Error::AttachmentPositionOutOfOrder(..) => {},
                            _ => {
                                break;
                            }
                        }
                    }
                }

                // 回复 chunk 写入
                let _ = cmd.res.send(result);

                // 从 metadata 获取 size_bytes 判断是否完成
                if current_pos >= meta.info.size_bytes {
                    break;
                }
            }

            // 结束上传流程
            channels.remove(&transfer_id);

            // 从 metadata 获取 size_bytes 判断是否完成
            if current_pos >= meta.info.size_bytes {
                // 写入完成，rename
                let target_path = dir.join(uuid_ch.as_str());
                std::fs::rename(&temp_path, &target_path)?;
                // 如果是图片则生成缩略图
                if meta.info.mime_type.starts_with("image/") {
                    if let Ok(data) = std::fs::read(&target_path) {
                        if let Ok(img) = image::load_from_memory(&data) {
                            let thumb_path = dir.join(format!("thumb_{}", uuid));
                            let thumb = img.thumbnail(200, 200);
                            if thumb.save(&thumb_path).is_ok() {
                                // 更新 metadata 中的 has_thumbnail
                                let updated_meta = AttachmentMeta {
                                    key: meta.key.clone(),
                                    info: meta.info.clone(),
                                    has_thumbnail: true,
                                };
                                let _ = std::fs::write(&meta_path, serde_json::to_string(&updated_meta).unwrap());
                            }
                        }
                    }
                }
            }

            let result: Result<()> = Ok(());
            result
        });

        Ok(Arc::new(AttachmentInfoResponse {
            key,
            info,
            transfer_id,
        }))
    }
}
