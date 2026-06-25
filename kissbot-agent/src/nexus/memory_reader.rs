use serde_json::json;

use crate::nexus::types::{Mode, ContextMessage, Result, Error};
use crate::nexus::config_manager::ConfigManager;

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
    #[allow(dead_code)]
    pub async fn read_memory_struct_index(
        &self,
        config: &ConfigManager,
        _mode: &Mode,
    ) -> Result<Vec<String>> {
        let struct_url = config.memory_struct_url().await;
        if struct_url.is_empty() {
            return Ok(Vec::new()); // memory-struct 未配置，忽略
        }

        let agent_id = config.agent_id().await;
        let role_name = config.current_role().await;

        let url = format!("{}/index", struct_url.trim_end_matches('/'));

        let body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
        });

        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆索引失败: {}", e)))?;

        if !resp.status().is_success() {
            return Ok(Vec::new()); // memory-struct 未就绪时静默忽略
        }

        let data: serde_json::Value = resp.json().await?;
        let items = data["items"].as_array()
            .map(|arr| arr.iter().filter_map(|v| v["summary"].as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        Ok(items)
    }

    /// 按当前模式读取最近历史记录
    pub async fn read_history(
        &self,
        config: &ConfigManager,
        mode: &Mode,
    ) -> Result<Vec<ContextMessage>> {
        let agent_id = config.agent_id().await;
        let role_name = config.current_role().await;
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
                    "role_name": format!("{}:{}", role_name, event_id),
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
    #[allow(dead_code)]
    pub async fn list_events(
        &self,
        config: &ConfigManager,
    ) -> Result<Vec<String>> {
        let agent_id = config.agent_id().await;
        let role_name = config.current_role().await;
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

    fn records_to_messages(&self, records: &[serde_json::Value]) -> Vec<ContextMessage> {
        records.iter().filter_map(|r| {
            let msg_type = r["msg_type"].as_str().unwrap_or("");
            let content = r["content"].as_str().unwrap_or("").to_string();
            let time = r["time"].as_str().unwrap_or("").to_string();

            match msg_type {
                "channel" | "text" => Some(ContextMessage::User {
                    messenger_id: r["messenger_id"].as_str().unwrap_or("").to_string(),
                    user_id: r["user_id"].as_str().unwrap_or("").to_string(),
                    group_id: r["group_id"].as_str().unwrap_or("").to_string(),
                    content,
                    time,
                }),
                "think" => Some(ContextMessage::Assistant { content, time }),
                "tool_call" => {
                    let params = r["tool_params"].as_str()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(serde_json::Value::Null);
                    Some(ContextMessage::ToolCall {
                        tool_name: r["tool_name"].as_str().unwrap_or("").to_string(),
                        parameters: params,
                        time,
                    })
                }
                "tool_result" => {
                    let result = r["tool_result"].clone();
                    Some(ContextMessage::ToolResult {
                        tool_name: r["tool_name"].as_str().unwrap_or("").to_string(),
                        result,
                        time,
                    })
                }
                _ => None,
            }
        }).collect()
    }
}
