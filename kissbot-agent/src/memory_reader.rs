use std::sync::Arc;

use serde_json::json;

use crate::types::{Mode, Message, Result, Error, ToolCall};
use crate::config_manager::ConfigManager;

/// 最近读取的最大记录数
const MAX_RECENT_RECORDS: usize = 50;

pub struct MemoryReader {
    client: reqwest::Client,
}

impl MemoryReader {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// 从 memory-struct 读取顶层记忆索引（摘要列表）
    /// 在启动、切换模式、重置时作为初始上下文的一部分
    /// 注意：memory-struct 尚未实现，此方法当前调用无实际效果
    /// agent_id/role_name 为 coordinator 传入的当前运行状态快照
    #[allow(dead_code)]
    pub async fn read_memory_struct_index(
        &self,
        config: &ConfigManager,
        _agent_id: &str,
        _role_name: &str,
        _mode: &Mode,
    ) -> Result<Vec<String>> {
        let structs = config.memory_structs().await;
        if structs.is_empty() {
            return Ok(Vec::new());
        }
        // memory-struct 功能未实现，暂占位
        Ok(Vec::new())
    }

    /// 按当前模式读取最近历史记录
    /// agent_id/role_name 为 coordinator 传入的当前运行状态快照
    pub async fn read_history(
        &self,
        _config: &ConfigManager,
        agent_id: &str,
        role_name: &str,
        mode: &Mode,
    ) -> Result<Vec<Message>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();

        let url = format!("{}/query", store_url.trim_end_matches('/'));

        let body = match mode {
            Mode::Role => {
                json!({
                    "agent_id": agent_id,
                    "role_name": role_name,
                    "limit": MAX_RECENT_RECORDS,
                })
            }
            Mode::Event(event_id) => {
                json!({
                    "agent_id": agent_id,
                    "role_name": format!("{}-{}", role_name, event_id),
                    "limit": MAX_RECENT_RECORDS,
                })
            }
        };

        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;

        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!(
                "记忆读取返回 {}", resp.status()
            )));
        }

        let data: serde_json::Value = resp.json().await?;
        let records = data["records"].as_array()
            .map(|arr| self.records_to_messages(arr))
            .unwrap_or_default();

        Ok(records)
    }

    /// 查询事件列表
    /// agent_id/role_name 为 coordinator 传入的当前运行状态快照
    #[allow(dead_code)]
    pub async fn list_events(
        &self,
        _config: &ConfigManager,
        agent_id: &str,
        role_name: &str,
    ) -> Result<Vec<String>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();

        let url = format!("{}/events", store_url.trim_end_matches('/'));

        let body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
        });

        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("查询事件失败: {}", e)))?;

        let data: serde_json::Value = resp.json().await?;
        let events = data["events"].as_array()
            .map(|arr| {
                arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
            })
            .unwrap_or_default();

        Ok(events)
    }

    fn records_to_messages(&self, records: &[serde_json::Value]) -> Vec<Message> {
        records.iter().filter_map(|r| {
            let msg_type = r["msg_type"].as_str().unwrap_or("");
            let content = Arc::new(extract_record_text(&r["content"]));

            match msg_type {
                "channel" | "text" => Some(Message::User { content }),
                "think" => Some(Message::Assistant { content, reasoning_content: None, tool_calls: None }),
                "tool_call" => {
                    let params = r["tool_params"].as_str()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(serde_json::Value::Null);
                    Some(Message::Assistant {
                        content: Arc::new(String::new()),
                        reasoning_content: None,
                        tool_calls: Some(vec![ToolCall {
                            id: Arc::new(String::new()),
                            name: Arc::new(r["tool_name"].as_str().unwrap_or("").to_string()),
                            arguments: Arc::new(params),
                        }]),
                    })
                }
                "tool_result" => {
                    Some(Message::Tool {
                        tool_call_id: Arc::new(String::new()),
                        name: Arc::new(r["tool_name"].as_str().unwrap_or("").to_string()),
                        content: Arc::new(r["tool_result"].to_string()),
                    })
                }
                _ => None,
            }
        }).collect()
    }
}

/// 从 Content 枚举 JSON（{"Text": "...", "Multi": [...]}）提取文本（与 coordinator.extract_text 同语义）
fn extract_record_text(content: &serde_json::Value) -> String {
    match content.get("Text") {
        Some(t) => t.as_str().unwrap_or("").to_string(),
        None => match content.get("Multi") {
            Some(arr) => arr.as_array().map(|items| items.iter()
                .filter_map(|c| c.get("Text").and_then(|t| t.as_str()).map(String::from))
                .collect::<Vec<_>>().join("\n")).unwrap_or_default(),
            None => String::new(),
        },
    }
}
