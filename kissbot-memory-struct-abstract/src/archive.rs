//! 归档器：批次分组、分组落位与摘要级联重算。
//!
//! 角色模式一次处理某通道水位之后的全部记录，分两段：
//! 1. 第 1 轮（分组轮）：把当前事件与全部待处理消息一起交给模型，模型按事件分组并标记属于当前事件的组；
//!    标记为当前事件的组直接并入当前事件（不下潜）；
//! 2. 第 2 轮起分层归并：其余组以组为单位，自顶层逐层归并（非叶子继续下潜），或在本层新增叶子事件。
//!
//! 事件之间的关联关系与「多个事件合并为大事件」由后续定时程序产生，因此当前新建的事件都是叶子。

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use kissbot_api::{ChannelKey, EventBrief, EventMode, RecordKind};
use tracing::warn;

use crate::error::{Error, Result};
use crate::model::{DecisionModel, LevelDecision};
use crate::records::{
    RecordEntry, append_event_records, read_event_records, render_entry, render_event_record,
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

/// 归档器
pub struct Archiver {
    model: Arc<dyn DecisionModel>,
    max_depth: usize,
}

impl Archiver {
    pub fn new(model: Arc<dyn DecisionModel>, max_depth: usize) -> Self {
        Self { model, max_depth }
    }

    /// 归档某通道待处理的全部记录：分组落位 → 重算摘要与级联祖先 → 追加原文副本 → 更新该通道的当前事件指针 → 推进该通道水位。
    ///
    /// 所有模型调用都在落盘之前完成；任一模型调用失败时恢复内存状态，磁盘与水位保持不变，
    /// 这批记录留待下次查询重试（见 spec：解析失败按失败处理并保留原状态）。
    pub async fn archive_pending(
        &self,
        handle: &ScopeHandle,
        state: &mut ScopeState,
        channel: &ChannelKey,
        records: &[RecordEntry],
    ) -> Result<ArchiveOutcome> {
        if records.is_empty() {
            return Ok(ArchiveOutcome { archived: 0, event_ids: Vec::new() });
        }
        let snapshot = state.clone();
        match self.archive_inner(handle, state, channel, records).await {
            Ok(outcome) => Ok(outcome),
            Err(e) => {
                // 判定阶段会在内存树上新建节点：失败时回到调用前状态，避免留下空节点
                *state = snapshot;
                Err(e)
            }
        }
    }

    async fn archive_inner(
        &self,
        handle: &ScopeHandle,
        state: &mut ScopeState,
        channel: &ChannelKey,
        records: &[RecordEntry],
    ) -> Result<ArchiveOutcome> {
        let now = crate::records::now_string();
        let scope = handle.id.clone();

        // 1. 分组落位：得到「事件 → 本批落入该事件的记录」（按组序）
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
                vec![(event_id, records.to_vec())]
            }
            EventMode::Role => self.group_and_place(&scope, state, channel, records).await?,
        };

        let mut new_records: HashMap<String, Vec<RecordEntry>> = HashMap::new();
        let mut touched: BTreeSet<String> = BTreeSet::new();
        let mut event_ids: Vec<String> = Vec::new();
        for (event_id, group) in placements {
            new_records.entry(event_id.clone()).or_default().extend(group);
            touched.insert(event_id.clone());
            if !event_ids.contains(&event_id) {
                event_ids.push(event_id);
            }
        }

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

        // 4. 内存：应用记录与摘要；当前事件指针指向最后一批消息落入的事件
        for (event_id, group) in &new_records {
            state.tree.apply_records(event_id, group, &now);
        }
        for (id, summary) in &summaries {
            state.tree.set_summary(id, summary, &now);
        }
        if let Some(last) = event_ids.last() {
            state.current_events.insert(channel.clone(), last.clone());
        }
        advance_watermark(state.watermarks.entry(channel.clone()).or_default(), records);

        Ok(ArchiveOutcome { archived: records.len() as u64, event_ids })
    }

    /// 角色模式的分组落位：第 1 轮分组（并标记当前事件组），当前事件组直接并入、
    /// 其余组各自走第 2 轮起的分层归并
    async fn group_and_place(
        &self,
        scope: &ScopeId,
        state: &mut ScopeState,
        channel: &ChannelKey,
        records: &[RecordEntry],
    ) -> Result<Vec<(String, Vec<RecordEntry>)>> {
        let text = render_batch(records);
        let current = state.current_events.get(channel).cloned();
        let current_brief = current.as_deref().and_then(|id| state.tree.brief(id));
        let groups = self.model.group(scope, &text, records.len(), current_brief.as_ref()).await?;

        let mut placements: Vec<(String, Vec<RecordEntry>)> = Vec::with_capacity(groups.len());
        for group in groups {
            let group_records: Vec<RecordEntry> = group
                .messages
                .iter()
                .map(|index| records[*index].clone())
                .collect();
            let event_id = if group.is_current {
                // 标记为当前事件的组直接并入当前事件
                let current = current.clone().ok_or_else(|| {
                    Error::ModelApiError(format!(
                        "第 1 轮把分组标记为当前事件，但通道 {} 没有当前事件（作用域 {}）",
                        channel.messenger_id, scope
                    ))
                })?;
                if !state.tree.contains(&current) {
                    return Err(Error::StateError(format!("当前事件不存在: {}（作用域 {}）", current, scope)));
                }
                current
            } else {
                // 其余组以组为单位自顶层逐层归并
                self.merge_from(scope, &mut state.tree, &render_batch(&group_records), None)
                    .await?
            };
            placements.push((event_id, group_records));
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
pub fn render_batch(batch: &[RecordEntry]) -> String {
    batch
        .iter()
        .enumerate()
        .map(|(index, entry)| format!("[{}] {}", index, render_entry(entry)))
        .collect::<Vec<_>>()
        .join("\n")
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
    use crate::records::RecordBody;
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
        let batch = vec![channel("2026-09-22 10:00:00", 1, 0, "a"), channel("2026-09-22 10:01:00", 2, 1, "b")];
        let text = render_batch(&batch);
        assert_eq!(text, "[0] [2026-09-22 10:00:00] alice: a\n[1] [2026-09-22 10:01:00] alice: b");
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
