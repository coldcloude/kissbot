//! 服务层：把通知、触发判定、归档、定位、查询与管理串成组件的对外能力。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use kissbot_api::{
    ChannelEventPointer, ChannelKey, EventCursor, EventListResponse, EventNode,
    EventRecordsResponse, EventSummaryResponse, LocateMessage, LocateRequest, LocateResponse,
    LocatedGroup, RebuildResponse, RecordKind, RecordNotification, TreeGetResponse,
};
use tracing::info;

use crate::archive::{ArchiveOutcome, Archiver, PlaceGroup};
use crate::error::{Error, Result};
use crate::locate::{group_indices_by_channel, merge_located};
use crate::model::DecisionModel;
use crate::records::{
    RecordEntry, StoreView, read_event_records, read_store_records, records_after, records_tail,
};
use crate::scope::{ScopeHandle, ScopeId, ScopeManager, ScopeState};
use crate::tree::Tree;

/// 摘要记忆服务
pub struct AbstractService {
    manager: Arc<ScopeManager>,
    /// 归档器：按通道归档待处理记录，并（可选）一并定位本批新消息
    archiver: Arc<Archiver>,
}

impl AbstractService {
    pub fn new(
        manager: Arc<ScopeManager>,
        model: Arc<dyn DecisionModel>,
        max_depth: usize,
        max_batch_records: usize,
    ) -> Self {
        Self {
            manager,
            archiver: Arc::new(Archiver::new(model, max_depth, max_batch_records)),
        }
    }

    #[cfg(test)]
    pub fn manager(&self) -> &Arc<ScopeManager> {
        &self.manager
    }

    /// 启动恢复：加载磁盘上已有作用域（加载即为待处理，首次查询时统一计算）
    pub async fn restore_scopes(&self) -> Result<Vec<Arc<ScopeHandle>>> {
        self.manager.load_all().await
    }

    // ========== 通知登记与懒处理 ==========

    /// 处理一条新数据通知：只登记「哪个作用域哪个通道有变化」，不做任何计算。
    ///
    /// 通道消息通知携带通道，只标记该通道；思考/工具通知（以及未带通道的通知）无法确定通道，
    /// 标记整个作用域，查询时按该作用域全部通道处理。
    pub async fn on_notification(&self, kind: RecordKind, notification: &RecordNotification) -> Result<()> {
        let id = ScopeId::new(notification.agent_id.as_str(), notification.role.as_str());
        let handle = self.manager.open(id).await?;
        let mut state = handle.state.lock().await;
        match notification.channel.as_ref().filter(|_| kind == RecordKind::Channel) {
            Some(channel) => state.mark_channel(channel),
            None => state.mark_all(),
        }
        Ok(())
    }

    /// 把全部已加载作用域标记为待处理（通知重连后通道未知，按作用域全部通道处理）
    pub async fn mark_all_dirty(&self) {
        for handle in self.manager.loaded() {
            handle.state.lock().await.mark_all();
        }
    }

    /// 归档某通道并把消息分组映射回请求全局下标（`archive` + 日志 + `place_groups` 的封装）。
    ///
    /// `records` 是该通道水位之后的待归档记录（可为空）；`messages` / `indices` 是请求消息
    /// 与其中属于该通道的全局下标（可为空）。只在 `process_pending` 的逐通道循环里调用，
    /// 每个通道一次。
    async fn archive_located(
        &self,
        handle: &Arc<ScopeHandle>,
        state: &mut ScopeState,
        channel: &ChannelKey,
        records: &[RecordEntry],
        messages: &[LocateMessage],
        indices: &[usize],
    ) -> Result<(Vec<ArchiveOutcome>, Vec<(usize, ChannelKey, LocatedGroup)>)> {
        let channel_messages: Vec<&LocateMessage> = indices.iter().map(|index| &messages[*index]).collect();
        let result = self.archiver.archive(handle, state, channel, records, &channel_messages).await?;
        if !records.is_empty() || result.new_messages > 0 {
            let archived: u64 = result.outcomes.iter().map(|outcome| outcome.archived).sum();
            let event_ids: Vec<&str> =
                result.outcomes.iter().flat_map(|outcome| outcome.event_ids.iter().map(String::as_str)).collect();
            info!(
                "[abstract] process {} [{}]: archived {} records into {:?} (+{} new messages)",
                handle.id,
                render_channel(channel),
                archived,
                event_ids,
                result.new_messages,
            );
        }
        Ok((result.outcomes, place_groups(channel, indices, result.groups)))
    }

    /// 提取「各通道水位之后的待归档记录」与本批消息，统一按通道分组后逐通道执行「归档 + 定位」。
    ///
    /// - `messages` 为空：即原来的归档（查询前的统一处理 / 手动触发 / 重建），不产出定位结果；
    /// - `messages` 非空：同一批里 pending 记录落盘归档，只在消息里出现的条目仅定位、不落盘；
    /// - 没有待处理标记且没有消息时直接返回（不读盘）；否则读一次共享记忆提取各通道 pending。
    ///
    /// 返回（各通道归档结果，消息分组，按请求全局下标）。
    async fn process_pending(
        &self,
        handle: &Arc<ScopeHandle>,
        state: &mut ScopeState,
        force: bool,
        messages: &[LocateMessage],
    ) -> Result<(Vec<ArchiveOutcome>, Vec<(usize, ChannelKey, LocatedGroup)>)> {
        let (all, marked) = (state.dirty_all, state.dirty_channels.clone());
        if !force && !state.is_dirty() && messages.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let view = self.store_view(handle).await?;
        // 统一按通道分组：本批消息所在的通道 ∪ 存储里出现过的通道（pending / 消息都可以为空）
        let mut grouped: BTreeMap<ChannelKey, Vec<usize>> = group_indices_by_channel(messages).into_iter().collect();
        for channel in view.known_channels() {
            grouped.entry(channel).or_default();
        }
        let mut processed: BTreeSet<ChannelKey> = marked;
        let mut outcomes = Vec::new();
        let mut located = Vec::new();
        for (channel, indices) in grouped {
            processed.insert(channel.clone());
            let watermark = state.watermarks.get(&channel).cloned().unwrap_or_default();
            let pending = view.pending_for(&channel, &watermark);
            if pending.is_empty() && indices.is_empty() {
                continue;
            }
            let (channel_outcomes, channel_groups) =
                self.archive_located(handle, state, &channel, &pending, messages, &indices).await?;
            outcomes.extend(channel_outcomes);
            located.extend(channel_groups);
        }
        // 全部通道处理成功后才清除待处理标记（失败时保留，下次查询重试）
        state.clear_dirty(all, &processed);
        Ok((outcomes, located))
    }

    /// 手动触发：忽略待处理标记，把各通道水位之后的记录全部处理一次
    pub async fn manual_trigger(&self, id: &ScopeId) -> Result<ArchiveOutcome> {
        let handle = self.manager.open(id.clone()).await?;
        let mut state = handle.state.lock().await;
        let (outcomes, _) = self.process_pending(&handle, &mut state, true, &[]).await?;
        Ok(sum_outcomes(outcomes))
    }

    /// 重建：清空事件数据与各通道水位后按共享记忆文件重新构建
    pub async fn rebuild(&self, id: &ScopeId) -> Result<RebuildResponse> {
        let handle = self.manager.open(id.clone()).await?;
        self.manager.reset_scope_data(&handle).await?;
        let mut state = handle.state.lock().await;
        state.tree = Tree::new();
        state.current_events.clear();
        state.watermarks.clear();
        state.mark_all();
        let (outcomes, _) = self.process_pending(&handle, &mut state, true, &[]).await?;
        let archived = outcomes.iter().map(|outcome| outcome.archived).sum();
        let event_count = state.tree.len() as u64;
        drop(state);
        info!("[abstract] rebuild {}: {} records, {} events", id, archived, event_count);
        Ok(RebuildResponse { archived, event_count })
    }

    /// 共享记忆文件的全量视图（记录 + 通道归属）；水位过滤按通道进行
    pub async fn store_view(&self, handle: &ScopeHandle) -> Result<StoreView> {
        read_store_records(self.manager.root(), &handle.id.agent_id, &handle.id.role).await
    }

    // ========== 定位 ==========

    /// 定位一批消息涉及的事件：提取各通道的待归档记录与本批消息（两者都可为空），
    /// 统一按通道分组后逐通道执行「归档 + 定位」——记录落盘、消息只定位。
    pub async fn locate(&self, request: &LocateRequest) -> Result<LocateResponse> {
        let id = ScopeId::new(request.agent_id.as_str(), request.role.as_str());
        let handle = self.manager.open(id).await?;
        let mut state = handle.state.lock().await;
        let (_, located) = self.process_pending(&handle, &mut state, false, &request.messages).await?;
        Ok(merge_located(located, &state.current_events))
    }

    // ========== 查询 ==========

    /// 事件列表：只读已归档的事件树，不触发归档（要处理待归档记录请用 `/abstract/rebuild` 的 rebuild=false）
    pub async fn event_list(&self, agent_id: &str, role: &str, parent_id: Option<&str>) -> Result<EventListResponse> {
        let handle = self.manager.open(ScopeId::new(agent_id, role)).await?;
        let state = handle.state.lock().await;
        let events: Vec<Arc<EventNode>> = state
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
        Ok(EventListResponse { events })
    }

    /// 事件摘要：节点本身（摘要文本 + 层级 + 记录数 + 起止时间），不含原文副本；只读，不触发归档
    pub async fn event_summary(&self, agent_id: &str, role: &str, event_id: &str) -> Result<EventSummaryResponse> {
        let handle = self.manager.open(ScopeId::new(agent_id, role)).await?;
        let state = handle.state.lock().await;
        let node = state
            .tree
            .get(event_id)
            .cloned()
            .ok_or_else(|| Error::StateError(format!("事件不存在: {}/{}", handle.id, event_id)))?;
        Ok(EventSummaryResponse { event: Arc::new(node) })
    }

    /// 事件全部原文副本
    pub async fn event_records(&self, agent_id: &str, role: &str, event_id: &str) -> Result<EventRecordsResponse> {
        Ok(response_of(self.read_event(agent_id, role, event_id).await?))
    }

    /// 事件中游标之后的原文副本（分页续读）
    pub async fn event_records_after(
        &self,
        agent_id: &str,
        role: &str,
        event_id: &str,
        after: &EventCursor,
    ) -> Result<EventRecordsResponse> {
        let records = records_after(&self.read_event(agent_id, role, event_id).await?, after);
        Ok(response_of(records))
    }

    /// 事件结尾若干条原文副本
    pub async fn event_records_tail(
        &self,
        agent_id: &str,
        role: &str,
        event_id: &str,
        count: u64,
    ) -> Result<EventRecordsResponse> {
        let records = records_tail(&self.read_event(agent_id, role, event_id).await?, count);
        Ok(response_of(records))
    }

    /// 读取事件原文副本（事件不存在时报错，与摘要查询一致）；只读，不触发归档
    async fn read_event(&self, agent_id: &str, role: &str, event_id: &str) -> Result<Vec<kissbot_api::EventRecord>> {
        let handle = self.manager.open(ScopeId::new(agent_id, role)).await?;
        {
            let state = handle.state.lock().await;
            if !state.tree.contains(event_id) {
                return Err(Error::StateError(format!("事件不存在: {}/{}", handle.id, event_id)));
            }
        }
        read_event_records(&handle.event_dir(event_id)).await
    }

    /// 事件树与各通道当前事件指针；只读，不触发归档
    pub async fn tree_get(&self, agent_id: &str, role: &str) -> Result<TreeGetResponse> {
        let handle = self.manager.open(ScopeId::new(agent_id, role)).await?;
        let state = handle.state.lock().await;
        let current_events = state
            .current_events
            .iter()
            .map(|(channel, event_id)| {
                Arc::new(ChannelEventPointer {
                    channel: channel.clone(),
                    event_id: Arc::new(event_id.clone()),
                })
            })
            .collect();
        Ok(TreeGetResponse {
            current_events,
            nodes: state.tree.nodes().into_iter().cloned().map(Arc::new).collect(),
        })
    }

    /// 作用域事件树节点数（重建响应用；排障可读事件树，水位与指针见磁盘状态文件）
    pub async fn event_count(&self, agent_id: &str, role: &str) -> Result<u64> {
        let handle = self.manager.open(ScopeId::new(agent_id, role)).await?;
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
        event_ids: outcomes.into_iter().flat_map(|outcome| outcome.event_ids).collect(),
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
                LocatedGroup { messages, event_id: group.event_id.map(Arc::new), is_current: false },
            ))
        })
        .collect()
}

/// 原文副本响应（统一装配成 Arc）
fn response_of(records: Vec<kissbot_api::EventRecord>) -> EventRecordsResponse {
    EventRecordsResponse { records: records.into_iter().map(Arc::new).collect() }
}

/// 通道的可读文本（日志用）
fn render_channel(channel: &ChannelKey) -> String {
    format!("{}/{}/{}", channel.messenger_id, channel.self_user_id, channel.group_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LevelDecision;
    use crate::stub::StubModel;
    use kissbot_api::{ChannelEventPointer, Content, EventMode, EventRecord, LocateMessage, TreeGetResponse};
    use std::path::Path;

    /// 测试用通道 A：identity 与 append_store_channel 写入的记录一致
    fn channel_a() -> ChannelKey {
        ChannelKey::new("web", "selfA", "gA")
    }

    /// 测试用通道 B：同一作用域下的另一个通道
    fn channel_b() -> ChannelKey {
        ChannelKey::new("web", "selfB", "gB")
    }

    /// 往共享记忆文件追加一行记录（模拟 memory-store 的落盘）
    async fn append_store_line(root: &Path, agent_id: &str, role: &str, stem: &str, record: serde_json::Value, time: &str) {
        let date = &time[..10];
        let year = &time[..4];
        let dir = root.join(agent_id).join("memory-store").join(format!("{}-{}", year, role));
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
    async fn append_store_channel(root: &Path, agent_id: &str, role: &str, channel: &ChannelKey, time: &str, sn: u64, is_self: usize, text: &str) {
        append_store_channel_with_id(root, agent_id, role, channel, time, sn, is_self, text, "").await;
    }

    /// 追加一条带 msg_id 的通道记录（消息与记录按「通道三元组 + msg_id」去重）
    async fn append_store_channel_with_id(
        root: &Path,
        agent_id: &str,
        role: &str,
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
        append_store_line(root, agent_id, role, "channel", record, time).await;
    }

    /// 追加一条思考记录：先写占位通道记录（内容为该 key），再写思考记录本体
    async fn append_store_think(root: &Path, agent_id: &str, role: &str, channel: &ChannelKey, key: &str, time: &str, sn: u64, text: &str) {
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
        append_store_line(root, agent_id, role, "channel", placeholder, time).await;
        let think = serde_json::json!({
            "reasoning_content": text,
            "thinking": null,
            "key": key,
            "time": time,
            "sn": sn
        });
        append_store_line(root, agent_id, role, "think", think, time).await;
    }

    /// 追加一条没有占位通道记录的思考记录（key 无人认领）
    async fn append_store_orphan_think(root: &Path, agent_id: &str, role: &str, time: &str, sn: u64, key: &str) {
        let think = serde_json::json!({
            "reasoning_content": "无占位记录",
            "thinking": null,
            "key": key,
            "time": time,
            "sn": sn
        });
        append_store_line(root, agent_id, role, "think", think, time).await;
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
        let service = AbstractService::new(manager, model.clone(), 4, max_batch_records);
        Fixture { _temp: temp, service, model }
    }

    /// 通道消息通知（懒处理：只登记，不计算）
    fn channel_notification(role: &str, channel: &ChannelKey, is_self: usize) -> RecordNotification {
        RecordNotification {
            agent_id: Arc::new("0".into()),
            role: Arc::new(role.into()),
            time: Arc::new("2026-09-22 00:00:00".into()),
            is_self: Some(is_self),
            channel: Some(channel.clone()),
        }
    }

    /// 登记某通道有新数据
    async fn notify(f: &Fixture, role: &str, channel: &ChannelKey) {
        f.service
            .on_notification(RecordKind::Channel, &channel_notification(role, channel, 0))
            .await
            .unwrap();
    }

    /// 登记整个作用域有新数据（思考/工具通知的形态）
    async fn notify_scope(f: &Fixture, role: &str) {
        f.service
            .on_notification(RecordKind::Think, &RecordNotification {
                agent_id: Arc::new("0".into()),
                role: Arc::new(role.into()),
                time: Arc::new("2026-09-22 00:00:00".into()),
                is_self: None,
                channel: None,
            })
            .await
            .unwrap();
    }

    /// 手动归档一次（`/abstract/rebuild` 的 rebuild=false 路径）
    async fn process(f: &Fixture, role: &str) -> ArchiveOutcome {
        f.service.manual_trigger(&ScopeId::new("0", role)).await.unwrap()
    }

    /// 手动归档一次后读事件树（查询接口本身只读、不触发归档）
    async fn query(f: &Fixture, role: &str) -> TreeGetResponse {
        process(f, role).await;
        f.service.tree_get("0", role).await.unwrap()
    }

    /// 作用域内存中的事件节点数（不触发懒处理）
    async fn tree_len(f: &Fixture, role: &str) -> usize {
        let handle = f.service.manager().get(&ScopeId::new("0", role)).expect("作用域已加载");
        handle.state.lock().await.tree.len()
    }

    /// 作用域是否还有待处理标记
    async fn is_dirty(f: &Fixture, role: &str) -> bool {
        let handle = f.service.manager().get(&ScopeId::new("0", role)).expect("作用域已加载");
        handle.state.lock().await.is_dirty()
    }

    /// 某通道的水位（测试直接读内存状态）：(时间, sn)；无水位时 sn 为 0
    async fn watermark_point(f: &Fixture, role: &str, channel: &ChannelKey) -> (String, u64) {
        let handle = f.service.manager().get(&ScopeId::new("0", role)).expect("作用域已加载");
        let state = handle.state.lock().await;
        match state.watermarks.get(channel).map(|w| &w.channel) {
            Some(point) => (point.time.as_str().to_string(), point.sn),
            None => (String::new(), 0),
        }
    }

    /// 某通道待处理记录条数（水位之后、归属该通道）
    async fn pending_len(f: &Fixture, role: &str, channel: &ChannelKey) -> usize {
        let handle = f.service.manager().get(&ScopeId::new("0", role)).expect("作用域已加载");
        let watermark = {
            let state = handle.state.lock().await;
            state.watermarks.get(channel).cloned().unwrap_or_default()
        };
        f.service.store_view(&handle).await.unwrap().pending_for(channel, &watermark).len()
    }

    /// 某通道的当前事件 id
    async fn current_event(f: &Fixture, role: &str, channel: &ChannelKey) -> Option<String> {
        let handle = f.service.manager().get(&ScopeId::new("0", role)).expect("作用域已加载");
        handle.state.lock().await.current_events.get(channel).cloned()
    }

    /// 事件节点
    async fn node(f: &Fixture, role: &str, event_id: &str) -> Option<Arc<EventNode>> {
        query(f, role).await.nodes.into_iter().find(|n| n.event_id.as_str() == event_id)
    }

    /// 事件原文副本
    async fn records_of(f: &Fixture, role: &str, event_id: &str) -> Vec<Arc<EventRecord>> {
        f.service.event_records("0", role, event_id).await.unwrap().records
    }

    /// 定位用消息（指定通道；msg_id 为空表示不参与与已存记录的去重）
    fn locate_message(channel: &ChannelKey, text: &str) -> LocateMessage {
        locate_message_at(channel, "2099-01-01 12:00:00", 0, text, "")
    }

    /// 定位结果中的事件 id
    fn events_of(response: &LocateResponse) -> Vec<&str> {
        response.events.iter().map(|event| event.event_id.as_str()).collect()
    }

    /// 当前事件指针表
    fn current_pointer<'a>(tree: &'a TreeGetResponse, channel: &ChannelKey) -> Option<&'a Arc<ChannelEventPointer>> {
        tree.current_events.iter().find(|pointer| &pointer.channel == channel)
    }

    #[tokio::test]
    async fn notification_only_marks_dirty_until_manual_trigger() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "答").await;

        notify(&f, "dev", &a).await;
        assert!(is_dirty(&f, "dev").await, "通知只登记待处理标记");
        assert_eq!(tree_len(&f, "dev").await, 0, "登记本身不做任何计算");
        assert_eq!(f.model.prompts().len(), 0, "登记不调用模型");

        // 查询只读：不触发归档、不清标记
        let tree = f.service.tree_get("0", "dev").await.unwrap();
        assert_eq!(tree.nodes.len(), 0, "查询不触发归档");
        assert_eq!(f.service.event_count("0", "dev").await.unwrap(), 0);
        assert!(is_dirty(&f, "dev").await, "查询不清除待处理标记");
        assert_eq!(f.model.prompts().len(), 0);
        assert!(f.service.event_list("0", "dev", None).await.unwrap().events.is_empty());
        assert!(f.service.event_summary("0", "dev", "e1").await.is_err(), "事件还没归档出来");

        // 手动触发才统一计算
        let outcome = process(&f, "dev").await;
        assert_eq!(outcome.archived, 2, "手动触发处理该作用域水位之后的记录");
        let tree = f.service.tree_get("0", "dev").await.unwrap();
        assert_eq!(tree.nodes.len(), 1, "手动触发归档出事件");
        assert_eq!(tree.nodes[0].record_count, 2, "该通道水位之后的全部记录一起处理");
        assert_eq!(current_pointer(&tree, &a).unwrap().event_id.as_str(), "e1");
        assert!(!is_dirty(&f, "dev").await, "处理成功后标记清空");
        assert_eq!(watermark_point(&f, "dev", &a).await.1, 2, "水位推进到该通道最后一条记录");
    }

    #[tokio::test]
    async fn manual_trigger_without_changes_does_nothing() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问").await;
        notify(&f, "dev", &a).await;
        query(&f, "dev").await;

        let prompts = f.model.prompts().len();
        let outcome = process(&f, "dev").await;
        assert_eq!(outcome.archived, 0, "没有新变化时不再重复计算");
        let tree = f.service.tree_get("0", "dev").await.unwrap();
        assert_eq!(tree.nodes.len(), 1, "事件树保持");
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
        notify(&f, "dev", &a).await;

        let tree = query(&f, "dev").await;
        assert_eq!(tree.nodes.len(), 2, "两个通道各自成批、各得一个事件");
        assert_eq!(node(&f, "dev", "e1").await.unwrap().record_count, 2);
        assert_eq!(node(&f, "dev", "e2").await.unwrap().record_count, 1);
        assert_eq!(current_pointer(&tree, &a).unwrap().event_id.as_str(), "e1");
        assert_eq!(current_pointer(&tree, &b).unwrap().event_id.as_str(), "e2");
        assert_eq!(watermark_point(&f, "dev", &a).await.1, 2);
        assert_eq!(watermark_point(&f, "dev", &b).await.1, 3);
        assert_eq!(pending_len(&f, "dev", &a).await, 0);
        assert_eq!(pending_len(&f, "dev", &b).await, 0);
    }

    #[tokio::test]
    async fn scope_level_mark_processes_all_channels() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "A 的消息").await;
        append_store_channel(&root, "0", "dev", &b, "2099-01-01 11:00:00", 2, 0, "B 的消息").await;
        // 思考/工具通知无法确定通道 → 标记整个作用域
        notify_scope(&f, "dev").await;

        let tree = query(&f, "dev").await;
        assert_eq!(tree.nodes.len(), 2, "作用域级标记会处理全部通道");
        assert!(current_pointer(&tree, &a).is_some());
        assert!(current_pointer(&tree, &b).is_some());
    }

    #[tokio::test]
    async fn current_group_merges_directly_and_other_group_descends() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        // 先归档出 e1，并把它变成 e1（父）/ e2（子）的结构，当前事件指向父事件 e1
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "旧内容").await;
        notify(&f, "dev", &a).await;
        query(&f, "dev").await;
        {
            let handle = f.service.manager().get(&ScopeId::new("0", "dev")).unwrap();
            let mut state = handle.state.lock().await;
            state.tree.create_node("e2", Some("e1"), EventMode::Role, "t");
            state.current_events.insert(a.clone(), "e1".into());
        }

        // 第 1 轮分组：消息 0 属于当前事件（直接并入父事件 e1），消息 1、2 属于新事件（分层归并）
        f.model.push_groups(vec![
            crate::model::MessageGroup { messages: vec![0], is_current: true },
            crate::model::MessageGroup { messages: vec![1, 2], is_current: false },
        ]);
        f.model.push_level(LevelDecision::Merge("e1".into()));
        f.model.push_level(LevelDecision::New);
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:00:00", 2, 1, "当前话题的延续").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:01:00", 3, 0, "新话题").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:02:00", 4, 1, "新话题的回应").await;
        notify(&f, "dev", &a).await;
        query(&f, "dev").await;

        let parent = node(&f, "dev", "e1").await.unwrap();
        assert_eq!(parent.record_count, 2, "当前事件组直接并入父事件本身（不下潜）");
        assert_eq!(parent.children.len(), 2, "其他组经分层归并：Merge(e1) 下潜后 New → e1 下新建叶子");
        let child = node(&f, "dev", "e3").await.unwrap();
        assert_eq!(child.parent_id.as_ref().map(|p| p.as_str()), Some("e1"));
        assert_eq!(child.record_count, 2);
        assert_eq!(current_event(&f, "dev", &a).await.as_deref(), Some("e3"), "指针指向最后一批消息落入的事件");
    }

    #[tokio::test]
    async fn groups_become_separate_events() {
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
        notify(&f, "dev", &a).await;

        let tree = query(&f, "dev").await;
        assert_eq!(tree.nodes.len(), 2, "一批消息按分组落成两个事件");
        assert_eq!(node(&f, "dev", "e1").await.unwrap().record_count, 2);
        assert_eq!(node(&f, "dev", "e2").await.unwrap().record_count, 1);
        assert_eq!(watermark_point(&f, "dev", &a).await.1, 3, "水位覆盖整批");
        assert_eq!(pending_len(&f, "dev", &a).await, 0);
    }

    #[tokio::test]
    async fn current_group_without_current_event_fails_and_keeps_marks() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问").await;
        // 模型错误地把分组标记为当前事件，但该通道还没有当前事件 → 按失败处理
        f.model.push_group(&[0], true);
        notify(&f, "dev", &a).await;
        assert!(f.service.manual_trigger(&ScopeId::new("0", "dev")).await.is_err(), "非法分组按失败处理");
        assert!(is_dirty(&f, "dev").await, "失败后保留待处理标记，等待下次处理重试");
        assert_eq!(pending_len(&f, "dev", &a).await, 1, "水位不动");

        // 重新给出合法分组 → 重试成功且记录不重复
        f.model.push_group(&[0], false);
        let tree = query(&f, "dev").await;
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.nodes[0].record_count, 1);
    }

    #[tokio::test]
    async fn model_failure_keeps_state_and_retries_cleanly() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 1, "回答").await;
        notify(&f, "dev", &a).await;

        // 摘要调用失败：归档整体失败，状态与磁盘保持不变
        f.model.push_summarize_failure();
        assert!(f.service.manual_trigger(&ScopeId::new("0", "dev")).await.is_err(), "模型失败应向上报错");
        let handle = f.service.manager().get(&ScopeId::new("0", "dev")).unwrap();
        {
            let state = handle.state.lock().await;
            assert!(state.tree.is_empty(), "失败不留下空事件");
            assert!(state.watermarks.get(&a).is_none(), "失败不推进水位");
            assert!(state.current_events.get(&a).is_none());
        }
        assert!(!handle.event_dir("e1").exists(), "失败不落盘原文副本");
        assert!(is_dirty(&f, "dev").await, "失败保留待处理标记");

        // 再次查询（模型恢复）：批次被完整归档一次，无重复
        let tree = query(&f, "dev").await;
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.nodes[0].record_count, 1, "重试后记录不重复");
        assert_eq!(watermark_point(&f, "dev", &a).await.1, 1);
    }

    #[tokio::test]
    async fn batch_is_split_by_max_batch_records() {
        let f = fixture_with_batch(2).await;
        let root = f.service.root();
        let a = channel_a();
        for i in 1..=5u64 {
            append_store_channel(&root, "0", "dev", &a, &format!("2099-01-01 10:0{}:00", i), i, (i % 2) as usize, "内容").await;
        }
        notify(&f, "dev", &a).await;
        let tree = query(&f, "dev").await;

        let group_calls = f.model.prompts().iter().filter(|prompt| prompt.contains("\"groups\"")).count();
        assert_eq!(group_calls, 3, "5 条按每块 2 条分成 3 块，每块一次分组");
        assert_eq!(tree.nodes.len(), 1, "块的落位由桩模型决定（命中当前事件）");
        assert_eq!(tree.nodes[0].record_count, 5, "全部记录都被处理");
        assert_eq!(watermark_point(&f, "dev", &a).await.1, 5);
        assert_eq!(pending_len(&f, "dev", &a).await, 0);
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
        notify(&f, "dev", &a).await;

        let tree = query(&f, "dev").await;
        assert_eq!(tree.nodes.len(), 2, "两个通道各自成批");
        let think_keys = |records: Vec<Arc<EventRecord>>| -> Vec<String> {
            records
                .iter()
                .filter_map(|r| match r.as_ref() {
                    EventRecord::Think(t) => Some(t.key.as_str().to_string()),
                    _ => None,
                })
                .collect()
        };
        let mut all = vec![
            think_keys(records_of(&f, "dev", "e1").await),
            think_keys(records_of(&f, "dev", "e2").await),
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
        notify(&f, "dev", &a).await;
        query(&f, "dev").await;

        assert_eq!(pending_len(&f, "dev", &a).await, 0, "该通道记录已处理完");
        let records = records_of(&f, "dev", "e1").await;
        assert_eq!(records.len(), 2, "孤儿思考记录不属于任何通道，不进事件");
        assert!(!records.iter().any(|r| matches!(r.as_ref(), EventRecord::Think(_))));
    }

    /// 定位用消息（指定通道、时间、is_self）
    fn locate_message_at(channel: &ChannelKey, time: &str, is_self: usize, text: &str, msg_id: &str) -> LocateMessage {
        LocateMessage {
            msg_id: Arc::new(msg_id.to_string()),
            time: Arc::new(time.to_string()),
            user_name: Arc::new("alice".into()),
            is_self,
            messenger_id: channel.messenger_id.clone(),
            self_user_id: channel.self_user_id.clone(),
            group_id: channel.group_id.clone(),
            content: Content::Text(Arc::new(text.to_string())),
        }
    }

    /// 定位请求
    fn locate_request(role: &str, messages: Vec<LocateMessage>) -> LocateRequest {
        LocateRequest { agent_id: Arc::new("0".into()), role: Arc::new(role.into()), messages }
    }

    /// 第 1 轮（分组轮）提示词
    fn group_prompts(f: &Fixture) -> Vec<String> {
        f.model.prompts().into_iter().filter(|prompt| prompt.contains("\"groups\"")).collect()
    }

    #[tokio::test]
    async fn locate_merges_pending_records_with_new_messages_once() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "已存的问题", "m1").await;
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "已存的回答", "m2").await;
        notify(&f, "dev", &a).await;

        // 定位请求：第一条与已存记录同 msg_id（内容 / 时间 / 发送者都可以不同），第二条是新消息
        let response = f
            .service
            .locate(&locate_request("dev", vec![
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

        // 记录归档一次；新消息只返回所属事件，不进原文副本
        assert_eq!(events_of(&response), vec!["e1"]);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1], "重复的新消息映射到已存记录所在事件");
        assert!(response.groups[0].is_current);
        assert_eq!(records_of(&f, "dev", "e1").await.len(), 2, "只有已存记录落盘");
        assert_eq!(pending_len(&f, "dev", &a).await, 0, "懒处理完成");
        assert!(!is_dirty(&f, "dev").await);
    }

    #[tokio::test]
    async fn locate_with_only_duplicates_archives_records_once() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "已存的问题", "m1").await;
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "已存的回答", "m2").await;
        notify(&f, "dev", &a).await;

        // 请求的两条消息都与已存记录重复：批次只剩已存记录，仍只做一次分组
        let response = f
            .service
            .locate(&locate_request("dev", vec![
                locate_message_at(&a, "2099-01-01 10:00:00", 0, "已存的问题", "m1"),
                locate_message_at(&a, "2099-01-01 10:01:00", 1, "已存的回答", "m2"),
            ]))
            .await
            .unwrap();

        let prompts = group_prompts(&f);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].matches("已存的问题").count(), 1);
        assert_eq!(events_of(&response), vec!["e1"]);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(records_of(&f, "dev", "e1").await.len(), 2, "记录不因定位重复落盘");
    }

    /// 没有待处理标记时调 locate：仍按「pending 空 + 本批消息」处理（定位不依赖 dirty）
    #[tokio::test]
    async fn locate_without_dirty_marks_still_locates_messages() {
        let f = fixture().await;
        let a = channel_a();
        // 显式清掉首次加载置的待处理标记，覆盖「不 dirty」分支
        {
            let handle = f.service.manager().open(ScopeId::new("0", "dev")).await.unwrap();
            let mut state = handle.state.lock().await;
            state.clear_dirty(true, &BTreeSet::new());
        }

        let response = f
            .service
            .locate(&locate_request("dev", vec![
                locate_message_at(&a, "2099-01-01 12:00:00", 0, "一问", "m1"),
                locate_message_at(&a, "2099-01-01 12:01:00", 0, "二问", "m2"),
            ]))
            .await
            .unwrap();

        assert!(response.events.is_empty(), "空树不涉及任何事件");
        assert_eq!(response.groups.len(), 1, "同通道整批一组");
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert!(response.groups[0].event_id.is_none());
        assert!(f.model.prompts().is_empty(), "空树不调模型");
    }

    /// locate 对每个通道只调用一次 archive：同一次提取里不会重复处理某个通道
    #[tokio::test]
    async fn locate_archives_each_message_channel_once() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        // A 已归档过：存储里存在该通道，但水位之后没有待归档记录
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "旧记录", "m1").await;
        notify(&f, "dev", &a).await;
        query(&f, "dev").await;
        let before = f.model.prompts().len();

        notify(&f, "dev", &a).await;
        f.model.push_group(&[0, 1], false);
        f.model.push_level(LevelDecision::New);
        let response = f
            .service
            .locate(&locate_request("dev", vec![
                locate_message_at(&a, "2099-01-01 12:00:00", 0, "一问", "m2"),
                locate_message_at(&a, "2099-01-01 12:01:00", 0, "二问", "m3"),
            ]))
            .await
            .unwrap();

        let prompts: Vec<String> = f.model.prompts().into_iter().skip(before).collect();
        assert_eq!(
            prompts.iter().filter(|prompt| prompt.contains("\"groups\"")).count(),
            1,
            "该通道只归档一次（补齐循环不会重复处理已被 process_pending 覆盖的通道）"
        );
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert!(response.groups[0].event_id.is_none());
        assert_eq!(pending_len(&f, "dev", &a).await, 0);
    }

    /// 请求跨通道、且靠后的通道有待归档记录（走合并路径）时，
    /// 返回的分组下标必须是**请求全局下标**，不能是该通道批次内的局部下标
    #[tokio::test]
    async fn locate_reports_request_indices_for_merged_channels() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        append_store_channel_with_id(&root, "0", "dev", &b, "2099-01-01 10:00:00", 1, 0, "B 的记录", "m1").await;
        notify(&f, "dev", &b).await;

        let response = f
            .service
            .locate(&locate_request("dev", vec![
                locate_message_at(&a, "2099-01-01 12:00:00", 0, "A 的消息", "ma"),
                locate_message_at(&b, "2099-01-01 12:01:00", 0, "B 的消息", "m1"),
            ]))
            .await
            .unwrap();

        let mut groups: Vec<(Vec<usize>, Option<String>)> = response
            .groups
            .iter()
            .map(|group| (group.messages.clone(), group.event_id.as_ref().map(|id| id.as_str().to_string())))
            .collect();
        groups.sort();
        assert_eq!(
            groups,
            vec![(vec![0], None), (vec![1], Some("e1".to_string()))],
            "B 的消息下标应是请求全局下标 1，而不是该通道批次内的 0"
        );
        assert_eq!(events_of(&response), vec!["e1"]);
    }

    #[tokio::test]
    async fn locate_dedups_only_within_the_message_channel() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        // 两个通道的记录用同一个 msg_id：去重只看本通道的记录
        append_store_channel_with_id(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "A 自己的记录", "m1").await;
        append_store_channel_with_id(&root, "0", "dev", &b, "2099-01-01 10:00:00", 2, 0, "B 自己的记录", "m1").await;
        notify(&f, "dev", &a).await;
        notify(&f, "dev", &b).await;

        // 消息属于 B 通道：与 B 通道同 msg_id 的记录去重（不能命中 A 通道的同 msg_id 记录）
        let response = f
            .service
            .locate(&locate_request("dev", vec![locate_message_at(&b, "2099-01-01 10:00:00", 0, "B 通道的问题", "m1")]))
            .await
            .unwrap();

        let prompts = group_prompts(&f);
        assert_eq!(prompts.len(), 2, "两个通道各一次分组");
        let b_prompt = prompts.last().unwrap();
        assert!(b_prompt.contains("B 自己的记录"), "B 的消息与 B 通道的记录合并分析");
        assert!(!b_prompt.contains("B 通道的问题"), "重复的新消息自身不进批次");
        assert_eq!(records_of(&f, "dev", "e1").await.len(), 1, "A 通道照常归档");
        assert_eq!(records_of(&f, "dev", "e2").await.len(), 1, "B 通道只有自己的记录落盘");
        assert_eq!(events_of(&response), vec!["e2"], "消息命中 B 通道的事件");
        assert_eq!(pending_len(&f, "dev", &b).await, 0);
    }

    #[tokio::test]
    async fn locate_new_message_group_reads_through_without_creating_events() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "旧话题").await;
        notify(&f, "dev", &a).await;
        query(&f, "dev").await;
        assert_eq!(current_event(&f, "dev", &a).await.as_deref(), Some("e1"));

        // 待归档记录 C 与新消息 B 分属两组：两组都走同一套逐层判定，只有有记录的 C 会落地
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:00:00", 2, 0, "待归档的新话题").await;
        notify(&f, "dev", &a).await;
        f.model.push_groups(vec![
            crate::model::MessageGroup { messages: vec![0], is_current: false },
            crate::model::MessageGroup { messages: vec![1], is_current: false },
        ]);
        f.model.push_level(LevelDecision::New);
        f.model.push_level(LevelDecision::Merge("e1".into()));
        let before = f.model.prompts().len();
        let response = f
            .service
            .locate(&locate_request("dev", vec![locate_message_at(&a, "2099-01-01 12:00:00", 0, "延续旧话题", "")]))
            .await
            .unwrap();

        let level_prompts = f
            .model
            .prompts()
            .into_iter()
            .skip(before)
            .filter(|prompt| prompt.contains("当层事件"))
            .count();
        assert_eq!(level_prompts, 2, "有记录的组与只定位的组走同一套逐层判定");
        assert_eq!(tree_len(&f, "dev").await, 2, "只含新消息的组不新建事件");
        assert_eq!(node(&f, "dev", "e2").await.unwrap().record_count, 1, "待归档记录落到新事件");
        assert_eq!(node(&f, "dev", "e1").await.unwrap().record_count, 1, "新消息不落盘");
        assert_eq!(events_of(&response), vec!["e1"]);
        assert!(!response.events[0].is_current, "当前事件是待归档记录新建的 e2");
        assert_eq!(response.groups[0].messages, vec![0]);
        assert_eq!(response.groups[0].event_id.as_ref().map(|id| id.as_str()), Some("e1"));
    }

    #[tokio::test]
    async fn locate_chunks_the_merged_batch() {
        let f = fixture_with_batch(2).await;
        let root = f.service.root();
        let a = channel_a();
        for i in 1..=3u64 {
            append_store_channel(&root, "0", "dev", &a, &format!("2099-01-01 10:0{}:00", i), i, 0, &format!("记录{}", i)).await;
        }
        notify(&f, "dev", &a).await;
        let response = f
            .service
            .locate(&locate_request("dev", vec![locate_message_at(&a, "2099-01-01 12:00:00", 0, "新消息", "")]))
            .await
            .unwrap();

        assert_eq!(group_prompts(&f).len(), 2, "3 条记录 + 1 条新消息按每块 2 条分成 2 块");
        assert_eq!(pending_len(&f, "dev", &a).await, 0);
        assert_eq!(watermark_point(&f, "dev", &a).await.1, 3, "新消息不影响水位");
        assert_eq!(records_of(&f, "dev", "e1").await.len(), 3, "新消息不落盘");
        assert_eq!(events_of(&response), vec!["e1"]);
        assert_eq!(response.groups[0].messages, vec![0]);
    }

    #[tokio::test]
    async fn locate_uses_current_event_of_the_message_channel() {
        let f = fixture().await;
        let root = f.service.root();
        let (a, b) = (channel_a(), channel_b());
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "A 的消息").await;
        append_store_channel(&root, "0", "dev", &b, "2099-01-01 11:00:00", 2, 0, "B 的消息").await;
        notify(&f, "dev", &a).await;
        let tree = query(&f, "dev").await;

        let current = |channel: &ChannelKey| -> String {
            current_pointer(&tree, channel).map(|pointer| pointer.event_id.as_str().to_string()).expect("通道应有当前事件")
        };
        let (current_a, current_b) = (current(&a), current(&b));
        assert_ne!(current_a, current_b, "两个通道各有自己的当前事件");

        let locate = |channel: &ChannelKey, text: &str| {
            let request = LocateRequest {
                agent_id: Arc::new("0".into()),
                role: Arc::new("dev".into()),
                messages: vec![locate_message(channel, text)],
            };
            let service = &f.service;
            async move { service.locate(&request).await.unwrap() }
        };

        f.model.push_group(&[0], true);
        let from_a = locate(&a, "继续 A 的话题").await;
        assert_eq!(events_of(&from_a), vec![current_a.as_str()]);
        assert!(from_a.events[0].is_current);
        f.model.push_group(&[0], true);
        let from_b = locate(&b, "继续 B 的话题").await;
        assert_eq!(events_of(&from_b), vec![current_b.as_str()]);
        assert!(from_b.events[0].is_current);

        // 同一个事件用另一个通道的消息定位：不是该通道的当前事件
        f.model.push_group(&[0], false);
        f.model.push_level(LevelDecision::Merge(current_b.clone()));
        let cross = locate(&a, "从 A 通道问 B 的话题").await;
        assert_eq!(events_of(&cross), vec![current_b.as_str()]);
        assert!(!cross.events[0].is_current, "is_current 按新消息通道判定");
    }

    #[tokio::test]
    async fn event_queries_split_summary_and_records() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 0, "问一").await;
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:01:00", 2, 1, "答一").await;
        notify(&f, "dev", &a).await;
        process(&f, "dev").await;

        // 摘要：只有节点（摘要文本 + 记录数），不带原文
        let summary = f.service.event_summary("0", "dev", "e1").await.unwrap();
        assert_eq!(summary.event.event_id.as_str(), "e1");
        assert_eq!(summary.event.record_count, 2);
        assert!(summary.event.summary.as_str().starts_with("摘要"));

        // 全部原文
        let all = f.service.event_records("0", "dev", "e1").await.unwrap();
        assert_eq!(all.records.len(), 2);
        assert_eq!(all.records[0].sn(), 1);
        assert_eq!(all.records[1].sn(), 2);

        // 指定之后：把上一条记录的 time/sn/record_type 当游标回传
        let after = f
            .service
            .event_records_after("0", "dev", "e1", &EventCursor::of(&all.records[0]))
            .await
            .unwrap();
        assert_eq!(after.records.len(), 1);
        assert_eq!(after.records[0].sn(), 2);

        // 结尾若干条（升序返回；0 条为空）
        let tail = f.service.event_records_tail("0", "dev", "e1", 1).await.unwrap();
        assert_eq!(tail.records.len(), 1);
        assert_eq!(tail.records[0].sn(), 2);
        assert!(f.service.event_records_tail("0", "dev", "e1", 0).await.unwrap().records.is_empty());

        // 事件不存在：摘要与原文查询都报错
        assert!(f.service.event_summary("0", "dev", "e9").await.is_err());
        assert!(f.service.event_records("0", "dev", "e9").await.is_err());
        assert!(f
            .service
            .event_records_after("0", "dev", "e9", &EventCursor::of(&all.records[0]))
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
        let first = f.service.manual_trigger(&ScopeId::new("0", "dev")).await.unwrap();
        assert_eq!(first.archived, 4, "手动触发处理全部待处理记录");
        assert_eq!(first.event_ids, vec!["e1".to_string()]);
        let second = f.service.manual_trigger(&ScopeId::new("0", "dev")).await.unwrap();
        assert_eq!(second.archived, 0, "水位已推进，无剩余记录");

        let response = f.service.rebuild(&ScopeId::new("0", "dev")).await.unwrap();
        assert_eq!(response.archived, 4, "重建归档全部历史记录");
        assert_eq!(response.event_count, 1, "重建结果与首次一致");
        assert_eq!(pending_len(&f, "dev", &a).await, 0);
        assert!(current_event(&f, "dev", &a).await.is_some(), "重建后通道当前事件指针保留");
    }

    #[tokio::test]
    async fn event_mode_scope_keeps_single_event() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev-e1", &a, "2099-01-01 10:00:00", 1, 1, "事件模式").await;
        append_store_channel(&root, "0", "dev-e1", &a, "2099-01-01 10:01:00", 2, 0, "继续").await;
        notify(&f, "dev-e1", &a).await;
        let tree = query(&f, "dev-e1").await;

        assert_eq!(tree.nodes.len(), 1, "事件模式整体一个事件");
        assert_eq!(tree.nodes[0].mode, EventMode::Event);
        assert_eq!(tree.nodes[0].record_count, 2);
        assert_eq!(current_pointer(&tree, &a).unwrap().event_id.as_str(), "e1");
        // 事件模式不参与分组与分层归并：模型只被要求生成摘要
        assert!(f.model.prompts().iter().all(|p| !p.contains("分组") && !p.contains("当层事件")));
    }

    #[tokio::test]
    async fn nested_events_cascade_summaries() {
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 10:00:00", 1, 1, "父内容").await;
        notify(&f, "dev", &a).await;
        query(&f, "dev").await;

        // 模拟后续「合并为大事件」程序的产物：e1 成为父事件，子事件 e2 承载内容
        {
            let handle = f.service.manager().get(&ScopeId::new("0", "dev")).unwrap();
            let mut state = handle.state.lock().await;
            state.tree.create_node("e2", Some("e1"), EventMode::Role, "t");
            state.current_events.insert(a.clone(), "e2".into());
        }

        // 下一批标记为当前事件组 → 直接并入 e2 → e2 摘要重算后级联重算父事件 e1
        append_store_channel(&root, "0", "dev", &a, "2099-01-01 11:00:00", 2, 1, "子内容").await;
        notify(&f, "dev", &a).await;
        query(&f, "dev").await;

        assert_eq!(node(&f, "dev", "e2").await.unwrap().record_count, 1, "本批只有一条记录");
        let inputs = f.model.summary_inputs();
        assert!(inputs.iter().any(|i| i.contains("子事件摘要")), "父摘要涵盖子事件内容");
        assert!(inputs.last().unwrap().contains("子事件摘要"), "祖先最后重算");
    }

    #[tokio::test]
    async fn appended_records_are_visible_to_the_next_read() {
        // 回归：tokio::fs::File 的 write_all 可能只把数据交给后台阻塞任务就返回（偶发丢记录），
        // 写入路径必须 flush，否则紧接着的读取（懒处理读共享记忆文件）会看不到刚写入的记录
        let f = fixture().await;
        let root = f.service.root();
        let a = channel_a();
        let handle = f.service.manager().open(ScopeId::new("0", "dev")).await.unwrap();
        for i in 0..100u64 {
            let time = format!("2099-01-01 10:{:02}:{:02}", i / 60, i % 60);
            append_store_channel(&root, "0", "dev", &a, &time, i + 1, (i % 2) as usize, "内容").await;
            let view = f.service.store_view(&handle).await.unwrap();
            assert_eq!(view.records.len(), i as usize + 1, "第 {} 轮写入后立即读取的记录数不对", i + 1);
        }
    }
}
