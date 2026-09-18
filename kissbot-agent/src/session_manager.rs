use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Local;
use dashmap::DashMap;
use tokio::{io::AsyncWriteExt, sync::Mutex};

use crate::types::{Error, Message, Mode, Result, SessionKey};

/// 会话上下文：内存消息 + 本地缓存 + 历史归档一体管理（持久化由 SessionContext 自身负责，Nexus 不感知）
///
/// ========== 会话上下文本地缓存 ==========
/// 缓存文件：<data_dir>/context/<session_key编码>.jsonl，每行一条 Message（JSON）
/// 首行 System（如有当前系统消息），其后为消息行
/// 存储时不截断（tokio::fs 追加）；读取时全量回读（LineReader 正序）
///
/// ========== 历史上下文归档 ==========
/// 归档与清空缓存永远配对（archive_and_clear_cache）：把当前内存（含当前系统消息，System 首行）
/// 写成一个历史文件并加上时间戳文件名（无包装格式，不复制缓存文件），然后清空缓存文件，本轮只写不读
/// 历史与当前缓存格式完全一致（都是 <session_key编码>.jsonl 每行一条 Message），
/// 因此内存、缓存、历史三者统一在本结构体实现（会话上下文唯一入口）。
pub struct SessionContext {
    messages: Vec<Message>,
    /// 当前系统消息（模型上下文首条；含从缓存恢复的，直到下次发送前对比替换）
    system_message: Option<String>,
    /// 待定系统消息（set 只写这里，多次 set 只保留最近一次；下次发送前对比应用）
    pending_system: Option<String>,
    /// 数据目录：缓存/历史路径用时按 data_dir + key 编码构造（不冗余存路径）
    data_dir: PathBuf,
    /// session_key 文件名编码（缓存文件与历史文件的 key 编码段）
    key_enc: String,
}

impl SessionContext {
    pub fn new(data_dir: &str, key: &SessionKey) -> Self {
        // session_key → 文件名：{agent_id}-{role_name}（角色模式）或 {agent_id}-{role_name}-{event}（事件模式）
        let key_enc = match &key.mode {
            Mode::Role => format!("{}-{}", key.agent_id, key.role_name),
            Mode::Event(e) => format!("{}-{}-{}", key.agent_id, key.role_name, e),
        };
        Self {
            messages: Vec::new(),
            system_message: None,
            pending_system: None,
            data_dir: PathBuf::from(data_dir),
            key_enc,
        }
    }

    /// 当前缓存文件路径（<data_dir>/context/<session_key编码>.jsonl）
    fn cache_path(&self) -> PathBuf {
        self.data_dir.join("context").join(format!("{}.jsonl", self.key_enc))
    }

    /// 历史归档目录（<data_dir>/context-history）
    fn history_dir(&self) -> PathBuf {
        self.data_dir.join("context-history")
    }

    /// 设置系统消息（会话创建/重置时，配置或 ego 生成后调用）：只保存待定，
    /// 当前系统消息保留到下次发送前对比应用（多次 set 只保留最近一次）
    pub fn set_system_message(&mut self, content: String) {
        self.pending_system = Some(content);
    }

    /// 发送前应用待定系统消息（下次发送时调用）：无待定直接返回；
    /// 与当前一致 → 仅清空待定；不一致 → 归档并清空当前上下文 →
    /// 应用新系统消息 → 从内存写回缓存（System + 消息行）
    pub async fn apply_pending_system(&mut self) -> Result<()> {
        let Some(pending) = self.pending_system.take() else { return Ok(()); };
        if self.system_message.as_deref() == Some(pending.as_str()) {
            return Ok(());   // 一致：无需变更（set 消息已清空）
        }
        self.archive_and_clear_cache_and_reset_messages(None).await?;
        self.system_message = Some(pending);
        // 从内存写回缓存（只写消息行；System 首行由 open_cache_and_write_system_line 对新文件落，避免 System 重复）
        // 消息源为自身内存：直接借用 self.messages；无消息不写
        if self.messages.is_empty() {
            return Ok(());
        }
        let mut file = self.open_cache_and_write_system_line().await?;
        write_cache_lines(&mut file, &self.messages).await
    }

    /// 追加消息（内存 + 缓存一体，每行一条 Message JSON；不截断）
    /// best-effort：缓存失败仅丢缓存不阻塞流程（内存已装入，调用方按 Result 决定）
    pub async fn append(&mut self, mut messages: Vec<Message>) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        // 缓存追加（新文件先落 System 首行；无消息不写）
        let mut file = self.open_cache_and_write_system_line().await?;
        write_cache_lines(&mut file, &messages).await?;
        // 内存追加
        for message in messages.drain(..) {
            self.messages.push(message);
        }
        Ok(())
    }

    /// 从缓存恢复上下文：正序全量回读（LineReader 按时间顺序）装入内存；
    /// 首行 System（如有）放当前系统消息；文件不存在清空
    pub async fn recover_from_cache(&mut self) -> Result<()> {
        let path = self.cache_path();
        if !path.exists() {
            self.messages = Vec::new();
            self.system_message = None;
            return Ok(());
        }
        let mut reader = kai_file::LineReader::new(&path, None, None).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        let mut msgs = Vec::new();
        while let Some(line) = reader.next_line().await
            .map_err(|e| Error::IoError(e.to_string()))?
        {
            let s = line.line.trim();
            if s.is_empty() { continue; }
            if let Ok(m) = serde_json::from_str::<Message>(s) {
                msgs.push(m);
            }
        }
        // 首行 System → 当前系统消息（缓存格式：System 行在最前，其后为消息行）
        if let Some(Message::System { content }) = msgs.first() {
            self.system_message = Some(content.as_str().to_string());
            msgs.remove(0);
        }
        // 崩溃一致性清理：恢复应从完整轮次开始。
        // 1) 若末尾是带 tool_calls 的 assistant（崩溃发生在追加 assistant(tool_calls) 后、
        //    工具响应写入前），丢弃这条悬挂的 assistant——否则恢复后 tool_calls 悬空无 Tool 响应。
        // 2) 丢弃开头的 Tool 消息（恢复起点之前的残留）。
        if let Some(Message::Assistant { tool_calls: tcs, .. }) = msgs.last() {
            if !tcs.is_null() {
                msgs.pop();
            }
        }
        while matches!(msgs.first(), Some(Message::Tool { .. })) {
            msgs.remove(0);
        }
        self.messages = msgs;
        Ok(())
    }

    /// 归档当前上下文到历史并清空缓存文件（归档与清空永远配对；重置/重建/系统切换前的持久化清理）：
    /// 1) 把当前内存（含当前系统消息，System 首行）写成一个历史文件（<key编码>-<时间戳>.jsonl，无包装格式），
    ///    不复制缓存文件；内存为空则无可归档内容（仅有系统消息不归档，幂等）
    /// 2) 清空缓存文件（文件不存在幂等）
    /// 3) 重置上下文（可选）：清空内存（system 保留）→ messages 写入内存、缓存
    ///
    /// &mut self 强制排他：调用方须持独占引用（经 session.context 互斥锁），避免并发归档/清空交错
    pub async fn archive_and_clear_cache_and_reset_messages(&mut self, messages: Option<Vec<Message>>) -> Result<()> {
        // 1. 归档：当前内存（含当前系统消息）→ 历史
        if !self.messages.is_empty() {
            tokio::fs::create_dir_all(&self.history_dir()).await
                .map_err(|e| Error::IoError(e.to_string()))?;
            let ts = Local::now().format("%Y-%m-%d-%H%M%S").to_string();
            // 历史文件名 = <key编码>-<时间戳>.jsonl（key 编码即缓存文件名段）
            let dest = self.history_dir().join(format!("{}-{}.jsonl", self.key_enc, ts));
            let mut file = tokio::fs::OpenOptions::new()
                .create(true).write(true).truncate(true).open(&dest).await
                .map_err(|e| Error::IoError(e.to_string()))?;
            // 历史格式与缓存一致：System 首行（如有），其后为消息行
            let mut lines = Vec::with_capacity(self.messages.len() + 1);
            if let Some(system) = &self.system_message {
                lines.push(Message::System { content: Arc::new(system.clone()) });
            }
            lines.extend(self.messages.iter().cloned());
            write_cache_lines(&mut file, &lines).await?;
        }
        // 2. 清空缓存文件（文件不存在幂等）
        if let Err(e) = tokio::fs::remove_file(&self.cache_path()).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(Error::IoError(e.to_string()));
            }
        }
        // 3. 重置消息（只有清空缓存文件后才允许重置消息）
        if let Some(messages) = messages {
            self.messages.clear();
            // append 复用内存装入 + 缓存写回
            self.append(messages).await?;
        }
        Ok(())
    }

    /// 构建模型消息列表（system 在最前）
    pub fn build(&self) -> Vec<Message> {
        let mut items = Vec::new();
        if let Some(system) = &self.system_message {
            items.push(Message::System { content: Arc::new(system.clone()) });
        }
        items.extend(self.messages.iter().cloned());
        items
    }

    // ========== 私有：缓存文件读写 ==========

    /// 打开缓存文件（追加模式；新文件先落 System 首行）——&mut self 排他：写入须独占，避免并发交叉写坏行
    async fn open_cache_and_write_system_line(&mut self) -> Result<tokio::fs::File> {
        let path = self.cache_path();
        let is_new = !path.exists();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| Error::IoError(e.to_string()))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true).append(true).open(&path).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        if is_new {
            // 缓存格式：System 行在最前；新文件创建时落当前系统消息
            if let Some(system) = &self.system_message {
                let line = serde_json::to_string(&Message::System { content: Arc::new(system.clone()) })?;
                file.write_all(line.as_bytes()).await
                    .map_err(|e| Error::IoError(e.to_string()))?;
                file.write_all(b"\n").await
                    .map_err(|e| Error::IoError(e.to_string()))?;
            }
        }
        Ok(file)
    }
}

/// 逐行写 Message JSON（每行一条，\n 结尾；缓存/历史共用）
/// 自由函数（不依赖 self 字段）：排他性由 &mut self 调用方（append / apply_pending_system / archive_and_clear_cache）保证
async fn write_cache_lines(file: &mut tokio::fs::File, messages: &[Message]) -> Result<()> {
    for m in messages {
        let line = serde_json::to_string(m)?;
        file.write_all(line.as_bytes()).await
            .map_err(|e| Error::IoError(e.to_string()))?;
        file.write_all(b"\n").await
            .map_err(|e| Error::IoError(e.to_string()))?;
    }
    Ok(())
}

/// 单个会话：独立上下文（模型与模式状态不在会话上保存，模型/参数由 (agent, role) 与 session 配置在发送时解析）
pub struct Session {
    /// 会话上下文（内存消息 + 本地缓存 + 历史归档；Nexus 不直接访问，统一经 Session 方法/内部逻辑）
    pub context: tokio::sync::Mutex<SessionContext>,
}

impl Session {
    pub async fn build_context(&self) -> Vec<Message> {
        let mut ctx = self.context.lock().await;
        let _ = ctx.apply_pending_system().await;
        ctx.build()
    }

    pub async fn context_append(&self, messages: Vec<Message>) {
        let mut ctx = self.context.lock().await;
        let _ = ctx.append(messages).await;
    }

    pub async fn context_archive_and_clear_cache_and_reset_messages(&self, new_messages: Vec<Message>) {
        let mut ctx = self.context.lock().await;
        let _ = ctx.archive_and_clear_cache_and_reset_messages(Some(new_messages)).await;
    }

    pub async fn set_system_message(&self, content: String) {
        let mut ctx = self.context.lock().await;
        ctx.set_system_message(content);
    }
}

/// 会话管理器：汇总所有绑定 channel 的 (agent_id, role_name, mode) 去重维护会话集合
/// （session_key 仅用于去重；agent_id 直接取 key 的 agent_id 字段）
pub struct SessionManager {
    sessions: DashMap<SessionKey, Arc<Session>>,
    /// 数据目录（会话上下文缓存/历史归档路径基准）
    data_dir: String,
}

impl SessionManager {
    pub fn new(data_dir: &str) -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
            data_dir: data_dir.to_string(),
        })
    }

    /// 定位会话，不存在则创建（返回 (会话, 是否新建)）；
    /// 仅完成上下文恢复（recover_from_cache）；pipeline 同步与系统提示词初始化由调用方在拿到会话后执行
    /// （见 Nexus::ensure_session——那两步会反查本函数取会话，不能在持有 DashMap 分片锁时执行）
    pub async fn get_or_create(&self, key: &SessionKey) -> (Arc<Session>, bool) {
        if let Some(s) = self.sessions.get(&key) {
            return (s.clone(), false);
        }
        match self.sessions.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(e) => {
                (e.get().clone(), false)
            },
            dashmap::mapref::entry::Entry::Vacant(e) => {
                // context 先建
                let mut context = SessionContext::new(&self.data_dir, &key);
                // 新建会话初始化
                let _ = context.recover_from_cache().await;
                // 新建 session
                let session = Arc::new(Session {
                    context: Mutex::new(context),
                });
                // 保存 session
                e.insert(session.clone());
                (session, true)
            }
        }
    }

    /// 只保留仍在绑定集合中的会话（绑定信息变化后清理无绑定会话）
    pub fn retain(&self, keys: &HashSet<SessionKey>) {
        self.sessions.retain(|k, _| keys.contains(k));
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    use serde_json::Value;

    use crate::nexus::Nexus;
use crate::{config_manager::ConfigManager, station::Station};

    fn key(agent: &str, role: &str) -> SessionKey {
        SessionKey { agent_id: agent.into(), role_name: role.into(), mode: Mode::Role }
    }

    /// Assistant 消息助手（content/reasoning_content/tool_calls 均为 Value，Null 由 skip_serializing_if 省略）
    fn assistant(content: Value, reasoning_content: Value, tool_calls: Value) -> Message {
        Message::Assistant { content, reasoning_content, tool_calls }
    }

    /// Tool 消息助手（无 name 字段：工具名由 assistant.tool_calls 承载）
    fn tool_msg(id: &str, content: &str) -> Message {
        Message::Tool { tool_call_id: Arc::new(id.into()), content: Arc::new(content.into()) }
    }

    /// 测试进程级装配（幂等）：ConfigManager/Station/Nexus 单例各注册一次。
    /// Nexus::ensure_session 在新建会话时会同步 pipeline 并初始化系统提示词（经这几个单例），
    /// get_or_create 相关测试前需先装配；data_dir 目录经 OnceLock 保活，避免 tempdir drop 后单例路径失效
    static TEST_GLOBAL_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static TEST_INIT_DONE: AtomicBool = AtomicBool::new(false);
    async fn ensure_test_globals() {
        if !TEST_INIT_DONE.load(Ordering::Relaxed) {
            let dir = TEST_GLOBAL_DIR.get_or_init(|| tempfile::tempdir().unwrap());
            let cfg_path = dir.path().join("config.json");
            let cfg_json = format!(
                r#"{{"api":{{"memory_store_url":"","memory_ego_url":""}},"security":{{"api_key":"user-key-456","admin_api_key":"admin-key-123"}},"agent":{{"data_dir":"{}","mgmt_host":"127.0.0.1","mgmt_port":9091,"ws_reconnect_interval_secs":5}}}}"#,
                dir.path().join("data").to_str().unwrap()
            );
            std::fs::write(&cfg_path, cfg_json).unwrap();
            // 2024 edition：设置环境变量需要 unsafe
            unsafe { std::env::set_var("KISSBOT_CONFIG", cfg_path.to_str().unwrap()) };
            // 幂等：ConfigManager::new() 注册一次（第二实例丢弃）；Station/Nexus 同理（SINGLETON.set 失败被忽略）
            let _ = ConfigManager::new().await;
            let _ = Station::new().await;
            let _ = Nexus::new().await;
            TEST_INIT_DONE.store(true, Ordering::Relaxed);
        }
    }

    /// 测试用 SessionManager（data_dir 指向临时目录；会话持久化路径仅构造不落盘）
    fn mgr() -> Arc<SessionManager> {
        let dir = tempfile::tempdir().unwrap();
        SessionManager::new(dir.path().to_str().unwrap())
    }

    #[tokio::test]
    async fn get_or_create_dedupes() {
        ensure_test_globals().await;
        let mgr = mgr();
        let k = key("a1", "r1");
        // 会话创建不再接收 model（模型/参数由 (agent, role) + session 配置在消息发送时解析）
        let (s1, _) = mgr.get_or_create(&k).await;
        let (s2, _) = mgr.get_or_create(&k).await;
        assert!(Arc::ptr_eq(&s1, &s2), "同 key 应返回同一 Session");
        // 不同 mode 是不同会话
        let k_event = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let (s3, _) = mgr.get_or_create(&k_event).await;
        assert!(!Arc::ptr_eq(&s1, &s3), "不同 mode 应返回不同 Session");
    }

    #[tokio::test]
    async fn retain_keeps_only_bound_sessions() {
        ensure_test_globals().await;
        let mgr = mgr();
        let k1 = key("a1", "r1");
        let k2 = key("a2", "r2");
        let (s1, _) = mgr.get_or_create(&k1).await;
        let (s2, _) = mgr.get_or_create(&k2).await;
        // 绑定信息变化后只保留仍在绑定集合中的会话
        mgr.retain(&HashSet::from([k1.clone()]));
        let (again, _) = mgr.get_or_create(&k1).await;
        assert!(Arc::ptr_eq(&s1, &again), "保留的会话应复用");
        let (recreated, _) = mgr.get_or_create(&k2).await;
        assert!(!Arc::ptr_eq(&s2, &recreated), "被清理的会话应重建");
    }

    #[tokio::test]
    async fn session_wrappers_delegate_to_context() {
        let dir = tempfile::tempdir().unwrap();
        let k = key("a1", "r1");
        let session = Session {
            context: tokio::sync::Mutex::new(SessionContext::new(dir.path().to_str().unwrap(), &k)),
        };
        // set_system_message 只写待定（SessionContext.build 不立即带 system）
        session.set_system_message("系统".into()).await;
        // build_context 是发送前入口：应用待定系统消息（System 首条）
        let ctx = session.build_context().await;
        assert_eq!(ctx.len(), 1, "仅有系统消息");
        assert!(matches!(&ctx[0], Message::System { content } if content.as_str() == "系统"));
        // 追加消息 → build_context 应用待定系统消息（System 首条）
        session.context_append(vec![Message::User { content: Arc::new("你好".into()) }]).await;
        let ctx = session.build_context().await;
        assert_eq!(ctx.len(), 2, "system + 消息");
        assert!(matches!(&ctx[0], Message::System { content } if content.as_str() == "系统"));
        // 归档 + 重建：内容整体替换，system 保留
        session.context_archive_and_clear_cache_and_reset_messages(
            vec![Message::User { content: Arc::new("新话题".into()) }]).await;
        let ctx = session.build_context().await;
        assert_eq!(ctx.len(), 2, "system + 新消息");
        assert!(matches!(&ctx[1], Message::User { content } if content.as_str() == "新话题"));
        assert!(dir.path().join("context-history").exists(), "归档目录已建立");
    }

    // ===== 会话上下文（内存 + 缓存 + 历史一体）测试 =====

    fn cache_key() -> SessionKey {
        SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) }
    }

    fn role_key() -> SessionKey {
        SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role }
    }

    fn sample_msgs() -> Vec<Message> {
        vec![
            Message::User { content: Arc::new("你好".into()) },
            assistant(Value::String("在的".into()), Value::String("思考".into()), Value::Null),
        ]
    }

    /// 上下文里的 assistant(tool_calls)：raw_tool_calls 原样透传（与 API 返回 JSON 一致）
    fn wire_tool_calls() -> Value {
        serde_json::json!([{ "id": "c1", "type": "function", "function": { "name": "read", "arguments": "{}" } }])
    }

    #[tokio::test]
    async fn append_persists_and_recover_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let k = cache_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        assert!(ctx.build().is_empty(), "初始为空");
        assert!(!dir.path().join("context").exists(), "未写入前不落盘");
        ctx.append(sample_msgs()).await.unwrap();
        // 同 key 新上下文从缓存恢复（内存为空）：append 已同时写内存 + 缓存
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), &k);
        recovered.recover_from_cache().await.unwrap();
        let back = recovered.build();
        assert_eq!(back.len(), 2);
        assert!(matches!(&back[0], Message::User { content } if content.as_str() == "你好"));
        assert!(matches!(&back[1], Message::Assistant { reasoning_content: r, .. }
            if r.as_str() == Some("思考")), "reasoning_content 应保留");
    }

    #[tokio::test]
    async fn append_twice_accumulates_and_rebuild_clears() {
        let dir = tempfile::tempdir().unwrap();
        let k = cache_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        ctx.append(sample_msgs()).await.unwrap();
        ctx.append(vec![Message::User { content: Arc::new("再问".into()) }]).await.unwrap();
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), &k);
        recovered.recover_from_cache().await.unwrap();
        assert_eq!(recovered.build().len(), 3, "追加不截断");
        // 归档 + 重建：清空内存，缓存不残留
        ctx.archive_and_clear_cache_and_reset_messages(Some(vec![])).await.unwrap();
        assert!(ctx.build().is_empty(), "重建后内存为空");
        let mut after = SessionContext::new(dir.path().to_str().unwrap(), &k);
        after.recover_from_cache().await.unwrap();
        assert!(after.build().is_empty(), "重建后缓存为空");
        // 无内容时重复重建幂等
        ctx.archive_and_clear_cache_and_reset_messages(Some(vec![])).await.unwrap();
    }

    #[tokio::test]
    async fn append_empty_is_noop() {
        // 空 Vec 不写文件也不改变内存（apply_pending_system / 重建路径复用）
        let dir = tempfile::tempdir().unwrap();
        let k = cache_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        ctx.append(vec![]).await.unwrap();
        assert!(ctx.build().is_empty());
        assert!(!dir.path().join("context").exists(), "空追加不建缓存");
    }

    #[tokio::test]
    async fn recover_sanitizes_dangling_tool_turn() {
        let dir = tempfile::tempdir().unwrap();
        // 用例 A：完整轮次 user → assistant(tool_calls) → tool，末尾再追一条悬挂 assistant(tool_calls)
        // （崩溃发生在追加 assistant(tool_calls) 后、Tool 响应写入前）→ 回读丢弃悬挂尾巴，保留完整轮次
        let k_a = cache_key();
        let mut ctx_a = SessionContext::new(dir.path().to_str().unwrap(), &k_a);
        ctx_a.append(vec![
            Message::User { content: Arc::new("查一下".into()) },
            assistant(Value::String(String::new()), Value::Null, wire_tool_calls()),
            tool_msg("c1", "内容"),
        ]).await.unwrap();
        ctx_a.append(vec![assistant(Value::String(String::new()), Value::Null, wire_tool_calls())]).await.unwrap();
        let mut rec_a = SessionContext::new(dir.path().to_str().unwrap(), &k_a);
        rec_a.recover_from_cache().await.unwrap();
        let back = rec_a.build();
        assert_eq!(back.len(), 3, "悬挂的 assistant(tool_calls) 应被丢弃，保留完整轮次");
        assert!(matches!(&back[1], Message::Assistant { tool_calls, .. } if !tool_calls.is_null()),
            "完整轮次的 assistant(tool_calls) 保留");
        assert!(matches!(&back[2], Message::Tool { .. }), "tool 响应保留");

        // 用例 B：仅一条悬挂 assistant(tool_calls) → 恢复为空
        let k_b = SessionKey { agent_id: "a1".into(), role_name: "r2".into(), mode: Mode::Role };
        let mut ctx_b = SessionContext::new(dir.path().to_str().unwrap(), &k_b);
        ctx_b.append(vec![assistant(Value::String(String::new()), Value::Null, wire_tool_calls())]).await.unwrap();
        let mut rec_b = SessionContext::new(dir.path().to_str().unwrap(), &k_b);
        rec_b.recover_from_cache().await.unwrap();
        assert!(rec_b.build().is_empty(), "仅悬挂 assistant 时恢复为空");

        // 用例 C：开头的 Tool 残留（恢复起点之前的半条轮次）被丢弃
        let k_c = SessionKey { agent_id: "a1".into(), role_name: "r3".into(), mode: Mode::Role };
        let mut ctx_c = SessionContext::new(dir.path().to_str().unwrap(), &k_c);
        ctx_c.append(vec![
            tool_msg("c9", "残留"),
            Message::User { content: Arc::new("继续".into()) },
        ]).await.unwrap();
        let mut rec_c = SessionContext::new(dir.path().to_str().unwrap(), &k_c);
        rec_c.recover_from_cache().await.unwrap();
        let back_c = rec_c.build();
        assert_eq!(back_c.len(), 1, "开头 Tool 残留被丢弃，保留后续完整消息");
        assert!(matches!(&back_c[0], Message::User { content } if content.as_str() == "继续"));
    }

    #[tokio::test]
    async fn archive_and_clear_writes_history_and_removes_cache() {
        let dir = tempfile::tempdir().unwrap();
        let k = role_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        ctx.append(sample_msgs()).await.unwrap();
        let cache_path = dir.path().join("context").join("a1-r1.jsonl");
        assert!(cache_path.exists(), "append 已建缓存");
        // 归档 + 清空一体：历史生成，缓存移除
        ctx.archive_and_clear_cache_and_reset_messages(None).await.unwrap();
        // 目标文件名 = <key编码>-<时间戳>.jsonl（历史目录内恰有一个文件；role_key = a1-r1）
        let history_dir = dir.path().join("context-history");
        let mut files = Vec::new();
        let mut rd = tokio::fs::read_dir(&history_dir).await.unwrap();
        while let Some(entry) = rd.next_entry().await.unwrap() {
            files.push(entry);
        }
        assert_eq!(files.len(), 1, "归档生成一个历史文件");
        let dest = files.remove(0).path();
        let fname = dest.file_name().unwrap().to_str().unwrap().to_string();
        assert!(fname.starts_with("a1-r1"), "文件名以 key 编码开头: {}", fname);
        assert!(fname.ends_with(".jsonl"));
        // 内容与内存一致（逐行 Message JSON；不复制缓存）
        let expect = sample_msgs().iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect::<Vec<_>>().join("\n") + "\n";
        assert_eq!(tokio::fs::read_to_string(&dest).await.unwrap(), expect);
        // 归档同时清空缓存（重写/重建前旧缓存不残留）
        assert!(!cache_path.exists(), "归档并清空后缓存不存在");
    }

    #[tokio::test]
    async fn archive_and_clear_with_empty_memory_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let k = role_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        // 内存为空：无归档、无报错、不创建历史目录
        ctx.archive_and_clear_cache_and_reset_messages(None).await.unwrap();
        assert!(!dir.path().join("context-history").exists(), "不创建历史目录");
        // 仅有系统消息同样不归档（历史只记录有消息的上下文）
        ctx.set_system_message("仅系统".into());
        ctx.apply_pending_system().await.unwrap();
        ctx.archive_and_clear_cache_and_reset_messages(None).await.unwrap();
        assert!(!dir.path().join("context-history").exists(), "系统消息不产生历史");
    }

    // ===== 系统消息：缓存/历史持久化 + 待定懒切换 =====

    #[tokio::test]
    async fn set_system_message_is_lazy_and_applies_on_send() {
        let dir = tempfile::tempdir().unwrap();
        let k = role_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        // set 只保存待定（多次 set 只保留最近一次），build 不立即带 system
        ctx.set_system_message("旧系统".into());
        ctx.set_system_message("新系统".into());
        assert!(ctx.build().iter().all(|m| !matches!(m, Message::System { .. })), "set 不立即生效");
        // 发送前应用：当前系统为空 → 不一致 → 替换（仅有系统消息不落缓存）
        ctx.apply_pending_system().await.unwrap();
        assert!(matches!(&ctx.build()[0], Message::System { content } if content.as_str() == "新系统"), "取最近一次 set");
        assert!(!dir.path().join("context").exists(), "仅有系统消息不写缓存");
        // 追加消息后缓存含 System 首行：同 key 新上下文恢复得到系统消息
        ctx.append(vec![Message::User { content: Arc::new("你好".into()) }]).await.unwrap();
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), &k);
        recovered.recover_from_cache().await.unwrap();
        assert!(matches!(&recovered.build()[0], Message::System { content } if content.as_str() == "新系统"), "缓存读出的系统消息放当前");
        let back = recovered.build();
        assert_eq!(back.len(), 2, "系统 + 消息");
        assert!(matches!(&back[1], Message::User { content } if content.as_str() == "你好"));
        // 再次 set 同值：一致 → 无变更（set 消息清空）
        ctx.set_system_message("新系统".into());
        ctx.apply_pending_system().await.unwrap();
        assert!(matches!(&ctx.build()[0], Message::System { content } if content.as_str() == "新系统"));
        assert!(!dir.path().join("context-history").exists(), "一致时不产生归档");
    }

    #[tokio::test]
    async fn system_change_archives_old_context_with_old_system() {
        let dir = tempfile::tempdir().unwrap();
        let k = role_key();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), &k);
        ctx.set_system_message("旧系统".into());
        ctx.apply_pending_system().await.unwrap();
        ctx.append(vec![Message::User { content: Arc::new("你好".into()) }]).await.unwrap();
        // 变更系统：不一致 → 旧上下文（含原系统消息）写成历史 → 替换 → 重建缓存
        ctx.set_system_message("新系统".into());
        ctx.apply_pending_system().await.unwrap();
        assert!(matches!(&ctx.build()[0], Message::System { content } if content.as_str() == "新系统"));
        // 历史：System 旧 + User 你好
        let history_dir = dir.path().join("context-history");
        let mut files = Vec::new();
        let mut rd = tokio::fs::read_dir(&history_dir).await.unwrap();
        while let Some(entry) = rd.next_entry().await.unwrap() {
            files.push(entry);
        }
        assert_eq!(files.len(), 1, "系统变更时旧上下文归档一次");
        let dest_text = tokio::fs::read_to_string(files.remove(0).path()).await.unwrap();
        assert!(dest_text.contains("旧系统"), "历史含原系统消息");
        assert!(dest_text.contains("你好"), "历史含原内存消息");
        // 缓存：System 新 + User 你好（消息保留）
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), &k);
        recovered.recover_from_cache().await.unwrap();
        assert!(matches!(&recovered.build()[0], Message::System { content } if content.as_str() == "新系统"));
        let back = recovered.build();
        assert_eq!(back.len(), 2);
        assert!(matches!(&back[1], Message::User { content } if content.as_str() == "你好"), "消息保留");
    }

    // 注：原 should_reset（"上次响应的 usage.total_tokens 超过 max_tokens_usage × 阈值才重置上下文"）单测已移除：
    // 该判定随上下文重置逻辑移入 pipeline —— pipelines/message_sender.rs 的
    // TokenLimitMessageSender::send_messages 内联（阈值 = effective.max_tokens_usage × EffectiveCompressConfig
    // .compress_threshold，max_tokens_usage=0 视为未启用）。判定现在嵌在发送流程中，需要
    // ConfigManager/Nexus/provider 全局装配才能驱动，不再是可独立断言的纯函数。
}
