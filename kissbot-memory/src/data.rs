use std::path::PathBuf;
use std::sync::Arc;

use arcstr::ArcStr;
use async_trait::async_trait;
use kai_date::{as_date, as_year, get_date_time_segments};
use kai_file::index::{FilePathGenerator, QueryParser};
use crate::DirectoryManager;

use kissbot_api::memory::*;


/// 拼记忆后缀：角色模式为角色名，事件模式为 `角色名-事件标识`（只在构造文件/目录名时拼接）。
pub fn role_event_suffix(role_event: &RoleEvent) -> ArcStr {
    match &role_event.event_id {
        Some(event_id) => ArcStr::from(format!("{}-{}", role_event.role_name, event_id)),
        None => role_event.role_name.clone(),
    }
}

//===================== functions ======================

async fn ensure_year_role_dir(agent_id: &str, suffix: &str, date: &str) -> std::result::Result<PathBuf,kai_file::Error> {
    let year = as_year(date);
    match DirectoryManager::get().ensure_agent_store_dir(agent_id).await {
        Ok(store_dir) => {
            let year_role_dir = store_dir.join(format!("{}-{}", year, suffix));
            
            if !year_role_dir.exists() {
                tokio::fs::create_dir_all(&year_role_dir).await?;
            }
            
            Ok(year_role_dir)
        }
        Err(e) => {
            return Err(kai_file::Error::ExternalError(Box::new(e)));
        }
    }
}

pub struct ChannelParser;

#[async_trait]
impl FilePathGenerator<Arc<RecordKey>> for ChannelParser {
    async fn get_path(&self, key: &Arc<RecordKey>) -> std::result::Result<PathBuf,kai_file::Error> {
        let year_role_dir = ensure_year_role_dir(key.agent_id.as_str(), &role_event_suffix(&key.role_event), key.date.as_str()).await?;
        let file_name = format!("channel-records-{}.jsonl", key.date);
        Ok(year_role_dir.join(file_name))
    }
}

impl ChannelParser {
    /// 写入请求 → (记录文件 key, 通道记录)
    pub fn parse_request(&self, request: ChannelRequest) -> (Arc<RecordKey>, ChannelRecord) {
        // 所有 channel 记录归入同一文件（每 agent+role+date）；
        // record 保存完整身份：user_id=发送者、self_user_id=agent 绑定用户（接收方）、messenger_id/group_id
        let key = Arc::new(RecordKey {
            agent_id: request.agent_id.clone(),
            role_event: request.role_event.clone(),
            date: ArcStr::from(as_date(&request.time)),
        });
        let record = ChannelRecord {
            user_id: request.user_id,
            self_user_id: request.self_user_id,
            messenger_id: request.messenger_id,
            group_id: request.group_id,
            is_self: request.is_self,
            msg_id: request.msg_id,
            messenger_name: request.messenger_name,
            user_name: request.user_name,
            group_name: request.group_name,
            content: request.content,
            time: request.time,
            sn: 0,
            meta: request.meta,
        };
        (key, record)
    }
}

impl QueryParser<QueryRequest, Arc<RecordKey>> for ChannelParser {
    fn parse_query(&self, query: QueryRequest) -> Vec<(Arc<RecordKey>, (String, String))> {
        let mut results = Vec::new();
        if let Ok(date_times) = get_date_time_segments(&query.start_time, &query.end_time) {
            for time in date_times {
                results.push((Arc::new(RecordKey {
                    agent_id: query.agent_id.clone(),
                    role_event: query.role_event.clone(),
                    date: ArcStr::from(as_date(&time.0)),
                }), time));
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kissbot_api::{Content, ContentBlock, ThinkRecord, ToolCallRecord, ToolResultRecord};

use super::*;

    #[test]
    fn role_event_suffix_concatenates_role_and_event() {
        assert_eq!(role_event_suffix(&RoleEvent::new(ArcStr::from("dev"))), "dev");
        assert_eq!(role_event_suffix(&RoleEvent::with_event(ArcStr::from("dev"), ArcStr::from("e1"))), "dev-e1");
        // 事件标识本身可以含 '-'（UUID）
        assert_eq!(role_event_suffix(&RoleEvent::with_event(ArcStr::from("dev"), ArcStr::from("3f2a-c3d4"))), "dev-3f2a-c3d4");
        assert_eq!(role_event_suffix(&RoleEvent::new(ArcStr::from(""))), "");
    }

    // ========== FilePathGenerator ==========

    use std::sync::{Once, OnceLock};

    static TEST_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static INIT_CONFIG: Once = Once::new();

    /// 初始化全局 Config（临时目录），供依赖文件系统路径的测试使用（与 index.rs 测试同款模式）
    fn init_test_config() {
        let dir = TEST_DIR.get_or_init(|| tempfile::tempdir().unwrap());
        let config_path = dir.path().join("config.json");
        let root_dir_str = dir.path().display().to_string();
        // SAFETY: 单线程测试初始化，仅执行一次
        INIT_CONFIG.call_once(|| {
            std::fs::write(&config_path, format!(r#"{{"memory":{{"root_dir":"{}"}}}}"#, root_dir_str)).unwrap();
            unsafe { std::env::set_var("KISSBOT_CONFIG", config_path.to_str().unwrap()); }
            crate::Config::get();
        });
    }

    #[tokio::test]
    async fn test_channel_file_name() {
        init_test_config();
        let key = Arc::new(RecordKey {
            agent_id: ArcStr::from("agent1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
            date: ArcStr::from("2026-06-24"),
        });
        let parser = ChannelParser;
        let path = parser.get_path(&key).await.unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(file_name, "channel-records-2026-06-24.jsonl");
    }

    #[tokio::test]
    async fn test_channel_file_dir_empty_role() {
        // 空记忆后缀目录应为 `2026-`（非 `2026`）
        init_test_config();
        let key = Arc::new(RecordKey {
            agent_id: ArcStr::from("agent1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from(""))),
            date: ArcStr::from("2026-06-24"),
        });
        let parser = ChannelParser;
        let path = parser.get_path(&key).await.unwrap();
        let dir_name = path.parent().unwrap().file_name().unwrap().to_str().unwrap();
        assert_eq!(dir_name, "2026-");
    }

    // ========== ChannelParser::parse_request ==========

    fn detail_content() -> Content {
        // 思考/工具内容直接内嵌记录，随通道记录一起写入
        Content::Multi(Arc::new(vec![
            Content::Think(Arc::new(ThinkRecord {
                reasoning_content: Some(ArcStr::from("推理")),
                thinking: None,
            })),
            Content::ToolCall(Arc::new(ToolCallRecord {
                tool_name: ArcStr::from("search"),
                tool_params: Arc::new(serde_json::json!({"q": "weather"})),
            })),
            Content::ToolResult(Arc::new(ToolResultRecord {
                tool_result: Some(Arc::new(vec![ContentBlock::text(ArcStr::from(r#"{"temp":25}"#))])),
            })),
            Content::Text(ArcStr::from("回复")),
        ]))
    }

    #[test]
    fn test_channel_request_parser() {
        let request = ChannelRequest {
            meta: None,
            agent_id: ArcStr::from("agent1"),
            role_event: Arc::new(RoleEvent::new(ArcStr::from("default"))),
            messenger_id: ArcStr::from("telegram"),
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            is_self: 1,
            msg_id: ArcStr::from("m-1001"),
            messenger_name: ArcStr::from("TelegramName"),
            user_name: ArcStr::from("U1Name"),
            group_name: ArcStr::from("G1Name"),
            content: detail_content(),
            time: ArcStr::from("2026-06-24 10:00:00"),
        };
        let parser = ChannelParser;
        let (key, record) = parser.parse_request(request);
        // key 为公共 RecordKey（无 messenger/user/group 字段）
        assert_eq!(key.agent_id, "agent1");
        assert_eq!(key.role_event.as_ref(), &RoleEvent::new(ArcStr::from("default")));
        assert_eq!(key.date, "2026-06-24");
        // record 保存完整身份：user_id=发送者、self_user_id=绑定用户、messenger_id/group_id
        assert_eq!(record.user_id, "u1");
        assert_eq!(record.self_user_id, "self1");
        assert_eq!(record.messenger_id, "telegram");
        assert_eq!(record.group_id, "g1");
        assert_eq!(record.is_self, 1);
        assert_eq!(record.msg_id, "m-1001", "消息标识原样带入记录（消息与记录去重用）");
        // 思考/工具内容随通道记录原样保留，不解析成独立记录
        let Content::Multi(items) = &record.content else { panic!("应为 Multi 内容") };
        assert_eq!(items.len(), 4);
        assert!(matches!(&items[0], Content::Think(r) if r.reasoning_content.as_deref() == Some("推理")));
        assert!(matches!(&items[1], Content::ToolCall(r) if r.tool_name.as_str() == "search"));
        assert!(matches!(&items[2], Content::ToolResult(r) if r.tool_result.is_some()));
        assert!(matches!(&items[3], Content::Text(v) if v.as_str() == "回复"));
        assert_eq!(record.sn, 0);
    }
}
