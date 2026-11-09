use std::path::{Path, PathBuf};

use arcstr::ArcStr;
use chrono::Local;
use tokio::fs::{self, File};
use tokio::{io::AsyncWriteExt, sync::RwLock};

use crate::error::{Error, Result};
use crate::types::Message;

/// 会话上下文：内存消息 + 本地缓存 + 版本一体管理（持久化由 SessionContext 自身负责，调用方不感知）
///
/// ========== 会话上下文本地缓存（系统提示词与消息分文件） ==========
/// 系统提示词：<data_dir>/<session_id>.sys.jsonl，单行 System Message JSON（无系统提示词时不存在该文件）
/// 消息：<data_dir>/<session_id>.msgs.jsonl，每行一条 Message（JSON）
/// 消息文件追加写、不截断；读取时两个文件分别回读（消息经 LineReader 正序）。
/// 系统提示词 set 只写待定值；构建完整上下文时对比应用，需要变更才重写 sys 文件
/// （不保存当前版本、不改动消息文件）。
///
/// ========== 上下文版本 ==========
/// 归档与清空缓存永远配对（archive_and_clear）：把当前缓存的 sys / msgs 文件直接移动成
/// <data_dir>/versions/<session_id>-<后缀>.sys.jsonl 与 .msgs.jsonl（不重新写入，sys 使用原先的
/// 系统提示词、不应用待定值；内存为空时不产生版本，原 sys 不存在时版本也不写 sys），
/// 然后清空缓存与内存；Session 在归档后调用 append 写入新消息，互换则移入目标版本。
/// 版本后缀自动生成（本地时间 %Y%m%d%H%M%S，重名等一秒后重试）。
/// 版本与当前缓存格式完全一致，因此内存、缓存、版本三者统一在本结构体实现（会话上下文唯一入口）；
/// swap_version_with_cache 把某个版本换回当前缓存（当前缓存非空时先存成新版本，仅 rename，不复制）后重新读取缓存。
pub struct SessionContext {
    messages: Vec<Message>,
    /// 当前系统消息（模型上下文首条；含从缓存恢复的，直到下次发送前对比替换）
    system_message: Option<ArcStr>,
    /// 待定系统消息（set 只写这里，多次 set 只保留最近一次；下次构建完整上下文时对比应用）
    pending_system: Option<ArcStr>,
    /// 缓存目录：缓存直接放其下，版本放其 versions/ 子目录（路径用时按 data_dir + session_id 构造，不冗余存路径）
    data_dir: PathBuf,
    /// 会话标识（缓存文件与版本文件的文件名段；须为文件名安全字符串，由调用方编码）
    session_id: ArcStr,
}

impl SessionContext {
    /// data_dir 即当前缓存目录（缓存直接放其下，版本放其 versions/ 子目录）；
    /// session_id 由调用方按自己的会话身份编码（须为文件名安全字符串，如 "{agent}-{role}"）
    pub fn new(data_dir: &str, session_id: ArcStr) -> Self {
        Self {
            messages: Vec::new(),
            system_message: None,
            pending_system: None,
            data_dir: PathBuf::from(data_dir),
            session_id,
        }
    }

    /// 当前系统提示词文件路径（<data_dir>/<session_id>.sys.jsonl）
    fn sys_path(&self) -> PathBuf {
        self.data_dir.join(format!("{}.sys.jsonl", self.session_id))
    }

    /// 当前消息文件路径（<data_dir>/<session_id>.msgs.jsonl）
    fn msgs_path(&self) -> PathBuf {
        self.data_dir.join(format!("{}.msgs.jsonl", self.session_id))
    }

    /// 版本目录（<data_dir>/versions）
    fn versions_dir(&self) -> PathBuf {
        self.data_dir.join("versions")
    }

    /// 版本的系统提示词文件路径（<data_dir>/versions/<session_id>-<后缀>.sys.jsonl）；
    /// 后缀来自自动生成或版本文件名，路径分隔符按 `_` 处理，避免逃出 versions/ 目录
    fn version_sys_path(&self, version_suffix: &str) -> PathBuf {
        let suffix = version_suffix.replace(['/', '\\'], "_");
        self.versions_dir().join(format!("{}-{}.sys.jsonl", self.session_id, suffix))
    }

    /// 版本的消息文件路径（<data_dir>/versions/<session_id>-<后缀>.msgs.jsonl）
    fn version_msgs_path(&self, version_suffix: &str) -> PathBuf {
        let suffix = version_suffix.replace(['/', '\\'], "_");
        self.versions_dir().join(format!("{}-{}.msgs.jsonl", self.session_id, suffix))
    }

    /// 自动生成版本后缀：本地时间 %Y%m%d%H%M%S；重名时等一秒后重试（不使用 _N 后缀）
    async fn next_version_suffix(&self) -> ArcStr {
        loop {
            let suffix = ArcStr::from(Local::now().format("%Y%m%d%H%M%S").to_string());
            if !self.version_sys_path(&suffix).exists() && !self.version_msgs_path(&suffix).exists() {
                return suffix;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    /// 设置系统消息：只保存待定值（不会失败、不写文件、不生成版本）；
    /// 多次 set 只保留最近一次，直到下次构建完整上下文时对比应用
    pub fn set_system_message(&mut self, content: ArcStr) {
        self.pending_system = Some(content);
    }

    /// 应用待定系统消息（构建完整上下文时调用）：无待定直接返回；与当前一致 → 仅清空待定；
    /// 不一致 → 重写 sys 文件并把待定值切为当前系统消息（不归档、不动消息文件）
    pub async fn apply_pending_system(&mut self) -> Result<()> {
        let Some(pending) = self.pending_system.clone() else {
            return Ok(());
        };
        if self.system_message.as_deref() == Some(pending.as_str()) {
            self.pending_system = None;
            return Ok(());
        }
        write_sys_file(&self.sys_path(), &pending).await?;
        self.system_message = Some(pending);
        self.pending_system = None;
        Ok(())
    }

    /// 追加消息（内存 + 消息文件一体，每行一条 Message JSON；不截断）
    /// best-effort：缓存失败仅丢缓存不阻塞流程（内存已装入，调用方按 Result 决定）
    pub async fn append(&mut self, messages: Vec<Message>) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        // 消息文件追加（首次落盘时如有系统提示词则建立 sys 文件；没有系统提示词不建空文件）
        let mut file = self.open_msgs_and_ensure_sys_file().await?;
        write_lines(&mut file, &messages).await?;
        // 内存追加
        self.messages.extend(messages);
        Ok(())
    }

    /// 从缓存恢复上下文：sys 文件读出当前系统消息，msgs 文件正序全量回读装入内存；
    /// 任一文件不存在视为对应内容为空
    pub async fn recover_from_cache(&mut self) -> Result<()> {
        // sys 文件只取一行 System Message JSON；文件不存在或为空视为没有系统提示词
        let sys = self.sys_path();
        self.system_message = if sys.exists() {
            let text = fs::read_to_string(&sys).await?;
            let mut system = None;
            for line in text.lines() {
                let s = line.trim();
                if s.is_empty() { continue; }
                if let Ok(Message::System { content }) = serde_json::from_str::<Message>(s) {
                    system = Some(content);
                    break;
                }
            }
            system
        } else {
            None
        };
        self.messages = Vec::new();
        let path = self.msgs_path();
        if !path.exists() {
            return Ok(());
        }
        let mut reader = kai_file::LineReader::new(&path, None, None).await?;
        while let Some(line) = reader.next_line().await? {
            let s = line.line.trim();
            if s.is_empty() { continue; }
            if let Ok(m) = serde_json::from_str::<Message>(s) {
                self.messages.push(m);
            }
        }
        Ok(())
    }

    /// 归档当前上下文为版本并清空（归档与清空永远配对；调用方随后重建或移入当前缓存）：
    /// 1) 内存为空则无可归档内容（仅有系统消息不归档，幂等）
    /// 2) 自动生成版本后缀（本地时间 %Y%m%d%H%M%S，重名等一秒后重试）
    /// 3) 把当前缓存的 sys / msgs 文件直接移动到版本（版本使用原先文件，不应用待定值，也不重新写入）
    /// 4) 清空缓存文件（文件不存在幂等），清空内存（system 保留）
    ///
    /// 版本是只读快照：需要复用以前的 session 时用 swap_version_with_cache 换回当前缓存，
    /// 后续内容变更只发生在当前缓存。
    pub async fn archive_and_clear(&mut self) -> Result<()> {
        let sys = self.sys_path();
        let msgs = self.msgs_path();
        // 1. 以内存消息为准判断是否有内容可归档（仅有系统消息不归档）
        if !self.messages.is_empty() {
            // 2. 自动生成日期后缀；重名时等一秒后重试
            let version_suffix = self.next_version_suffix().await;
            let version_sys = self.version_sys_path(&version_suffix);
            let version_msgs = self.version_msgs_path(&version_suffix);
            fs::create_dir_all(&self.versions_dir()).await?;
            // 3. 直接移动当前缓存文件成版本（sys 保留原先的系统提示词，不应用待定值）
            // 原 sys 文件不存在时版本也不写 sys，静默跳过
            if sys.exists() {
                fs::rename(&sys, &version_sys).await?;
            }
            // 原 msgs 文件不存在时版本也不写 msgs，静默跳过
            if msgs.exists() {
                fs::rename(&msgs, &version_msgs).await?;
            }
        }
        // 4. 清空缓存文件（已移走的文件不存在，幂等），清空内存（system 保留）
        remove_file_if_exists(&sys).await?;
        remove_file_if_exists(&msgs).await?;
        self.messages.clear();
        Ok(())
    }

    /// 把某个版本换回当前缓存（仅 rename，不复制文件），随后重新读取缓存：
    /// - 先校验后缀；格式不合法按版本不存在处理（不报错）
    /// - 当前缓存非空（msgs 文件非空，不看 sys）：先把当前缓存移动成一个新版本（自动生成后缀），
    ///   再把目标版本移入当前缓存；目标版本移入后即被消耗
    /// - 当前缓存为空：清掉残留文件（含只有 sys 的情况），再把目标版本移入；不产生新版本
    /// - 目标版本不存在：当前缓存非空则仍先存成新版本，然后当前缓存清空（新建空上下文）
    ///
    /// &mut self 强制排他：调用方须持独占引用（经 session 的写锁）
    pub async fn swap_version_with_cache(&mut self, version_suffix: &str) -> Result<()> {
        // 1. 校验后缀；不合法等同版本不存在，不构造路径、不报错
        let valid = !version_suffix.is_empty()
            && version_suffix.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let version_paths = if valid {
            let vs = self.version_sys_path(version_suffix);
            let vm = self.version_msgs_path(version_suffix);
            let exists = vs.exists() || vm.exists();
            Some((vs, vm, exists))
        } else {
            None
        };

        // 2. 将当前缓存生成一个新版本，旧内容不占用目标版本的名字
        self.archive_and_clear().await?;

        // 3. 把目标版本移入（目标版本被消耗）
        if let Some((version_sys, version_msgs, true)) = version_paths {
            let sys = self.sys_path();
            let msgs = self.msgs_path();
            if version_sys.exists() {
                fs::rename(&version_sys, &sys).await?;
            }
            if version_msgs.exists() {
                fs::rename(&version_msgs, &msgs).await?;
            }
        }

        // 4. 重新读取内存与系统消息
        self.pending_system = None;
        self.recover_from_cache().await
    }

    /// 构建模型消息列表（system 在最前）
    pub fn build(&self) -> Vec<Message> {
        let mut items = Vec::new();
        if let Some(system) = &self.system_message {
            items.push(Message::System { content: system.clone() });
        }
        items.extend(self.messages.iter().cloned());
        items
    }

    // ========== 私有：消息文件追加 ==========

    /// 打开消息文件（追加模式）；首次落盘时如有系统提示词则建立 sys 文件
    async fn open_msgs_and_ensure_sys_file(&self) -> Result<File> {
        let msgs = self.msgs_path();
        if let Some(parent) = msgs.parent() {
            fs::create_dir_all(parent).await?;
        }
        if let Some(system) = &self.system_message {
            let sys = self.sys_path();
            if !sys.exists() {
                write_sys_file(&sys, system).await?;
            }
        }
        Ok(fs::OpenOptions::new()
            .create(true).append(true).open(&msgs).await?)
    }
}

/// 重写系统提示词文件：单行 System Message JSON；没有系统提示词时不调用本函数
async fn write_sys_file(path: &Path, system: &ArcStr) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true).write(true).truncate(true).open(path).await?;
    let line = serde_json::to_string(&Message::System { content: system.clone() })?;
    file.write_all(line.as_bytes()).await?;
    file.write_all(b"\n").await?;
    Ok(())
}

/// 逐行写 Message JSON（每行一条，\n 结尾）
async fn write_lines(file: &mut File, messages: &[Message]) -> Result<()> {
    for m in messages {
        let line = serde_json::to_string(m)?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
    }
    Ok(())
}

/// 删除文件（不存在幂等）
async fn remove_file_if_exists(path: &Path) -> Result<()> {
    if let Err(e) = fs::remove_file(path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(Error::IoError(e));
        }
    }
    Ok(())
}

/// 单个会话：独立上下文（模型与模式状态不在会话上保存，模型/参数由调用方在发送时解析）
pub struct Session {
    /// 会话上下文（内存消息 + 本地缓存 + 版本；调用方不直接访问，统一经 Session 方法）。
    /// 读写锁：只读访问（`build_context` / `context_read`）可并发，写路径（追加 / 重建 / 设置系统消息）互斥
    pub context: RwLock<SessionContext>,
}

impl Session {
    /// 新建会话（不读缓存；需要恢复上一次上下文时另调 recover_from_cache）
    pub fn new(data_dir: &str, session_id: ArcStr) -> Self {
        Self {
            context: RwLock::new(SessionContext::new(data_dir, session_id)),
        }
    }

    /// 从本地缓存恢复上下文（新建会话时调用；文件不存在视为空上下文）
    pub async fn recover_from_cache(&self) -> Result<()> {
        let mut ctx = self.context.write().await;
        ctx.recover_from_cache().await
    }

    /// 构建模型消息（只读）：首条为当前系统消息；不应用待定系统消息、不写缓存。
    /// 压缩等只需读取上下文的路径用它，避免系统提示词变更触发写文件
    pub async fn build_context(&self) -> Vec<Message> {
        let ctx = self.context.read().await;
        ctx.build()
    }

    /// 构建模型消息并应用待定系统消息（发送前入口）：
    /// 待定值与当前不一致时重写 sys 文件（写失败保留当前系统消息，不阻塞构建）
    pub async fn build_context_full(&self) -> Vec<Message> {
        let mut ctx = self.context.write().await;
        let _ = ctx.apply_pending_system().await;
        ctx.build()
    }

    pub async fn context_append(&self, messages: Vec<Message>) -> Result<()>{
        let mut ctx = self.context.write().await;
        ctx.append(messages).await
    }

    /// 只读访问会话消息（不含系统消息，保持当前顺序）：闭包拿到的列表只读，上下文不变。
    /// 压缩 / 记忆回溯取尾部消息时用：只需读取，避免「先回退再重置」两次上下文变更
    pub async fn context_read<R>(&self, f: impl FnOnce(&Vec<Message>) -> R) -> R {
        let ctx = self.context.read().await;
        f(&ctx.messages)
    }

    pub async fn context_archive_and_clear_cache_and_reset_messages(&self, new_messages: Vec<Message>) -> Result<()> {
        let mut ctx = self.context.write().await;
        ctx.archive_and_clear().await?;
        ctx.append(new_messages).await
    }

    /// 把某个版本与当前缓存互换（仅移动文件），并重新读取缓存到内存
    pub async fn context_swap_version_with_cache(&self, version_suffix: &str) -> Result<()> {
        let mut ctx = self.context.write().await;
        ctx.swap_version_with_cache(version_suffix).await
    }

    /// 设置待定系统提示词（不写文件、不会失败；下次 build_context_full 时对比应用）
    pub async fn set_system_message(&self, content: ArcStr) {
        let mut ctx = self.context.write().await;
        ctx.set_system_message(content);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arcstr::ArcStr;
    use serde_json::Value;

    use super::*;

    /// Assistant 消息助手（content/reasoning_content/tool_calls 均为 Value，Null 由 skip_serializing_if 省略）
    fn assistant(content: Value, reasoning_content: Value, tool_calls: Value) -> Message {
        Message::Assistant { content, reasoning_content, tool_calls }
    }

    fn sample_msgs() -> Vec<Message> {
        vec![
            Message::User { content: ArcStr::from("你好") },
            assistant(Value::String("在的".into()), Value::String("思考".into()), Value::Null),
        ]
    }

    fn expected_lines(messages: &[Message]) -> String {
        messages.iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect::<Vec<_>>().join("\n") + "\n"
    }

    /// 读取某 session 在 versions/ 下的全部版本后缀（sys / msgs 任一文件都计入，去重后排序）
    async fn version_suffixes(dir: &Path, session_id: &str) -> Vec<String> {
        let versions = dir.join("versions");
        if !versions.exists() {
            return Vec::new();
        }
        let prefix = format!("{session_id}-");
        let mut suffixes = BTreeSet::new();
        let mut rd = fs::read_dir(&versions).await.unwrap();
        while let Some(entry) = rd.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(rest) = name.strip_prefix(&prefix) else { continue; };
            let Some(suffix) = rest.strip_suffix(".sys.jsonl").or_else(|| rest.strip_suffix(".msgs.jsonl")) else { continue; };
            suffixes.insert(suffix.to_string());
        }
        suffixes.into_iter().collect()
    }

    async fn only_version_suffix(dir: &Path, session_id: &str) -> String {
        let mut suffixes = version_suffixes(dir, session_id).await;
        assert_eq!(suffixes.len(), 1, "应恰有一个版本");
        suffixes.remove(0)
    }

    #[tokio::test]
    async fn append_persists_and_recover_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1-e1"));
        assert!(ctx.build().is_empty(), "初始为空");
        assert!(!dir.path().join("a1-r1-e1.msgs.jsonl").exists(), "未写入前不落盘");
        ctx.append(sample_msgs()).await.unwrap();
        assert!(dir.path().join("a1-r1-e1.msgs.jsonl").exists(), "消息文件已建立");
        assert!(!dir.path().join("a1-r1-e1.sys.jsonl").exists(), "无系统提示词不建空 sys 文件");
        // 同 session_id 新上下文从缓存恢复（内存为空）：append 已同时写内存 + 缓存
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1-e1"));
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
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1-e1"));
        ctx.append(sample_msgs()).await.unwrap();
        ctx.append(vec![Message::User { content: ArcStr::from("再问") }]).await.unwrap();
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1-e1"));
        recovered.recover_from_cache().await.unwrap();
        assert_eq!(recovered.build().len(), 3, "追加不截断");
        // 归档 + 重建：清空内存，缓存不残留，版本留下
        ctx.archive_and_clear().await.unwrap();
        assert!(ctx.build().is_empty(), "重建后内存为空");
        let mut after = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1-e1"));
        after.recover_from_cache().await.unwrap();
        assert!(after.build().is_empty(), "重建后缓存为空");
        let suffix = only_version_suffix(dir.path(), "a1-r1-e1").await;
        let versions = dir.path().join("versions");
        assert!(!versions.join(format!("a1-r1-e1-{suffix}.sys.jsonl")).exists(), "无系统提示词时版本不写 sys");
        assert!(versions.join(format!("a1-r1-e1-{suffix}.msgs.jsonl")).exists(), "版本消息文件");
        // 无内容时重复重建幂等（不产生新版本）
        ctx.archive_and_clear().await.unwrap();
        assert_eq!(version_suffixes(dir.path(), "a1-r1-e1").await.len(), 1, "空内存不产生新版本");
    }

    #[tokio::test]
    async fn append_empty_is_noop() {
        // 空 Vec 不写文件也不改变内存（重建路径复用）
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1-e1"));
        ctx.append(vec![]).await.unwrap();
        assert!(ctx.build().is_empty());
        assert!(!dir.path().join("a1-r1-e1.sys.jsonl").exists(), "空追加不建 sys 文件");
        assert!(!dir.path().join("a1-r1-e1.msgs.jsonl").exists(), "空追加不建消息文件");
    }

    #[tokio::test]
    async fn archive_writes_two_version_files_and_removes_cache() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        ctx.set_system_message(ArcStr::from("系统提示"));
        ctx.apply_pending_system().await.unwrap();
        ctx.append(sample_msgs()).await.unwrap();
        let sys = dir.path().join("a1-r1.sys.jsonl");
        let msgs = dir.path().join("a1-r1.msgs.jsonl");
        assert!(sys.exists() && msgs.exists(), "缓存分开两个文件");
        // 归档 + 清空一体：版本生成（系统提示词与消息各一个文件），缓存移除
        ctx.archive_and_clear().await.unwrap();
        let suffix = only_version_suffix(dir.path(), "a1-r1").await;
        assert!(suffix.chars().all(|c| c.is_ascii_digit() || c == '_'), "自动后缀只含数字/下划线: {}", suffix);
        let version_sys = dir.path().join("versions").join(format!("a1-r1-{suffix}.sys.jsonl"));
        let version_msgs = dir.path().join("versions").join(format!("a1-r1-{suffix}.msgs.jsonl"));
        assert!(version_sys.exists(), "版本系统提示词文件名 = <session_id>-<后缀>.sys.jsonl");
        assert!(version_msgs.exists(), "版本消息文件名 = <session_id>-<后缀>.msgs.jsonl");
        let sys_text = fs::read_to_string(&version_sys).await.unwrap();
        assert!(sys_text.contains("系统提示") && sys_text.contains("\"system\""), "版本系统提示词为 System 行");
        assert_eq!(fs::read_to_string(&version_msgs).await.unwrap(), expected_lines(&sample_msgs()));
        assert!(!sys.exists() && !msgs.exists(), "归档并清空后缓存不存在");
    }

    #[tokio::test]
    async fn archive_and_clear_with_empty_memory_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        // 内存为空：无版本、无报错、不创建版本目录
        ctx.archive_and_clear().await.unwrap();
        assert!(!dir.path().join("versions").exists(), "不创建版本目录");
        // 仅有系统消息同样不归档（版本只记录有消息的上下文）
        ctx.set_system_message(ArcStr::from("仅系统"));
        ctx.archive_and_clear().await.unwrap();
        assert!(!dir.path().join("versions").exists(), "系统消息不产生版本");
    }

    // ===== 系统提示词：set 只 pending，构建完整上下文时应用 =====

    #[tokio::test]
    async fn set_system_message_is_pending_and_applied_on_build() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        // 多次 set 只保留最近一次；set 不写文件、不改变当前系统消息
        ctx.set_system_message(ArcStr::from("旧系统"));
        ctx.set_system_message(ArcStr::from("新系统"));
        assert!(ctx.build().is_empty(), "set 不立即生效");
        let sys = dir.path().join("a1-r1.sys.jsonl");
        assert!(!sys.exists(), "set 不写 sys 文件");
        // 构建完整上下文时应用：重写 sys 文件，当前系统消息切为待定值
        ctx.apply_pending_system().await.unwrap();
        assert!(matches!(&ctx.build()[0], Message::System { content } if content.as_str() == "新系统"), "应用后生效");
        let sys_text = fs::read_to_string(&sys).await.unwrap();
        assert!(sys_text.contains("新系统") && !sys_text.contains("旧系统"), "sys 文件被重写");
        assert_eq!(sys_text.lines().count(), 1, "sys 文件单行");
        // 仅有系统消息时不产生消息文件/版本
        assert!(!dir.path().join("a1-r1.msgs.jsonl").exists());
        assert!(!dir.path().join("versions").exists(), "改系统提示词不产生版本");
    }

    #[tokio::test]
    async fn system_change_is_pending_and_does_not_archive_or_clear_messages() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        ctx.set_system_message(ArcStr::from("旧系统"));
        ctx.apply_pending_system().await.unwrap();
        ctx.append(vec![Message::User { content: ArcStr::from("你好") }]).await.unwrap();
        // 改系统提示词：只存待定，不归档、不清消息
        ctx.set_system_message(ArcStr::from("新系统"));
        assert!(!dir.path().join("versions").exists(), "改系统提示词不产生版本");
        // 未应用：磁盘上仍是旧系统
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        recovered.recover_from_cache().await.unwrap();
        let back = recovered.build();
        assert_eq!(back.len(), 2, "系统 + 原消息");
        assert!(matches!(&back[0], Message::System { content } if content.as_str() == "旧系统"), "待定尚未落盘");
        assert!(matches!(&back[1], Message::User { content } if content.as_str() == "你好"), "消息保留");
        // 应用待定：只重写 sys，不归档、不动消息
        ctx.apply_pending_system().await.unwrap();
        assert!(matches!(&ctx.build()[0], Message::System { content } if content.as_str() == "新系统"));
        assert_eq!(ctx.build().len(), 2);
        assert!(!dir.path().join("versions").exists(), "应用待定也不产生版本");
        let sys_text = fs::read_to_string(dir.path().join("a1-r1.sys.jsonl")).await.unwrap();
        assert!(sys_text.contains("新系统"), "应用后 sys 文件更新");
        let msgs_text = fs::read_to_string(dir.path().join("a1-r1.msgs.jsonl")).await.unwrap();
        assert!(msgs_text.contains("你好"), "消息文件不变");
    }

    #[tokio::test]
    async fn recover_reads_sys_and_msgs_files_separately() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        ctx.set_system_message(ArcStr::from("系统"));
        ctx.apply_pending_system().await.unwrap();
        ctx.append(sample_msgs()).await.unwrap();
        let mut recovered = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        recovered.recover_from_cache().await.unwrap();
        assert_eq!(recovered.build().len(), 3, "系统 + 两条消息");
        // 文件缺失视为空
        let mut empty = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("other"));
        empty.recover_from_cache().await.unwrap();
        assert!(empty.build().is_empty());
    }

    #[tokio::test]
    async fn archive_moves_original_sys_and_ignores_pending() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        ctx.set_system_message(ArcStr::from("旧系统"));
        ctx.apply_pending_system().await.unwrap();
        ctx.append(sample_msgs()).await.unwrap();
        // 设置新系统但尚未应用：生成版本时必须使用磁盘上的原 sys 文件
        ctx.set_system_message(ArcStr::from("新系统"));
        ctx.archive_and_clear().await.unwrap();
        let suffix = only_version_suffix(dir.path(), "a1-r1").await;
        let version_sys = fs::read_to_string(
            dir.path().join("versions").join(format!("a1-r1-{suffix}.sys.jsonl"))).await.unwrap();
        assert!(version_sys.contains("旧系统"), "版本使用原先的系统提示词");
        assert!(!version_sys.contains("新系统"), "版本不使用待定值");
        let version_msgs = fs::read_to_string(
            dir.path().join("versions").join(format!("a1-r1-{suffix}.msgs.jsonl"))).await.unwrap();
        assert_eq!(version_msgs, expected_lines(&sample_msgs()));
        // 待定仍在：应用后写新的 sys 文件，版本不变
        ctx.apply_pending_system().await.unwrap();
        assert!(matches!(&ctx.build()[0], Message::System { content } if content.as_str() == "新系统"));
        let cache_sys = fs::read_to_string(dir.path().join("a1-r1.sys.jsonl")).await.unwrap();
        assert!(cache_sys.contains("新系统"));
        assert_eq!(version_suffixes(dir.path(), "a1-r1").await.len(), 1, "仍只有一个版本");
    }

    // ===== 版本：自动后缀 / 互换 =====

    #[tokio::test]
    async fn archive_generates_unique_suffixes() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        ctx.append(sample_msgs()).await.unwrap();
        ctx.archive_and_clear().await.unwrap();
        ctx.append(vec![Message::User { content: ArcStr::from("新消息") }]).await.unwrap();
        ctx.archive_and_clear().await.unwrap();
        let suffixes = version_suffixes(dir.path(), "a1-r1").await;
        assert_eq!(suffixes.len(), 2, "两次归档生成两个版本");
        assert_ne!(suffixes[0], suffixes[1], "同秒重名自动追加序号");
        for suffix in &suffixes {
            let msgs = dir.path().join("versions").join(format!("a1-r1-{suffix}.msgs.jsonl"));
            assert!(msgs.exists(), "每个版本都有消息文件: {}", suffix);
            let _ = fs::read_to_string(&msgs).await.unwrap();
        }
    }

    #[tokio::test]
    async fn swap_saves_current_as_new_version_and_restores_target() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap();
        // 先造一个带 System 的版本
        let mut first = SessionContext::new(base, ArcStr::from("a1-r1"));
        first.set_system_message(ArcStr::from("旧系统"));
        first.apply_pending_system().await.unwrap();
        first.append(sample_msgs()).await.unwrap();
        first.archive_and_clear().await.unwrap();
        let target = only_version_suffix(dir.path(), "a1-r1").await;
        // 另一个上下文写入新缓存（System 新 + 一条消息）
        let mut second = SessionContext::new(base, ArcStr::from("a1-r1"));
        second.set_system_message(ArcStr::from("新系统"));
        second.apply_pending_system().await.unwrap();
        second.append(vec![Message::User { content: ArcStr::from("新问题") }]).await.unwrap();
        // 互换：当前缓存先存成新版本，再把目标版本移入当前缓存
        second.swap_version_with_cache(&target).await.unwrap();
        let ctx = second.build();
        assert_eq!(ctx.len(), 3, "System 旧 + 两条消息");
        assert!(matches!(&ctx[0], Message::System { content } if content.as_str() == "旧系统"));
        assert!(matches!(&ctx[1], Message::User { content } if content.as_str() == "你好"));
        assert!(matches!(&ctx[2], Message::Assistant { .. }));
        // 目标版本被移入缓存后不存在；当前缓存内容存成了一个新版本
        let versions = dir.path().join("versions");
        assert!(!versions.join(format!("a1-r1-{target}.sys.jsonl")).exists(), "目标版本已移入缓存");
        assert!(!versions.join(format!("a1-r1-{target}.msgs.jsonl")).exists(), "目标版本已移入缓存");
        let saved = only_version_suffix(dir.path(), "a1-r1").await;
        assert_ne!(saved, target, "当前缓存使用新的版本后缀");
        let saved_sys = fs::read_to_string(versions.join(format!("a1-r1-{saved}.sys.jsonl"))).await.unwrap();
        let saved_msgs = fs::read_to_string(versions.join(format!("a1-r1-{saved}.msgs.jsonl"))).await.unwrap();
        assert!(saved_sys.contains("新系统") && !saved_sys.contains("旧系统"), "新版本保存旧缓存的系统提示词");
        assert!(saved_msgs.contains("新问题"), "新版本保存旧缓存的消息");
        let cache_sys = fs::read_to_string(dir.path().join("a1-r1.sys.jsonl")).await.unwrap();
        let cache_msgs = fs::read_to_string(dir.path().join("a1-r1.msgs.jsonl")).await.unwrap();
        assert!(cache_sys.contains("旧系统"), "缓存收到目标版本的系统提示词");
        assert!(cache_msgs.contains("你好") && !cache_msgs.contains("新问题"), "缓存收到目标版本的消息");
    }

    #[tokio::test]
    async fn swap_version_without_cache_moves_both_files_to_cache() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap();
        let mut ctx = SessionContext::new(base, ArcStr::from("a1-r1"));
        ctx.set_system_message(ArcStr::from("系统"));
        ctx.apply_pending_system().await.unwrap();
        ctx.append(sample_msgs()).await.unwrap();
        ctx.archive_and_clear().await.unwrap();
        let suffix = only_version_suffix(dir.path(), "a1-r1").await;
        assert!(!dir.path().join("a1-r1.sys.jsonl").exists(), "归档后无 sys 缓存");
        assert!(!dir.path().join("a1-r1.msgs.jsonl").exists(), "归档后无消息缓存");
        let versions = dir.path().join("versions");
        assert!(versions.join(format!("a1-r1-{suffix}.sys.jsonl")).exists());
        assert!(versions.join(format!("a1-r1-{suffix}.msgs.jsonl")).exists());
        // 空上下文互换：两个版本文件直接移到缓存路径并重新读取
        let mut restored = SessionContext::new(base, ArcStr::from("a1-r1"));
        restored.swap_version_with_cache(&suffix).await.unwrap();
        assert_eq!(restored.build().len(), 3, "从版本重新读取（系统 + 两条消息）");
        assert!(dir.path().join("a1-r1.sys.jsonl").exists());
        assert!(dir.path().join("a1-r1.msgs.jsonl").exists());
        assert!(!versions.join(format!("a1-r1-{suffix}.sys.jsonl")).exists(), "互换后该版本文件不存在");
        assert!(!versions.join(format!("a1-r1-{suffix}.msgs.jsonl")).exists(), "互换后该版本文件不存在");
    }

    #[tokio::test]
    async fn swap_treats_sys_only_cache_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap();
        // 先造完整版本（系统旧 + 两条消息）
        let mut first = SessionContext::new(base, ArcStr::from("a1-r1"));
        first.set_system_message(ArcStr::from("旧系统"));
        first.apply_pending_system().await.unwrap();
        first.append(sample_msgs()).await.unwrap();
        first.archive_and_clear().await.unwrap();
        let suffix = only_version_suffix(dir.path(), "a1-r1").await;
        // 当前缓存只有 sys、没有 msgs：按空缓存处理，sys 不保存成新版本
        let mut second = SessionContext::new(base, ArcStr::from("a1-r1"));
        second.set_system_message(ArcStr::from("新系统"));
        second.apply_pending_system().await.unwrap();
        assert!(!dir.path().join("a1-r1.msgs.jsonl").exists());
        second.swap_version_with_cache(&suffix).await.unwrap();
        let ctx = second.build();
        assert_eq!(ctx.len(), 3, "恢复目标版本（系统旧 + 两条消息）");
        assert!(matches!(&ctx[0], Message::System { content } if content.as_str() == "旧系统"));
        assert!(version_suffixes(dir.path(), "a1-r1").await.is_empty(), "没有生成新版本");
        let versions = dir.path().join("versions");
        assert!(!versions.join(format!("a1-r1-{suffix}.sys.jsonl")).exists(), "目标版本已移入缓存");
        assert!(!versions.join(format!("a1-r1-{suffix}.msgs.jsonl")).exists(), "目标版本已移入缓存");
        let cache_sys = fs::read_to_string(dir.path().join("a1-r1.sys.jsonl")).await.unwrap();
        assert!(cache_sys.contains("旧系统") && !cache_sys.contains("新系统"), "只有 sys 的缓存被丢弃");
    }

    #[tokio::test]
    async fn missing_sys_file_is_not_written_and_does_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap();
        // 一直没有系统提示词：缓存没有 sys 文件
        let mut ctx = SessionContext::new(base, ArcStr::from("a1-r1"));
        ctx.append(sample_msgs()).await.unwrap();
        assert!(!dir.path().join("a1-r1.sys.jsonl").exists());
        // 保存版本：版本只有 msgs，没有写空 sys，也不报错
        ctx.archive_and_clear().await.unwrap();
        let suffix = only_version_suffix(dir.path(), "a1-r1").await;
        let versions = dir.path().join("versions");
        assert!(!versions.join(format!("a1-r1-{suffix}.sys.jsonl")).exists(), "版本不写空 sys");
        assert!(versions.join(format!("a1-r1-{suffix}.msgs.jsonl")).exists());
        // 交换版本：缓存没有 sys、版本也没有 sys，不报错、不写空文件
        let mut restored = SessionContext::new(base, ArcStr::from("a1-r1"));
        restored.swap_version_with_cache(&suffix).await.unwrap();
        assert!(!dir.path().join("a1-r1.sys.jsonl").exists(), "交换后仍不写空 sys");
        assert_eq!(restored.build().len(), 2, "只恢复消息");
    }

    #[tokio::test]
    async fn swap_missing_or_invalid_version_saves_current_then_empties() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = SessionContext::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        ctx.set_system_message(ArcStr::from("系统"));
        ctx.apply_pending_system().await.unwrap();
        ctx.append(sample_msgs()).await.unwrap();
        assert!(dir.path().join("a1-r1.sys.jsonl").exists());
        assert!(dir.path().join("a1-r1.msgs.jsonl").exists());
        // 版本不存在：不报错；当前缓存非空，先存成新版本，再变成空上下文
        ctx.swap_version_with_cache("nope").await.unwrap();
        assert!(ctx.build().is_empty(), "空上下文");
        assert!(!dir.path().join("a1-r1.sys.jsonl").exists(), "当前缓存清空");
        assert!(!dir.path().join("a1-r1.msgs.jsonl").exists(), "当前缓存清空");
        let saved = only_version_suffix(dir.path(), "a1-r1").await;
        let versions = dir.path().join("versions");
        let saved_sys = fs::read_to_string(versions.join(format!("a1-r1-{saved}.sys.jsonl"))).await.unwrap();
        let saved_msgs = fs::read_to_string(versions.join(format!("a1-r1-{saved}.msgs.jsonl"))).await.unwrap();
        assert!(saved_sys.contains("系统") && saved_msgs.contains("你好"), "原当前缓存保存为新版本");

        // 非法后缀同样按版本不存在处理（不报错），当前缓存仍先存成新版本
        ctx.set_system_message(ArcStr::from("新系统"));
        ctx.apply_pending_system().await.unwrap();
        ctx.append(vec![Message::User { content: ArcStr::from("新问题") }]).await.unwrap();
        ctx.swap_version_with_cache("a/b").await.unwrap();
        assert!(ctx.build().is_empty(), "空上下文");
        assert!(!dir.path().join("a1-r1.sys.jsonl").exists(), "当前缓存清空");
        assert!(!dir.path().join("a1-r1.msgs.jsonl").exists(), "当前缓存清空");
        assert_eq!(version_suffixes(dir.path(), "a1-r1").await.len(), 2, "两次都保存了新版本");
    }

    // ===== Session 包装 =====

    #[tokio::test]
    async fn session_wrappers_delegate_to_context() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        // set 只写待定：只读构建看不到系统消息
        session.set_system_message(ArcStr::from("系统")).await;
        assert!(session.build_context().await.is_empty(), "待定系统消息未应用");
        // build_context_full 是发送前入口：应用待定并重写 sys 文件
        let ctx = session.build_context_full().await;
        assert_eq!(ctx.len(), 1, "仅有系统消息");
        assert!(matches!(&ctx[0], Message::System { content } if content.as_str() == "系统"));
        assert!(dir.path().join("a1-r1.sys.jsonl").exists(), "应用后 sys 文件存在");
        // 追加消息 → build_context_full 带出 System + 消息
        let _ = session.context_append(vec![Message::User { content: ArcStr::from("你好") }]).await;
        let ctx = session.build_context_full().await;
        assert_eq!(ctx.len(), 2, "system + 消息");
        // 归档 + 重建：内容整体替换，system 保留
        let _ = session.context_archive_and_clear_cache_and_reset_messages(
            vec![Message::User { content: ArcStr::from("新话题") }]).await;
        let ctx = session.build_context_full().await;
        assert_eq!(ctx.len(), 2, "system + 新消息");
        assert!(matches!(&ctx[1], Message::User { content } if content.as_str() == "新话题"));
        let suffix = only_version_suffix(dir.path(), "a1-r1").await;
        assert!(dir.path().join("versions").join(format!("a1-r1-{suffix}.sys.jsonl")).exists(), "版本目录已建立");
        assert!(dir.path().join("versions").join(format!("a1-r1-{suffix}.msgs.jsonl")).exists(), "版本目录已建立");
    }

    #[tokio::test]
    async fn archive_and_reset_keeps_system_and_new_messages() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        session.set_system_message(ArcStr::from("旧系统")).await;
        let _ = session.build_context_full().await;   // 应用旧系统
        let _ = session.context_append(vec![Message::User { content: ArcStr::from("你好") }]).await;
        // 归档旧上下文到 v1（系统旧 + 你好），重建为新话题
        let _ = session.context_archive_and_clear_cache_and_reset_messages(
            vec![Message::User { content: ArcStr::from("新话题") }]).await;
        let v1 = only_version_suffix(dir.path(), "a1-r1").await;
        // 改系统提示词：只 pending，不归档、不动消息
        session.set_system_message(ArcStr::from("新系统")).await;
        assert_eq!(version_suffixes(dir.path(), "a1-r1").await.len(), 1, "改系统提示词不增加版本");
        // pending 尚未应用：只读构建仍是旧系统
        let ctx = session.build_context().await;
        assert_eq!(ctx.len(), 2, "旧系统 + 新话题");
        assert!(matches!(&ctx[0], Message::System { content } if content.as_str() == "旧系统"));
        // 发送前入口应用 pending
        let ctx = session.build_context_full().await;
        assert_eq!(ctx.len(), 2, "新系统 + 新话题");
        assert!(matches!(&ctx[0], Message::System { content } if content.as_str() == "新系统"));
        // 再显式归档 v2（系统新 + 新话题）
        let _ = session.context_archive_and_clear_cache_and_reset_messages(Vec::new()).await;
        let suffixes = version_suffixes(dir.path(), "a1-r1").await;
        assert_eq!(suffixes.len(), 2, "两个版本");
        let v2 = suffixes.iter().find(|s| **s != v1).expect("应有 v2");
        let version_sys = fs::read_to_string(
            dir.path().join("versions").join(format!("a1-r1-{v2}.sys.jsonl"))).await.unwrap();
        assert!(version_sys.contains("新系统"), "v2 保存当前系统提示词");
        let mut count = 0;
        let mut rd = fs::read_dir(dir.path().join("versions")).await.unwrap();
        while rd.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 4, "两个版本各两个文件");
    }

    #[tokio::test]
    async fn context_read_sees_messages_only_and_keeps_context() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_str().unwrap(), ArcStr::from("a1-r1"));
        session.set_system_message(ArcStr::from("系统")).await;
        let _ = session.context_append(sample_msgs()).await;
        // 只读访问：不含系统消息，且不改变上下文
        let count = session.context_read(|msgs| msgs.len()).await;
        assert_eq!(count, 2, "只有消息，不含系统消息");
        assert!(!dir.path().join("versions").exists(), "只读不产生版本");
        assert_eq!(session.build_context().await.len(), 2, "只读构建不应用待定");
        assert_eq!(session.build_context_full().await.len(), 3, "发送前入口应用待定");
    }

    #[tokio::test]
    async fn session_recover_reads_previous_cache() {
        // 新建会话 → 落上下文 → 另一 Session 实例恢复（调用方的重启恢复路径）
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap();
        let first = Session::new(base, ArcStr::from("a1-r1"));
        let _ = first.context_append(sample_msgs()).await;
        let second = Session::new(base, ArcStr::from("a1-r1"));
        second.recover_from_cache().await.unwrap();
        assert_eq!(second.build_context().await.len(), 2, "恢复出上一次上下文");
    }

    #[tokio::test]
    async fn session_swap_version_wrapper_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap();
        let session = Session::new(base, ArcStr::from("a1-r1"));
        let _ = session.context_append(sample_msgs()).await;
        let _ = session.context_archive_and_clear_cache_and_reset_messages(Vec::new()).await;
        let suffix = only_version_suffix(dir.path(), "a1-r1").await;
        assert!(session.build_context().await.is_empty(), "归档并清空后为空");
        session.context_swap_version_with_cache(&suffix).await.unwrap();
        assert_eq!(session.build_context().await.len(), 2, "互换后重新读取版本");
        assert!(!dir.path().join("versions").join(format!("a1-r1-{suffix}.msgs.jsonl")).exists(), "版本已移入缓存");
    }
}
