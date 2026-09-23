//! 定位器：把一批消息按通道用「当前事件 + 消息」分组（与归档同一套），再只读地逐层下探出每组所属事件。
//!
//! 定位只读：不新建事件、不读原文副本。触发懒处理时，待归档记录与新消息的去重合并、以及
//! 只含新消息的组的只读下探由归档器一并完成（见 [`crate::archive`]）；本模块同时提供逐层下探
//! 与结果汇总的公共实现（`pub(crate)`），供归档器复用。

use std::collections::BTreeMap;
use std::sync::Arc;

use kissbot_api::{
    ChannelKey, EventMode, LocateMessage, LocateResponse, LocatedEventRef, LocatedGroup,
};

use crate::error::Result;
use crate::model::DecisionModel;
use crate::records::render_content;
use crate::scope::{ScopeHandle, ScopeId, ScopeState};
use crate::tree::Tree;

pub struct Locator {
    model: Arc<dyn DecisionModel>,
    max_depth: usize,
}

impl Locator {
    pub fn new(model: Arc<dyn DecisionModel>, max_depth: usize) -> Self {
        Self { model, max_depth }
    }

    /// 定位一批消息涉及的全部事件（只读，不含懒处理）：按通道分别做第 1 轮分组，
    /// 属于当前事件的组命中该通道当前事件，其余组只读地逐层下探到所属事件；
    /// 返回涉及的事件（去重）与分组（哪几条消息属于哪个事件）。
    pub async fn locate(
        &self,
        handle: &ScopeHandle,
        state: &ScopeState,
        messages: &[LocateMessage],
    ) -> Result<LocateResponse> {
        if messages.is_empty() {
            return Ok(LocateResponse::empty());
        }

        // 事件模式：作用域整体一个事件，不需要判定
        if handle.mode == EventMode::Event {
            let Some(event_id) = state.tree.top_level_ids().first().cloned() else {
                return Ok(LocateResponse::empty());
            };
            let is_current = state.current_events.values().any(|id| id == &event_id);
            return Ok(LocateResponse {
                events: vec![Arc::new(LocatedEventRef { event_id: Arc::new(event_id.clone()), is_current })],
                groups: vec![Arc::new(LocatedGroup {
                    messages: (0..messages.len()).collect(),
                    event_id: Some(Arc::new(event_id)),
                    is_current,
                })],
            });
        }

        // 按通道分别处理：每条通道用自己的当前事件做第 1 轮分组
        let mut located = Vec::new();
        for (channel, indices) in group_indices_by_channel(messages) {
            located.extend(self.locate_channel(handle, state, &channel, &indices, messages).await?);
        }
        Ok(merge_located(located, &state.current_events))
    }

    /// 定位某通道的一批消息：返回（组内首条消息在请求中的下标, 通道, 分组）。
    ///
    /// 标记为当前事件的组命中该通道当前事件；其余组只读地逐层下探；
    /// 空树与事件模式不调模型。`is_current` 留待 [`merge_located`] 按最终状态统一判定。
    pub(crate) async fn locate_channel(
        &self,
        handle: &ScopeHandle,
        state: &ScopeState,
        channel: &ChannelKey,
        indices: &[usize],
        messages: &[LocateMessage],
    ) -> Result<Vec<(usize, ChannelKey, LocatedGroup)>> {
        let Some(first) = indices.first().copied() else {
            return Ok(Vec::new());
        };

        // 事件模式：作用域整体一个事件，不需要判定
        if handle.mode == EventMode::Event {
            let event_id = state.tree.top_level_ids().first().cloned();
            return Ok(vec![(
                first,
                channel.clone(),
                LocatedGroup { messages: indices.to_vec(), event_id: event_id.map(Arc::new), is_current: false },
            )]);
        }

        // 空树：没有事件可涉及，整批一组（不调模型）
        if state.tree.is_empty() {
            return Ok(vec![(
                first,
                channel.clone(),
                LocatedGroup { messages: indices.to_vec(), event_id: None, is_current: false },
            )]);
        }

        let channel_messages: Vec<&LocateMessage> = indices.iter().map(|index| &messages[*index]).collect();
        let current = state.current_events.get(channel).cloned();
        let current_brief = current.as_deref().and_then(|id| state.tree.brief(id));
        let text = render_messages(&channel_messages);
        let groups = self.model.group(&handle.id, &text, channel_messages.len(), current_brief.as_ref()).await?;

        let mut located = Vec::new();
        for group in groups {
            let group_messages: Vec<&LocateMessage> = group.messages.iter().map(|index| channel_messages[*index]).collect();
            let event_id = if group.is_current {
                // 标记为当前事件的组：命中该通道当前事件；没有当前事件时容忍（定位只读，退回下探）
                match current.clone() {
                    Some(current) => Some(current),
                    None => {
                        find_event(self.model.as_ref(), &handle.id, &state.tree, &render_messages(&group_messages), self.max_depth).await?
                    }
                }
            } else {
                find_event(self.model.as_ref(), &handle.id, &state.tree, &render_messages(&group_messages), self.max_depth).await?
            };
            let global: Vec<usize> = group.messages.iter().map(|index| indices[*index]).collect();
            let first = *global.first().expect("分组非空（parse_groups 保证）");
            located.push((
                first,
                channel.clone(),
                LocatedGroup { messages: global, event_id: event_id.map(Arc::new), is_current: false },
            ));
        }
        Ok(located)
    }
}

/// 只读地自顶层逐层下探：命中叶子事件返回它；命中非叶子后其子层都未命中则返回该父事件；均未命中返回 None
pub(crate) async fn find_event(
    model: &dyn DecisionModel,
    scope: &ScopeId,
    tree: &Tree,
    text: &str,
    max_depth: usize,
) -> Result<Option<String>> {
    let top = tree.top_level_ids();
    if top.is_empty() {
        return Ok(None);
    }
    let Some(mut event_id) = model.locate(scope, text, &tree.briefs(&top)).await? else {
        return Ok(None);
    };
    if !tree.contains(&event_id) {
        return Err(crate::error::Error::ModelApiError(format!(
            "定位给出不存在的事件 {}（作用域 {}）",
            event_id, scope
        )));
    }
    for _ in 0..max_depth {
        if tree.is_leaf(&event_id) {
            return Ok(Some(event_id));
        }
        let children = tree.children_ids(&event_id);
        match model.locate(scope, text, &tree.briefs(&children)).await? {
            Some(child_id) => {
                if !tree.contains(&child_id) {
                    return Err(crate::error::Error::ModelApiError(format!(
                        "定位给出不存在的事件 {}（作用域 {}）",
                        child_id, scope
                    )));
                }
                event_id = child_id;
            }
            // 子事件都未命中：命中该父事件（整体话题的代表）
            None => return Ok(Some(event_id)),
        }
    }
    Ok(Some(event_id))
}

/// 汇总各通道的定位分组：`is_current` 按各通道的最终当前事件判定，
/// 分组按各组首条消息下标升序，事件按首次出现顺序去重。
pub(crate) fn merge_located(
    mut located: Vec<(usize, ChannelKey, LocatedGroup)>,
    current_events: &BTreeMap<ChannelKey, String>,
) -> LocateResponse {
    located.sort_by_key(|(first, _, _)| *first);

    let mut groups: Vec<Arc<LocatedGroup>> = Vec::with_capacity(located.len());
    let mut events: Vec<Arc<LocatedEventRef>> = Vec::new();
    for (_, channel, group) in located {
        let is_current = group
            .event_id
            .as_ref()
            .map(|event_id| current_events.get(&channel).map(|current| current.as_str() == event_id.as_str()).unwrap_or(false))
            .unwrap_or(false);
        let group = Arc::new(LocatedGroup { messages: group.messages, event_id: group.event_id, is_current });
        if let Some(event_id) = group.event_id.as_ref() {
            match events.iter_mut().find(|event| event.event_id == *event_id) {
                Some(event) => {
                    if is_current {
                        *event = Arc::new(LocatedEventRef { event_id: event_id.clone(), is_current: true });
                    }
                }
                None => events.push(Arc::new(LocatedEventRef { event_id: event_id.clone(), is_current })),
            }
        }
        groups.push(group);
    }
    LocateResponse { events, groups }
}

/// 按通道分组消息下标（通道按三元组升序，组内下标升序）
pub(crate) fn group_indices_by_channel(messages: &[LocateMessage]) -> Vec<(ChannelKey, Vec<usize>)> {
    let mut by_channel: BTreeMap<ChannelKey, Vec<usize>> = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        by_channel.entry(message.channel()).or_default().push(index);
    }
    by_channel.into_iter().collect()
}

/// 待定位消息的批次文本（每行 `[下标] 时间 用户名: 内容`）
fn render_messages(messages: &[&LocateMessage]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| format!("[{}] {}", index, render_message(message)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 新消息的可判定文本
pub fn render_message(message: &LocateMessage) -> String {
    let self_mark = if message.is_self > 0 { " (本人)" } else { "" };
    format!("[{}] {}: {}{}", message.time, message.user_name, render_content(&message.content), self_mark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::Archiver;
    use crate::model::MessageGroup;
    use crate::records::{RecordBody, RecordEntry};
    use crate::scope::{ScopeId, ScopeManager};
    use crate::stub::StubModel;
    use kissbot_api::{ChannelRecord, Content, RecordKind};

    fn channel(time: &str, sn: u64, text: &str) -> RecordEntry {
        RecordEntry {
            kind: RecordKind::Channel,
            time: Arc::new(time.to_string()),
            sn,
            body: RecordBody::Channel(Arc::new(ChannelRecord {
                user_id: Arc::new("u1".into()),
                self_user_id: Arc::new("self".into()),
                messenger_id: Arc::new("web".into()),
                group_id: Arc::new("g1".into()),
                is_self: 0,
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new("alice".into()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new(text.to_string())),
                time: Arc::new(time.to_string()),
                sn,
            })),
        }
    }

    fn channel_key() -> ChannelKey {
        ChannelKey::new("web", "self1", "g1")
    }

    fn message(text: &str) -> LocateMessage {
        LocateMessage {
            time: Arc::new("2099-01-01 14:00:00".into()),
            user_name: Arc::new("alice".into()),
            is_self: 0,
            messenger_id: Arc::new("web".into()),
            self_user_id: Arc::new("self1".into()),
            group_id: Arc::new("g1".into()),
            content: Content::Text(Arc::new(text.to_string())),
        }
    }

    fn message_in(channel: &ChannelKey, text: &str) -> LocateMessage {
        LocateMessage {
            messenger_id: channel.messenger_id.clone(),
            self_user_id: channel.self_user_id.clone(),
            group_id: channel.group_id.clone(),
            ..message(text)
        }
    }

    /// 归档一批记录（返回归档器，模型桩由调用方持有）
    async fn archive(handle: &ScopeHandle, model: &Arc<StubModel>, channel: &ChannelKey, entries: &[RecordEntry]) {
        let archiver = Archiver::new(model.clone(), 4);
        let mut state = handle.state.lock().await;
        archiver.archive_pending(handle, &mut state, channel, entries).await.unwrap();
    }

    fn event_ids(response: &LocateResponse) -> Vec<String> {
        response.events.iter().map(|event| event.event_id.as_str().to_string()).collect()
    }

    #[tokio::test]
    async fn empty_messages_or_empty_tree_returns_no_events() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let state = handle.state.lock().await;
        let model = Arc::new(StubModel::new());
        let locator = Locator::new(model.clone(), 4);

        // 没有消息
        let response = locator.locate(&handle, &state, &[]).await.unwrap();
        assert!(response.events.is_empty());

        // 空树：按通道各成一组，但不涉及任何事件，且不调模型
        let response = locator.locate(&handle, &state, &[message("你好"), message("在吗")]).await.unwrap();
        assert!(response.events.is_empty());
        assert_eq!(response.groups.len(), 1, "同一通道归一组");
        assert!(response.groups[0].event_id.is_none());
        assert!(model.prompts().is_empty(), "空树不调模型");
    }

    #[tokio::test]
    async fn current_event_group_hits_current_event() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[channel("2099-01-01 10:00:00", 1, "聊天气")]).await;
        let state = handle.state.lock().await;

        // 第 1 轮把整批标记为当前事件组 → 命中该通道当前事件
        model.push_group(&[0, 1], true);
        let locator = Locator::new(model.clone(), 4);
        let response = locator.locate(&handle, &state, &[message("今天下雨吗"), message("带伞吗")]).await.unwrap();
        assert_eq!(event_ids(&response), vec!["e1"]);
        assert!(response.events[0].is_current);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(response.groups[0].event_id.as_ref().map(|id| id.as_str()), Some("e1"));
    }

    #[tokio::test]
    async fn new_topic_group_has_no_event() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[channel("2099-01-01 10:00:00", 1, "聊天气")]).await;
        let state = handle.state.lock().await;

        // 组未标记当前事件 → 只读下探；顶层未命中 → 该组不涉及任何事件
        model.push_group(&[0], false);
        model.push_locate(None);
        let locator = Locator::new(model.clone(), 4);
        let response = locator.locate(&handle, &state, &[message("换个话题")]).await.unwrap();
        assert!(response.events.is_empty(), "新话题不涉及已有事件");
        assert_eq!(response.groups.len(), 1);
        assert!(response.groups[0].event_id.is_none());
    }

    #[tokio::test]
    async fn multiple_messages_map_to_multiple_events() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[channel("2099-01-01 10:00:00", 1, "事件一")]).await;
        // 第二批未标记当前事件 → 顶层归并新建 e2
        model.push_group(&[0], false);
        archive(&handle, &model, &channel_key(), &[channel("2099-01-01 11:00:00", 2, "事件二")]).await;
        let state = handle.state.lock().await;
        assert_eq!(state.tree.len(), 2);
        let current = state.current_events.get(&channel_key()).cloned().unwrap();
        assert_eq!(current, "e2");

        // 4 条消息分成三组：前两条延续当前事件 e2；第三条命中已有事件 e1；第四条是新话题
        model.push_groups(vec![
            MessageGroup { messages: vec![0, 1], is_current: true },
            MessageGroup { messages: vec![2], is_current: false },
            MessageGroup { messages: vec![3], is_current: false },
        ]);
        model.push_locate(Some("e1")); // 第三条：顶层命中 e1（叶子）
        model.push_locate(None); // 第四条：顶层未命中 → 新话题
        let locator = Locator::new(model.clone(), 4);
        let response = locator
            .locate(&handle, &state, &[message("a"), message("b"), message("c"), message("d")])
            .await
            .unwrap();

        assert_eq!(event_ids(&response), vec!["e2", "e1"], "涉及的事件按首次出现顺序去重");
        let flags: Vec<bool> = response.events.iter().map(|event| event.is_current).collect();
        assert_eq!(flags, vec![true, false]);
        assert_eq!(response.groups.len(), 3);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(response.groups[1].event_id.as_ref().map(|id| id.as_str()), Some("e1"));
        assert!(response.groups[0].is_current);
        assert!(!response.groups[1].is_current);
        assert!(response.groups[2].event_id.is_none(), "新话题组没有事件");
    }

    #[tokio::test]
    async fn level_descent_picks_child_or_returns_parent() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[channel("2099-01-01 10:00:00", 1, "父事件内容")]).await;
        // 构造 e1（父）/ e2（子）结构
        {
            let mut state = handle.state.lock().await;
            state.tree.create_node("e2", Some("e1"), EventMode::Role, "t");
        }
        let state = handle.state.lock().await;

        // 第三条消息：顶层命中 e1（非叶子）→ 下探子层命中 e2
        model.push_group(&[0], false);
        model.push_locate(Some("e1"));
        model.push_locate(Some("e2"));
        let locator = Locator::new(model.clone(), 4);
        let response = locator.locate(&handle, &state, &[message("子事件相关")]).await.unwrap();
        assert_eq!(event_ids(&response), vec!["e2"]);

        // 第四条消息：顶层命中 e1（非叶子）→ 子层都未命中 → 命中父事件 e1
        model.push_group(&[0], false);
        model.push_locate(Some("e1"));
        model.push_locate(None);
        let response = locator.locate(&handle, &state, &[message("父事件相关")]).await.unwrap();
        assert_eq!(event_ids(&response), vec!["e1"]);
    }

    #[tokio::test]
    async fn messages_on_different_channels_use_their_own_current_event() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        let other = ChannelKey::new("web", "self2", "g2");
        archive(&handle, &model, &channel_key(), &[channel("2099-01-01 10:00:00", 1, "通道一")]).await;
        model.push_group(&[0], false);
        archive(&handle, &model, &other, &[channel("2099-01-01 11:00:00", 2, "通道二")]).await;
        let state = handle.state.lock().await;
        let current_first = state.current_events.get(&channel_key()).cloned().unwrap();
        let current_other = state.current_events.get(&other).cloned().unwrap();
        assert_ne!(current_first, current_other);

        // 两个通道各一条消息，各自标记为当前事件组 → 各命中自己的当前事件
        model.push_group(&[0], true);
        model.push_group(&[0], true);
        let locator = Locator::new(model.clone(), 4);
        let response = locator
            .locate(&handle, &state, &[message("通道一问"), message_in(&other, "通道二问")])
            .await
            .unwrap();
        assert_eq!(event_ids(&response), vec![current_first.as_str(), current_other.as_str()]);
        assert_eq!(response.groups[0].messages, vec![0]);
        assert_eq!(response.groups[1].messages, vec![1]);
        assert!(response.groups.iter().all(|group| group.is_current));
    }

    #[tokio::test]
    async fn locate_does_not_read_records() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[channel("2099-01-01 10:00:00", 1, "聊天气")]).await;
        // 删掉原文副本目录：定位不读原文，仍应命中
        tokio::fs::remove_dir_all(handle.events_dir()).await.unwrap();
        let state = handle.state.lock().await;

        model.push_group(&[0], true);
        let locator = Locator::new(model.clone(), 4);
        let response = locator.locate(&handle, &state, &[message("今天下雨吗")]).await.unwrap();
        assert_eq!(event_ids(&response), vec!["e1"]);
    }

    #[tokio::test]
    async fn event_mode_returns_single_event_without_model() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev-e1")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[channel("2099-01-01 10:00:00", 1, "事件模式内容")]).await;
        let state = handle.state.lock().await;

        let locator = Locator::new(model.clone(), 4);
        let response = locator.locate(&handle, &state, &[message("任意消息"), message("又一条")]).await.unwrap();
        assert_eq!(event_ids(&response), vec!["e1"]);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert!(model.prompts().is_empty(), "事件模式不触发定位判定");
    }

    #[test]
    fn render_message_marks_self() {
        let mut msg = message("你好");
        assert_eq!(render_message(&msg), "[2099-01-01 14:00:00] alice: 你好");
        msg.is_self = 1;
        assert!(render_message(&msg).ends_with("(本人)"));
    }
}
