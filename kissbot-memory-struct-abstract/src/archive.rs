//! 归档器：批次分组、分组落位与摘要级联重算。
//!
//! 角色模式一次处理某通道的一批条目，分两段：
//! 1. 第 1 轮（分组轮）：把当前事件与该批条目一起交给模型，模型按事件分组并标记属于当前事件的组；
//!    标记为当前事件的组直接并入当前事件（不下潜）；
//! 2. 第 2 轮起分层归并：其余组以组为单位，自顶层逐层归并（非叶子继续下潜），或在本层新增叶子事件。
//!
//! 批次条目既可以是待归档记录，也可以是只用于定位的新消息（见 [`BatchItem`]）：定位请求触发懒处理时
//! 先按通道与待归档记录去重（重复的新消息用已存版本分析一次、只返回所属事件），其余新消息随批次一起
//! 参与分组但不落盘。只含新消息的组按定位处理：只读下探，不新建事件。
//!
//! 事件之间的关联关系与「多个事件合并为大事件」由后续定时程序产生，因此当前新建的事件都是叶子。

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use kissbot_api::{ChannelKey, EventBrief, EventMode, LocateMessage, RecordKind};
use tracing::warn;

use crate::error::{Error, Result};
use crate::locate::{find_event, render_message};
use crate::model::{DecisionModel, LevelDecision};
use crate::records::{
    RecordBody, RecordEntry, append_event_records, read_event_records, render_entry,
    render_event_record,
};
use crate::scope::{ScopeHandle, ScopeId, ScopeState};
use crate::tree::Tree;

/// 归档结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveOutcome {
    /// 本次归档的记录条数
    pub archived: u64,
    /// 内容落入的事件（按落位顺序）
    pub event_ids: Vec<String>,
}

/// 统一分析批次的一个条目
#[derive(Debug, Clone)]
pub enum BatchItem {
    /// 共享记忆中的待归档记录；`aliases` 是与之重复的新消息在请求中的下标
    /// （分析只用已存版本一次，重复的新消息只返回所属事件）
    Stored { record: RecordEntry, aliases: Vec<usize> },
    /// 只用于定位的新消息（请求内下标）
    New { index: usize, message: LocateMessage },
}

impl BatchItem {
    /// 排序键：条目时间
    fn time(&self) -> &str {
        match self {
            BatchItem::Stored { record, .. } => record.time.as_str(),
            BatchItem::New { message, .. } => message.time.as_str(),
        }
    }

    /// 送模型的文本形态
    fn render(&self) -> String {
        match self {
            BatchItem::Stored { record, .. } => render_entry(record),
            BatchItem::New { message, .. } => render_message(message),
        }
    }

    /// 待归档记录（只用于定位的新消息没有）
    fn record(&self) -> Option<&RecordEntry> {
        match self {
            BatchItem::Stored { record, .. } => Some(record),
            BatchItem::New { .. } => None,
        }
    }

    /// 是否是待归档记录（会落盘归档）
    pub fn is_stored(&self) -> bool {
        matches!(self, BatchItem::Stored { .. })
    }

    /// 该条目涉及的新消息下标
    fn messages(&self) -> &[usize] {
        match self {
            BatchItem::Stored { aliases, .. } => aliases,
            BatchItem::New { index, .. } => std::slice::from_ref(index),
        }
    }
}

/// 批次分析后的分组落位（定位返回给调用方）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceGroup {
    /// 组内新消息在请求中的下标（升序；只含待归档记录的组为空）
    pub messages: Vec<usize>,
    /// 该组所属事件；`None` 表示该组只含新消息且只读下探未命中（新话题）
    pub event_id: Option<String>,
}

/// 分组落位（内部）：批次内条目下标、待归档记录与所属事件
struct Placement {
    event_id: Option<String>,
    items: Vec<usize>,
    records: Vec<RecordEntry>,
}

/// 把某通道的待归档记录与新消息合并为统一分析批次。
///
/// 与新消息重复的记录保留已存版本、把新消息记为该记录的别名（同一内容只分析一次）；
/// 其余新消息独立成条。按时间升序排列，同一时刻已存记录在前，各自保持原有顺序。
pub fn merge_batch(records: &[RecordEntry], messages: &[&LocateMessage]) -> Vec<BatchItem> {
    let mut items: Vec<BatchItem> = records
        .iter()
        .map(|record| BatchItem::Stored { record: record.clone(), aliases: Vec::new() })
        .collect();
    let mut taken = vec![false; records.len()];
    for (index, message) in messages.iter().enumerate() {
        // 一条已存记录只吸收一条重复的新消息：同一内容在两侧都出现多次时，多出来的新消息按新消息处理
        let matched = records
            .iter()
            .enumerate()
            .find(|(record_index, record)| !taken[*record_index] && duplicates(record, message));
        match matched {
            Some((record_index, _)) => {
                taken[record_index] = true;
                if let BatchItem::Stored { aliases, .. } = &mut items[record_index] {
                    aliases.push(index);
                }
            }
            None => items.push(BatchItem::New { index, message: (*message).clone() }),
        }
    }
    items.sort_by(|a, b| a.time().cmp(b.time()));
    items
}

/// 新消息与已存记录是否同一条：时间 + 发送者 + is_self + 内容全同
/// （与调用侧输入处理器的去重键对齐）。
fn duplicates(record: &RecordEntry, message: &LocateMessage) -> bool {
    let RecordBody::Channel(channel) = &record.body else {
        return false;
    };
    channel.time.as_str() == message.time.as_str()
        && channel.user_name.as_str() == message.user_name.as_str()
        && channel.is_self == message.is_self
        && channel.content == message.content
}

/// 归档器
pub struct Archiver {
    model: Arc<dyn DecisionModel>,
    max_depth: usize,
}

impl Archiver {
    pub fn new(model: Arc<dyn DecisionModel>, max_depth: usize) -> Self {
        Self { model, max_depth }
    }

    /// 归档某通道待处理的全部记录（只含待归档记录，不含只用于定位的新消息）。
    pub async fn archive_pending(
        &self,
        handle: &ScopeHandle,
        state: &mut ScopeState,
        channel: &ChannelKey,
        records: &[RecordEntry],
    ) -> Result<ArchiveOutcome> {
        let items: Vec<BatchItem> = records
            .iter()
            .map(|record| BatchItem::Stored { record: record.clone(), aliases: Vec::new() })
            .collect();
        self.archive_items(handle, state, channel, &items).await.map(|(outcome, _)| outcome)
    }

    /// 分析并归档一个批次：已存记录落盘归档，只用于定位的新消息只产出所属事件。
    ///
    /// 与 [`Self::archive_pending`] 同一套分组与落位；区别是批次里可能夹带新消息，
    /// 且只含新消息的组按定位处理（命中当前事件或只读下探，不新建事件）。
    ///
    /// 所有模型调用都在落盘之前完成；任一模型调用失败时恢复内存状态，磁盘与水位保持不变，
    /// 这批条目留待下次查询重试（见 spec：解析失败按失败处理并保留原状态）。
    pub async fn archive_items(
        &self,
        handle: &ScopeHandle,
        state: &mut ScopeState,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<(ArchiveOutcome, Vec<PlaceGroup>)> {
        if items.is_empty() {
            return Ok((ArchiveOutcome { archived: 0, event_ids: Vec::new() }, Vec::new()));
        }
        let snapshot = state.clone();
        match self.archive_items_inner(handle, state, channel, items).await {
            Ok(result) => Ok(result),
            Err(e) => {
                // 判定阶段会在内存树上新建节点：失败时回到调用前状态，避免留下空节点
                *state = snapshot;
                Err(e)
            }
        }
    }

    async fn archive_items_inner(
        &self,
        handle: &ScopeHandle,
        state: &mut ScopeState,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<(ArchiveOutcome, Vec<PlaceGroup>)> {
        let now = crate::records::now_string();
        let scope = handle.id.clone();

        // 1. 分组落位：得到「事件 → 本批落入该事件的记录」与各组的消息映射（按组序）
        let placements = match handle.mode {
            // 事件模式：作用域整体一个顶层叶子事件，不分组、不参与判定
            EventMode::Event => {
                let event_id = handle
                    .event_id
                    .clone()
                    .ok_or_else(|| Error::StateError(format!("事件模式作用域缺少事件标识: {}", scope)))?;
                if !state.tree.contains(&event_id) {
                    state.tree.create_node(&event_id, None, EventMode::Event, &now);
                }
                let indices: Vec<usize> = (0..items.len()).collect();
                let records = stored_records(items, &indices);
                vec![Placement { event_id: Some(event_id), items: indices, records }]
            }
            EventMode::Role => self.group_and_place(&scope, state, channel, items).await?,
        };

        let mut new_records: HashMap<String, Vec<RecordEntry>> = HashMap::new();
        let mut touched: BTreeSet<String> = BTreeSet::new();
        let mut event_ids: Vec<String> = Vec::new();
        let mut groups: Vec<PlaceGroup> = Vec::with_capacity(placements.len());
        for placement in &placements {
            let mut messages: Vec<usize> =
                placement.items.iter().flat_map(|index| items[*index].messages().iter().copied()).collect();
            messages.sort_unstable();
            groups.push(PlaceGroup { messages, event_id: placement.event_id.clone() });
            // 只含新消息的组不落盘、不推进任何状态
            let Some(event_id) = placement.event_id.as_ref() else {
                debug_assert!(placement.records.is_empty(), "没有事件的组只可能只含新消息");
                continue;
            };
            if placement.records.is_empty() {
                continue;
            }
            new_records.entry(event_id.clone()).or_default().extend(placement.records.iter().cloned());
            touched.insert(event_id.clone());
            if !event_ids.contains(event_id) {
                event_ids.push(event_id.clone());
            }
        }
        let records: Vec<RecordEntry> = items.iter().filter_map(BatchItem::record).cloned().collect();

        // 2. 摘要重算：叶子在前（自底向上），父事件摘要涵盖自身记录与子事件摘要；全部成功后落盘
        let mut summaries: HashMap<String, String> = HashMap::new();
        for id in self.recompute_order(&state.tree, &touched) {
            if let Some(summary) = self.compute_summary(handle, state, &id, &new_records, &summaries).await? {
                summaries.insert(id, summary);
            }
        }

        // 3. 落盘原文副本（按事件聚合）
        for (event_id, group) in &new_records {
            append_event_records(&handle.event_dir(event_id), group).await?;
        }

        // 4. 内存：应用记录与摘要；当前事件指针指向最后一批有内容的记录落入的事件
        for (event_id, group) in &new_records {
            state.tree.apply_records(event_id, group, &now);
        }
        for (id, summary) in &summaries {
            state.tree.set_summary(id, summary, &now);
        }
        if let Some(last) = event_ids.last() {
            state.current_events.insert(channel.clone(), last.clone());
        }
        advance_watermark(state.watermarks.entry(channel.clone()).or_default(), &records);

        Ok((ArchiveOutcome { archived: records.len() as u64, event_ids }, groups))
    }

    /// 角色模式的分组落位：第 1 轮分组（并标记当前事件组），当前事件组直接并入、
    /// 其余组各自走第 2 轮起的分层归并；只含新消息的组按定位处理（只读下探）。
    async fn group_and_place(
        &self,
        scope: &ScopeId,
        state: &mut ScopeState,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<Vec<Placement>> {
        let text = render_batch(items);
        let current = state.current_events.get(channel).cloned();
        let current_brief = current.as_deref().and_then(|id| state.tree.brief(id));
        let groups = self.model.group(scope, &text, items.len(), current_brief.as_ref()).await?;

        let mut placements: Vec<Placement> = Vec::with_capacity(groups.len());
        for group in groups {
            let group_items = group.messages.clone();
            let records = stored_records(items, &group_items);
            let text = render_group(items, &group_items);
            let event_id = if records.is_empty() {
                // 组里只有新消息：定位只读，不新建事件——命中当前事件，或只读下探，未命中即新话题
                match current.clone().filter(|_| group.is_current) {
                    Some(current) => Some(current),
                    None => find_event(self.model.as_ref(), scope, &state.tree, &text, self.max_depth).await?,
                }
            } else if group.is_current {
                // 标记为当前事件的组直接并入当前事件
                Some(current.clone().ok_or_else(|| {
                    Error::ModelApiError(format!(
                        "第 1 轮把分组标记为当前事件，但通道 {} 没有当前事件（作用域 {}）",
                        channel.messenger_id, scope
                    ))
                })?)
            } else {
                // 其余组以组为单位自顶层逐层归并
                Some(self.merge_from(scope, &mut state.tree, &text, None).await?)
            };
            placements.push(Placement { event_id, items: group_items, records });
        }
        Ok(placements)
    }

    /// 自 `parent`（None 为顶层）起逐层归并：并入当层某事件（非叶子继续下潜）/ 在本层新增叶子
    async fn merge_from(
        &self,
        scope: &ScopeId,
        tree: &mut Tree,
        text: &str,
        parent: Option<String>,
    ) -> Result<String> {
        let now = crate::records::now_string();
        let mut parent = parent;
        let mut level = match &parent {
            Some(id) => tree.children_ids(id),
            None => tree.top_level_ids(),
        };

        for _ in 0..self.max_depth {
            if level.is_empty() {
                let event_id = tree.allocate_event_id();
                tree.create_node(&event_id, parent.as_deref(), EventMode::Role, &now);
                return Ok(event_id);
            }
            let briefs: Vec<EventBrief> = tree.briefs(&level);
            match self.model.decide_level(scope, text, &briefs).await? {
                LevelDecision::New => {
                    let event_id = tree.allocate_event_id();
                    tree.create_node(&event_id, parent.as_deref(), EventMode::Role, &now);
                    return Ok(event_id);
                }
                LevelDecision::Merge(event_id) => {
                    if !tree.contains(&event_id) {
                        return Err(Error::ModelApiError(format!(
                            "分层归并给出不存在的事件 {}（作用域 {}）",
                            event_id, scope
                        )));
                    }
                    if tree.is_leaf(&event_id) {
                        return Ok(event_id);
                    }
                    // 非叶子：下潜到其子层继续判定
                    level = tree.children_ids(&event_id);
                    parent = Some(event_id);
                }
            }
        }

        // 层数达到上限：在当层新开叶子事件（防御模型环路）
        warn!("[abstract] reach max event depth {} for scope {}", self.max_depth, scope);
        let event_id = tree.allocate_event_id();
        tree.create_node(&event_id, parent.as_deref(), EventMode::Role, &now);
        Ok(event_id)
    }

    /// 摘要重算顺序：受影响的叶子在前、祖先在后
    fn recompute_order(&self, tree: &Tree, touched: &BTreeSet<String>) -> Vec<String> {
        let mut all: BTreeSet<String> = BTreeSet::new();
        for event_id in touched {
            all.insert(event_id.clone());
            for ancestor in tree.ancestors(event_id) {
                all.insert(ancestor);
            }
        }
        let mut ordered: Vec<(usize, String)> = all
            .into_iter()
            .filter(|id| tree.contains(id))
            .map(|id| (tree.ancestors(&id).len(), id))
            .collect();
        // 深度大（更靠近叶子）先算；同深度按 id 稳定排序
        ordered.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        ordered.into_iter().map(|(_, id)| id).collect()
    }

    /// 计算单个事件的新摘要：自身直接记录（已落盘 + 本批新增）+ 子事件摘要 → 模型摘要。
    ///
    /// 返回 `None` 表示该事件没有内容（无记录且无子事件），无需摘要。
    async fn compute_summary(
        &self,
        handle: &ScopeHandle,
        state: &ScopeState,
        event_id: &str,
        new_records: &HashMap<String, Vec<RecordEntry>>,
        computed: &HashMap<String, String>,
    ) -> Result<Option<String>> {
        let Some(node) = state.tree.get(event_id) else {
            return Ok(None);
        };
        let records = read_event_records(&handle.event_dir(event_id)).await?;
        let extra = new_records.get(event_id);
        if records.is_empty() && extra.map(|e| e.is_empty()).unwrap_or(true) && node.children.is_empty() {
            return Ok(None);
        }

        let mut content = String::new();
        if !records.is_empty() || extra.map(|e| !e.is_empty()).unwrap_or(false) {
            content.push_str("记录：\n");
            for record in &records {
                content.push_str(&render_event_record(record));
                content.push('\n');
            }
            for entry in extra.into_iter().flatten() {
                content.push_str(&render_entry(entry));
                content.push('\n');
            }
        }
        if !node.children.is_empty() {
            content.push_str("\n子事件摘要：\n");
            for child in state.tree.children_ids(event_id) {
                // 本批刚重算的子事件摘要优先（父摘要必须涵盖子事件的最新内容）
                let summary = computed
                    .get(&child)
                    .cloned()
                    .or_else(|| state.tree.brief(&child).map(|b| b.summary.as_str().to_string()))
                    .unwrap_or_default();
                content.push_str(&format!("- {}: {}\n", child, summary));
            }
        }

        Ok(Some(self.model.summarize(&handle.id, &content).await?))
    }
}

/// 批次文本：每行 `[序号] 时间 内容`
pub fn render_batch(items: &[BatchItem]) -> String {
    render_group(items, &(0..items.len()).collect::<Vec<_>>())
}

/// 组内条目文本：`[序号]` 为组内序号
fn render_group(items: &[BatchItem], indices: &[usize]) -> String {
    indices
        .iter()
        .enumerate()
        .map(|(number, index)| format!("[{}] {}", number, items[*index].render()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 批次内指定下标的待归档记录（保持下标顺序）
fn stored_records(items: &[BatchItem], indices: &[usize]) -> Vec<RecordEntry> {
    indices.iter().filter_map(|index| items[*index].record().cloned()).collect()
}

/// 水位推进到已归档范围末尾
pub fn advance_watermark(watermark: &mut kissbot_api::Watermark, batch: &[RecordEntry]) {
    for kind in RecordKind::ALL {
        let point = watermark.point_mut(kind);
        for entry in batch.iter().filter(|e| e.kind == kind) {
            if (entry.time.as_str(), entry.sn) > (point.time.as_str(), point.sn) {
                point.time = Arc::new(entry.time.as_str().to_string());
                point.sn = entry.sn;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_api::ChannelRecord;
    use kissbot_api::Content;

    fn channel(time: &str, sn: u64, is_self: usize, text: &str) -> RecordEntry {
        RecordEntry {
            kind: RecordKind::Channel,
            time: Arc::new(time.to_string()),
            sn,
            body: RecordBody::Channel(Arc::new(ChannelRecord {
                user_id: Arc::new("u1".into()),
                self_user_id: Arc::new("self".into()),
                messenger_id: Arc::new("web".into()),
                group_id: Arc::new("g1".into()),
                is_self,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new("alice".into()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new(text.to_string())),
                time: Arc::new(time.to_string()),
                sn,
            })),
        }
    }

    /// 待归档记录形态的批次条目
    fn stored(time: &str, sn: u64, is_self: usize, text: &str) -> BatchItem {
        BatchItem::Stored { record: channel(time, sn, is_self, text), aliases: Vec::new() }
    }

    fn message(time: &str, text: &str) -> LocateMessage {
        LocateMessage {
            time: Arc::new(time.to_string()),
            user_name: Arc::new("alice".into()),
            is_self: 0,
            messenger_id: Arc::new("web".into()),
            self_user_id: Arc::new("self".into()),
            group_id: Arc::new("g1".into()),
            content: Content::Text(Arc::new(text.to_string())),
        }
    }

    #[test]
    fn advance_watermark_uses_batch_max_per_kind() {
        let mut watermark = kissbot_api::Watermark::default();
        let batch = vec![
            channel("2026-09-22 10:00:00", 1, 0, "a"),
            channel("2026-09-22 10:05:00", 2, 1, "b"),
        ];
        advance_watermark(&mut watermark, &batch);
        assert_eq!(watermark.channel.time.as_str(), "2026-09-22 10:05:00");
        assert_eq!(watermark.channel.sn, 2);
        assert_eq!(watermark.think.sn, 0, "未归档类型水位不变");
    }

    #[test]
    fn render_batch_numbers_lines() {
        let batch = vec![stored("2026-09-22 10:00:00", 1, 0, "a"), stored("2026-09-22 10:01:00", 2, 1, "b")];
        let text = render_batch(&batch);
        assert_eq!(text, "[0] [2026-09-22 10:00:00] alice: a\n[1] [2026-09-22 10:01:00] alice: b");
    }

    #[test]
    fn merge_batch_dedups_messages_against_records() {
        let records = vec![channel("2099-01-01 10:00:00", 1, 0, "A"), channel("2099-01-01 10:02:00", 2, 1, "C")];
        // 新消息：A 与已存记录重复，B 是新的
        let a = message("2099-01-01 10:00:00", "A");
        let b = message("2099-01-01 10:03:00", "B");
        let items = merge_batch(&records, &[&a, &b]);

        assert_eq!(items.len(), 3, "重复的新消息不单独成条");
        let text = render_batch(&items);
        assert_eq!(
            text,
            "[0] [2099-01-01 10:00:00] alice: A\n[1] [2099-01-01 10:02:00] alice: C\n[2] [2099-01-01 10:03:00] alice: B",
            "重复的新消息只以已存版本出现一次"
        );
        match &items[0] {
            BatchItem::Stored { aliases, .. } => assert_eq!(aliases, &[0], "已存 A 吸收重复的新消息 0"),
            other => panic!("应为已存记录: {:?}", other),
        }
        match &items[1] {
            BatchItem::Stored { aliases, .. } => assert!(aliases.is_empty(), "已存 C 没有重复的新消息"),
            other => panic!("应为已存记录: {:?}", other),
        }
        match &items[2] {
            BatchItem::New { index, message } => {
                assert_eq!(*index, 1, "新消息保留请求下标");
                assert_eq!(message.content, Content::Text(Arc::new("B".into())));
            }
            other => panic!("应为新消息: {:?}", other),
        }
    }

    #[test]
    fn merge_batch_requires_same_time_user_self_and_content() {
        let records = vec![channel("2099-01-01 10:00:00", 1, 0, "A")];

        let same = message("2099-01-01 10:00:00", "A");
        assert_eq!(merge_batch(&records, &[&same]).len(), 1, "全同才去重");

        let other = message("2099-01-01 10:00:00", "A2");
        assert_eq!(merge_batch(&records, &[&other]).len(), 2, "内容不同不去重");
        let other = message("2099-01-01 10:01:00", "A");
        assert_eq!(merge_batch(&records, &[&other]).len(), 2, "时间不同不去重");
        let mut other = message("2099-01-01 10:00:00", "A");
        other.is_self = 1;
        assert_eq!(merge_batch(&records, &[&other]).len(), 2, "is_self 不同不去重");
        let mut other = message("2099-01-01 10:00:00", "A");
        other.user_name = Arc::new("bob".into());
        assert_eq!(merge_batch(&records, &[&other]).len(), 2, "发送者不同不去重");
        // 思考/工具记录不参与新消息去重（body 不是通道记录）
        let time = Arc::new("2099-01-01 10:00:00".to_string());
        let think = RecordEntry {
            kind: RecordKind::Think,
            time: time.clone(),
            sn: 1,
            body: RecordBody::Think(Arc::new(kissbot_api::ThinkRecord {
                reasoning_content: Some(Arc::new("A".into())),
                thinking: None,
                key: Arc::new("k1".into()),
                time,
                sn: 1,
            })),
        };
        assert_eq!(merge_batch(&[think], &[&same]).len(), 2, "非通道记录不去重");
    }

    #[test]
    fn merge_batch_matches_duplicates_one_to_one() {
        let records = vec![channel("2099-01-01 10:00:00", 1, 0, "A")];
        let first = message("2099-01-01 10:00:00", "A");
        let second = message("2099-01-01 10:00:00", "A");
        let items = merge_batch(&records, &[&first, &second]);

        assert_eq!(items.len(), 2, "一条已存记录只吸收一条重复新消息，多出来的按新消息处理");
        match &items[0] {
            BatchItem::Stored { aliases, .. } => assert_eq!(aliases, &[0]),
            other => panic!("应为已存记录: {:?}", other),
        }
        match &items[1] {
            BatchItem::New { index, .. } => assert_eq!(*index, 1),
            other => panic!("应为新消息: {:?}", other),
        }
    }

    #[test]
    fn merge_batch_orders_by_time_with_records_first() {
        let records = vec![channel("2099-01-01 10:00:00", 1, 0, "已存")];
        let earlier = message("2099-01-01 09:00:00", "更早的新消息");
        let same_time = message("2099-01-01 10:00:00", "同一时刻的新消息");
        let items = merge_batch(&records, &[&earlier, &same_time]);

        let times: Vec<&str> = items.iter().map(BatchItem::time).collect();
        assert_eq!(times, vec!["2099-01-01 09:00:00", "2099-01-01 10:00:00", "2099-01-01 10:00:00"]);
        assert!(matches!(&items[0], BatchItem::New { .. }));
        assert!(matches!(&items[1], BatchItem::Stored { .. }), "同一时刻已存记录在前");
        assert!(matches!(&items[2], BatchItem::New { .. }));
    }

    #[test]
    fn recompute_order_puts_descendants_first() {
        let mut tree = Tree::new();
        tree.create_node("e1", None, EventMode::Role, "t");
        tree.create_node("e2", Some("e1"), EventMode::Role, "t");
        tree.create_node("e3", Some("e2"), EventMode::Role, "t");
        let touched: BTreeSet<String> = ["e3".to_string()].into_iter().collect();
        let archiver = Archiver::new(Arc::new(crate::stub::StubModel::new()), 4);
        let order = archiver.recompute_order(&tree, &touched);
        assert_eq!(order, vec!["e3".to_string(), "e2".to_string(), "e1".to_string()]);
    }

}
