//! 归档器：批次分组、分组落位与摘要级联重算。
//!
//! 角色模式一次处理某记忆后缀某通道的一批消息，分两个阶段：
//! 1. **先归档 pending**：待归档记录（已存记忆）不再定位，带 `meta.memory_summary.case_name` 的直接
//!    落到该 case，没有 meta 的直接跳过；meta 指向非叶子 case 时跳过（不更新当前指针）。全部
//!    pending 处理完后统一推进一次水位；
//! 2. **再定位新消息**：第 1 轮按 case 分组并标记属于当前 case 的组，当前 case 组直接并入
//!    当前 case；其余组以组为单位自顶层逐层归并（非叶子继续下潜），或在本层新增叶子 case。
//!
//! 事件模式（给出 `event_id`）不调分组轮：新消息归入事件根 `event-{event_id}`（叶子直接归入，
//! 非叶子时以它为根继续下潜；case 名即查找键，节点本身不带事件字段）。两种 case 共用一片森林、靠名称
//! 前缀区分（角色 `role-`、事件 `event-`），角色模式的逐层判定候选只取 `role-` 前缀，看不到事件叶子。
//!
//! 批次条目是**通道消息**（见 [`BatchItem`]）：归档阶段只包含已存记录，定位阶段只包含新消息。
//! 一条消息对应一条原文（`msg_id` 是分组标识），内容里的思考/工具记录直接内嵌、不单独成条；新消息不与已存
//! 记录去重（同一 `msg_id` 各占一个条目，加 `#序号` 区分），只参与分组但不落盘。只含新消息的组同样
//! 命中或建出 case（新话题建出的 case 记忆条数为 0）。
//!
//! Case 之间的关联关系与「多个 case 合并为大 case」由后续定时程序产生，因此当前新建的 case 都是叶子。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::{CaseBrief, ChannelKey, Content, LocateMessage, LocatedGroup, RecordPosition, RoleEvent};

use crate::error::{Error, Result};
use crate::model::{DecisionModel, GroupItem, LevelDecision};
use crate::records::{
    RecordEntry, StoreMessage, append_case_records, render_entry, synthesize_content,
};
use crate::agent_role::{AgentRole, AgentRoleHandle, AgentRoleState, role_event_of};
use crate::tree::{CaseTree, event_case_name, is_role_root};

/// 归档结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveOutcome {
    /// 本次归档的记录条数
    pub archived: u64,
    /// 内容落入的 case（按落位顺序）
    pub case_names: Vec<ArcStr>,
}

/// 一次归档调用的结果（记录 + 只用于定位的新消息）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveResult {
    /// pending 归档结果（无 pending 时为空）
    pub outcomes: Vec<ArchiveOutcome>,
    /// 消息分组（`LocatedGroup::messages` 是消息的 `msg_id`；只含记录的组不出现在定位结果里）
    pub groups: Vec<LocatedGroup>,
    /// 本次参与定位的新消息条数
    pub new_messages: usize,
}

/// 统一分析批次的一个条目：一条通道消息（原文 + 待归档记录）
///
/// 思考/工具不是独立条目：它们内嵌在所属消息 `content` 的 `Multi` 里，分组、落位与归档
/// 都以消息为单位（一整条 multi 消息合成一条原文），内嵌记录随所属消息一起送模型与落盘。
#[derive(Debug, Clone)]
pub struct BatchItem {
    /// 分组标识：正常等于 `msg_id`；`msg_id` 为空（历史记录）或批次内重复时加兜底后缀
    id: ArcStr,
    /// 消息自身的 `msg_id`（历史记录为空；定位结果按它返回）
    msg_id: ArcStr,
    time: ArcStr,
    user_id: ArcStr,
    user_name: ArcStr,
    is_self: usize,
    /// 消息原文（内容中的思考/工具为内嵌记录）
    content: Content,
    /// 待归档记录（通道记录本身）；新消息为空
    record: Option<RecordEntry>,
    /// 写入方已给出的所属 case 名（`meta.memory_summary.case_name`，只来自已存记录）
    meta_case: Option<ArcStr>,
}

impl BatchItem {
    fn stored(id: ArcStr, message: &StoreMessage) -> Self {
        let record = &message.record;
        Self {
            id,
            msg_id: record.msg_id.clone(),
            time: record.time.clone(),
            user_id: record.user_id.clone(),
            user_name: record.user_name.clone(),
            is_self: record.is_self,
            content: record.content.clone(),
            record: Some(message.record.clone()),
            meta_case: record.meta.as_ref().and_then(|meta| meta.case_name()).cloned(),
        }
    }

    fn new(id: ArcStr, message: &LocateMessage) -> Self {
        Self {
            id,
            msg_id: message.msg_id.clone(),
            time: message.time.clone(),
            user_id: message.user_id.clone(),
            user_name: message.user_name.clone(),
            is_self: message.is_self,
            content: message.content.clone(),
            record: None,
            meta_case: None,
        }
    }

    /// 是否是待归档记录（会落盘归档）；只用于定位的新消息没有待归档记录
    pub fn is_stored(&self) -> bool {
        self.record.is_some()
    }

    /// 该条目涉及的定位消息 `msg_id`（只有新消息才有）
    fn located_message_id(&self) -> Option<&ArcStr> {
        (!self.is_stored()).then_some(&self.msg_id)
    }

    /// 原文文本（思考/工具记录按内嵌顺序渲染）
    fn text(&self) -> String {
        synthesize_content(&self.content)
    }

    /// 送模型的文本形态：`[时间] 说话人: 原文`
    fn render(&self) -> String {
        let self_mark = if self.is_self > 0 { " (本人)" } else { "" };
        format!("[{}] {}: {}{}", self.time, self.user_name, self.text(), self_mark)
    }

    /// 分组输入（固定字段：msg_id / time / user_id / content）
    fn group_item(&self) -> GroupItem {
        GroupItem {
            msg_id: self.id.clone(),
            time: self.time.clone(),
            user_id: self.user_id.clone(),
            content: ArcStr::from(self.text()),
        }
    }
}

/// 一个组的落点判定（只读结论，尚未落地）
#[derive(Debug, Clone, PartialEq, Eq)]
enum GroupVerdict {
    /// 组里的内容归入该已有 case
    Case(ArcStr),
    /// 角色模式新建叶子 case：名称与摘要同一次判定就由模型给出（id 为 `role-{名称}`）
    NewRole { parent: Option<ArcStr>, name: ArcStr, summary: ArcStr },
    /// 事件模式新建：id 由记忆后缀固定（`event-{事件标识}`），摘要按内容生成
    NewEvent,
    /// 事件根非叶子、下潜遍历时新建子 case：`event-{事件标识}-{名称}`，摘要由判定给出
    NewEventChild { parent: ArcStr, name: ArcStr, summary: ArcStr },
}

/// 分组落位（内部）：落点判定 + 批次内条目下标
struct Placement {
    verdict: GroupVerdict,
    items: Vec<usize>,
}

/// 逐层判定的两种候选集：角色模式只看 `role-` case，事件模式看事件根下的全部子 case。
#[derive(Clone, Copy)]
enum PlacementMode {
    Role,
    Event,
}

impl PlacementMode {
    fn label(self) -> &'static str {
        match self {
            Self::Role => "角色模式",
            Self::Event => "事件模式",
        }
    }

    fn children(self, tree: &CaseTree, parent: &str) -> Vec<ArcStr> {
        match self {
            Self::Role => tree.role_children_ids(parent),
            Self::Event => tree.children_ids(parent),
        }
    }

    fn new_verdict(self, parent: Option<ArcStr>, name: ArcStr, summary: ArcStr) -> Result<GroupVerdict> {
        match self {
            Self::Role => Ok(GroupVerdict::NewRole { parent, name, summary }),
            Self::Event => {
                let parent =
                    parent.ok_or_else(|| Error::StateError("事件模式逐层判定缺少父 case".into()))?;
                Ok(GroupVerdict::NewEventChild { parent, name, summary })
            }
        }
    }
}

/// 待归档记录的处理结果。
///
/// 记录已应用到内存树，但原文副本要等全部摘要成功后才统一追加；`latest_case` 只统计真正
/// 归档成功的条目，跳过的非叶子 case 不会影响当前 case 指针。
struct PendingResult {
    outcome: ArchiveOutcome,
    latest_case: Option<ArcStr>,
    records: BTreeMap<ArcStr, Vec<RecordEntry>>,
}

/// 把同通道的待归档消息与新消息合并为分析批次（按时间升序，同一时刻已存消息在前）。
///
/// 当前归档流程分两阶段调用（先 pending、再 messages），函数本身保留两者合并的能力。
/// 调用方必须已**按通道分开调用**（归档与定位都逐通道进行）。新消息在调用 locate 时还没有写入记忆，
/// 因此不与已存记录按 `msg_id` 去重：同一 `msg_id` 的已存记录与新消息各占一个分组条目
/// （分组标识加 `#序号` 后缀区分，定位结果仍按原 `msg_id` 返回）。
pub fn merge_batch(channel: &ChannelKey, pending: &[StoreMessage], messages: &[&LocateMessage]) -> Vec<BatchItem> {
    debug_assert!(
        pending.iter().all(|message| message.channel == *channel),
        "merge_batch 只接受同一通道的待归档消息"
    );
    let mut items: Vec<BatchItem> = Vec::with_capacity(pending.len() + messages.len());
    let mut used: HashSet<ArcStr> = HashSet::new();
    for message in pending {
        let record = &message.record;
        // 历史记录没有 msg_id：用记录自身的 (time, sn) 兜底，保证每条消息都有分组标识
        let base = if record.msg_id.is_empty() {
            ArcStr::from(format!("{}-{}", record.time, record.sn))
        } else {
            record.msg_id.clone()
        };
        let id = unique_id(base, &mut used);
        items.push(BatchItem::stored(id, message));
    }
    for message in messages {
        let id = unique_id(message.msg_id.clone(), &mut used);
        items.push(BatchItem::new(id, message));
    }
    // 稳定排序：同一时刻已存消息在前
    items.sort_by(|a, b| a.time.cmp(&b.time));
    items
}

/// 批次内唯一的分组标识：重复时追加 `#序号`
fn unique_id(base: ArcStr, used: &mut HashSet<ArcStr>) -> ArcStr {
    if used.insert(base.clone()) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = ArcStr::from(format!("{}#{}", base, n));
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// 归档器
pub struct Archiver {
    model: Arc<dyn DecisionModel>,
    /// 单通道单次定位（分块）的条目上限
    max_batch_records: usize,
}

impl Archiver {
    pub fn new(model: Arc<dyn DecisionModel>, max_batch_records: usize) -> Self {
        Self { model, max_batch_records: max_batch_records.max(1) }
    }

    /// 处理某记忆后缀某通道的一次调用：**先归档 pending，再定位新消息**。
    ///
    /// - 待归档记录按 meta 直接落 case；没有 meta 跳过，非叶子 case 跳过（不更新当前指针）；
    /// - 全部 pending 处理完成后统一推进一次通道水位、更新当前指针并落盘；
    /// - pending 归档完成后再对新消息做分组/逐层判定；新消息只定位、不落盘。
    ///
    /// pending 的原文副本在全部摘要成功后才统一追加；处理失败时回滚内存状态，磁盘不动。
    pub async fn archive(
        &self,
        handle: &AgentRoleHandle,
        state: &mut AgentRoleState,
        event_id: Option<ArcStr>,
        channel: &ChannelKey,
        pending: &[StoreMessage],
        messages: &[&LocateMessage],
    ) -> Result<ArchiveResult> {
        if pending.is_empty() && messages.is_empty() {
            return Ok(ArchiveResult::default());
        }
        // 事件标识同样要能用作 case 名（只允许英文字母、数字与 `-`）
        if let Some(event_id) = event_id.as_ref().filter(|id| !crate::model::valid_case_name(id)) {
            return Err(Error::StateError(format!(
                "事件标识不能用于 case 名（只允许英文字母、数字与 '-'，且不超过 {} 字符）: {}",
                crate::model::MAX_CASE_NAME_LEN,
                event_id
            ).into()));
        }

        let role_event = role_event_of(&handle.id, event_id.clone());
        let mut outcomes = Vec::new();
        let mut groups = Vec::new();
        let new_messages = messages.len();

        // 1. 先归档 pending：只按 meta 落位，不调分组/逐层判定。
        if !pending.is_empty() {
            let snapshot = state.clone();
            let pending_items = merge_batch(channel, pending, &[]);
            let staged_result = self.archive_pending(handle, state, event_id.clone(), &pending_items).await;
            let PendingResult { outcome, latest_case, records: staged } = match staged_result {
                Ok(result) => result,
                Err(e) => {
                    *state = snapshot;
                    return Err(e);
                }
            };
            outcomes.push(outcome);

            // 全部 pending 成功后统一提交：追加原文副本、推进一次水位、更新当前指针、落盘。
            let commit = async {
                for (case_name, records) in &staged {
                    append_case_records(&handle.case_dir(case_name), records).await?;
                }
                let records: Vec<RecordEntry> = pending.iter().map(|message| message.record.clone()).collect();
                advance_watermark(
                    state.watermarks.entry((role_event.clone(), channel.clone())).or_default(),
                    &records,
                );
                if let Some(case_name) = latest_case {
                    state.current_cases.insert((role_event.clone(), channel.clone()), case_name);
                }
                handle.save_state(&role_event, state).await
            }.await;
            if let Err(e) = commit {
                *state = snapshot;
                return Err(e);
            }
        }

        // 2. 再定位新消息：此时 pending 已归档，当前 case 指针与摘要都是最新的。
        if !messages.is_empty() {
            let items = merge_batch(channel, &[], messages);
            for chunk in items.chunks(self.max_batch_records) {
                // 定位阶段会建出空 case；失败时回滚本块，避免留下未落盘的节点。
                let snapshot = state.clone();
                let result = async {
                    let chunk_groups = self.locate_chunk(handle, state, event_id.clone(), channel, chunk).await?;
                    handle.save_state(&role_event, state).await?;
                    Ok::<_, Error>(chunk_groups)
                }.await;
                match result {
                    Ok(chunk_groups) => groups.extend(chunk_groups),
                    Err(e) => {
                        *state = snapshot;
                        return Err(e);
                    }
                }
            }
        }

        Ok(ArchiveResult { outcomes, groups, new_messages })
    }

    /// 归档全部 pending 记录：只按 meta 落位，不做分组/逐层判定。
    ///
    /// 非叶子 case 不接收原始记录：跳过该 case（以及同一 case 的其他记录），当前指针不动。
    /// 先为全部受影响的叶子 case 生成摘要并追加记录，再统一按受影响分支自底向上重算复合 case，
    /// 不按分块拆开重算；水位由调用方在全部处理完后统一推进。
    async fn archive_pending(
        &self,
        handle: &AgentRoleHandle,
        state: &mut AgentRoleState,
        event_id: Option<ArcStr>,
        items: &[BatchItem],
    ) -> Result<PendingResult> {
        let now = crate::records::now_string();
        let mut new_records: HashMap<ArcStr, Vec<RecordEntry>> = HashMap::new();
        let mut item_case: Vec<Option<ArcStr>> = vec![None; items.len()];
        let mut skipped: BTreeSet<ArcStr> = BTreeSet::new();
        for (index, item) in items.iter().enumerate() {
            let Some(case_name) = item.meta_case.as_ref().filter(|name| usable_meta_case(name)) else {
                continue;
            };
            if state.tree.contains(case_name) && !state.tree.is_leaf(case_name) {
                if skipped.insert(case_name.clone()) {
                    tracing::warn!("跳过非叶子 case 的待归档记录: {}", case_name);
                }
                continue;
            }
            item_case[index] = Some(case_name.clone());
            new_records.entry(case_name.clone()).or_default().extend(item.record.iter().cloned());
        }

        // meta 指向的 case 可能还不存在（例如重建后森林里还没有该节点）：按 meta 建出叶子 case。
        for case_name in new_records.keys() {
            if !state.tree.contains(case_name) {
                state.tree.create_node(case_name, None, &now);
            }
        }

        // 1. 先为所有叶子 case 生成摘要：当前摘要 + 该 case 的全部待归档记录。
        let mut leaf_summaries: HashMap<ArcStr, ArcStr> = HashMap::new();
        for (case_name, records) in &new_records {
            if !state.tree.is_leaf(case_name) {
                continue;
            }
            let mut single: HashMap<ArcStr, Vec<RecordEntry>> = HashMap::new();
            single.insert(case_name.clone(), records.clone());
            if let Some(summary) =
                self.compute_summary(handle, state, event_id.clone(), case_name, &single, &HashMap::new()).await?
            {
                leaf_summaries.insert(case_name.clone(), summary);
            }
        }

        // 2. 追加记录并更新叶子摘要；此时全部叶子 case 才算更新完成。
        for (case_name, group) in &new_records {
            let _ = state.tree.apply_records(case_name, group, &now);
        }
        for (case_name, summary) in &leaf_summaries {
            state.tree.set_summary(case_name, summary, &now);
        }

        // 3. 全部叶子更新完成后，按受影响分支自底向上重算复合 case。
        let touched: BTreeSet<ArcStr> = new_records.keys().cloned().collect();
        let no_records: HashMap<ArcStr, Vec<RecordEntry>> = HashMap::new();
        for id in self.recompute_order(&state.tree, &touched) {
            if state.tree.is_leaf(&id) {
                continue;
            }
            if let Some(summary) =
                self.compute_summary(handle, state, event_id.clone(), &id, &no_records, &HashMap::new()).await?
            {
                state.tree.set_summary(&id, &summary, &now);
            }
        }

        let mut case_names: Vec<ArcStr> = new_records.keys().cloned().collect();
        case_names.sort();
        let archived: u64 = new_records.values().map(|group| group.len() as u64).sum();
        let latest_case = item_case.iter().rposition(|case| case.is_some()).and_then(|index| item_case[index].clone());

        Ok(PendingResult {
            outcome: ArchiveOutcome { archived, case_names },
            latest_case,
            records: new_records.into_iter().collect(),
        })
    }

    /// 定位一个只含新消息的分块：分组/逐层判定、建出 case，不落盘记录。
    ///
    /// 定位结果落到非叶子 case（例如陈旧的当前指针）时跳过该组，不报错、不更新指针；
    /// 其他组继续处理。被跳过的消息不出现在返回分组里，调用方按覆盖检查处理。
    async fn locate_chunk(
        &self,
        handle: &AgentRoleHandle,
        state: &mut AgentRoleState,
        event_id: Option<ArcStr>,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<Vec<LocatedGroup>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let now = crate::records::now_string();
        let role_event = role_event_of(&handle.id, event_id.clone());

        // 1. 只读判定：已有 case / 需要新建在哪个父 case 之下。
        let placements = match event_id.as_ref() {
            Some(event_id) => {
                let root_case = event_case_name(event_id.as_str());
                let indices: Vec<usize> = (0..items.len()).collect();
                let verdict = match state.tree.event_case(event_id.as_str()) {
                    // 事件根还不存在：由落地阶段建出（只含消息时记忆条数为 0）
                    None => GroupVerdict::NewEvent,
                    // 事件根是叶子：直接命中（按 event_id 定位，不看层级）
                    Some(_) if state.tree.is_leaf(&root_case) => GroupVerdict::Case(root_case),
                    // 事件根非叶子：以它为根继续遍历（新 case 用 `event-{事件标识}-*`）
                    Some(_) => {
                        let text = render_group(items, &indices);
                        self.decide_placement(
                            &handle.id,
                            Some(event_id.clone()),
                            &state.tree,
                            &text,
                            Some(root_case.clone()),
                            PlacementMode::Event,
                        )
                        .await?
                    }
                };
                vec![Placement { verdict, items: indices }]
            }
            // 角色模式：模型分组 + 逐层判定，只看角色 case（事件叶子不在候选里）
            None => {
                let current = state.current_case(&role_event, channel);
                self.group_and_place(&handle.id, state, &role_event, current, items).await?
            }
        };

        // 2. 落地 + 建出/命中 case；新建 case 的摘要由这次判定一并给出。
        let mut item_case: Vec<Option<ArcStr>> = vec![None; items.len()];
        let mut groups: Vec<LocatedGroup> = Vec::with_capacity(placements.len());
        for placement in &placements {
            let messages: Vec<ArcStr> =
                placement.items.iter().filter_map(|index| items[*index].located_message_id().cloned()).collect();
            let case_name = match &placement.verdict {
                GroupVerdict::Case(case_name) => {
                    if state.tree.contains(case_name) && !state.tree.is_leaf(case_name) {
                        tracing::warn!("跳过定位结果指向的非叶子 case: {}", case_name);
                        continue;
                    }
                    // 当前 case 可能还没落盘（例如本批带 meta 的待归档记录指向它，或状态指针陈旧）：
                    // 定位结果必须是叶子，此时按顶层叶子建出。
                    if !state.tree.contains(case_name) {
                        state.tree.create_node(case_name, None, &now);
                    }
                    case_name.clone()
                }
                GroupVerdict::NewRole { parent, name, summary } => {
                    let case_name = state.tree.unique_role_case_name(name);
                    state.tree.create_node(&case_name, parent.as_deref(), &now);
                    state.tree.set_summary(&case_name, summary, &now);
                    case_name
                }
                GroupVerdict::NewEventChild { parent, name, summary } => {
                    let Some(event_id) = event_id.as_ref() else {
                        return Err(Error::StateError("事件模式缺少事件标识".into()));
                    };
                    let case_name = state.tree.unique_event_child_case_name(event_id, name);
                    if case_name.len() > 128 {
                        return Err(Error::StateError(format!(
                            "事件子 case 名过长（超过 128 字符）: {}",
                            case_name
                        ).into()));
                    }
                    state.tree.create_node(&case_name, Some(parent.as_str()), &now);
                    state.tree.set_summary(&case_name, summary, &now);
                    case_name
                }
                GroupVerdict::NewEvent => {
                    // 名称由 event_id 固定，可能已被建出（并发/重复请求）
                    let Some(event_id) = event_id.as_ref() else {
                        return Err(Error::StateError("事件模式缺少事件标识".into()));
                    };
                    let case_name = event_case_name(event_id.as_str());
                    if state.tree.event_case(event_id.as_str()).is_none() {
                        state.tree.create_node(&case_name, None, &now);
                        // 只有消息、还没有记录：立刻按本组内容生成摘要（有记录时交给归档阶段按记录生成）
                        let text = render_group(items, &placement.items);
                        let summary = self.model.summarize(&handle.id, Some(event_id.clone()), &text).await?;
                        state.tree.set_summary(&case_name, &summary, &now);
                    }
                    case_name
                }
            };
            for &index in &placement.items {
                item_case[index] = Some(case_name.clone());
            }
            groups.push(LocatedGroup { messages: messages.into_iter().collect(), case_name: case_name.clone() });
        }

        // 3. 新建 case 的摘要已由判定给出；祖先复合 case 的摘要留到归档阶段，
        //    等这些消息真正写入并归档后再按受影响分支重算。
        //
        // 4. 当前 case 指针指向本批最晚一条消息落入的 case（被跳过的组不算）。
        if let Some(case_name) =
            item_case.iter().rposition(|case| case.is_some()).and_then(|index| item_case[index].clone())
        {
            state.current_cases.insert((role_event, channel.clone()), case_name);
        }

        Ok(groups)
    }

    /// 角色模式的分组与定位（只读）：第 1 轮按 case 分组（并标记当前 case 组），
    /// 当前 case 组直接命中当前 case，其余组各自逐层判定落点；判定不改树，
    /// 由落盘阶段逐组建出/命中 case。
    async fn group_and_place(
        &self,
        agent_role: &AgentRole,
        state: &AgentRoleState,
        role_event: &RoleEvent,
        current: Option<ArcStr>,
        items: &[BatchItem],
    ) -> Result<Vec<Placement>> {
        // 当前 case 可能还没落盘（例如状态指针陈旧）：给模型一个空摘要的 brief，
        // 让「当前 case 组」仍能直接命中它；真正的摘要由后续归档按记录生成。
        let current_brief = current
            .as_deref()
            .map(|id| state.tree.brief(id).unwrap_or_else(|| CaseBrief { case_name: ArcStr::from(id), summary: ArcStr::new() }));
        let group_items: Vec<GroupItem> = items.iter().map(BatchItem::group_item).collect();
        let groups = self.model.group(agent_role, role_event.event_id.clone(), &group_items, current_brief.as_ref()).await?;

        let index_of: HashMap<&str, usize> =
            items.iter().enumerate().map(|(index, item)| (item.id.as_str(), index)).collect();
        // 先把每组解析成批次下标；组内按批次顺序（时间升序）排列，便于渲染与落位
        let mut parsed_groups: Vec<(Vec<usize>, bool)> = Vec::with_capacity(groups.len());
        for group in groups {
            let mut group_items: Vec<usize> = Vec::with_capacity(group.msg_ids.len());
            for msg_id in &group.msg_ids {
                let Some(index) = index_of.get(msg_id.as_str()) else {
                    return Err(Error::ModelApiError(format!(
                        "分组给出的 msg_id {} 不在本批消息里（角色 {}，事件 {:?}）",
                        msg_id, agent_role, role_event.event_id
                    ).into()));
                };
                group_items.push(*index);
            }
            group_items.sort_unstable();
            parsed_groups.push((group_items, group.is_current));
        }
        // 非当前 case 的组按组内首条记忆的时间顺序依次处理；当前 case 组也一并按时间排序，
        // 使逐层判定的调用顺序与记忆时间一致。
        parsed_groups.sort_by(|a, b| {
            let ta = a.0.first().map(|index| items[*index].time.as_str()).unwrap_or("");
            let tb = b.0.first().map(|index| items[*index].time.as_str()).unwrap_or("");
            ta.cmp(tb)
        });

        let mut placements: Vec<Placement> = Vec::with_capacity(parsed_groups.len());
        for (group_items, is_current) in parsed_groups {
            let text = render_group(items, &group_items);
            let verdict = if is_current {
                match current.clone() {
                    // 标记为当前 case 的组直接命中该通道当前 case
                    Some(current) => GroupVerdict::Case(current),
                    // 没有当前 case：只定位的组容忍该不一致，退回逐层判定
                    None => self
                        .decide_placement(agent_role, role_event.event_id.clone(), &state.tree, &text, None, PlacementMode::Role)
                        .await?,
                }
            } else {
                self.decide_placement(agent_role, role_event.event_id.clone(), &state.tree, &text, None, PlacementMode::Role)
                    .await?
            };
            placements.push(Placement { verdict, items: group_items });
        }
        Ok(placements)
    }

    /// 逐层判定一个组的落点（只读）：命中叶子 case 即 [`GroupVerdict::Case`]；
    /// 当层未命中（或命中非叶子后其子层都未命中）即新建叶子 case——名称与摘要
    /// 由这次判定一并给出，因此**即使当层没有候选 case 也要调用模型**（空候选提示词里会说明）。
    ///
    /// `PlacementMode` 决定候选集：角色模式只取 `role-` case；事件模式取事件根下的全部子 case。
    /// 事件模式由调用方把 `parent` 设为事件根，角色模式顶层为 `None`。
    ///
    /// 没有层数上限：每判定一次要么落在当层（并入叶子 / 新建），要么下潜一层；
    /// case 森林有限且无环，下潜次数必然有限，因此循环一定终止。
    async fn decide_placement(
        &self,
        agent_role: &AgentRole,
        event_id: Option<ArcStr>,
        tree: &CaseTree,
        text: &str,
        parent: Option<ArcStr>,
        mode: PlacementMode,
    ) -> Result<GroupVerdict> {
        let mut parent = parent;
        let mut level = match (&parent, mode) {
            (Some(id), PlacementMode::Role) => tree.role_children_ids(id),
            (None, PlacementMode::Role) => tree.role_top_level_ids(),
            (Some(id), PlacementMode::Event) => tree.children_ids(id),
            (None, PlacementMode::Event) => {
                return Err(Error::StateError("事件模式逐层判定缺少事件根".into()));
            }
        };

        loop {
            let briefs: Vec<CaseBrief> = tree.briefs(&level);
            match self.model.decide_level(agent_role, event_id.clone(), text, &briefs).await? {
                LevelDecision::New { name, summary } => return mode.new_verdict(parent, name, summary),
                LevelDecision::Merge(case_name) => {
                    if !level.contains(&case_name) {
                        return Err(Error::ModelApiError(format!(
                            "{}当层判定给出的 case {} 不在当层候选里（角色 {}，事件 {:?}，候选 {:?}）",
                            mode.label(),
                            case_name,
                            agent_role,
                            event_id,
                            level
                        ).into()));
                    }
                    if tree.is_leaf(&case_name) {
                        return Ok(GroupVerdict::Case(case_name));
                    }
                    // 非叶子：下潜到其子层继续判定
                    level = mode.children(tree, &case_name);
                    parent = Some(case_name);
                }
            }
        }
    }

    /// 摘要重算顺序：受影响的叶子在前、祖先在后
    fn recompute_order(&self, tree: &CaseTree, touched: &BTreeSet<ArcStr>) -> Vec<ArcStr> {
        let mut all: BTreeSet<ArcStr> = BTreeSet::new();
        for case_name in touched {
            all.insert(case_name.clone());
            for ancestor in tree.ancestors(case_name) {
                all.insert(ancestor);
            }
        }
        let mut ordered: Vec<(usize, ArcStr)> = all
            .into_iter()
            .filter(|id| tree.contains(id))
            .map(|id| (tree.ancestors(&id).len(), id))
            .collect();
        // 深度大（更靠近叶子）先算；同深度按 id 稳定排序
        ordered.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        ordered.into_iter().map(|(_, id)| id).collect()
    }

    /// 计算单个 case 的新摘要：叶子取 **当前摘要 + 本批全部待归档记录**，复合取
    /// **当前摘要 + 子 case 摘要** → 模型摘要。归档不再读 case 的全部历史记录，也不再重新定位。
    ///
    /// 返回 `None` 表示该 case 本批没有变化（无待归档记录且子 case 未变），无需摘要。
    async fn compute_summary(
        &self,
        handle: &AgentRoleHandle,
        state: &AgentRoleState,
        event_id: Option<ArcStr>,
        case_name: &str,
        new_records: &HashMap<ArcStr, Vec<RecordEntry>>,
        computed: &HashMap<ArcStr, ArcStr>,
    ) -> Result<Option<ArcStr>> {
        if is_role_root(case_name) {
            return Ok(None);
        }
        let Some(node) = state.tree.get(case_name) else {
            return Ok(None);
        };
        let extra = new_records.get(case_name).filter(|entries| !entries.is_empty());
        let leaf = state.tree.is_leaf(case_name);

        let mut content = String::new();
        // 当前摘要作为增量上下文：没有历史记录时就是新建 case 的当前摘要（通常为空）
        if !node.summary.is_empty() {
            content.push_str("当前摘要：\n");
            content.push_str(node.summary.as_str());
            content.push('\n');
        }
        if leaf {
            let Some(entries) = extra else {
                return Ok(None);
            };
            content.push_str("\n待归档记录：\n");
            for entry in entries {
                content.push_str(&render_entry(entry));
                content.push('\n');
            }
        } else {
            // 复合 case 只由子 case 组成，不再持有自己的原始记录（记录只能写叶子）
            if extra.is_some() {
                return Err(Error::StateError(format!(
                    "复合 case {} 不能有待归档记录",
                    case_name
                ).into()));
            }
            content.push_str("\n子 case 摘要：\n");
            for child in state.tree.children_ids(case_name) {
                // 本批刚重算的子 case 摘要优先（父摘要必须涵盖子 case 的最新内容）
                let summary = computed
                    .get(&child)
                    .cloned()
                    .or_else(|| state.tree.brief(&child).map(|b| b.summary.clone()))
                    .unwrap_or_default();
                content.push_str(&format!("- {}: {}\n", child, summary));
            }
        }

        Ok(Some(self.model.summarize(&handle.id, event_id, &content).await?))
    }
}

/// 组内条目文本：每条消息一行 `[时间] 说话人: 原文`（思考/工具已内嵌在原文里）
fn render_group(items: &[BatchItem], indices: &[usize]) -> String {
    indices.iter().map(|index| items[*index].render()).collect::<Vec<_>>().join("\n")
}

/// 写入方给出的 case 名能否直接用作节点/目录名：只允许英文字母、数字与 `-`（与组件生成的
/// `role-` / `event-` case 名一致），并限制长度以避免异常 meta 造成路径穿越或超长目录名。
fn usable_meta_case(case_name: &str) -> bool {
    !case_name.is_empty()
        && case_name.len() <= 128
        && case_name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// 水位推进到已归档范围末尾
pub fn advance_watermark(watermark: &mut RecordPosition, batch: &[RecordEntry]) {
    for entry in batch {
        if watermark.is_before(entry.time.as_str(), entry.sn) {
            *watermark = RecordPosition { time: entry.time.clone(), sn: entry.sn };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_api::{ChannelRecord, Content};

    fn channel(time: &str, sn: u64, is_self: usize, text: &str) -> RecordEntry {
        channel_with_id(time, sn, is_self, text, "")
    }

    /// 带 msg_id 的通道记录（消息与记录按「通道三元组 + msg_id」去重）
    fn channel_with_id(time: &str, sn: u64, is_self: usize, text: &str, msg_id: &str) -> RecordEntry {
        Arc::new(ChannelRecord {
            meta: None,
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self"),
            messenger_id: ArcStr::from("web"),
            group_id: ArcStr::from("g1"),
            is_self,
            msg_id: ArcStr::from(msg_id),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::from("alice"),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from(text)),
            time: ArcStr::from(time),
            sn,
        })
    }

    /// 待归档消息：通道记录
    fn pending(time: &str, sn: u64, is_self: usize, text: &str) -> StoreMessage {
        StoreMessage { channel: channel_key(), record: channel(time, sn, is_self, text) }
    }

    fn message(time: &str, text: &str) -> LocateMessage {
        message_with_id(time, "", text)
    }

    /// 带 msg_id 的定位消息（msg_id 为空时按不可去重处理）
    fn message_with_id(time: &str, msg_id: &str, text: &str) -> LocateMessage {
        LocateMessage {
            msg_id: ArcStr::from(msg_id),
            time: ArcStr::from(time),
            user_id: ArcStr::from("u1"),
            user_name: ArcStr::from("alice"),
            is_self: 0,
            content: Content::Text(ArcStr::from(text)),
        }
    }

    #[test]
    fn advance_watermark_uses_batch_max() {
        let mut watermark = RecordPosition::default();
        let batch = vec![
            channel("2026-09-22 10:00:00", 1, 0, "a"),
            channel("2026-09-22 10:05:00", 2, 1, "b"),
        ];
        advance_watermark(&mut watermark, &batch);
        assert_eq!(watermark.time.as_str(), "2026-09-22 10:05:00");
        assert_eq!(watermark.sn, 2);
    }

    #[test]
    fn render_group_prints_message_lines() {
        let pending = vec![pending("2026-09-22 10:00:00", 1, 0, "a"), pending("2026-09-22 10:01:00", 2, 1, "b")];
        let items = merge_batch(&channel_key(), &pending, &[]);
        let text = render_group(&items, &[0, 1]);
        assert_eq!(text, "[2026-09-22 10:00:00] alice: a\n[2026-09-22 10:01:00] alice: b (本人)");
    }

    /// 批次所属通道（与各测试构造的记录/消息一致）
    fn channel_key() -> ChannelKey {
        ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"))
    }

    #[test]
    fn merge_batch_keeps_records_and_messages() {
        // 新消息在 locate 时还没有写入记忆：不与已存记录去重，各自成条（按时间排序）；
        // 分组标识必须唯一，同一 msg_id 的新消息加兜底后缀，但定位结果仍按原 msg_id 返回
        let pending = vec![
            StoreMessage { channel: channel_key(), record: channel_with_id("2099-01-01 10:00:00", 1, 0, "A", "m1") },
            StoreMessage { channel: channel_key(), record: channel_with_id("2099-01-01 10:02:00", 2, 1, "C", "m2") },
        ];
        let same_id = message_with_id("2099-01-01 23:59:59", "m1", "与 A 同 msg_id 的新消息");
        let new = message_with_id("2099-01-01 10:03:00", "m3", "B");
        let items = merge_batch(&channel_key(), &pending, &[&same_id, &new]);

        assert_eq!(items.len(), 4, "2 条已存消息 + 2 条新消息");
        let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, vec!["m1", "m2", "m3", "m1#2"], "分组标识唯一");
        let text = items.iter().map(BatchItem::render).collect::<Vec<_>>().join("\n");
        assert!(text.contains("alice: A") && text.contains("alice: C"), "已存记录参与分析");
        assert!(text.contains("与 A 同 msg_id 的新消息") && text.contains("alice: B"), "新消息都参与分析");
        assert_eq!(items[0].time.as_str(), "2099-01-01 10:00:00");
        assert!(items[0].is_stored(), "已存消息先于新消息");
        assert_eq!(items[2].msg_id.as_str(), "m3", "新消息保留 msg_id");
        assert_eq!(items[2].text(), "B");
        assert!(!items[2].is_stored());
        assert_eq!(items[3].located_message_id().map(|id| id.as_str()), Some("m1"), "定位结果按原 msg_id 返回");
    }

    #[test]
    fn merge_batch_orders_by_time_with_records_first() {
        let records = vec![pending("2099-01-01 10:00:00", 1, 0, "已存")];
        let earlier = message("2099-01-01 09:00:00", "更早的新消息");
        let same_time = message("2099-01-01 10:00:00", "同一时刻的新消息");
        let items = merge_batch(&channel_key(), &records, &[&earlier, &same_time]);

        let times: Vec<&str> = items.iter().map(|item| item.time.as_str()).collect();
        assert_eq!(times, vec!["2099-01-01 09:00:00", "2099-01-01 10:00:00", "2099-01-01 10:00:00"]);
        assert!(!items[0].is_stored());
        assert!(items[1].is_stored(), "同一时刻已存记录在前");
        assert!(!items[2].is_stored());
    }

    #[test]
    fn recompute_order_puts_descendants_first() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, "t");
        tree.create_node("role-2", Some("role-1"), "t");
        tree.create_node("role-3", Some("role-2"), "t");
        let touched: BTreeSet<ArcStr> = [ArcStr::from("role-3")].into_iter().collect();
        let archiver = Archiver::new(Arc::new(crate::stub::StubModel::new()), 200);
        let order = archiver.recompute_order(&tree, &touched);
        assert_eq!(order, vec![ArcStr::from("role-3"), ArcStr::from("role-2"), ArcStr::from("role-1")]);
    }

}
