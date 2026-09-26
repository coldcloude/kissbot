//! 归档器：批次分组、分组落位与摘要级联重算。
//!
//! 角色模式一次处理某记忆后缀某通道的一批条目，分两段：
//! 1. 第 1 轮（分组轮）：把当前 case 与该批条目一起交给模型，模型按 case 分组并标记属于当前 case 的组；
//!    标记为当前 case 的组直接并入当前 case（不下潜）；
//! 2. 第 2 轮起分层归并：其余组以组为单位，自顶层逐层归并（非叶子继续下潜），或在本层新增叶子 case。
//!
//! 事件模式（记忆后缀含 event）不分组、不参与判定：整个后缀固定归入角色树里的顶层叶子
//! `event-{caseid}`（见 [`crate::tree::EVENT_CASE_PREFIX`]），角色模式的判定看不到它。
//!
//! 批次条目既可以是待归档记录，也可以是只用于定位的新消息（见 [`BatchItem`]）：定位请求触发懒处理时
//! 先按通道与待归档记录去重（重复的新消息用已存版本分析一次、只返回所属 case），其余新消息随批次一起
//! 参与分组但不落盘。只含新消息的组同样命中或建出 case（新话题建出的 case 记忆条数为 0）。
//!
//! Case 之间的关联关系与「多个 case 合并为大 case」由后续定时程序产生，因此当前新建的 case 都是叶子。

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use kissbot_api::{CaseBrief, CaseMode, ChannelKey, LocateMessage, RecordKind};
use tracing::warn;

use crate::error::{Error, Result};
use crate::locate::render_message;
use crate::model::{DecisionModel, LevelDecision};
use crate::records::{
    RecordBody, RecordEntry, append_case_records, read_case_records, render_case_record,
    render_entry,
};
use crate::scope::{ModelScope, RoleEvent, ScopeHandle, ScopeState};
use crate::tree::CaseTree;

/// 归档结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveOutcome {
    /// 本次归档的记录条数
    pub archived: u64,
    /// 内容落入的 case（按落位顺序）
    pub case_ids: Vec<String>,
}

/// 一次归档调用的结果（记录 + 只用于定位的新消息）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveResult {
    /// 各分块的归档结果（按块序）
    pub outcomes: Vec<ArchiveOutcome>,
    /// 消息分组（`PlaceGroup::messages` 是传入 `messages` 的下标）
    pub groups: Vec<PlaceGroup>,
    /// 本次并入分析的新消息条数（与已存记录重复的不计）
    pub new_messages: usize,
}

/// 统一分析批次的一个条目
#[derive(Debug, Clone)]
pub enum BatchItem {
    /// 共享记忆中的待归档记录；`aliases` 是与之重复的新消息在请求中的下标
    /// （分析只用已存版本一次，重复的新消息只返回所属 case）
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
    /// 该组所属 case：命中已有 case 即该 case，新话题即本次新建的 case（记忆条数为 0）
    pub case_id: String,
}

/// 一个组的落点判定（只读结论，尚未落地）
#[derive(Debug, Clone, PartialEq, Eq)]
enum GroupVerdict {
    /// 组里的内容归入该已有 case
    Case(String),
    /// 角色模式新建叶子 case：名称与摘要同一次判定就由模型给出（id 为 `role-{名称}`）
    NewRole { parent: Option<String>, name: String, summary: String },
    /// 事件模式新建：id 由记忆后缀固定（`event-{事件标识}`），摘要按内容生成
    NewEvent,
}

/// 分组落位（内部）：落点判定 + 批次内条目下标 + 该组的待归档记录
struct Placement {
    verdict: GroupVerdict,
    items: Vec<usize>,
    records: Vec<RecordEntry>,
}

/// 把某通道的待归档记录与该通道的新消息合并为统一分析批次。
///
/// 调用方必须已**按通道分开调用**（归档与定位都逐通道进行）：批次内条目同属 `channel`，
/// 因此判重只比 `msg_id`。与新消息重复的记录保留已存版本、把新消息记为该记录的别名
/// （同一条消息只分析一次）；其余新消息独立成条。按时间升序排列，同一时刻已存记录在前。
pub fn merge_batch(channel: &ChannelKey, records: &[RecordEntry], messages: &[&LocateMessage]) -> Vec<BatchItem> {
    // 通道记录（含思考/工具占位记录）必须属于本通道；思考/工具记录本身没有通道身份，按 key 归属后随批次传入
    debug_assert!(
        records.iter().all(|record| record.channel_key().is_none_or(|key| key == *channel)),
        "merge_batch 只接受同一通道的记录"
    );
    debug_assert!(
        messages.iter().all(|message| &message.channel() == channel),
        "merge_batch 只接受同一通道的新消息"
    );
    let mut items: Vec<BatchItem> = records
        .iter()
        .map(|record| BatchItem::Stored { record: record.clone(), aliases: Vec::new() })
        .collect();
    let mut taken = vec![false; records.len()];
    for (index, message) in messages.iter().enumerate() {
        // 一条已存记录只吸收一条重复的新消息：同一条消息在两侧都出现多次时，多出来的新消息按新消息处理
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

/// 批次内新消息与已存记录是否同一条：只比 `msg_id`（批次已按通道切分，通道无需再比）；
/// `msg_id` 为空（思考/工具占位记录、旧记录）不参与去重、原样保留。
fn duplicates(record: &RecordEntry, message: &LocateMessage) -> bool {
    let RecordBody::Channel(channel) = &record.body else {
        return false;
    };
    if message.msg_id.is_empty() || channel.msg_id.is_empty() {
        return false;
    }
    channel.msg_id == message.msg_id
}

/// 归档器
pub struct Archiver {
    model: Arc<dyn DecisionModel>,
    max_depth: usize,
    /// 单通道单次分析（分块）的条目上限
    max_batch_records: usize,
}

impl Archiver {
    pub fn new(model: Arc<dyn DecisionModel>, max_depth: usize, max_batch_records: usize) -> Self {
        Self { model, max_depth, max_batch_records: max_batch_records.max(1) }
    }

    /// 归档某记忆后缀某通道的待处理记录，并（可选）一并定位本批新消息。
    ///
    /// - `messages` 为空：即原来的归档——只有记录落盘，不产出消息分组（手动触发 / 重建走这条）；
    /// - `messages` 非空：先按 `msg_id`（单通道批次内）与记录**去重合并**为一个批次一起分析：
    ///   记录落盘归档，消息只定位；每组都会命中或建出 case（新话题建出的 case 记忆条数为 0），
    ///   按 [`PlaceGroup`] 返回（`PlaceGroup::messages` 是 `messages` 里的下标）。
    ///
    /// 记录按 `max_batch_records` 分块，逐块归档并落盘：一块失败时已完成的块保留进度，
    /// 失败块回到处理前状态，留待下次查询重试。返回（各块归档结果，消息分组，并入的新消息条数）。
    pub async fn archive(
        &self,
        handle: &ScopeHandle,
        state: &mut ScopeState,
        role_event: &RoleEvent,
        channel: &ChannelKey,
        records: &[RecordEntry],
        messages: &[&LocateMessage],
    ) -> Result<ArchiveResult> {
        if records.is_empty() && messages.is_empty() {
            return Ok(ArchiveResult::default());
        }

        let items = merge_batch(channel, records, messages);
        let new_messages = items.iter().filter(|item| !item.is_stored()).count();
        let mut outcomes = Vec::new();
        let mut groups = Vec::new();
        for chunk in items.chunks(self.max_batch_records) {
            let (outcome, chunk_groups) = self.archive_chunk(handle, state, role_event, channel, chunk).await?;
            if outcome.archived > 0 {
                // 逐块落盘：块失败时前面的进度不回退，失败块由 archive_chunk 恢复内存状态
                handle.save_state(role_event, state).await?;
            }
            outcomes.push(outcome);
            groups.extend(chunk_groups);
        }
        Ok(ArchiveResult { outcomes, groups, new_messages })
    }

    /// 分析并归档一个分块：批次条目里的记录落盘归档，只用于定位的新消息只产出所属 case。
    ///
    /// 所有模型调用都在落盘之前完成；任一模型调用失败时恢复内存状态，磁盘与水位保持不变，
    /// 这批条目留待下次查询重试（见 spec：解析失败按失败处理并保留原状态）。
    async fn archive_chunk(
        &self,
        handle: &ScopeHandle,
        state: &mut ScopeState,
        role_event: &RoleEvent,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<(ArchiveOutcome, Vec<PlaceGroup>)> {
        if items.is_empty() {
            return Ok((ArchiveOutcome { archived: 0, case_ids: Vec::new() }, Vec::new()));
        }
        let snapshot = state.clone();
        match self.archive_chunk_inner(handle, state, role_event, channel, items).await {
            Ok(result) => Ok(result),
            Err(e) => {
                // 判定阶段会在内存树上新建节点：失败时回到调用前状态，避免留下空节点
                *state = snapshot;
                Err(e)
            }
        }
    }

    async fn archive_chunk_inner(
        &self,
        handle: &ScopeHandle,
        state: &mut ScopeState,
        role_event: &RoleEvent,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<(ArchiveOutcome, Vec<PlaceGroup>)> {
        let now = crate::records::now_string();
        let scope = ModelScope::new(&handle.id, role_event);

        // 1. 定位（只读）：逐组判定落点，得到「已有 case / 需在何处新建」，此时不改树
        let placements = match role_event.event_case_id() {
            // 事件模式：整个后缀固定一个顶层叶子 case，不分组、不参与判定
            Some(case_id) => {
                let indices: Vec<usize> = (0..items.len()).collect();
                let records = stored_records(items, &indices);
                let verdict = if state.tree.contains(&case_id) {
                    GroupVerdict::Case(case_id)
                } else {
                    // case 还不存在：由落盘阶段建出（只含消息时记忆条数为 0）
                    GroupVerdict::NewEvent
                };
                vec![Placement { verdict, items: indices, records }]
            }
            // 角色模式：模型分组 + 逐层判定，只看角色子树（事件模式固定的 case 不在候选里）
            None => self.group_and_place(&scope, state, channel, items).await?,
        };

        // 2. 落地 + 存储：逐组建出/命中 case（新话题建出记忆条数为 0 的 case），
        //    有记录的组再聚合记录；随后摘要重算、追加原文副本、应用内存状态、推进水位。
        //    新建的 case 在落盘阶段就带上名称与摘要（角色模式来自模型判定，事件模式按内容生成），
        //    不留到后续查询再去调用模型；这些 case 不再参与第 3 步的摘要重算（祖先仍会级联）。
        let mut new_records: HashMap<String, Vec<RecordEntry>> = HashMap::new();
        let mut touched: BTreeSet<String> = BTreeSet::new();
        let mut created: BTreeSet<String> = BTreeSet::new();
        let mut case_ids: Vec<String> = Vec::new();
        let mut groups: Vec<PlaceGroup> = Vec::with_capacity(placements.len());
        for placement in &placements {
            let mut messages: Vec<usize> =
                placement.items.iter().flat_map(|index| items[*index].messages().iter().copied()).collect();
            messages.sort_unstable();
            let case_id = match &placement.verdict {
                GroupVerdict::Case(case_id) => case_id.clone(),
                GroupVerdict::NewRole { parent, name, summary } => {
                    let case_id = state.tree.unique_role_case_id(name);
                    state.tree.create_node(&case_id, parent.as_deref(), CaseMode::Role, &now);
                    state.tree.set_summary(&case_id, summary, &now);
                    created.insert(case_id.clone());
                    touched.insert(case_id.clone());
                    case_id
                }
                GroupVerdict::NewEvent => {
                    // id 由记忆后缀固定，可能已被建出（并发/重复请求）
                    let case_id = role_event
                        .event_case_id()
                        .ok_or_else(|| Error::StateError(format!("事件模式缺少事件标识: {}", role_event)))?;
                    if !state.tree.contains(&case_id) {
                        state.tree.create_node(&case_id, None, CaseMode::Event, &now);
                        // 只有消息、还没有记录：立刻按本组内容生成摘要（有记录时交给第 3 步按记录生成）
                        if placement.records.is_empty() {
                            let text = render_group(items, &placement.items);
                            let summary = self.model.summarize(&scope, &text).await?;
                            state.tree.set_summary(&case_id, &summary, &now);
                            created.insert(case_id.clone());
                            touched.insert(case_id.clone());
                        }
                    }
                    case_id
                }
            };
            groups.push(PlaceGroup { messages, case_id: case_id.clone() });
            if placement.records.is_empty() {
                continue;
            }
            new_records.entry(case_id.clone()).or_default().extend(placement.records.iter().cloned());
            touched.insert(case_id.clone());
            if !case_ids.contains(&case_id) {
                case_ids.push(case_id);
            }
        }
        let records: Vec<RecordEntry> = items.iter().filter_map(BatchItem::record).cloned().collect();

        // 3. 摘要重算：叶子在前（自底向上），父 case 摘要涵盖自身记录与子 case 摘要；全部成功后落盘。
        //    本批新建的 case 已有摘要（见上一步），只重算它们的祖先。
        let mut summaries: HashMap<String, String> = HashMap::new();
        for id in self.recompute_order(&state.tree, &touched) {
            if created.contains(&id) {
                continue;
            }
            if let Some(summary) = self.compute_summary(handle, state, &scope, &id, &new_records, &summaries).await? {
                summaries.insert(id, summary);
            }
        }

        // 4. 落盘原文副本（按 case 聚合）
        for (case_id, group) in &new_records {
            append_case_records(&handle.case_dir(case_id), group).await?;
        }

        // 5. 内存：应用记录与摘要；当前 case 指针指向本批最后一个组落入的 case
        for (case_id, group) in &new_records {
            state.tree.apply_records(case_id, group, &now);
        }
        for (id, summary) in &summaries {
            state.tree.set_summary(id, summary, &now);
        }
        let key = (role_event.clone(), channel.clone());
        if let Some(last) = groups.last() {
            state.current_cases.insert(key.clone(), last.case_id.clone());
        }
        advance_watermark(state.watermarks.entry(key).or_default(), &records);

        Ok((ArchiveOutcome { archived: records.len() as u64, case_ids }, groups))
    }

    /// 角色模式的分组与定位（只读）：第 1 轮按 case 分组（并标记当前 case 组），
    /// 当前 case 组直接命中当前 case，其余组各自逐层判定落点；判定不改树，
    /// 由落盘阶段逐组建出/命中 case——有记录的组再追加记录。
    async fn group_and_place(
        &self,
        scope: &ModelScope,
        state: &ScopeState,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<Vec<Placement>> {
        let text = render_batch(items);
        let current = state.current_case(&scope.role_event, channel).cloned();
        let current_brief = current.as_deref().and_then(|id| state.tree.brief(id));
        let groups = self.model.group(scope, &text, items.len(), current_brief.as_ref()).await?;

        let mut placements: Vec<Placement> = Vec::with_capacity(groups.len());
        for group in groups {
            let group_items = group.messages.clone();
            let records = stored_records(items, &group_items);
            let text = render_group(items, &group_items);
            let verdict = if group.is_current {
                match current.clone() {
                    // 标记为当前 case 的组直接命中该通道当前 case
                    Some(current) => GroupVerdict::Case(current),
                    // 没有当前 case：只定位的组容忍（退回逐层判定）；有记录要落盘的组无法落位，按失败重试
                    None if records.is_empty() => self.decide_placement(scope, &state.tree, &text, None).await?,
                    None => {
                        return Err(Error::ModelApiError(format!(
                            "第 1 轮把分组标记为当前 case，但通道 {} 没有当前 case（记忆后缀 {}）",
                            channel.messenger_id, scope
                        )))
                    }
                }
            } else {
                self.decide_placement(scope, &state.tree, &text, None).await?
            };
            placements.push(Placement { verdict, items: group_items, records });
        }
        Ok(placements)
    }

    /// 逐层判定一个组的落点（只读）：命中叶子 case 即 [`GroupVerdict::Case`]；
    /// 当层未命中（或命中非叶子后其子层都未命中）即 [`GroupVerdict::NewRole`]——名称与摘要
    /// 由这次判定一并给出，因此**即使当层没有候选 case 也要调用模型**（空候选提示词里会说明）。
    ///
    /// 候选只取角色子树（[`CaseTree::role_top_level_ids`] / [`CaseTree::role_children_ids`]）：
    /// 事件模式固定的 case 不参与判定，两种模式互不并入。
    async fn decide_placement(
        &self,
        scope: &ModelScope,
        tree: &CaseTree,
        text: &str,
        parent: Option<String>,
    ) -> Result<GroupVerdict> {
        let mut parent = parent;
        let mut level = match &parent {
            Some(id) => tree.role_children_ids(id),
            None => tree.role_top_level_ids(),
        };

        for _ in 0..self.max_depth {
            let briefs: Vec<CaseBrief> = tree.briefs(&level);
            match self.model.decide_level(scope, text, &briefs).await? {
                LevelDecision::New { name, summary } => return Ok(GroupVerdict::NewRole { parent, name, summary }),
                LevelDecision::Merge(case_id) => {
                    if !tree.contains(&case_id) {
                        return Err(Error::ModelApiError(format!(
                            "当层判定给出不存在的 case {}（记忆后缀 {}）",
                            case_id, scope
                        )));
                    }
                    if tree.is_leaf(&case_id) {
                        return Ok(GroupVerdict::Case(case_id));
                    }
                    // 非叶子：下潜到其子层继续判定
                    level = tree.role_children_ids(&case_id);
                    parent = Some(case_id);
                }
            }
        }

        // 层数达到上限（防御模型环路）：不再下潜，按「在当前父 case 之下新建」要一次名称与摘要
        warn!("[abstract] reach max case depth {} for suffix {}, ask a new case instead", self.max_depth, scope);
        match self.model.decide_level(scope, text, &[]).await? {
            LevelDecision::New { name, summary } => Ok(GroupVerdict::NewRole { parent, name, summary }),
            LevelDecision::Merge(case_id) => Err(Error::ModelApiError(format!(
                "达到最大 case 层数 {} 后仍未给出新 case（记忆后缀 {}，模型给出 {}）",
                self.max_depth, scope, case_id
            ))),
        }
    }

    /// 摘要重算顺序：受影响的叶子在前、祖先在后
    fn recompute_order(&self, tree: &CaseTree, touched: &BTreeSet<String>) -> Vec<String> {
        let mut all: BTreeSet<String> = BTreeSet::new();
        for case_id in touched {
            all.insert(case_id.clone());
            for ancestor in tree.ancestors(case_id) {
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

    /// 计算单个 case 的新摘要：自身直接记录（已落盘 + 本批新增）+ 子 case 摘要 → 模型摘要。
    ///
    /// 返回 `None` 表示该 case 没有内容（无记录且无子 case），无需摘要。
    async fn compute_summary(
        &self,
        handle: &ScopeHandle,
        state: &ScopeState,
        scope: &ModelScope,
        case_id: &str,
        new_records: &HashMap<String, Vec<RecordEntry>>,
        computed: &HashMap<String, String>,
    ) -> Result<Option<String>> {
        let Some(node) = state.tree.get(case_id) else {
            return Ok(None);
        };
        let records = read_case_records(&handle.case_dir(case_id)).await?;
        let extra = new_records.get(case_id);
        if records.is_empty() && extra.map(|e| e.is_empty()).unwrap_or(true) && node.children.is_empty() {
            return Ok(None);
        }

        let mut content = String::new();
        if !records.is_empty() || extra.map(|e| !e.is_empty()).unwrap_or(false) {
            content.push_str("记录：\n");
            for record in &records {
                content.push_str(&render_case_record(record));
                content.push('\n');
            }
            for entry in extra.into_iter().flatten() {
                content.push_str(&render_entry(entry));
                content.push('\n');
            }
        }
        if !node.children.is_empty() {
            content.push_str("\n子 case 摘要：\n");
            for child in state.tree.children_ids(case_id) {
                // 本批刚重算的子 case 摘要优先（父摘要必须涵盖子 case 的最新内容）
                let summary = computed
                    .get(&child)
                    .cloned()
                    .or_else(|| state.tree.brief(&child).map(|b| b.summary.as_str().to_string()))
                    .unwrap_or_default();
                content.push_str(&format!("- {}: {}\n", child, summary));
            }
        }

        Ok(Some(self.model.summarize(scope, &content).await?))
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
        channel_with_id(time, sn, is_self, text, "")
    }

    /// 带 msg_id 的通道记录（消息与记录按「通道三元组 + msg_id」去重）
    fn channel_with_id(time: &str, sn: u64, is_self: usize, text: &str, msg_id: &str) -> RecordEntry {
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
                msg_id: Arc::new(msg_id.to_string()),
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
        message_with_id(time, "", text)
    }

    /// 带 msg_id 的定位消息（msg_id 为空时按不可去重处理）
    fn message_with_id(time: &str, msg_id: &str, text: &str) -> LocateMessage {
        LocateMessage {
            msg_id: Arc::new(msg_id.to_string()),
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

    /// 批次所属通道（与各测试构造的记录/消息一致）
    fn channel_key() -> ChannelKey {
        ChannelKey::new("web", "self", "g1")
    }

    #[test]
    fn merge_batch_dedups_messages_against_records() {
        let records = vec![
            channel_with_id("2099-01-01 10:00:00", 1, 0, "A", "m1"),
            channel_with_id("2099-01-01 10:02:00", 2, 1, "C", "m2"),
        ];
        // 新消息：msg_id 与已存 A 相同（内容 / 时间 / 发送者都可以不同）；B 的 msg_id 是新的
        let a = message_with_id("2099-01-01 23:59:59", "m1", "内容不同也判为同一条");
        let b = message_with_id("2099-01-01 10:03:00", "m3", "B");
        let items = merge_batch(&channel_key(), &records, &[&a, &b]);

        assert_eq!(items.len(), 3, "重复的新消息不单独成条");
        let text = render_batch(&items);
        assert_eq!(
            text,
            "[0] [2099-01-01 10:00:00] alice: A\n[1] [2099-01-01 10:02:00] alice: C\n[2] [2099-01-01 10:03:00] alice: B",
            "重复的新消息只以已存版本出现一次"
        );
        assert!(!text.contains("内容不同也判为同一条"), "重复的新消息自身不进分析批次");
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
    fn merge_batch_dedups_by_msg_id() {
        let records = vec![channel_with_id("2099-01-01 10:00:00", 1, 0, "A", "m1")];

        // 批次已按通道切分，判重只比 msg_id：内容 / 时间 / 发送者 / is_self 都不参与
        let same = message_with_id("2099-01-01 23:00:00", "m1", "另一个内容");
        assert_eq!(merge_batch(&channel_key(), &records, &[&same]).len(), 1, "msg_id 相同即同一条");

        let other = message_with_id("2099-01-01 10:00:00", "m2", "A");
        assert_eq!(merge_batch(&channel_key(), &records, &[&other]).len(), 2, "msg_id 不同不去重");
        let other = message_with_id("2099-01-01 10:00:00", "", "A");
        assert_eq!(merge_batch(&channel_key(), &records, &[&other]).len(), 2, "新消息 msg_id 为空时不参与去重");

        // 已存记录 msg_id 为空（旧记录、思考/工具占位记录）不参与去重、原样保留
        let legacy = vec![channel("2099-01-01 10:00:00", 1, 0, "A")];
        assert_eq!(merge_batch(&channel_key(), &legacy, &[&same]).len(), 2, "已存记录 msg_id 为空时不参与去重");

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
        assert_eq!(merge_batch(&channel_key(), &[think], &[&same]).len(), 2, "非通道记录不去重");
    }

    /// 调用方必须按通道分开调用：混入其他通道的记录/消息会被契约断言拦住
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "只接受同一通道")]
    fn merge_batch_rejects_mixed_channels() {
        let records = vec![channel_with_id("2099-01-01 10:00:00", 1, 0, "A", "m1")];
        let mut foreign = message_with_id("2099-01-01 10:00:00", "m1", "A");
        foreign.self_user_id = Arc::new("self2".into());
        merge_batch(&channel_key(), &records, &[&foreign]);
    }

    #[test]
    fn merge_batch_matches_duplicates_one_to_one() {
        let records = vec![channel_with_id("2099-01-01 10:00:00", 1, 0, "A", "m1")];
        let first = message_with_id("2099-01-01 10:00:00", "m1", "A");
        let second = message_with_id("2099-01-01 10:01:00", "m1", "A");
        let items = merge_batch(&channel_key(), &records, &[&first, &second]);

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
        let items = merge_batch(&channel_key(), &records, &[&earlier, &same_time]);

        let times: Vec<&str> = items.iter().map(BatchItem::time).collect();
        assert_eq!(times, vec!["2099-01-01 09:00:00", "2099-01-01 10:00:00", "2099-01-01 10:00:00"]);
        assert!(matches!(&items[0], BatchItem::New { .. }));
        assert!(matches!(&items[1], BatchItem::Stored { .. }), "同一时刻已存记录在前");
        assert!(matches!(&items[2], BatchItem::New { .. }));
    }

    #[test]
    fn recompute_order_puts_descendants_first() {
        let mut tree = CaseTree::new();
        tree.create_node("role-1", None, CaseMode::Role, "t");
        tree.create_node("role-2", Some("role-1"), CaseMode::Role, "t");
        tree.create_node("role-3", Some("role-2"), CaseMode::Role, "t");
        let touched: BTreeSet<String> = ["role-3".to_string()].into_iter().collect();
        let archiver = Archiver::new(Arc::new(crate::stub::StubModel::new()), 4, 200);
        let order = archiver.recompute_order(&tree, &touched);
        assert_eq!(order, vec!["role-3".to_string(), "role-2".to_string(), "role-1".to_string()]);
    }

}
