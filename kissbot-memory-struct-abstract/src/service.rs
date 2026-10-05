//! 服务层：把通知、触发判定、归档、定位、查询与管理串成组件的对外能力。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::{
    CaseCursor, CaseListResponse, CaseMode, CaseNode, CaseRecordsResponse, CaseSummaryResponse,
    ChannelCasePointer, ChannelKey, LocateMessage, LocateRequest, LocateResponse, LocatedGroup,
    RebuildResponse, RecordKind, RecordNotification, RoleEvent, TreeGetResponse,
};
use tracing::{info, warn};

use crate::archive::{ArchiveOutcome, Archiver, PlaceGroup};
use crate::error::{Error, Result};
use crate::locate::{group_indices_by_channel, merge_located};
use crate::model::DecisionModel;
use crate::records::{
    RecordEntry, StoreView, list_store_suffixes, read_case_records, read_store_records,
    records_after, records_tail,
};
use crate::scope::{ScopeHandle, ScopeManager, ScopeState, agent_role, event_case_name};

/// 摘要记忆服务
pub struct AbstractService {
    manager: Arc<ScopeManager>,
    /// 归档器：按通道归档待处理记录，并（可选）一并定位本批新消息
    archiver: Arc<Archiver>,
}

impl AbstractService {
    pub fn new(manager: Arc<ScopeManager>, model: Arc<dyn DecisionModel>, max_batch_records: usize) -> Self {
        Self {
            manager,
            archiver: Arc::new(Archiver::new(model, max_batch_records)),
        }
    }

    #[cfg(test)]
    pub fn manager(&self) -> &Arc<ScopeManager> {
        &self.manager
    }

    /// 启动恢复：加载磁盘上已有角色（启动时按记忆存储里出现过的后缀登记待处理）
    pub async fn restore_scopes(&self) -> Result<Vec<Arc<ScopeHandle>>> {
        self.manager.load_all().await
    }

    /// 打开某记忆后缀所属的角色：case 树按角色共享，输入与状态按后缀区分
    async fn open(&self, agent_id: &str, role_event: &RoleEvent) -> Result<Arc<ScopeHandle>> {
        self.manager.open(agent_role(agent_id, role_event)).await
    }

    // ========== 通知登记与懒处理 ==========

    /// 处理一条新数据通知：只登记「哪个记忆后缀哪个通道有变化」，不做任何计算。
    ///
    /// 通道消息通知携带通道，只标记该通道；思考/工具通知（以及未带通道的通知）无法确定通道，
    /// 标记该记忆后缀的全部通道，查询时按它全部通道处理。
    pub async fn on_notification(&self, kind: RecordKind, notification: &RecordNotification) -> Result<()> {
        let handle = self.open(notification.agent_id.as_str(), &notification.role_event).await?;
        let mut state = handle.state.lock().await;
        match notification.channel.as_ref().filter(|_| kind == RecordKind::Channel) {
            Some(channel) => state.mark_channel(&notification.role_event, channel),
            None => state.mark_role(&notification.role_event),
        }
        Ok(())
    }

    /// 把全部已加载角色重新登记为待处理（通知重连后通道未知）：按记忆存储里出现过的后缀全部通道处理
    pub async fn mark_all_dirty(&self) {
        for handle in self.manager.loaded() {
            let suffixes = match list_store_suffixes(self.manager.root(), &handle.id.agent_id, &handle.id.role).await {
                Ok(suffixes) => suffixes,
                Err(e) => {
                    warn!("[abstract] list store suffixes for {} failed: {}", handle.id, e);
                    continue;
                }
            };
            let mut state = handle.state.lock().await;
            // 磁盘后缀串可直接作待处理标记的键（标记按记忆后缀字符串登记）
            state.dirty_roles.extend(suffixes);
        }
    }

    /// 归档某后缀某通道并把消息分组映射回请求全局下标（`archive` + 日志 + `place_groups` 的封装）。
    ///
    /// `records` 是该通道水位之后的待归档记录（可为空）；`messages` / `indices` 是请求消息
    /// 与其中属于该通道的全局下标（可为空）。只在 `process_pending` 的逐通道循环里调用，
    /// 每个通道一次。
    async fn archive_located(
        &self,
        handle: &Arc<ScopeHandle>,
        state: &mut ScopeState,
        role_event: &RoleEvent,
        channel: &ChannelKey,
        records: &[RecordEntry],
        messages: &[LocateMessage],
        indices: &[usize],
    ) -> Result<(Vec<ArchiveOutcome>, Vec<(usize, ChannelKey, LocatedGroup)>)> {
        let channel_messages: Vec<&LocateMessage> = indices.iter().map(|index| &messages[*index]).collect();
        let result = self.archiver.archive(handle, state, role_event, channel, records, &channel_messages).await?;
        if !records.is_empty() || result.new_messages > 0 {
            let archived: u64 = result.outcomes.iter().map(|outcome| outcome.archived).sum();
            let case_names: Vec<&str> =
                result.outcomes.iter().flat_map(|outcome| outcome.case_names.iter().map(|name| name.as_str())).collect();
            info!(
                "[abstract] process {:?} [{}]: archived {} records into {:?} (+{} new messages)",
                role_event,
                render_channel(channel),
                archived,
                case_names,
                result.new_messages,
            );
        }
        Ok((result.outcomes, place_groups(channel, indices, result.groups)))
    }

    /// 提取该记忆后缀「各通道水位之后的待归档记录」与本批消息，统一按通道分组后逐通道执行「归档 + 定位」。
    ///
    /// - `messages` 为空：即原来的归档（手动触发 / 重建），不产出定位结果；
    /// - `messages` 非空：同一批里 pending 记录落盘归档，只在消息里出现的条目仅定位、不落盘；
    /// - 该后缀没有待处理标记且没有消息时直接返回（不读盘）；否则读一次共享记忆提取各通道 pending。
    ///
    /// 返回（各通道归档结果，消息分组，按请求全局下标）。
    async fn process_pending(
        &self,
        handle: &Arc<ScopeHandle>,
        state: &mut ScopeState,
        role_event: &RoleEvent,
        force: bool,
        messages: &[LocateMessage],
    ) -> Result<(Vec<ArchiveOutcome>, Vec<(usize, ChannelKey, LocatedGroup)>)> {
        if !force && !state.is_dirty(role_event) && messages.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        handle.ensure_suffix(state, role_event).await?;

        let view = self.store_view(handle, role_event).await?;
        // 统一按通道分组：本批消息所在的通道 ∪ 存储里出现过的通道（pending / 消息都可以为空）
        let mut grouped: BTreeMap<Arc<ChannelKey>, Vec<usize>> = group_indices_by_channel(messages).into_iter().collect();
        for channel in view.known_channels() {
            grouped.entry(channel).or_default();
        }
        let mut processed: BTreeSet<Arc<ChannelKey>> = state.dirty_channels_of(role_event);
        let mut outcomes = Vec::new();
        let mut located = Vec::new();
        for (channel, indices) in grouped {
            processed.insert(channel.clone());
            let watermark = state.watermark(role_event, &channel);
            let pending = view.pending_for(&channel, &watermark);
            if pending.is_empty() && indices.is_empty() {
                continue;
            }
            let (channel_outcomes, channel_groups) =
                self.archive_located(handle, state, role_event, &channel, &pending, messages, &indices).await?;
            outcomes.extend(channel_outcomes);
            located.extend(channel_groups);
        }
        // 全部通道处理成功后才清除该后缀的待处理标记（失败时保留，下次查询重试）
        state.clear_dirty(role_event, &processed);
        Ok((outcomes, located))
    }

    /// 手动触发：忽略待处理标记，把该记忆后缀各通道水位之后的记录全部处理一次
    pub async fn manual_trigger(&self, agent_id: &str, role_event: &RoleEvent) -> Result<ArchiveOutcome> {
        let handle = self.open(agent_id, role_event).await?;
        let mut state = handle.state.lock().await;
        let (outcomes, _) = self.process_pending(&handle, &mut state, role_event, true, &[]).await?;
        Ok(sum_outcomes(outcomes))
    }

    /// 重建：只清空本次记忆后缀自己的 case 与该后缀的指针/水位，再按共享记忆文件重新构建。
    ///
    /// 角色模式清角色子树（该角色的全部 `role-*` case）；事件模式只清本后缀固定的
    /// `event-{事件id}`——同一角色的其他事件后缀与角色子树都不受影响。
    pub async fn rebuild(&self, agent_id: &str, role_event: &RoleEvent) -> Result<RebuildResponse> {
        let handle = self.open(agent_id, role_event).await?;
        let mut state = handle.state.lock().await;
        handle.ensure_suffix(&mut state, role_event).await?;
        let cleared: BTreeSet<ArcStr> = match event_case_name(role_event) {
            Some(case_name) if state.tree.contains(&case_name) => [case_name].into_iter().collect(),
            Some(_) => BTreeSet::new(),
            None => state.tree.ids_of_mode(CaseMode::Role),
        };
        for case_name in &cleared {
            handle.reset_case_records(case_name).await?;
        }
        state.tree.remove_nodes(&cleared);
        state.clear_role_event(role_event);
        handle.reset_role_event_state(role_event).await?;
        state.mark_role(role_event);
        let (outcomes, _) = self.process_pending(&handle, &mut state, role_event, true, &[]).await?;
        let archived = outcomes.iter().map(|outcome| outcome.archived).sum();
        let case_count = state.tree.len() as u64;
        drop(state);
        info!("[abstract] rebuild {}/{:?}: {} records, {} cases", agent_id, role_event, archived, case_count);
        Ok(RebuildResponse { archived, case_count })
    }

    /// 该记忆后缀的共享记忆文件全量视图（记录 + 通道归属）；水位过滤按通道进行
    pub async fn store_view(&self, handle: &ScopeHandle, role_event: &RoleEvent) -> Result<StoreView> {
        read_store_records(self.manager.root(), &handle.id.agent_id, &kissbot_memory::role_event_suffix(role_event)).await
    }

    // ========== 定位 ==========

    /// 定位一批消息涉及的 case：提取该后缀各通道的待归档记录与本批消息（两者都可为空），
    /// 统一按通道分组后逐通道执行「归档 + 定位」——记录落盘、消息只定位。
    pub async fn locate(&self, request: &LocateRequest) -> Result<LocateResponse> {
        let role_event = &request.role_event;
        let handle = self.open(request.agent_id.as_str(), role_event).await?;
        let mut state = handle.state.lock().await;
        let (_, located) = self.process_pending(&handle, &mut state, role_event, false, &request.messages).await?;
        Ok(merge_located(located, &state.current_of(role_event)))
    }

    // ========== 查询 ==========

    /// Case 列表：只读角色的 case 树（角色模式与事件模式的 case 都在一棵树上），
    /// 不触发归档（要处理待归档记录请用 `/abstract/rebuild` 的 rebuild=false）
    pub async fn case_list(&self, agent_id: &str, role_event: &RoleEvent, parent_id: Option<&str>) -> Result<CaseListResponse> {
        let handle = self.open(agent_id, role_event).await?;
        let state = handle.state.lock().await;
        let cases: Vec<Arc<CaseNode>> = state
            .tree
            .nodes()
            .into_iter()
            .filter(|node| match parent_id {
                Some(parent) => node.parent_id.as_ref().map(|p| p.as_str()) == Some(parent),
                None => node.parent_id.is_none(),
            })
            .cloned()
            .map(Arc::new)
            .collect();
        Ok(CaseListResponse { cases })
    }

    /// Case 摘要：节点本身（摘要文本 + 层级 + 记录数 + 起止时间），不含原文副本；只读，不触发归档
    pub async fn case_summary(&self, agent_id: &str, role_event: &RoleEvent, case_name: &str) -> Result<CaseSummaryResponse> {
        let handle = self.open(agent_id, role_event).await?;
        let state = handle.state.lock().await;
        let node = state
            .tree
            .get(case_name)
            .cloned()
            .ok_or_else(|| Error::StateError(format!("case 不存在: {}/{}", handle.id, case_name).into()))?;
        Ok(CaseSummaryResponse { case: Arc::new(node) })
    }

    /// Case 全部原文副本
    pub async fn case_records(&self, agent_id: &str, role_event: &RoleEvent, case_name: &str) -> Result<CaseRecordsResponse> {
        Ok(response_of(self.read_case(agent_id, role_event, case_name).await?))
    }

    /// Case 中游标之后的原文副本（分页续读）
    pub async fn case_records_after(
        &self,
        agent_id: &str,
        role_event: &RoleEvent,
        case_name: &str,
        after: &CaseCursor,
    ) -> Result<CaseRecordsResponse> {
        let records = records_after(&self.read_case(agent_id, role_event, case_name).await?, after);
        Ok(response_of(records))
    }

    /// Case 结尾若干条原文副本
    pub async fn case_records_tail(
        &self,
        agent_id: &str,
        role_event: &RoleEvent,
        case_name: &str,
        count: u64,
    ) -> Result<CaseRecordsResponse> {
        let records = records_tail(&self.read_case(agent_id, role_event, case_name).await?, count);
        Ok(response_of(records))
    }

    /// 读取 case 原文副本（case 不存在时报错，与摘要查询一致）；只读，不触发归档
    async fn read_case(&self, agent_id: &str, role_event: &RoleEvent, case_name: &str) -> Result<Vec<kissbot_api::CaseRecord>> {
        let handle = self.open(agent_id, role_event).await?;
        {
            let state = handle.state.lock().await;
            if !state.tree.contains(case_name) {
                return Err(Error::StateError(format!("case 不存在: {}/{}", handle.id, case_name).into()));
            }
        }
        read_case_records(&handle.case_dir(case_name)).await
    }

    /// Case 树与该记忆后缀各通道的当前 case 指针；只读，不触发归档。
    ///
    /// 当前指针按记忆后缀落盘，首次读取该后缀时先从磁盘加载一次（只读，不写盘）。
    pub async fn tree_get(&self, agent_id: &str, role_event: &RoleEvent) -> Result<TreeGetResponse> {
        let handle = self.open(agent_id, role_event).await?;
        let mut state = handle.state.lock().await;
        handle.ensure_suffix(&mut state, role_event).await?;
        let current_cases = state
            .current_of(role_event)
            .iter()
            .map(|(channel, case_name)| {
                Arc::new(ChannelCasePointer {
                    channel: channel.as_ref().clone(),
                    case_name: case_name.clone(),
                })
            })
            .collect();
        Ok(TreeGetResponse {
            current_cases,
            nodes: state.tree.nodes().into_iter().cloned().map(Arc::new).collect(),
        })
    }

    /// 角色 case 树节点数（重建响应用；排障可读 case 树，水位与指针见磁盘状态文件）
    pub async fn case_count(&self, agent_id: &str, role_event: &RoleEvent) -> Result<u64> {
        let handle = self.open(agent_id, role_event).await?;
        Ok(handle.state.lock().await.tree.len() as u64)
    }

    /// 存储根目录（测试用）
    #[cfg(test)]
    pub fn root(&self) -> std::path::PathBuf {
        self.manager.root().to_path_buf()
    }
}

/// 汇总多批处理结果
fn sum_outcomes(outcomes: Vec<ArchiveOutcome>) -> ArchiveOutcome {
    ArchiveOutcome {
        archived: outcomes.iter().map(|outcome| outcome.archived).sum(),
        case_names: outcomes.into_iter().flat_map(|outcome| outcome.case_names).collect(),
    }
}

/// 把某通道的分组结果（`PlaceGroup::messages` 是传入消息切片的下标）映射为请求全局下标；
/// 只含待归档记录的组不出现在定位结果里。
fn place_groups(channel: &ChannelKey, indices: &[usize], groups: Vec<PlaceGroup>) -> Vec<(usize, ChannelKey, LocatedGroup)> {
    groups
        .into_iter()
        .filter_map(|group| {
            let messages: Vec<usize> = group.messages.iter().map(|local| indices[*local]).collect();
            let first = *messages.first()?;
            Some((
                first,
                channel.clone(),
                LocatedGroup { messages, case_name: group.case_name, is_current: false },
            ))
        })
        .collect()
}

/// 原文副本响应（统一装配成 Arc）
fn response_of(records: Vec<kissbot_api::CaseRecord>) -> CaseRecordsResponse {
    CaseRecordsResponse { records: records.into_iter().map(Arc::new).collect() }
}

/// 通道的可读文本（日志用）
fn render_channel(channel: &ChannelKey) -> String {
    format!("{}/{}/{}", channel.messenger_id, channel.self_user_id, channel.group_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LevelDecision;
    use crate::scope::AgentRole;
    use crate::stub::StubModel;
    use kissbot_api::{ChannelCasePointer, Content, CaseMode, CaseRecord, LocateMessage, TreeGetResponse};
    use std::path::Path;

    /// 记忆后缀所属的角色（case 树按角色共享）
    fn agent_role(role_event: &RoleEvent) -> AgentRole {
        crate::scope::agent_role("0", role_event)
    }

    fn role_event(role_name: &str, event_id: Option<&str>) -> RoleEvent {
        RoleEvent {
            role_name: ArcStr::from(role_name),
            event_id: event_id.map(ArcStr::from),
        }
    }

    /// 测试用通道 A：identity 与 append_store_channel 写入的记录一致
    fn channel_a() -> ChannelKey {
        ChannelKey::new("web", "selfA", "gA")
    }

    /// 测试用通道 B：同一作用域下的另一个通道
    fn channel_b() -> ChannelKey {
        ChannelKey::new("web", "selfB", "gB")
    }

    /// 往共享记忆文件追加一行记录（模拟 memory-store 的落盘）
    async fn append_store_line(root: &Path, agent_id: &str, suffix: &str, stem: &str, record: serde_json::Value, time: &str) {
        let date = &time[..10];
        let year = &time[..4];
        let dir = root.join(agent_id).join("memory-store").join(format!("{}-{}", year, suffix));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{}-records-{}.jsonl", stem, date)))
            .await
            .unwrap();
        file.write_all(serde_json::to_string(&record).unwrap().as_bytes()).await.unwrap();
        file.write_all(b"\n").await.unwrap();
        // tokio::fs::File 的 write_all 可能只入队后台写入：flush 后随后的读取才能看到该记录
        file.flush().await.unwrap();
    }

    /// 追加一条通道记录：身份取通道三元组，self 记录的发送者即该通道绑定的用户
    async fn append_store_channel(root: &Path, agent_id: &str, suffix: &str, channel: &ChannelKey, time: &str, sn: u64, is_self: usize, text: &str) {
        append_store_channel_with_id(root, agent_id, suffix, channel, time, sn, is_self, text, "").await;
    }

    /// 追加一条带 msg_id 的通道记录（消息与记录按「通道三元组 + msg_id」去重）
    async fn append_store_channel_with_id(
        root: &Path,
        agent_id: &str,
        suffix: &str,
        channel: &ChannelKey,
        time: &str,
        sn: u64,
        is_self: usize,
        text: &str,
        msg_id: &str,
    ) {
        let record = serde_json::json!({
            "msg_id": msg_id,
            "user_id": if is_self == 1 { channel.self_user_id.as_str() } else { "u1" },
            "self_user_id": channel.self_user_id.as_str(),
            "messenger_id": channel.messenger_id.as_str(),
            "group_id": channel.group_id.as_str(),
            "is_self": is_self,
            "messenger_name": "",
            "user_name": "alice",
            "group_name": "",
            "content": {"msg_type": "Text", "data": text},
            "time": time,
            "sn": sn
        });
        append_store_line(root, agent_id, suffix, "channel", record, time).await;
    }

    /// 追加一条思考记录：先写占位通道记录（内容为该 key），再写思考记录本体
    async fn append_store_think(root: &Path, agent_id: &str, suffix: &str, channel: &ChannelKey, key: &str, time: &str, sn: u64, text: &str) {
        let placeholder = serde_json::json!({
            "user_id": channel.self_user_id.as_str(),
            "self_user_id": channel.self_user_id.as_str(),
            "messenger_id": channel.messenger_id.as_str(),
            "group_id": channel.group_id.as_str(),
            "is_self": 1,
            "messenger_name": "",
            "user_name": "",
            "group_name": "",
            "content": {"msg_type": "Think", "data": key},
            "time": time,
            "sn": sn
        });
        append_store_line(root, agent_id, suffix, "channel", placeholder, time).await;
        let think = serde_json::json!({
            "reasoning_content": text,
            "thinking": null,
            "key": key,
            "time": time,
            "sn": sn
        });
        append_store_line(root, agent_id, suffix, "think", think, time).await;
    }

    /// 追加一条没有占位通道记录的思考记录（key 无人认领）
    async fn append_store_orphan_think(root: &Path, agent_id: &str, suffix: &str, time: &str, sn: u64, key: &str) {
        let think = serde_json::json!({
            "reasoning_content": "无占位记录",
            "thinking": null,
            "key": key,
            "time": time,
            "sn": sn
        });
        append_store_line(root, agent_id, suffix, "think", think, time).await;
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        service: AbstractService,
        model: Arc<StubModel>,
    }

    async fn fixture() -> Fixture {
        fixture_with_batch(200).await
    }

    async fn fixture_with_batch(max_batch_records: usize) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let manager = Arc::new(ScopeManager::new(root));
        let model = Arc::new(StubModel::new());
        let service = AbstractService::new(manager, model.clone(), max_batch_records);
        Fixture { _temp: temp, service, model }
    }

    /// 通道消息通知（懒处理：只登记，不计算）
    fn channel_notification(role_event: Arc<RoleEvent>, channel: &ChannelKey, is_self: usize) -> RecordNotification {
        RecordNotification {
            agent_id: ArcStr::from("0"),
            role_event: role_event.clone(),
            time: ArcStr::from("2026-09-22 00:00:00"),
            is_self: Some(is_self),
            channel: Some(channel.clone()),
        }
    }

    /// 登记某通道有新数据
    async fn notify(f: &Fixture, role_event: Arc<RoleEvent>, channel: &ChannelKey) {
        f.service
            .on_notification(RecordKind::Channel, &channel_notification(role_event, channel, 0))
            .await
            .unwrap();
    }

    /// 登记整个作用域有新数据（思考/工具通知的形态）
    async fn notify_scope(f: &Fixture, role_event: Arc<RoleEvent>) {
        f.service
            .on_notification(RecordKind::Think, &RecordNotification {
                agent_id: ArcStr::from("0"),
                role_event: role_event.clone(),
                time: ArcStr::from("2026-09-22 00:00:00"),
                is_self: None,
                channel: None,
            })
            .await
            .unwrap();
    }

    /// 手动归档一次（`/abstract/rebuild` 的 rebuild=false 路径）
    async fn process(f: &Fixture, role_event: &RoleEvent) -> ArchiveOutcome {
        f.service.manual_trigger("0", role_event).await.unwrap()
    }

    /// 手动归档一次后读 case 树（查询接口本身只读、不触发归档）
    async fn query(f: &Fixture, role_event: &RoleEvent) -> TreeGetResponse {
        process(f, role_event).await;
        f.service.tree_get("0", role_event).await.unwrap()
    }

    /// 作用域内存中的 case 节点数（不触发懒处理）
    async fn tree_len(f: &Fixture, role_event: &RoleEvent) -> usize {
        let handle = f.service.manager().get(&agent_role(role_event)).expect("作用域已加载");
        handle.state.lock().await.tree.len()
    }

    /// 作用域是否还有待处理标记
    async fn is_dirty(f: &Fixture, role_event: &RoleEvent) -> bool {
        let handle = f.service.manager().get(&agent_role(role_event)).expect("作用域已加载");
        handle.state.lock().await.is_dirty(role_event)
    }

    /// 某通道的水位（测试直接读内存状态）：(时间, sn)；无水位时 sn 为 0
    async fn watermark_point(f: &Fixture, role_event: &RoleEvent, channel: &ChannelKey) -> (String, u64) {
        let handle = f.service.manager().get(&agent_role(role_event)).expect("作用域已加载");
        let state = handle.state.lock().await;
        let watermark = state.watermark(role_event, channel);
        (watermark.channel.time.as_str().to_string(), watermark.channel.sn)
    }

    /// 某通道待处理记录条数（水位之后、归属该通道）
    async fn pending_len(f: &Fixture, role_event: &RoleEvent, channel: &ChannelKey) -> usize {
        let handle = f.service.manager().get(&agent_role(role_event)).expect("作用域已加载");
        let watermark = {
            let state = handle.state.lock().await;
            state.watermark(role_event, channel)
        };
        f.service.store_view(&handle, role_event).await.unwrap().pending_for(channel, &watermark).len()
    }

    /// 某通道的当前 case id
    async fn current_case(f: &Fixture, role_event: &RoleEvent, channel: &ChannelKey) -> Option<ArcStr> {
        let handle = f.service.manager().get(&agent_role(role_event)).expect("作用域已加载");
        handle.state.lock().await.current_case(role_event, channel).cloned()
    }

    /// case 节点
    async fn node(f: &Fixture, role_event: &RoleEvent, case_name: &str) -> Option<Arc<CaseNode>> {
        query(f, role_event).await.nodes.into_iter().find(|n| n.case_name.as_str() == case_name)
    }

    /// case 原文副本
    async fn records_of(f: &Fixture, role_event: &RoleEvent, case_name: &str) -> Vec<Arc<CaseRecord>> {
        f.service.case_records("0", role_event, case_name).await.unwrap().records
    }

    /// 定位用消息（指定通道；msg_id 为空表示不参与与已存记录的去重）
    fn locate_message(channel: &ChannelKey, text: &str) -> LocateMessage {
        locate_message_at(channel, "2099-01-01 12:00:00", 0, text, "")
    }

    /// 定位结果中的 case id
    fn cases_of(response: &LocateResponse) -> Vec<&str> {
        response.cases.iter().map(|event| event.case_name.as_str()).collect()
    }

    /// 当前 case 指针表
    fn current_pointer<'a>(tree: &'a TreeGetResponse, channel: &ChannelKey) -> Option<&'a Arc<ChannelCasePointer>> {
        tree.current_cases.iter().find(|pointer| &pointer.channel == channel)
    }

    #[tokio::test]
    async fn notification_only_marks_dirty_until_manual_trigger() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "答").await;

        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        assert!(is_dirty(&f, &role_event("dev", None)).await, "通知只登记待处理标记");
        assert_eq!(tree_len(&f, &role_event("dev", None)).await, 0, "登记本身不做任何计算");
        assert_eq!(f.model.prompts().len(), 0, "登记不调用模型");

        // 查询只读：不触发归档、不清标记
        let tree = f.service.tree_get("0", &role_event("dev", None)).await.unwrap();
        assert_eq!(tree.nodes.len(), 0, "查询不触发归档");
        assert_eq!(f.service.case_count("0", &role_event("dev", None)).await.unwrap(), 0);
        assert!(is_dirty(&f, &role_event("dev", None)).await, "查询不清除待处理标记");
        assert_eq!(f.model.prompts().len(), 0);
        assert!(f.service.case_list("0", &role_event("dev", None), None).await.unwrap().cases.is_empty());
        assert!(f.service.case_summary("0", &role_event("dev", None), "role-case").await.is_err(), "case 还没归档出来");

        // 手动触发才统一计算
        let outcome = process(&f, &role_event("dev", None)).await;
        assert_eq!(outcome.archived, 2, "手动触发处理该作用域水位之后的记录");
        let tree = f.service.tree_get("0", &role_event("dev", None)).await.unwrap();
        assert_eq!(tree.nodes.len(), 1, "手动触发归档出 case");
        assert_eq!(tree.nodes[0].record_count, 2, "该通道水位之后的全部记录一起处理");
        assert_eq!(tree.nodes[0].summary.as_str(), "新 case 摘要", "新建 case 建立当次就带上模型给出的摘要");
        assert_eq!(current_pointer(&tree, &a).unwrap().case_name.as_str(), "role-case");
        assert!(!is_dirty(&f, &role_event("dev", None)).await, "处理成功后标记清空");
        assert_eq!(watermark_point(&f, &role_event("dev", None), &a).await.1, 2, "水位推进到该通道最后一条记录");
    }

    /// 重启后 tree/get 仍能读到落盘的当前 case 指针（该后缀的状态文件在首次读取时懒加载）
    #[tokio::test]
    async fn tree_get_loads_persisted_pointer_after_restart() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        process(&f, &role_event("dev", None)).await;

        // 重启：同一根目录上新建 manager 与 service（内存态为空，只剩磁盘状态）
        let restarted = AbstractService::new(Arc::new(ScopeManager::new(root)), Arc::new(StubModel::new()), 200);
        let tree = restarted.tree_get("0", &role_event("dev", None)).await.unwrap();
        assert_eq!(tree.nodes.len(), 1, "case 树来自磁盘");
        assert_eq!(current_pointer(&tree, &a).unwrap().case_name.as_str(), "role-case", "当前 case 指针来自磁盘");
        assert_eq!(restarted.case_count("0", &role_event("dev", None)).await.unwrap(), 1);

        // 同一角色的事件后缀：共用磁盘上的同一棵树，但有自己的（空的）指针状态
        let event_tree = restarted.tree_get("0", &role_event("dev", Some("e1"))).await.unwrap();
        assert_eq!(event_tree.nodes.len(), 1, "同一个角色共用一棵 case 树");
        assert!(current_pointer(&event_tree, &a).is_none(), "事件后缀的指针状态独立");
    }

    #[tokio::test]
    async fn manual_trigger_without_changes_does_nothing() {        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        query(&f, &role_event("dev", None)).await;

        let prompts = f.model.prompts().len();
        let outcome = process(&f, &role_event("dev", None)).await;
        assert_eq!(outcome.archived, 0, "没有新变化时不再重复计算");
        let tree = f.service.tree_get("0", &role_event("dev", None)).await.unwrap();
        assert_eq!(tree.nodes.len(), 1, "case 树保持");
        assert_eq!(f.model.prompts().len(), prompts, "没有新变化时不调用模型");
    }

    #[tokio::test]
    async fn lazy_flush_processes_all_channels_separately() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "A 问").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "A 答").await;
        append_store_channel(&root, "0", "dev", &b, "2099-01-01 11:00:00", 3, 0, "B 问").await;
        // 只登记 A：查询时按作用域把全部通道各算一次
        notify(&f, Arc::new(role_event("dev", None)), &a).await;

        let tree = query(&f, &role_event("dev", None)).await;
        assert_eq!(tree.nodes.len(), 2, "两个通道各自成批、各得一个 case");
        assert_eq!(node(&f, &role_event("dev", None), "role-case").await.unwrap().record_count, 2);
        assert_eq!(node(&f, &role_event("dev", None), "role-case-2").await.unwrap().record_count, 1);
        assert_eq!(current_pointer(&tree, &a).unwrap().case_name.as_str(), "role-case");
        assert_eq!(current_pointer(&tree, &b).unwrap().case_name.as_str(), "role-case-2");
        assert_eq!(watermark_point(&f, &role_event("dev", None), &a).await.1, 2);
        assert_eq!(watermark_point(&f, &role_event("dev", None), &b).await.1, 3);
        assert_eq!(pending_len(&f, &role_event("dev", None), &a).await, 0);
        assert_eq!(pending_len(&f, &role_event("dev", None), &b).await, 0);
    }

    #[tokio::test]
    async fn scope_level_mark_processes_all_channels() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "A 的消息").await;
        append_store_channel(&root, "0", "dev", &b, "2099-01-01 11:00:00", 2, 0, "B 的消息").await;
        // 思考/工具通知无法确定通道 → 标记整个作用域
        notify_scope(&f, Arc::new(role_event("dev", None))).await;

        let tree = query(&f, &role_event("dev", None)).await;
        assert_eq!(tree.nodes.len(), 2, "作用域级标记会处理全部通道");
        assert!(current_pointer(&tree, &a).is_some());
        assert!(current_pointer(&tree, &b).is_some());
    }

    #[tokio::test]
    async fn current_group_merges_directly_and_other_group_descends() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        // 先归档出 e1，并把它变成 e1（父）/ e2（子）的结构，当前 case 指向父 case e1
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "旧内容").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        query(&f, &role_event("dev", None)).await;
        {
            let handle = f.service.manager().get(&agent_role(&role_event("dev", None))).unwrap();
            let mut state = handle.state.lock().await;
            state.tree.create_node("role-2", Some("role-case"), CaseMode::Role, "t");
            state.current_cases.insert((role_event("dev", None), a.clone()), "role-case".into());
        }

        // 第 1 轮分组：消息 0 属于当前 case（直接并入父 case case），消息 1、2 属于新 case（分层归并）
        f.model.push_groups(vec![
            crate::model::MessageGroup { messages: vec![0], is_current: true },
            crate::model::MessageGroup { messages: vec![1, 2], is_current: false },
        ]);
        f.model.push_level(LevelDecision::Merge("role-case".into()));
        f.model.push_level(LevelDecision::new_case("child", "子摘要"));
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:00:00", 2, 1, "当前话题的延续").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:01:00", 3, 0, "新话题").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:02:00", 4, 1, "新话题的回应").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        query(&f, &role_event("dev", None)).await;

        let parent = node(&f, &role_event("dev", None), "role-case").await.unwrap();
        assert_eq!(parent.record_count, 2, "当前 case 组直接并入父 case 本身（不下潜）");
        assert_eq!(parent.children.len(), 2, "其他组经分层归并：Merge(case) 下潜后 New → case 下新建叶子");
        let child = node(&f, &role_event("dev", None), "role-child").await.unwrap();
        assert_eq!(child.parent_id.as_ref().map(|p| p.as_str()), Some("role-case"));
        assert_eq!(child.record_count, 2);
        assert_eq!(current_case(&f, &role_event("dev", None), &a).await.as_deref(), Some("role-child"), "指针指向最后一批消息落入的 case");
    }

    #[tokio::test]
    async fn groups_become_separate_cases() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "话题一").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "话题一回应").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:02:00", 3, 0, "话题二").await;
        f.model.push_groups(vec![
            crate::model::MessageGroup { messages: vec![0, 1], is_current: false },
            crate::model::MessageGroup { messages: vec![2], is_current: false },
        ]);
        notify(&f, Arc::new(role_event("dev", None)), &a).await;

        let tree = query(&f, &role_event("dev", None)).await;
        assert_eq!(tree.nodes.len(), 2, "一批消息按分组落成两个 case");
        assert_eq!(node(&f, &role_event("dev", None), "role-case").await.unwrap().record_count, 2);
        assert_eq!(node(&f, &role_event("dev", None), "role-case-2").await.unwrap().record_count, 1);
        assert_eq!(watermark_point(&f, &role_event("dev", None), &a).await.1, 3, "水位覆盖整批");
        assert_eq!(pending_len(&f, &role_event("dev", None), &a).await, 0);
    }

    #[tokio::test]
    async fn current_group_without_current_case_fails_and_keeps_marks() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问").await;
        // 模型错误地把分组标记为当前 case，但该通道还没有当前 case → 按失败处理
        f.model.push_group(&[0], true);
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        assert!(f.service.manual_trigger("0", &role_event("dev", None)).await.is_err(), "非法分组按失败处理");
        assert!(is_dirty(&f, &role_event("dev", None)).await, "失败后保留待处理标记，等待下次处理重试");
        assert_eq!(pending_len(&f, &role_event("dev", None), &a).await, 1, "水位不动");

        // 重新给出合法分组 → 重试成功且记录不重复
        f.model.push_group(&[0], false);
        let tree = query(&f, &role_event("dev", None)).await;
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.nodes[0].record_count, 1);
    }

    #[tokio::test]
    async fn model_failure_keeps_state_and_retries_cleanly() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 1, "回答").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;

        // 分组调用失败（桩模型的输出没有覆盖整批）：归档整体失败，状态与磁盘保持不变
        f.model.push_groups(Vec::new());
        assert!(f.service.manual_trigger("0", &role_event("dev", None)).await.is_err(), "模型失败应向上报错");
        let role = role_event("dev", None);
        let handle = f.service.manager().get(&agent_role(&role)).unwrap();
        {
            let state = handle.state.lock().await;
            assert!(state.tree.is_empty(), "失败不留下空 case");
            assert!(state.watermarks.get(&(role.clone(), a.clone())).is_none(), "失败不推进水位");
            assert!(state.current_cases.get(&(role, a.clone())).is_none());
        }
        assert!(!handle.case_dir("role-case").exists(), "失败不落盘原文副本");
        assert!(is_dirty(&f, &role_event("dev", None)).await, "失败保留待处理标记");

        // 再次查询（模型恢复）：批次被完整归档一次，无重复
        let tree = query(&f, &role_event("dev", None)).await;
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.nodes[0].record_count, 1, "重试后记录不重复");
        assert_eq!(watermark_point(&f, &role_event("dev", None), &a).await.1, 1);
    }

    /// 摘要调用失败（事件模式有记录 → 第 3 步按记录生成摘要）：状态回滚且可干净重试
    #[tokio::test]
    async fn summarize_failure_keeps_state_and_retries_cleanly() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev-e1", &a, "2099-01-01 10:00:00", 1, 0, "内容").await;
        notify(&f, Arc::new(role_event("dev", Some("e1"))), &a).await;

        f.model.push_summarize_failure();
        assert!(f.service.manual_trigger("0", &role_event("dev", Some("e1"))).await.is_err(), "摘要失败应向上报错");
        let role = role_event("dev", Some("e1"));
        let handle = f.service.manager().get(&agent_role(&role)).unwrap();
        {
            let state = handle.state.lock().await;
            assert!(state.tree.is_empty(), "失败不留下空 case");
            assert!(state.watermarks.get(&(role, a.clone())).is_none(), "失败不推进水位");
        }
        assert!(!handle.case_dir("event-e1").exists(), "失败不落盘原文副本");
        assert!(is_dirty(&f, &role_event("dev", Some("e1"))).await, "失败保留待处理标记");

        let tree = query(&f, &role_event("dev", Some("e1"))).await;
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.nodes[0].record_count, 1, "重试后记录不重复");
        assert!(!tree.nodes[0].summary.is_empty(), "重试成功后有摘要");
    }

    /// 事件模式的 case 名是 `event-{事件标识}`：标识含非法字符时按状态错误处理，不建 case、不落原文副本
    #[tokio::test]
    async fn invalid_event_id_is_rejected_without_creating_case() {
        // 非法事件标识只能显式构造 RoleEvent（后缀字符串仅用于磁盘路径），不再由后缀反推
        for (suffix, role_event) in [
            ("dev-e_1", role_event("dev", Some("e_1"))),
            ("dev-事件", role_event("dev", Some("事件"))),
        ] {
            let f = fixture().await;
            let root = f.service.root();
            let a = channel_a();
            append_store_channel(&root, "0", suffix, &a, "2099-01-01 10:00:00", 1, 0, "内容").await;
            notify(&f, Arc::new(role_event.clone()), &a).await;

            let result = f.service.manual_trigger("0", &role_event).await;
            assert!(result.is_err(), "非法事件标识 {} 应报错", suffix);

            // dev-e_1 与 dev-事件 都属于同一个角色 dev：共享的 case 树上不应有节点
            let handle = f.service.manager().get(&agent_role(&role_event)).unwrap();
            assert!(handle.state.lock().await.tree.is_empty(), "失败不建出 case ({})", suffix);
            let case_name = event_case_name(&role_event).unwrap();
            assert!(!handle.case_dir(&case_name).exists(), "失败不落原文副本 ({})", suffix);
        }
    }

    #[tokio::test]
    async fn batch_is_split_by_max_batch_records() {
        let f = fixture_with_batch(2).await;
        let root = f.service.root();
        let a = channel_a();
        for i in 1..=5u64 {
            append_store_channel(&root, "0", "dev", &a, &format!("2099-01-01 10:0{}:00", i), i, (i % 2) as usize, "内容").await;
        }
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        let tree = query(&f, &role_event("dev", None)).await;

        let group_calls = f.model.prompts().iter().filter(|prompt| prompt.contains("\"groups\"")).count();
        assert_eq!(group_calls, 3, "5 条按每块 2 条分成 3 块，每块一次分组");
        assert_eq!(tree.nodes.len(), 1, "块的落位由桩模型决定（命中当前 case）");
        assert_eq!(tree.nodes[0].record_count, 5, "全部记录都被处理");
        assert_eq!(watermark_point(&f, &role_event("dev", None), &a).await.1, 5);
        assert_eq!(pending_len(&f, &role_event("dev", None), &a).await, 0);
    }

    #[tokio::test]
    async fn think_records_follow_their_channel_by_key() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问").await;
        append_store_think(&root, "0", "dev", &a, "keyA", "2099-01-01 10:01:00", 2, "A 的思考").await;
        append_store_channel(&root, "0", "dev", &b, "2099-01-01 11:00:00", 3, 0, "另一个通道").await;
        append_store_think(&root, "0", "dev", &b, "keyB", "2099-01-01 11:01:00", 4, "B 的思考").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;

        let tree = query(&f, &role_event("dev", None)).await;
        assert_eq!(tree.nodes.len(), 2, "两个通道各自成批");
        let think_keys = |records: Vec<Arc<CaseRecord>>| -> Vec<String> {
            records
                .iter()
                .filter_map(|r| match r.as_ref() {
                    CaseRecord::Think(t) => Some(t.key.as_str().to_string()),
                    _ => None,
                })
                .collect()
        };
        let mut all = vec![
            think_keys(records_of(&f, &role_event("dev", None), "role-case").await),
            think_keys(records_of(&f, &role_event("dev", None), "role-case-2").await),
        ];
        all.sort();
        assert_eq!(all, vec![vec!["keyA".to_string()], vec!["keyB".to_string()]]);
    }

    #[tokio::test]
    async fn unclaimed_think_records_stay_out_of_channel_batches() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问一").await;
        append_store_orphan_think(&root, "0", "dev", "2099-01-01 10:01:00", 2, "孤儿").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:02:00", 3, 1, "答").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        query(&f, &role_event("dev", None)).await;

        assert_eq!(pending_len(&f, &role_event("dev", None), &a).await, 0, "该通道记录已处理完");
        let records = records_of(&f, &role_event("dev", None), "role-case").await;
        assert_eq!(records.len(), 2, "孤儿思考记录不属于任何通道，不进 case");
        assert!(!records.iter().any(|r| matches!(r.as_ref(), CaseRecord::Think(_))));
    }

    /// 定位用消息（指定通道、时间、is_self）
    fn locate_message_at(channel: &ChannelKey, time: &str, is_self: usize, text: &str, msg_id: &str) -> LocateMessage {
        LocateMessage {
            msg_id: ArcStr::from(msg_id),
            time: ArcStr::from(time),
            user_name: ArcStr::from("alice"),
            is_self,
            messenger_id: channel.messenger_id.clone(),
            self_user_id: channel.self_user_id.clone(),
            group_id: channel.group_id.clone(),
            content: Content::Text(ArcStr::from(text)),
        }
    }

    /// 定位请求
    fn locate_request(role_event: &RoleEvent, messages: Vec<LocateMessage>) -> LocateRequest {
        LocateRequest { agent_id: ArcStr::from("0"), role_event: role_event.clone(), messages }
    }

    /// 第 1 轮（分组轮）提示词
    fn group_prompts(f: &Fixture) -> Vec<ArcStr> {
        f.model.prompts().into_iter().filter(|prompt| prompt.contains("\"groups\"")).collect()
    }

    #[tokio::test]
    async fn locate_merges_pending_records_with_new_messages_once() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "已存的问题", "m1").await;
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "已存的回答", "m2").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;

        // 定位请求：第一条与已存记录同 msg_id（内容 / 时间 / 发送者都可以不同），第二条是新消息
        let response = f
            .service
            .locate(&locate_request(&role_event("dev", None), vec![
                locate_message_at(&a, "2099-01-01 23:59:59", 0, "内容不同也判为同一条", "m1"),
                locate_message_at(&a, "2099-01-01 12:00:00", 0, "新问题", "m3"),
            ]))
            .await
            .unwrap();

        // 懒处理与定位合并为一次分组：待归档记录 + 去重后的新消息
        let prompts = group_prompts(&f);
        assert_eq!(prompts.len(), 1, "懒处理与定位只做一次分组");
        assert_eq!(prompts[0].matches("已存的问题").count(), 1, "重复的新消息不重复送模型");
        assert!(!prompts[0].contains("内容不同也判为同一条"), "重复的新消息自身不进批次");
        assert!(prompts[0].contains("已存的回答"), "待归档记录参与同一次分析");
        assert!(prompts[0].contains("新问题"), "不重复的新消息参与同一次分析");

        // 记录归档一次；新消息只返回所属 case，不进原文副本
        assert_eq!(cases_of(&response), vec!["role-case"]);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1], "重复的新消息映射到已存记录所在 case");
        assert!(response.groups[0].is_current);
        assert_eq!(records_of(&f, &role_event("dev", None), "role-case").await.len(), 2, "只有已存记录落盘");
        assert_eq!(pending_len(&f, &role_event("dev", None), &a).await, 0, "懒处理完成");
        assert!(!is_dirty(&f, &role_event("dev", None)).await);
    }

    #[tokio::test]
    async fn locate_with_only_duplicates_archives_records_once() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "已存的问题", "m1").await;
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "已存的回答", "m2").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;

        // 请求的两条消息都与已存记录重复：批次只剩已存记录，仍只做一次分组
        let response = f
            .service
            .locate(&locate_request(&role_event("dev", None), vec![
                locate_message_at(&a, "2099-01-01 10:00:00", 0, "已存的问题", "m1"),
                locate_message_at(&a, "2099-01-01 10:01:00", 1, "已存的回答", "m2"),
            ]))
            .await
            .unwrap();

        let prompts = group_prompts(&f);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].matches("已存的问题").count(), 1);
        assert_eq!(cases_of(&response), vec!["role-case"]);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(records_of(&f, &role_event("dev", None), "role-case").await.len(), 2, "记录不因定位重复落盘");
    }

    /// 没有待处理标记时调 locate：仍按「pending 空 + 本批消息」处理（定位不依赖 dirty）
    #[tokio::test]
    async fn locate_without_dirty_marks_still_locates_messages() {
        let f = fixture().await;
        let a = channel_a();
        // 显式清掉首次加载置的待处理标记，覆盖「不 dirty」分支
        {
            let handle = f.service.manager().open(agent_role(&role_event("dev", None))).await.unwrap();
            let mut state = handle.state.lock().await;
            state.clear_dirty(&role_event("dev", None), &BTreeSet::new());
        }

        let response = f
            .service
            .locate(&locate_request(&role_event("dev", None), vec![
                locate_message_at(&a, "2099-01-01 12:00:00", 0, "一问", "m1"),
                locate_message_at(&a, "2099-01-01 12:01:00", 0, "二问", "m2"),
            ]))
            .await
            .unwrap();

        assert_eq!(cases_of(&response), vec!["role-case"], "新话题也建出 case（记忆条数 0）");
        assert_eq!(response.groups.len(), 1, "同通道整批一组");
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(response.groups[0].case_name.as_str(), "role-case");
        assert_eq!(f.model.prompts().len(), 2, "分组轮 + 一次逐层判定（当层为空也要问名称与摘要）");
        let created = node(&f, &role_event("dev", None), "role-case").await.unwrap();
        assert_eq!(created.record_count, 0);
        assert_eq!(created.summary.as_str(), "新 case 摘要", "新 case 建立当次就有摘要");
    }

    /// 逐层判定给出的 case 必须在当层候选里：给出树里存在但不在当层的深层 case → 按模型失败处理
    #[tokio::test]
    async fn level_merge_must_target_a_current_level_candidate() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "父内容").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        query(&f, &role_event("dev", None)).await;
        // 手工构造 case（顶层父）/ role-2（子）结构
        {
            let handle = f.service.manager().get(&agent_role(&role_event("dev", None))).unwrap();
            let mut state = handle.state.lock().await;
            state.tree.create_node("role-2", Some("role-case"), CaseMode::Role, "t");
        }

        // 顶层判定直接给出深层 case role-2（不在顶层候选里）→ 失败
        f.model.push_group(&[0], false);
        f.model.push_level(LevelDecision::Merge("role-2".into()));
        let result = f.service.locate(&locate_request(&role_event("dev", None), vec![locate_message(&a, "延续深层话题")])).await;
        assert!(result.is_err(), "合并到非当层 case 应按模型失败处理");
    }

    /// locate 对每个通道只调用一次 archive：同一次提取里不会重复处理某个通道
    #[tokio::test]
    async fn locate_archives_each_message_channel_once() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        // A 已归档过：存储里存在该通道，但水位之后没有待归档记录
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "旧记录", "m1").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        query(&f, &role_event("dev", None)).await;
        let before = f.model.prompts().len();

        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        f.model.push_group(&[0, 1], false);
        f.model.push_level(LevelDecision::new_case("two", "第二摘要"));
        let response = f
            .service
            .locate(&locate_request(&role_event("dev", None), vec![
                locate_message_at(&a, "2099-01-01 12:00:00", 0, "一问", "m2"),
                locate_message_at(&a, "2099-01-01 12:01:00", 0, "二问", "m3"),
            ]))
            .await
            .unwrap();

        let prompts: Vec<ArcStr> = f.model.prompts().into_iter().skip(before).collect();
        assert_eq!(
            prompts.iter().filter(|prompt| prompt.contains("\"groups\"")).count(),
            1,
            "该通道只归档一次（补齐循环不会重复处理已被 process_pending 覆盖的通道）"
        );
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(response.groups[0].case_name.as_str(), "role-two", "新话题建出记忆条数为 0 的 case");
        assert_eq!(pending_len(&f, &role_event("dev", None), &a).await, 0);
    }

    /// 请求跨通道、且靠后的通道有待归档记录（走合并路径）时，
    /// 返回的分组下标必须是**请求全局下标**，不能是该通道批次内的局部下标
    #[tokio::test]
    async fn locate_reports_request_indices_for_merged_channels() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        append_store_channel_with_id(&root, "0", "dev", &b, "2099-01-01 10:00:00", 1, 0, "B 的记录", "m1").await;
        notify(&f, Arc::new(role_event("dev", None)), &b).await;

        let response = f
            .service
            .locate(&locate_request(&role_event("dev", None), vec![
                locate_message_at(&a, "2099-01-01 12:00:00", 0, "A 的消息", "ma"),
                locate_message_at(&b, "2099-01-01 12:01:00", 0, "B 的消息", "m1"),
            ]))
            .await
            .unwrap();

        let mut groups: Vec<(Vec<usize>, String)> = response
            .groups
            .iter()
            .map(|group| (group.messages.clone(), group.case_name.as_str().to_string()))
            .collect();
        groups.sort();
        assert_eq!(
            groups,
            vec![(vec![0], "role-case".to_string()), (vec![1], "role-case-2".to_string())],
            "分组下标是请求全局下标（B 的消息是 1，不是该通道批次内的 0），且每个组都有 case"
        );
        assert_eq!(cases_of(&response), vec!["role-case", "role-case-2"]);
    }

    #[tokio::test]
    async fn locate_dedups_only_within_the_message_channel() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        // 两个通道的记录用同一个 msg_id：去重只看本通道的记录
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "A 自己的记录", "m1").await;
        append_store_channel_with_id(&root, "0", "dev", &b, "2099-01-01 10:00:00", 2, 0, "B 自己的记录", "m1").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        notify(&f, Arc::new(role_event("dev", None)), &b).await;

        // 消息属于 B 通道：与 B 通道同 msg_id 的记录去重（不能命中 A 通道的同 msg_id 记录）
        let response = f
            .service
            .locate(&locate_request(&role_event("dev", None), vec![locate_message_at(&b, "2099-01-01 10:00:00", 0, "B 通道的问题", "m1")]))
            .await
            .unwrap();

        let prompts = group_prompts(&f);
        assert_eq!(prompts.len(), 2, "两个通道各一次分组");
        let b_prompt = prompts.last().unwrap();
        assert!(b_prompt.contains("B 自己的记录"), "B 的消息与 B 通道的记录合并分析");
        assert!(!b_prompt.contains("B 通道的问题"), "重复的新消息自身不进批次");
        assert_eq!(records_of(&f, &role_event("dev", None), "role-case").await.len(), 1, "A 通道照常归档");
        assert_eq!(records_of(&f, &role_event("dev", None), "role-case-2").await.len(), 1, "B 通道只有自己的记录落盘");
        assert_eq!(cases_of(&response), vec!["role-case-2"], "消息命中 B 通道的 case");
        assert_eq!(pending_len(&f, &role_event("dev", None), &b).await, 0);
    }

    #[tokio::test]
    async fn locate_message_group_merges_into_existing_case() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "旧话题").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        query(&f, &role_event("dev", None)).await;
        assert_eq!(current_case(&f, &role_event("dev", None), &a).await.as_deref(), Some("role-case"));

        // 待归档记录 C 与新消息 B 分属两组：两组都走同一套逐层判定，只有有记录的 C 会落地
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:00:00", 2, 0, "待归档的新话题").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        f.model.push_groups(vec![
            crate::model::MessageGroup { messages: vec![0], is_current: false },
            crate::model::MessageGroup { messages: vec![1], is_current: false },
        ]);
        f.model.push_level(LevelDecision::new_case("two", "第二摘要"));
        f.model.push_level(LevelDecision::Merge("role-case".into()));
        let before = f.model.prompts().len();
        let response = f
            .service
            .locate(&locate_request(&role_event("dev", None), vec![locate_message_at(&a, "2099-01-01 12:00:00", 0, "延续旧话题", "")]))
            .await
            .unwrap();

        let level_prompts = f
            .model
            .prompts()
            .into_iter()
            .skip(before)
            .filter(|prompt| prompt.contains("当层 case"))
            .count();
        assert_eq!(level_prompts, 2, "有记录的组与只定位的组走同一套逐层判定");
        assert_eq!(tree_len(&f, &role_event("dev", None)).await, 2, "只有记录组建 case（消息组并入 case）");
        assert_eq!(node(&f, &role_event("dev", None), "role-two").await.unwrap().record_count, 1, "待归档记录落到新 case");
        assert_eq!(node(&f, &role_event("dev", None), "role-case").await.unwrap().record_count, 1, "新消息不落盘");
        assert_eq!(cases_of(&response), vec!["role-case"]);
        assert!(response.cases[0].is_current, "指针指向本批最后一个组的 case case");
        assert_eq!(response.groups[0].messages, vec![0]);
        assert_eq!(response.groups[0].case_name.as_str(), "role-case");
    }

    #[tokio::test]
    async fn locate_chunks_the_merged_batch() {
        let f = fixture_with_batch(2).await;
        let root = f.service.root();
        let a = channel_a();
        for i in 1..=3u64 {
            append_store_channel(&root, "0", "dev", &a, &format!("2099-01-01 10:0{}:00", i), i, 0, &format!("记录{}", i)).await;
        }
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        let response = f
            .service
            .locate(&locate_request(&role_event("dev", None), vec![locate_message_at(&a, "2099-01-01 12:00:00", 0, "新消息", "")]))
            .await
            .unwrap();

        assert_eq!(group_prompts(&f).len(), 2, "3 条记录 + 1 条新消息按每块 2 条分成 2 块");
        assert_eq!(pending_len(&f, &role_event("dev", None), &a).await, 0);
        assert_eq!(watermark_point(&f, &role_event("dev", None), &a).await.1, 3, "新消息不影响水位");
        assert_eq!(records_of(&f, &role_event("dev", None), "role-case").await.len(), 3, "新消息不落盘");
        assert_eq!(cases_of(&response), vec!["role-case"]);
        assert_eq!(response.groups[0].messages, vec![0]);
    }

    #[tokio::test]
    async fn locate_uses_current_case_of_the_message_channel() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "A 的消息").await;
        append_store_channel(&root, "0", "dev", &b, "2099-01-01 11:00:00", 2, 0, "B 的消息").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        let tree = query(&f, &role_event("dev", None)).await;

        let current = |channel: &ChannelKey| -> ArcStr {
            current_pointer(&tree, channel).map(|pointer| pointer.case_name.clone()).expect("通道应有当前 case")
        };
        let (current_a, current_b) = (current(&a), current(&b));
        assert_ne!(current_a, current_b, "两个通道各有自己的当前 case");

        let locate = |channel: &ChannelKey, text: &str| {
            let request = LocateRequest {
                agent_id: ArcStr::from("0"),
                role_event: role_event("dev", None),
                messages: vec![locate_message(channel, text)],
            };
            let service = &f.service;
            async move { service.locate(&request).await.unwrap() }
        };

        f.model.push_group(&[0], true);
        let from_a = locate(&a, "继续 A 的话题").await;
        assert_eq!(cases_of(&from_a), vec![current_a.as_str()]);
        assert!(from_a.cases[0].is_current);
        f.model.push_group(&[0], true);
        let from_b = locate(&b, "继续 B 的话题").await;
        assert_eq!(cases_of(&from_b), vec![current_b.as_str()]);
        assert!(from_b.cases[0].is_current);

        // 同一个 case 用另一个通道的消息定位：命中 B 的 case，A 通道的当前 case 指针也跟随到它
        f.model.push_group(&[0], false);
        f.model.push_level(LevelDecision::Merge(current_b.clone()));
        let cross = locate(&a, "从 A 通道问 B 的话题").await;
        assert_eq!(cases_of(&cross), vec![current_b.as_str()]);
        assert!(cross.cases[0].is_current, "定位后该 case 成为 A 通道的当前 case");
        assert_eq!(current_case(&f, &role_event("dev", None), &a).await.as_deref(), Some(current_b.as_str()));
        assert_eq!(current_case(&f, &role_event("dev", None), &b).await.as_deref(), Some(current_b.as_str()), "B 通道指针不变");
    }

    #[tokio::test]
    async fn case_queries_split_summary_and_records() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问一").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "答一").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        process(&f, &role_event("dev", None)).await;

        // 摘要：只有节点（摘要文本 + 记录数），不带原文；新建 case 的摘要来自逐层判定
        let summary = f.service.case_summary("0", &role_event("dev", None), "role-case").await.unwrap();
        assert_eq!(summary.case.case_name.as_str(), "role-case");
        assert_eq!(summary.case.record_count, 2);
        assert_eq!(summary.case.summary.as_str(), "新 case 摘要");

        // 全部原文
        let all = f.service.case_records("0", &role_event("dev", None), "role-case").await.unwrap();
        assert_eq!(all.records.len(), 2);
        assert_eq!(all.records[0].sn(), 1);
        assert_eq!(all.records[1].sn(), 2);

        // 指定之后：把上一条记录的 time/sn/record_type 当游标回传
        let after = f
            .service
            .case_records_after("0", &role_event("dev", None), "role-case", &CaseCursor::of(&all.records[0]))
            .await
            .unwrap();
        assert_eq!(after.records.len(), 1);
        assert_eq!(after.records[0].sn(), 2);

        // 结尾若干条（升序返回；0 条为空）
        let tail = f.service.case_records_tail("0", &role_event("dev", None), "role-case", 1).await.unwrap();
        assert_eq!(tail.records.len(), 1);
        assert_eq!(tail.records[0].sn(), 2);
        assert!(f.service.case_records_tail("0", &role_event("dev", None), "role-case", 0).await.unwrap().records.is_empty());

        // case 不存在：摘要与原文查询都报错
        assert!(f.service.case_summary("0", &role_event("dev", None), "role-9").await.is_err());
        assert!(f.service.case_records("0", &role_event("dev", None), "role-9").await.is_err());
        assert!(f
            .service
            .case_records_after("0", &role_event("dev", None), "role-9", &CaseCursor::of(&all.records[0]))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn manual_trigger_and_rebuild_process_everything() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        for (i, text) in [(1u64, "答一"), (2, "问二"), (3, "答二"), (4, "问三")] {
            append_store_channel(&root, "0", "dev", &a, &format!("2099-01-01 10:0{}:00", i - 1), i, (i % 2) as usize, text).await;
        }

        // 手动触发：忽略待处理标记，把水位之后全部记录处理一次（桩模型：整批一组）
        let first = f.service.manual_trigger("0", &role_event("dev", None)).await.unwrap();
        assert_eq!(first.archived, 4, "手动触发处理全部待处理记录");
        assert_eq!(first.case_names, vec![ArcStr::from("role-case")]);
        let second = f.service.manual_trigger("0", &role_event("dev", None)).await.unwrap();
        assert_eq!(second.archived, 0, "水位已推进，无剩余记录");

        let response = f.service.rebuild("0", &role_event("dev", None)).await.unwrap();
        assert_eq!(response.archived, 4, "重建归档全部历史记录");
        assert_eq!(response.case_count, 1, "重建结果与首次一致");
        assert_eq!(pending_len(&f, &role_event("dev", None), &a).await, 0);
        assert!(current_case(&f, &role_event("dev", None), &a).await.is_some(), "重建后通道当前 case 指针保留");
    }

    #[tokio::test]
    async fn case_mode_scope_keeps_single_case() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev-e1", &a, "2099-01-01 10:00:00", 1, 1, "事件模式").await;
        append_store_channel(&root, "0", "dev-e1", &a, "2099-01-01 10:01:00", 2, 0, "继续").await;
        notify(&f, Arc::new(role_event("dev", Some("e1"))), &a).await;
        let tree = query(&f, &role_event("dev", Some("e1"))).await;

        assert_eq!(tree.nodes.len(), 1, "事件模式整体一个 case");
        assert_eq!(tree.nodes[0].mode, CaseMode::Event);
        assert_eq!(tree.nodes[0].record_count, 2);
        assert!(!tree.nodes[0].summary.is_empty(), "有记录的 event case 由第 3 步摘要重算生成摘要");
        assert_eq!(current_pointer(&tree, &a).unwrap().case_name.as_str(), "event-e1");
        // 事件模式不参与分组与分层归并：模型只被要求生成摘要
        assert!(f.model.prompts().iter().all(|p| !p.contains("分组") && !p.contains("当层 case")));
    }

    /// 重建只清本后缀自己的 case：角色子树与其他事件后缀的固定 case 都不受影响
    #[tokio::test]
    async fn rebuild_touches_only_its_own_case_subtree() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        // 后缀字符串只用于磁盘路径；内存侧一律显式构造 RoleEvent
        for (suffix, role_event) in [
            ("dev", role_event("dev", None)),
            ("dev-e1", role_event("dev", Some("e1"))),
            ("dev-e2", role_event("dev", Some("e2"))),
        ] {
            append_store_channel(&root, "0", suffix, &a, "2099-01-01 10:00:00", 1, 0, "内容").await;
            notify(&f, Arc::new(role_event.clone()), &a).await;
            process(&f, &role_event).await;
        }
        // 三个后缀落到同一棵角色树：角色子树 + 两个事件 case
        let tree = f.service.tree_get("0", &role_event("dev", None)).await.unwrap();
        assert_eq!(tree.nodes.len(), 3);

        // 重建事件后缀 dev-e1：只有 e1 清掉重建，e2 与 case 原样保留
        let response = f.service.rebuild("0", &role_event("dev", Some("e1"))).await.unwrap();
        assert_eq!(response.archived, 1, "只重建本后缀的记录");
        let tree = f.service.tree_get("0", &role_event("dev", None)).await.unwrap();
        let ids: Vec<&str> = tree.nodes.iter().map(|n| n.case_name.as_str()).collect();
        assert!(ids.contains(&"role-case"), "角色子树保留: {:?}", ids);
        assert!(ids.contains(&"event-e1"), "本后缀的 case 重建出来: {:?}", ids);
        assert!(ids.contains(&"event-e2"), "其他事件后缀的 case 不受影响: {:?}", ids);
        assert_eq!(current_pointer(&tree, &a).unwrap().case_name.as_str(), "role-case", "角色后缀的指针不动");

        // 重建角色模式：清角色子树，事件 case 都保留
        f.service.rebuild("0", &role_event("dev", None)).await.unwrap();
        let tree = f.service.tree_get("0", &role_event("dev", None)).await.unwrap();
        let ids: Vec<&str> = tree.nodes.iter().map(|n| n.case_name.as_str()).collect();
        let role_count = tree.nodes.iter().filter(|n| n.mode == CaseMode::Role).count();
        let event_count = tree.nodes.iter().filter(|n| n.mode == CaseMode::Event).count();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["event-e1", "event-e2", "role-case"], "角色子树重建 + 事件 case 都保留: {:?}", ids);
        assert_eq!(role_count, 1, "角色子树重建: {:?}", ids);
        assert_eq!(event_count, 2, "事件 case 都保留: {:?}", ids);
    }

    #[tokio::test]
    async fn nested_cases_cascade_summaries() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 1, "父内容").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        query(&f, &role_event("dev", None)).await;

        // 模拟后续「合并为大 case」程序的产物：case 成为父 case，子 case role-2 承载内容
        {
            let handle = f.service.manager().get(&agent_role(&role_event("dev", None))).unwrap();
            let mut state = handle.state.lock().await;
            state.tree.create_node("role-2", Some("role-case"), CaseMode::Role, "t");
            state.current_cases.insert((role_event("dev", None), a.clone()), "role-2".into());
        }

        // 下一批标记为当前 case 组 → 直接并入 role-2 → role-2 摘要重算后级联重算父 case case
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:00:00", 2, 1, "子内容").await;
        notify(&f, Arc::new(role_event("dev", None)), &a).await;
        query(&f, &role_event("dev", None)).await;

        assert_eq!(node(&f, &role_event("dev", None), "role-2").await.unwrap().record_count, 1, "本批只有一条记录");
        let inputs = f.model.summary_inputs();
        assert!(inputs.iter().any(|i| i.contains("子 case 摘要")), "父摘要涵盖子 case 内容");
        assert!(inputs.last().unwrap().contains("子 case 摘要"), "祖先最后重算");
    }

    #[tokio::test]
    async fn appended_records_are_visible_to_the_next_read() {
        // 回归：tokio::fs::File 的 write_all 可能只把数据交给后台阻塞任务就返回（偶发丢记录），
        // 写入路径必须 flush，否则紧接着的读取（懒处理读共享记忆文件）会看不到刚写入的记录
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        let handle = f.service.manager().open(agent_role(&role_event("dev", None))).await.unwrap();
        for i in 0..100u64 {
            let time = format!("2099-01-01 10:{:02}:{:02}", i / 60, i % 60);
            append_store_channel(&root, "0", "dev", &a, &time, i + 1, (i % 2) as usize, "内容").await;
            let view = f.service.store_view(&handle, &role_event("dev", None)).await.unwrap();
            assert_eq!(view.records.len(), i as usize + 1, "第 {} 轮写入后立即读取的记录数不对", i + 1);
        }
    }
}
