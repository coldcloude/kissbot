use std::sync::Arc;

use serde_json::json;

use crate::config_manager::ConfigManager;
use crate::context_config::EffectiveContextConfig;
use crate::types::{Message, Mode, Result, Error};

/// 记忆消息（channel record 的最小视图：name + content，id 类不保留）
#[derive(Debug, Clone)]
pub struct MemoryMsg {
    pub user_name: String,
    pub content: String,
    pub time: String,
}

/// 两查询比较决策：窗口首条更早 → true（窗口更大，用窗口）；recent 首条更早 → false（用 recent）
/// None 视为最晚（空集合：另一侧胜出）；两侧都空返回 true（外部处理为空）
pub fn window_wins(window_first: Option<&str>, recent_first: Option<&str>) -> bool {
    match (window_first, recent_first) {
        (Some(w), Some(r)) => w <= r,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => true,
    }
}

/// 打包记忆消息为一条 user 消息（content 逐行 "name: text"，name 为空只留 text）；空返回 None
pub fn pack_memory_messages(msgs: &[MemoryMsg]) -> Option<Message> {
    if msgs.is_empty() {
        return None;
    }
    let content = msgs.iter().map(|m| {
        if m.user_name.is_empty() { m.content.clone() } else { format!("{}: {}", m.user_name, m.content) }
    }).collect::<Vec<_>>().join("\n");
    Some(Message::User { content: Arc::new(content) })
}

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

    /// 时间窗查询（messenger/user/group 空 = 目录聚合）；返回升序记录
    async fn query_channel_range(
        &self,
        agent_id: &str,
        role_name: &str,
        start_time: &str,
        end_time: &str,
    ) -> Result<Vec<MemoryMsg>> {
        self.query_channel(agent_id, role_name, start_time, end_time, None).await
    }

    /// 最近 N 条（跨全史）
    async fn query_channel_recent(
        &self,
        agent_id: &str,
        role_name: &str,
        limit: usize,
    ) -> Result<Vec<MemoryMsg>> {
        self.query_channel(agent_id, role_name, "2000-01-01 00:00:00", "2099-12-31 23:59:59", Some(limit)).await
    }

    /// 查询 channel 记录（POST {store}/store/query/channel）；messenger/user/group 空串 = 目录聚合
    async fn query_channel(
        &self,
        agent_id: &str,
        role_name: &str,
        start_time: &str,
        end_time: &str,
        limit: Option<usize>,
    ) -> Result<Vec<MemoryMsg>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();
        let url = format!("{}/store/query/channel", store_url.trim_end_matches('/'));
        let mut body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
            "messenger_id": "",
            "user_id": "",
            "group_id": "",
            "start_time": start_time,
            "end_time": end_time,
        });
        if let Some(l) = limit {
            body["limit"] = json!(l);
        }
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("读取记忆失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("记忆读取返回 {}", resp.status())));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(self.parse_channel_records(&data["data"]))
    }

    /// 解析 ApiResponse.data：Vec<(key, Vec<(sn, ChannelRecord)>)>，返回按 time 升序的记录
    fn parse_channel_records(&self, data: &serde_json::Value) -> Vec<MemoryMsg> {
        let mut out = Vec::new();
        if let Some(groups) = data.as_array() {
            for group in groups {
                if let Some(records) = group[1].as_array() {
                    for entry in records {
                        let rec = &entry[1];
                        let time = rec["time"].as_str().unwrap_or("").to_string();
                        let user_name = rec["user_name"].as_str().unwrap_or("").to_string();
                        let content = extract_record_text(&rec["content"]);
                        if content.is_empty() {
                            continue;  // 非文本记录跳过
                        }
                        out.push(MemoryMsg { user_name, content, time });
                    }
                }
            }
        }
        out.sort_by(|a, b| a.time.cmp(&b.time));
        out
    }

    /// 两查询（时间窗 + 最近 N）比较首条时间取更早者，返回升序结果（role 模式记忆打包数据源）
    pub async fn read_recent_for_context(
        &self,
        agent_id: &str,
        role_name: &str,
        cfg: &EffectiveContextConfig,
    ) -> Result<Vec<MemoryMsg>> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let start = chrono::Local::now()
            .checked_sub_signed(chrono::Duration::seconds(cfg.memory_time_secs as i64))
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "2000-01-01 00:00:00".to_string());

        let window = self.query_channel_range(agent_id, role_name, &start, &now).await?;
        let recent = self.query_channel_recent(agent_id, role_name, cfg.memory_count).await?;

        let window_first = window.first().map(|m| m.time.as_str());
        let recent_first = recent.first().map(|m| m.time.as_str());
        Ok(if window_wins(window_first, recent_first) { window } else { recent })
    }
}

/// 从 Content 枚举 JSON（{"msg_type":"Text","data":...} / {"msg_type":"Multi","data":[...]}）提取文本
/// 注意：Content 用 #[serde(tag="msg_type", content="data")] 序列化，不是 {"Text":...} 形式
fn extract_record_text(content: &serde_json::Value) -> String {
    match content["msg_type"].as_str() {
        Some("Text") => content["data"].as_str().unwrap_or("").to_string(),
        Some("Multi") => content["data"].as_array().map(|items| items.iter()
            .filter_map(|c| {
                if c["msg_type"] == "Text" {
                    c["data"].as_str().map(String::from)
                } else { None }
            })
            .collect::<Vec<_>>().join("\n")).unwrap_or_default(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;

    #[test]
    fn window_wins_compares_first_record_time() {
        // 窗口首条更早 → true（窗口更大，用窗口）
        assert!(window_wins(Some("2026-08-05 09:00:00"), Some("2026-08-05 10:00:00")));
        // recent 首条更早 → false（recent 更大，用 recent）
        assert!(!window_wins(Some("2026-08-05 10:00:00"), Some("2026-08-05 09:00:00")));
        // 相等 → 窗口
        assert!(window_wins(Some("2026-08-05 10:00:00"), Some("2026-08-05 10:00:00")));
        // 一侧为空：窗口空 → 用 recent；recent 空 → 用窗口；都空 → 窗口（外部处理为空）
        assert!(!window_wins(None, Some("2026-08-05 10:00:00")));
        assert!(window_wins(Some("2026-08-05 10:00:00"), None));
        assert!(window_wins(None, None));
    }

    #[test]
    fn pack_memory_messages_builds_user_message() {
        let msgs = vec![
            MemoryMsg { user_name: "u1".into(), content: "你好".into(), time: "t1".into() },
            MemoryMsg { user_name: String::new(), content: "无名字".into(), time: "t2".into() },
        ];
        let m = pack_memory_messages(&msgs).expect("非空应打包");
        assert!(matches!(&m, Message::User { content } if content.as_str() == "u1: 你好\n无名字"));
    }

    #[test]
    fn pack_memory_messages_empty_returns_none() {
        assert!(pack_memory_messages(&[]).is_none());
    }
}
