//! 归档器：批次分组、分组落位与摘要级联重算。
//!
//! 角色模式一次处理某记忆后缀某通道的一批条目，分两段：
//! 1. 第 1 轮（分组轮）：把当前 case 与该批条目一起交给模型，模型按 case 分组并标记属于当前 case 的组；
//!    标记为当前 case 的组直接并入当前 case（不下潜）；
//! 2. 第 2 轮起分层归并：其余组以组为单位，自顶层逐层归并（非叶子继续下潜），或在本层新增叶子 case。
//!
//! 事件模式（给出 `event_id`）不分组、不参与判定：整个后缀固定归入同一棵 case 树里的顶层叶子
//! `event-{event_id}`（case 名即查找键，节点本身不带事件字段）。两种 case 共用一棵树、靠名称
//! 前缀区分（角色 `role-`、事件 `event-`），角色模式的逐层判定候选只取 `role-` 前缀，看不到事件叶子。
//!
//! 批次条目既可以是待归档记录，也可以是只用于定位的新消息（见 [`BatchItem`]）：定位请求触发懒处理时
//! 先按通道与待归档记录去重（重复的新消息用已存版本分析一次、只返回所属 case），其余新消息随批次一起
//! 参与分组但不落盘。只含新消息的组同样命中或建出 case（新话题建出的 case 记忆条数为 0）。
//!
//! Case 之间的关联关系与「多个 case 合并为大 case」由后续定时程序产生，因此当前新建的 case 都是叶子。

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::{CaseBrief, ChannelKey, LocateMessage, RecordKind, RoleEvent};

use crate::error::{Error, Result};
use crate::locate::render_message;
use crate::model::{DecisionModel, LevelDecision};
use crate::records::{
    RecordEntry, append_case_records, read_case_records, render_case_record,
    render_entry,
};
use crate::agent_role::{AgentRole, AgentRoleHandle, AgentRoleState, role_event_of};
use crate::tree::{CaseTree, event_case_name};

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
    /// 各分块的归档结果（按块序）
    pub outcomes: Vec<ArchiveOutcome>,
    /// 消息分组（`PlaceGroup::messages` 是传入 `messages` 的下标）
    pub groups: Vec<PlaceGroup>,
    /// 本次参与定位的新消息条数
    pub new_messages: usize,
}

/// 统一分析批次的一个条目
#[derive(Debug, Clone)]
pub enum BatchItem {
    /// 共享记忆中的待归档记录
    Stored(RecordEntry),
    /// 只用于定位的新消息（请求内下标）
    New { index: usize, message: LocateMessage },
}

impl BatchItem {
    /// 排序键：条目时间
    fn time(&self) -> &str {
        match self {
            BatchItem::Stored(record) => record.time.as_str(),
            BatchItem::New { message, .. } => message.time.as_str(),
        }
    }

    /// 送模型的文本形态
    fn render(&self) -> String {
        match self {
            BatchItem::Stored(record) => render_entry(record),
            BatchItem::New { message, .. } => render_message(message),
        }
    }

    /// 待归档记录（只用于定位的新消息没有）
    fn record(&self) -> Option<&RecordEntry> {
        match self {
            BatchItem::Stored(record) => Some(record),
            BatchItem::New { .. } => None,
        }
    }

    /// 是否是待归档记录（会落盘归档）
    pub fn is_stored(&self) -> bool {
        matches!(self, BatchItem::Stored(_))
    }

    /// 该条目涉及的新消息下标
    fn messages(&self) -> &[usize] {
        match self {
            BatchItem::Stored(_) => &[],
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
    pub case_name: ArcStr,
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
}

/// 分组落位（内部）：落点判定 + 批次内条目下标 + 该组的待归档记录
struct Placement {
    verdict: GroupVerdict,
    items: Vec<usize>,
    records: Vec<RecordEntry>,
}

/// 把某通道的待归档记录与该通道的新消息合并为统一分析批次（按时间升序，同一时刻已存记录在前）。
///
/// 调用方必须已**按通道分开调用**（归档与定位都逐通道进行）。新消息在调用 locate 时还没有写入记忆，
/// 因此不需要与已存记录按 `msg_id` 去重。
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
        .map(|record| BatchItem::Stored(record.clone()))
        .chain(messages.iter().enumerate().map(|(index, message)| BatchItem::New { index, message: (*message).clone() }))
        .collect();
    items.sort_by(|a, b| a.time().cmp(b.time()));
    items
}

/// 归档器
pub struct Archiver {
    model: Arc<dyn DecisionModel>,
    /// 单通道单次分析（分块）的条目上限
    max_batch_records: usize,
}

impl Archiver {
    pub fn new(model: Arc<dyn DecisionModel>, max_batch_records: usize) -> Self {
        Self { model, max_batch_records: max_batch_records.max(1) }
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
        handle: &AgentRoleHandle,
        state: &mut AgentRoleState,
        event_id: Option<ArcStr>,
        channel: &ChannelKey,
        records: &[RecordEntry],
        messages: &[&LocateMessage],
    ) -> Result<ArchiveResult> {
        if records.is_empty() && messages.is_empty() {
            return Ok(ArchiveResult::default());
        }

        let role_event = role_event_of(&handle.id, event_id.clone());
        let items = merge_batch(channel, records, messages);
        let new_messages = items.iter().filter(|item| !item.is_stored()).count();
        let mut outcomes = Vec::new();
        let mut groups = Vec::new();
        for chunk in items.chunks(self.max_batch_records) {
            let (outcome, chunk_groups) =
                self.archive_chunk(handle, state, event_id.clone(), channel, chunk).await?;
            if outcome.archived > 0 {
                // 逐块落盘：块失败时前面的进度不回退，失败块由 archive_chunk 恢复内存状态
                handle.save_state(&role_event, state).await?;
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
        handle: &AgentRoleHandle,
        state: &mut AgentRoleState,
        event_id: Option<ArcStr>,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<(ArchiveOutcome, Vec<PlaceGroup>)> {
        if items.is_empty() {
            return Ok((ArchiveOutcome { archived: 0, case_names: Vec::new() }, Vec::new()));
        }
        let snapshot = state.clone();
        match self.archive_chunk_inner(handle, state, event_id, channel, items).await {
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
        handle: &AgentRoleHandle,
        state: &mut AgentRoleState,
        event_id: Option<ArcStr>,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<(ArchiveOutcome, Vec<PlaceGroup>)> {
        let now = crate::records::now_string();
        let agent_role: &AgentRole = &handle.id;
        let role_event = role_event_of(agent_role, event_id.clone());

        // 0. 带 meta 的待归档记录：写入方已经给出所属 case，直接按 case 名定位，
        //    不参与分组/逐层判定（case 不存在时退回分组流程，例如重建后树里还没有该节点）
        let mut new_records: HashMap<ArcStr, Vec<RecordEntry>> = HashMap::new();
        let mut unplaced: Vec<BatchItem> = Vec::with_capacity(items.len());
        for item in items {
            let meta_case = item.record().and_then(RecordEntry::meta_case_name);
            match meta_case {
                Some(case_name) if state.tree.contains(case_name) => {
                    let record = item.record().expect("meta 只来自待归档记录").clone();
                    new_records.entry(case_name.clone()).or_default().push(record);
                }
                _ => unplaced.push(item.clone()),
            }
        }

        // 1. 定位（只读）：只对没有 meta 的记录与新消息逐组判定落点，得到「已有 case / 需在何处新建」
        let placements = if unplaced.is_empty() {
            Vec::new()
        } else {
            match event_id.as_ref() {
                // 事件模式：整个后缀固定一个顶层叶子 case，不分组、不参与判定。
                // 名称是 `event-{事件标识}`，与角色模式的 `role-*` 天然不会重名，无需跨模式判重
                Some(event_id) => {
                    // 事件标识必须满足 case 名称字符集（只能含英文字母、数字与 `-`）
                    if !crate::model::valid_case_name(event_id) {
                        return Err(Error::StateError(format!(
                            "事件标识不能用于 case 名（只允许英文字母、数字与 '-'，且不超过 {} 字符）: {}",
                            crate::model::MAX_CASE_NAME_LEN,
                            event_id
                        ).into()));
                    }
                    let case_name = event_case_name(event_id);
                    let indices: Vec<usize> = (0..unplaced.len()).collect();
                    let records = stored_records(&unplaced, &indices);
                    let verdict = match state.tree.event_case(event_id) {
                        // 事件 case 已存在：直接命中（按 event_id 定位，不看层级）
                        Some(_) => GroupVerdict::Case(case_name),
                        // case 还不存在：由落盘阶段建出（只含消息时记忆条数为 0）
                        None => GroupVerdict::NewEvent,
                    };
                    vec![Placement { verdict, items: indices, records }]
                }
                // 角色模式：模型分组 + 逐层判定，只看角色 case（事件叶子不在候选里）
                None => self.group_and_place(agent_role, state, &role_event, channel, &unplaced).await?,
            }
        };

        // 2. 落地 + 存储：逐组建出/命中 case（新话题建出记忆条数为 0 的 case），
        //    有记录的组再聚合记录；随后摘要重算、追加原文副本、应用内存状态、推进水位。
        //    新建的 case 在落盘阶段就带上名称与摘要（角色模式来自模型判定，事件模式按内容生成），
        //    不留到后续查询再去调用模型；这些 case 不再参与第 3 步的摘要重算（祖先仍会级联）。
        let mut touched: BTreeSet<ArcStr> = new_records.keys().cloned().collect();
        let mut created: BTreeSet<ArcStr> = BTreeSet::new();
        // 带 meta 的记录已经直接落到 case：按名称排序，保证结果的确定性
        let mut case_names: Vec<ArcStr> = new_records.keys().cloned().collect();
        case_names.sort();
        let mut groups: Vec<PlaceGroup> = Vec::with_capacity(placements.len());
        for placement in &placements {
            let mut messages: Vec<usize> =
                placement.items.iter().flat_map(|index| unplaced[*index].messages().iter().copied()).collect();
            messages.sort_unstable();
            let case_name = match &placement.verdict {
                GroupVerdict::Case(case_name) => case_name.clone(),
                GroupVerdict::NewRole { parent, name, summary } => {
                    let case_name = state.tree.unique_role_case_name(name);
                    state.tree.create_node(&case_name, parent.as_deref(), &now);
                    state.tree.set_summary(&case_name, summary, &now);
                    created.insert(case_name.clone());
                    touched.insert(case_name.clone());
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
                        // 只有消息、还没有记录：立刻按本组内容生成摘要（有记录时交给第 3 步按记录生成）
                        if placement.records.is_empty() {
                            let text = render_group(&unplaced, &placement.items);
                            let summary = self.model.summarize(agent_role, Some(event_id.clone()), &text).await?;
                            state.tree.set_summary(&case_name, &summary, &now);
                            created.insert(case_name.clone());
                            touched.insert(case_name.clone());
                        }
                    }
                    case_name
                }
            };
            groups.push(PlaceGroup { messages, case_name: case_name.clone() });
            if placement.records.is_empty() {
                continue;
            }
            new_records.entry(case_name.clone()).or_default().extend(placement.records.iter().cloned());
            touched.insert(case_name.clone());
            if !case_names.contains(&case_name) {
                case_names.push(case_name);
            }
        }
        let records: Vec<RecordEntry> = items.iter().filter_map(BatchItem::record).cloned().collect();

        // 3. 摘要重算：叶子在前（自底向上），父 case 摘要涵盖自身记录与子 case 摘要；全部成功后落盘。
        //    本批新建的 case 已有摘要（见上一步），只重算它们的祖先。
        let mut summaries: HashMap<ArcStr, ArcStr> = HashMap::new();
        for id in self.recompute_order(&state.tree, &touched) {
            if created.contains(&id) {
                continue;
            }
            if let Some(summary) =
                self.compute_summary(handle, state, event_id.clone(), &id, &new_records, &summaries).await?
            {
                summaries.insert(id, summary);
            }
        }

        // 4. 落盘原文副本（按 case 聚合）
        for (case_name, group) in &new_records {
            append_case_records(&handle.case_dir(case_name), group).await?;
        }

        // 5. 内存：应用记录与摘要；当前 case 指针指向本批最后一个组落入的 case
        for (case_name, group) in &new_records {
            state.tree.apply_records(case_name, group, &now);
        }
        for (id, summary) in &summaries {
            state.tree.set_summary(id, summary, &now);
        }
        let key = (role_event.clone(), channel.clone());
        if let Some(last) = groups.last() {
            state.current_cases.insert(key.clone(), last.case_name.clone());
        }
        advance_watermark(state.watermarks.entry(key).or_default(), &records);

        Ok((ArchiveOutcome { archived: records.len() as u64, case_names }, groups))
    }

    /// 角色模式的分组与定位（只读）：第 1 轮按 case 分组（并标记当前 case 组），
    /// 当前 case 组直接命中当前 case，其余组各自逐层判定落点；判定不改树，
    /// 由落盘阶段逐组建出/命中 case——有记录的组再追加记录。
    async fn group_and_place(
        &self,
        agent_role: &AgentRole,
        state: &AgentRoleState,
        role_event: &RoleEvent,
        channel: &ChannelKey,
        items: &[BatchItem],
    ) -> Result<Vec<Placement>> {
        let text = render_batch(items);
        let current = state.current_case(role_event, channel);
        let current_brief = current.as_deref().and_then(|id| state.tree.brief(id));
        let groups = self.model.group(agent_role, role_event.event_id.clone(), &text, items.len(), current_brief.as_ref()).await?;

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
                    None if records.is_empty() => {
                        self.decide_placement(agent_role, role_event.event_id.clone(), &state.tree, &text, None).await?
                    }
                    None => {
                        return Err(Error::ModelApiError(format!(
                            "第 1 轮把分组标记为当前 case，但通道 {} 没有当前 case（角色 {}，事件 {:?}）",
                            channel.messenger_id, agent_role, role_event.event_id
                        ).into()))
                    }
                }
            } else {
                self.decide_placement(agent_role, role_event.event_id.clone(), &state.tree, &text, None).await?
            };
            placements.push(Placement { verdict, items: group_items, records });
        }
        Ok(placements)
    }

    /// 逐层判定一个组的落点（只读）：命中叶子 case 即 [`GroupVerdict::Case`]；
    /// 当层未命中（或命中非叶子后其子层都未命中）即 [`GroupVerdict::NewRole`]——名称与摘要
    /// 由这次判定一并给出，因此**即使当层没有候选 case 也要调用模型**（空候选提示词里会说明）。
    ///
    /// 候选只取角色 case（[`CaseTree::role_top_level_ids`] / [`CaseTree::role_children_ids`]）：
    /// 事件叶子不参与判定，两种模式互不并入。
    ///
    /// 没有层数上限：每判定一次要么落在当层（并入叶子 / 新建），要么下潜一层；
    /// case 树有限且无环，下潜次数必然有限，因此循环一定终止。
    async fn decide_placement(
        &self,
        agent_role: &AgentRole,
        event_id: Option<ArcStr>,
        tree: &CaseTree,
        text: &str,
        parent: Option<ArcStr>,
    ) -> Result<GroupVerdict> {
        let mut parent = parent;
        let mut level = match &parent {
            Some(id) => tree.role_children_ids(id),
            None => tree.role_top_level_ids(),
        };

        loop {
            let briefs: Vec<CaseBrief> = tree.briefs(&level);
            match self.model.decide_level(agent_role, event_id.clone(), text, &briefs).await? {
                LevelDecision::New { name, summary } => return Ok(GroupVerdict::NewRole { parent, name, summary }),
                LevelDecision::Merge(case_name) => {
                    if !level.contains(&case_name) {
                        return Err(Error::ModelApiError(format!(
                            "当层判定给出的 case {} 不在当层候选里（角色 {}，事件 {:?}，候选 {:?}）",
                            case_name, agent_role, event_id, level
                        ).into()));
                    }
                    if tree.is_leaf(&case_name) {
                        return Ok(GroupVerdict::Case(case_name));
                    }
                    // 非叶子：下潜到其子层继续判定
                    level = tree.role_children_ids(&case_name);
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

    /// 计算单个 case 的新摘要：自身直接记录（已落盘 + 本批新增）+ 子 case 摘要 → 模型摘要。
    ///
    /// 返回 `None` 表示该 case 没有内容（无记录且无子 case），无需摘要。
    async fn compute_summary(
        &self,
        handle: &AgentRoleHandle,
        state: &AgentRoleState,
        event_id: Option<ArcStr>,
        case_name: &str,
        new_records: &HashMap<ArcStr, Vec<RecordEntry>>,
        computed: &HashMap<ArcStr, ArcStr>,
    ) -> Result<Option<ArcStr>> {
        let Some(node) = state.tree.get(case_name) else {
            return Ok(None);
        };
        let records = read_case_records(&handle.case_dir(case_name)).await?;
        let extra = new_records.get(case_name);
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
                point.time = entry.time.clone();
                point.sn = entry.sn;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::RecordBody;
    use kissbot_api::ChannelRecord;
    use kissbot_api::Content;

    fn channel(time: &str, sn: u64, is_self: usize, text: &str) -> RecordEntry {
        channel_with_id(time, sn, is_self, text, "")
    }

    /// 带 msg_id 的通道记录（消息与记录按「通道三元组 + msg_id」去重）
    fn channel_with_id(time: &str, sn: u64, is_self: usize, text: &str, msg_id: &str) -> RecordEntry {
        RecordEntry {
            kind: RecordKind::Channel,
            time: ArcStr::from(time),
            sn,
            body: RecordBody::Channel(Arc::new(ChannelRecord {
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
            })),
        }
    }

    /// 待归档记录形态的批次条目
    fn stored(time: &str, sn: u64, is_self: usize, text: &str) -> BatchItem {
        BatchItem::Stored(channel(time, sn, is_self, text))
    }

    fn message(time: &str, text: &str) -> LocateMessage {
        message_with_id(time, "", text)
    }

    /// 带 msg_id 的定位消息（msg_id 为空时按不可去重处理）
    fn message_with_id(time: &str, msg_id: &str, text: &str) -> LocateMessage {
        LocateMessage {
            msg_id: ArcStr::from(msg_id),
            time: ArcStr::from(time),
            user_name: ArcStr::from("alice"),
            is_self: 0,
            messenger_id: ArcStr::from("web"),
            self_user_id: ArcStr::from("self"),
            group_id: ArcStr::from("g1"),
            content: Content::Text(ArcStr::from(text)),
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
        ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1"))
    }

    #[test]
    fn merge_batch_keeps_records_and_messages() {
        // 新消息在 locate 时还没有写入记忆：不与已存记录去重，各自成条（按时间排序）
        let records = vec![
            channel_with_id("2099-01-01 10:00:00", 1, 0, "A", "m1"),
            channel_with_id("2099-01-01 10:02:00", 2, 1, "C", "m2"),
        ];
        let same_id = message_with_id("2099-01-01 23:59:59", "m1", "与 A 同 msg_id 的新消息");
        let new = message_with_id("2099-01-01 10:03:00", "m3", "B");
        let items = merge_batch(&channel_key(), &records, &[&same_id, &new]);

        assert_eq!(items.len(), 4, "2 条已存记录 + 2 条新消息");
        let text = render_batch(&items);
        assert!(text.contains("alice: A") && text.contains("alice: C"), "已存记录参与分析");
        assert!(text.contains("与 A 同 msg_id 的新消息") && text.contains("alice: B"), "新消息都参与分析");
        assert_eq!(items[0].time(), "2099-01-01 10:00:00");
        assert!(items[0].is_stored(), "同一时刻已存记录在前");
        match &items[2] {
            BatchItem::New { index, message } => {
                assert_eq!(*index, 1, "新消息保留请求下标");
                assert_eq!(message.content, Content::Text(ArcStr::from("B")));
            }
            other => panic!("应为新消息: {:?}", other),
        }
        assert!(items.iter().any(|item| matches!(item, BatchItem::New { index, .. } if *index == 0)));
    }

    /// 调用方必须按通道分开调用：混入其他通道的记录/消息会被契约断言拦住
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "只接受同一通道")]
    fn merge_batch_rejects_mixed_channels() {
        let records = vec![channel_with_id("2099-01-01 10:00:00", 1, 0, "A", "m1")];
        let mut foreign = message_with_id("2099-01-01 10:00:00", "m1", "A");
        foreign.self_user_id = ArcStr::from("self2");
        merge_batch(&channel_key(), &records, &[&foreign]);
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
        assert!(matches!(&items[1], BatchItem::Stored(_)), "同一时刻已存记录在前");
        assert!(matches!(&items[2], BatchItem::New { .. }));
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
