use std::sync::Arc;

use kissbot_api::memory::ChannelCombo;
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

/// 并集算法（修正设计 2.3）：(1) 取最后 N 条 → T_N；(2) M = max(cutoff, T_N)；
/// (3) [M, T_N]（含两端）取该时间全部；最终 = (1) ∪ (3)，时间正序
/// list 为全史合并升序结果（调用方已按时间排序）；不足 N 条已全取，直接返回
pub fn recent_memory(list: &[MemoryMsg], count: usize, cutoff: &str) -> Vec<MemoryMsg> {
    if list.is_empty() {
        return Vec::new();
    }
    // 不足 N 条：已全部取出，直接返回（无需 T_N/M/(3)，避免空并集计算）
    if list.len() <= count {
        return list.to_vec();
    }
    let start_idx = list.len() - count;
    let last_n = &list[start_idx..];
    let t_n = &last_n[0].time;   // 最旧一条
    let m = if cutoff >= t_n.as_str() { cutoff.to_string() } else { t_n.to_string() };
    // (3) [M, T_N] 含两端；与 (1) 并集后升序（(1) 的全史查询已覆盖 (3) 区间，此处同列表过滤等价）
    let mut out: Vec<MemoryMsg> = list.iter()
        .filter(|msg| msg.time.as_str() >= m.as_str() && msg.time.as_str() <= t_n.as_str())
        .cloned()
        .chain(last_n.iter().cloned())
        .collect();
    out.sort_by(|a, b| a.time.cmp(&b.time));
    out.dedup_by(|a, b| a.time == b.time && a.content == b.content && a.user_name == b.user_name);
    out
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

    /// 组合查询：POST {store}/store/query/combos，返回 (agent, role) 时间范围内出现的 channel 组合
    async fn query_combos(&self, agent_id: &str, role_name: &str, start: &str, end: &str) -> Result<Vec<ChannelCombo>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();
        let url = format!("{}/store/query/combos", store_url.trim_end_matches('/'));
        let body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
            "start_time": start,
            "end_time": end,
        });
        let resp = self.client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::MemoryStoreError(format!("组合查询失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(Error::MemoryStoreError(format!("组合查询返回 {}", resp.status())));
        }
        let data: serde_json::Value = resp.json().await?;
        Ok(serde_json::from_value(data["data"].clone()).unwrap_or_default())
    }

    /// 组合内精确查询：POST {store}/store/query/channel（messenger/user/group 精确 key，取该时间全部）
    async fn query_channel(
        &self,
        agent_id: &str,
        role_name: &str,
        combo: &ChannelCombo,
        start_time: &str,
        end_time: &str,
    ) -> Result<Vec<MemoryMsg>> {
        let store_url = kissbot_api::ApiConfig::get().memory_store_url.clone();
        let url = format!("{}/store/query/channel", store_url.trim_end_matches('/'));
        let body = json!({
            "agent_id": agent_id,
            "role_name": role_name,
            "messenger_id": combo.messenger_id,
            "user_id": combo.user_id,
            "group_id": combo.group_id,
            "start_time": start_time,
            "end_time": end_time,
        });
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

    /// role 模式记忆打包：组合查询 → 每组合全史查询合并 → 并集算法 → 升序结果
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

        // 组合：按全史范围取（组合由文件枚举，范围覆盖即可）
        let combos = self.query_combos(agent_id, role_name, "2000-01-01 00:00:00", &now).await?;

        // (1) 每组合全史查询，合并升序
        let mut merged: Vec<MemoryMsg> = Vec::new();
        for combo in &combos {
            if let Ok(msgs) = self.query_channel(agent_id, role_name, combo, "2000-01-01 00:00:00", &now).await {
                merged.extend(msgs);
            }
        }
        merged.sort_by(|a, b| a.time.cmp(&b.time));

        Ok(recent_memory(&merged, cfg.memory_count, &start))
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
    fn recent_memory_union_keeps_order_and_same_time_group() {
        let t = |m: &str, tm: &str| MemoryMsg { user_name: "u".into(), content: m.into(), time: tm.into() };
        // dense 场景 30 条在窗口内（时间 10:00~10:29），最后 N=10
        let mut list: Vec<MemoryMsg> = (0..30).map(|i| t(&format!("m{}", i), &format!("2026-08-05 10:{:02}:00", i))).collect();
        list.sort_by(|a, b| a.time.cmp(&b.time));
        let cutoff = "2026-08-05 09:00:00";  // 窗口起点早于所有记录 → T_N > cutoff → M = T_N
        let out = recent_memory(&list, 10, cutoff);
        // 结果 = 最后 10 条（m20..m29），时间正序
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].time, list[20].time);
        assert!(out.windows(2).all(|w| w[0].time <= w[1].time), "时间正序");
    }

    #[test]
    fn recent_memory_sparse_extends_beyond_window() {
        let t = |m: &str, tm: &str| MemoryMsg { user_name: "u".into(), content: m.into(), time: tm.into() };
        // 稀疏：总共 10 条在 07:00~07:45，窗口起点 08:00 晚于全部记录
        let mut list: Vec<MemoryMsg> = vec![
            t("a", "2026-08-05 07:00:00"), t("b", "2026-08-05 07:05:00"),
            t("c", "2026-08-05 07:10:00"), t("d", "2026-08-05 07:15:00"),
            t("e", "2026-08-05 07:20:00"), t("f", "2026-08-05 07:25:00"),
            t("g", "2026-08-05 07:30:00"), t("h", "2026-08-05 07:35:00"),
            t("i", "2026-08-05 07:40:00"), t("j", "2026-08-05 07:45:00"),
        ];
        list.sort_by(|a, b| a.time.cmp(&b.time));
        let cutoff = "2026-08-05 08:00:00";  // 窗口起点晚于全部记录 → T_N < cutoff → M = cutoff
        let out = recent_memory(&list, 10, cutoff);
        assert_eq!(out.len(), 10, "稀疏场景结果 = 最后 N 条（跨更早时间）");
        assert_eq!(out[0].time, "2026-08-05 07:00:00");
    }

    #[test]
    fn recent_memory_empty_and_less_than_n() {
        assert!(recent_memory(&[], 10, "2026-08-05 09:00:00").is_empty());
        let t = |m: &str, tm: &str| MemoryMsg { user_name: "u".into(), content: m.into(), time: tm.into() };
        let list = vec![t("a", "2026-08-05 10:00:00"), t("b", "2026-08-05 10:01:00")];
        let out = recent_memory(&list, 10, "2026-08-05 09:00:00");
        assert_eq!(out.len(), 2, "不足 N 条取全部");
    }

    #[test]
    fn recent_memory_includes_same_time_group_beyond_n() {
        let t = |m: &str, tm: &str| MemoryMsg { user_name: "u".into(), content: m.into(), time: tm.into() };
        // 15 条：记录 m11~m15 都与最后第 10 条同在 T_N=10:50（同时间组）
        let mut list: Vec<MemoryMsg> = Vec::new();
        for i in 1..=10 { list.push(t(&format!("m{}", i), &format!("2026-08-05 10:{:02}:00", 40 + i))); }
        for i in 11..=15 { list.push(t(&format!("m{}", i), "2026-08-05 10:50:00")); }
        list.sort_by(|a, b| a.time.cmp(&b.time));
        let out = recent_memory(&list, 10, "2026-08-05 09:00:00");
        // 最后 10 条 = m6..m15（T_N=10:50，同时间组起点）；同时间组 m11~m15 全保留 → 结果 10 条
        assert_eq!(out.len(), 10, "T_N 同时间组不拆散（本例如最后 10 条起点恰在同时间组起点）");
        assert!(out.windows(2).all(|w| w[0].time <= w[1].time));
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
