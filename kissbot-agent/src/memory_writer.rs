use std::sync::Arc;

use flume::{Sender, Receiver, bounded};
use serde_json::json;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::types::{WriteTask, Result, Error};

const DEFAULT_QUEUE_CAPACITY: usize = 1024;

pub struct MemoryWriter {
    sender: Sender<WriteTask>,
    handle: Arc<tokio::sync::Mutex<Option<JoinHandle<()>>>>,
}

impl MemoryWriter {
    /// 启动 MemoryWriter，创建后台写入任务
    pub fn start() -> Self {
        let (sender, receiver): (Sender<WriteTask>, Receiver<WriteTask>) =
            bounded(DEFAULT_QUEUE_CAPACITY);
        let memory_store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();

        let handle = tokio::spawn(async move {
            Self::run_background(receiver, memory_store_url).await;
        });

        Self {
            sender,
            handle: Arc::new(tokio::sync::Mutex::new(Some(handle))),
        }
    }

    /// 推送写入任务到队列（不阻塞 agentic loop）
    pub fn push(&self, task: WriteTask) -> Result<()> {
        self.sender.try_send(task).map_err(|e| {
            Error::MemoryStoreError(format!("写入队列已满: {}", e))
        })?;
        Ok(())
    }

    /// 后台任务：从队列消费并写入 memory-store
    async fn run_background(receiver: Receiver<WriteTask>, store_url: String) {
        let client = reqwest::Client::new();
        let base_url = store_url.trim_end_matches('/').to_string();

        while let Ok(task) = receiver.recv_async().await {
            let result = match &task {
                WriteTask::Think { agent_id, role_name, content, time } => {
                    let body = json!({
                        "requests": [{
                            "agent_id": agent_id,
                            "role_name": role_name,
                            "content": content,
                            "key": "",
                            "time": time,
                        }],
                        "force": 0,
                    });
                    client.post(&format!("{}/think", base_url))
                        .json(&body).send().await
                }
                WriteTask::ToolCall { agent_id, role_name, tool_name, tool_params, time } => {
                    let body = json!({
                        "requests": [{
                            "agent_id": agent_id,
                            "role_name": role_name,
                            "tool_name": tool_name,
                            "tool_params": tool_params,
                            "key": "",
                            "time": time,
                        }],
                        "force": 0,
                    });
                    client.post(&format!("{}/tool-call", base_url))
                        .json(&body).send().await
                }
                WriteTask::ToolResult { agent_id, role_name, tool_result, time } => {
                    let body = json!({
                        "requests": [{
                            "agent_id": agent_id,
                            "role_name": role_name,
                            "tool_result": tool_result,
                            "key": "",
                            "time": time,
                        }],
                        "force": 0,
                    });
                    client.post(&format!("{}/tool-result", base_url))
                        .json(&body).send().await
                }
            };

            if let Err(e) = result {
                error!("记忆写入失败（不重试）: {:?}", e);
            }
        }

        info!("MemoryWriter 后台任务已退出");
    }
}

impl Drop for MemoryWriter {
    fn drop(&mut self) {
        let handle = self.handle.try_lock()
            .ok()
            .and_then(|mut opt| opt.take());
        if let Some(h) = handle {
            h.abort();
        }
    }
}
