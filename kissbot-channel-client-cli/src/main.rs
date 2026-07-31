use std::sync::{Arc, Weak};
use async_trait::async_trait;
use bytes::Bytes;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel_client::{ChannelClient, Error, Result};
use kissbot_channel_client::Terminal;
use serde::Deserialize;
use tokio::sync::RwLock;

#[derive(Deserialize)]
struct CliConfig {
    channel_ws_url: String,
}

const UPLOAD_CHUNK_SIZE: usize = 64 * 1024;

struct CliTerminal {
    messenger_id: String,
    user_id: String,
    current_group: RwLock<Arc<String>>,
    download_dir: String,
    client: RwLock<Option<Arc<ChannelClient>>>,
}

impl CliTerminal {
    async fn current_group(&self) -> Arc<String> {
        self.current_group.read().await.clone()
    }

    async fn get_client(&self) -> Result<Arc<ChannelClient>> {
        self.client.read().await.as_ref().cloned()
            .ok_or_else(|| Error::InternalError("client not connected".to_string()))
    }

    async fn bind(&self) -> Result<()> {
        let client = self.get_client().await?;
        client.bind(BindRequest {
            messenger_id: Arc::new(self.messenger_id.clone()),
            user_id: Arc::new(self.user_id.clone()),
        }).await
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        let client = self.get_client().await?;
        let response = client.send_message(OutgoingMessage {
            messenger_id: Arc::new(self.messenger_id.clone()),
            user_id: Arc::new(self.user_id.clone()),
            group_id: self.current_group().await,
            content: Content::Text(Arc::new(text.to_string())),
        }).await?;
        println!(">> sent msg_id={}", response.msg_id);
        Ok(())
    }

    async fn download(&self, key: &str) -> Result<()> {
        let client = self.get_client().await?;
        let info = client.request_download(AttachmentDownloadRequest {
            messenger_id: Arc::new(self.messenger_id.clone()),
            user_id: Arc::new(self.user_id.clone()),
            group_id: self.current_group().await,
            key: Arc::new(key.to_string()),
        }).await?;
        // 重新下载时先删除旧文件，避免 append 叠加
        let path = format!("{}/{}", self.download_dir, info.info.file_name);
        let _ = std::fs::remove_file(&path);
        println!(">> downloading {} ({} bytes)", info.info.file_name, info.info.size_bytes);
        Ok(())
    }

    async fn upload(&self, path: &str) -> Result<()> {
        let data = std::fs::read(path)?;
        let file_name = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "upload.bin".to_string());
        let client = self.get_client().await?;
        let response = client.send_message(OutgoingMessage {
            messenger_id: Arc::new(self.messenger_id.clone()),
            user_id: Arc::new(self.user_id.clone()),
            group_id: self.current_group().await,
            content: Content::AttachmentInfo(Arc::new(AttachmentInfo {
                file_name: Arc::new(file_name.clone()),
                mime_type: Arc::new("application/octet-stream".to_string()),
                size_bytes: data.len() as u64,
            })),
        }).await?;
        // 响应 content 中取 transfer_id 用于上传
        let Content::AttachmentInfoResponse(att) = &response.content else {
            println!("!! unexpected response content, upload aborted");
            return Ok(());
        };
        let mut pos = 0u64;
        while pos < data.len() as u64 {
            let end = (pos as usize + UPLOAD_CHUNK_SIZE).min(data.len());
            let chunk = Bytes::copy_from_slice(&data[pos as usize..end]);
            let resp = client.send_upload_chunk(att.transfer_id, pos, chunk).await?;
            if resp.error_code != PAYLOAD_ERRCODE_OK {
                println!("!! upload chunk error: {:?}", resp.error_msg);
                return Ok(());
            }
            pos = end as u64;
        }
        println!(">> uploaded {} key={}", file_name, att.key);
        Ok(())
    }
}

#[async_trait]
impl Terminal for CliTerminal {
    async fn incoming_message(&self, _id: &str, message: Arc<IncomingMessage>) {
        // 打印 content 原始 JSON 串
        let json = serde_json::to_string(&message.content).unwrap();
        println!("<< [{}:{}] {}", message.user_id, message.group_id, json);
    }

    async fn join_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) {
        println!("<< join group: {} @ {}", notification.group_id, notification.messenger_id);
    }

    async fn leave_group(&self, _id: &str, notification: Arc<GroupChangeNotification>) {
        println!("<< leave group: {} @ {}", notification.group_id, notification.messenger_id);
    }

    async fn user_removed(&self, _id: &str, notification: Arc<UserRemoveNotification>) {
        println!("<< user removed: {} @ {}", notification.user_id, notification.messenger_id);
    }

    async fn download_chunk(&self, _id: &str, info: Arc<AttachmentInfoResponse>, pos: u64, data: Bytes) -> Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.download_dir)?;
        let path = format!("{}/{}", self.download_dir, info.info.file_name);
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(&data)?;
        if pos + data.len() as u64 >= info.info.size_bytes {
            println!(">> downloaded to {}", path);
        }
        Ok(())
    }

    async fn closed(&self, _id: &str) {
        println!("!! connection closed");
        std::process::exit(0);
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: {} <messenger_id> <user_id> <group_id> [download_dir]", args[0]);
        std::process::exit(1);
    }
    let messenger_id = args[1].clone();
    let user_id = args[2].clone();
    let group_id = args[3].clone();
    let download_dir = args.get(4).cloned().unwrap_or_else(|| "./downloads".to_string());

    let config: CliConfig = kissbot_config::Config::get().get_section("channel-client");
    let api_key = kissbot_security::SecurityConfig::get().api_key.clone();

    let cli_terminal = Arc::new(CliTerminal {
        messenger_id: messenger_id.clone(),
        user_id: user_id.clone(),
        current_group: RwLock::new(Arc::new(group_id.clone())),
        download_dir,
        client: RwLock::new(None),
    });

    let client = ChannelClient::new("cli".to_string(), Arc::downgrade(&cli_terminal) as Weak<dyn Terminal>);
    *cli_terminal.client.write().await = Some(client.clone());
    client.connect(&config.channel_ws_url, &api_key).await.expect("connect failed");
    cli_terminal.bind().await.expect("bind failed");
    println!(">> bound. 输入行发送文本；/group <id> 切换群组；/download <key>；/upload <path>；/send <text> 透传发送");

    // stdin 按行读取（独立线程，避免阻塞 tokio runtime）
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => if tx.send(line).is_err() { break; },
                Err(_) => break,
            }
        }
    });

    while let Ok(line) = rx.recv() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("/group ") {
            *cli_terminal.current_group.write().await = Arc::new(rest.trim().to_string());
            println!(">> current group: {}", rest.trim());
        } else if let Some(rest) = line.strip_prefix("/download ") {
            if let Err(e) = cli_terminal.download(rest.trim()).await {
                println!("!! download error: {}", e);
            }
        } else if let Some(rest) = line.strip_prefix("/upload ") {
            if let Err(e) = cli_terminal.upload(rest.trim()).await {
                println!("!! upload error: {}", e);
            }
        } else if let Some(rest) = line.strip_prefix("/send ") {
            // 透传命令：把剩余内容原样作为文本发送（用于发送 / 开头的管理命令文本）
            if let Err(e) = cli_terminal.send_text(rest.trim()).await {
                println!("!! send error: {}", e);
            }
        } else if line.starts_with('/') {
            println!("!! unknown command: {}", line);
        } else {
            if let Err(e) = cli_terminal.send_text(line).await {
                println!("!! send error: {}", e);
            }
        }
    }
}
