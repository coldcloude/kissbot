//! 定位结果汇总：归档器对「记录 + 只用于定位的新消息」分组并逐层判定所属事件后，
//! 本模块把各通道的分组按请求下标合并成对外响应（[`merge_located`]），并提供按通道切分
//! 消息下标的 [`group_indices_by_channel`] 与新消息的文本渲染 [`render_message`]。
//!
//! 定位本身不新建事件、不读原文副本：只含新消息的组由归档器用同一套逐层判定（不允许新建）
//! 得到所属事件，见 [`crate::archive`]。

use std::collections::BTreeMap;
use std::sync::Arc;

use kissbot_api::{
    ChannelKey, LocateMessage, LocateResponse, LocatedEventRef, LocatedGroup,
};

use crate::records::render_content;

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
        let is_current = current_events
            .get(&channel)
            .map(|current| current.as_str() == group.event_id.as_str())
            .unwrap_or(false);
        let group = Arc::new(LocatedGroup { messages: group.messages, event_id: group.event_id, is_current });
        match events.iter_mut().find(|event| event.event_id == group.event_id) {
            Some(event) => {
                if is_current {
                    *event = Arc::new(LocatedEventRef { event_id: group.event_id.clone(), is_current: true });
                }
            }
            None => events.push(Arc::new(LocatedEventRef { event_id: group.event_id.clone(), is_current })),
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

/// 新消息的可判定文本
pub fn render_message(message: &LocateMessage) -> String {
    let self_mark = if message.is_self > 0 { " (本人)" } else { "" };
    format!("[{}] {}: {}{}", message.time, message.user_name, render_content(&message.content), self_mark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::Archiver;
    use crate::model::{LevelDecision, MessageGroup};
    use crate::records::{RecordBody, RecordEntry};
    use crate::scope::{ScopeHandle, ScopeId, ScopeManager};
    use crate::stub::StubModel;
    use kissbot_api::{ChannelRecord, Content, EventMode, RecordKind};

    fn record_entry(time: &str, sn: u64, text: &str) -> RecordEntry {
        RecordEntry {
            kind: RecordKind::Channel,
            time: Arc::new(time.to_string()),
            sn,
            body: RecordBody::Channel(Arc::new(ChannelRecord {
                user_id: Arc::new("u1".into()),
                self_user_id: Arc::new("self1".into()),
                messenger_id: Arc::new("web".into()),
                group_id: Arc::new("g1".into()),
                is_self: 0,
                msg_id: Arc::new(String::new()),
                messenger_name: Arc::new(String::new()),
                user_name: Arc::new("alice".into()),
                group_name: Arc::new(String::new()),
                content: Content::Text(Arc::new(text.to_string())),
                time: Arc::new(time.to_string()),
                sn,
            })),
        }
    }

    /// 某通道的记录（身份取通道三元组）
    fn channel_in(channel: &ChannelKey, time: &str, sn: u64, text: &str) -> RecordEntry {
        let mut entry = record_entry(time, sn, text);
        let RecordBody::Channel(record) = &mut entry.body else {
            unreachable!("通道记录")
        };
        let record = Arc::make_mut(record);
        record.messenger_id = channel.messenger_id.clone();
        record.self_user_id = channel.self_user_id.clone();
        record.group_id = channel.group_id.clone();
        entry
    }

    fn channel_key() -> ChannelKey {
        ChannelKey::new("web", "self1", "g1")
    }

    fn message(text: &str) -> LocateMessage {
        LocateMessage {
            time: Arc::new("2099-01-01 14:00:00".into()),
            user_name: Arc::new("alice".into()),
            is_self: 0,
            msg_id: Arc::new(String::new()),
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

    /// 归档一批记录（不带新消息 = 原来的归档）
    async fn archive(handle: &ScopeHandle, model: &Arc<StubModel>, channel: &ChannelKey, entries: &[RecordEntry]) {
        let archiver = Archiver::new(model.clone(), 4, 200);
        let mut state = handle.state.lock().await;
        archiver.archive(handle, &mut state, channel, entries, &[]).await.unwrap();
    }

    /// 定位一批消息：逐通道走归档入口，但只传消息（不落盘；新话题会建出记忆条数为 0 的事件）
    async fn locate(handle: &ScopeHandle, model: &Arc<StubModel>, messages: &[LocateMessage]) -> LocateResponse {
        let archiver = Archiver::new(model.clone(), 4, 200);
        let mut state = handle.state.lock().await;
        let mut located = Vec::new();
        for (channel, indices) in group_indices_by_channel(messages) {
            let channel_messages: Vec<&LocateMessage> = indices.iter().map(|index| &messages[*index]).collect();
            let result = archiver.archive(handle, &mut state, &channel, &[], &channel_messages).await.unwrap();
            for group in result.groups {
                let global: Vec<usize> = group.messages.iter().map(|local| indices[*local]).collect();
                let first = *global.first().expect("分组非空（parse_groups 保证）");
                located.push((
                    first,
                    channel.clone(),
                    LocatedGroup { messages: global, event_id: Arc::new(group.event_id), is_current: false },
                ));
            }
        }
        merge_located(located, &state.current_events)
    }

    fn event_ids(response: &LocateResponse) -> Vec<String> {
        response.events.iter().map(|event| event.event_id.as_str().to_string()).collect()
    }

    #[tokio::test]
    async fn empty_messages_returns_nothing_and_empty_tree_creates_event_without_model() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());

        // 没有消息：不建事件
        let response = locate(&handle, &model, &[]).await;
        assert!(response.events.is_empty());
        assert!(response.groups.is_empty());

        // 空树 + 消息：整批一组、直接建出事件（记忆条数 0）；没有事件可比对，不调逐层判定
        let response = locate(&handle, &model, &[message("你好"), message("在吗")]).await;
        assert_eq!(event_ids(&response), vec!["e1"]);
        assert_eq!(response.groups.len(), 1, "同一通道归一组");
        assert_eq!(response.groups[0].event_id.as_str(), "e1");
        assert_eq!(model.prompts().len(), 1, "只有第 1 轮分组，没有逐层判定");
        assert!(model.prompts()[0].contains("\"groups\""));
        let (tree_len, record_count) = {
            let state = handle.state.lock().await;
            (state.tree.len(), state.tree.get("e1").unwrap().record_count)
        };
        assert_eq!(tree_len, 1, "定位为新话题建出事件");
        assert_eq!(record_count, 0, "事件记忆条数为 0");
    }

    #[tokio::test]
    async fn current_event_group_hits_current_event() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")]).await;

        // 第 1 轮把整批标记为当前事件组 → 命中该通道当前事件
        model.push_group(&[0, 1], true);
        let response = locate(&handle, &model, &[message("今天下雨吗"), message("带伞吗")]).await;
        assert_eq!(event_ids(&response), vec!["e1"]);
        assert!(response.events[0].is_current);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(response.groups[0].event_id.as_str(), "e1");
    }

    #[tokio::test]
    async fn new_topic_group_creates_empty_event() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")]).await;

        // 组未标记当前事件 → 逐层判定顶层未命中 → 建出记忆条数为 0 的新事件
        model.push_group(&[0], false);
        model.push_level(LevelDecision::New);
        let response = locate(&handle, &model, &[message("换个话题")]).await;
        assert_eq!(event_ids(&response), vec!["e2"], "新话题也建出事件");
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].event_id.as_str(), "e2");
        assert!(response.events[0].is_current, "指针指向本批最后一个组的事件");
        let state = handle.state.lock().await;
        assert_eq!(state.tree.get("e2").unwrap().record_count, 0, "新事件记忆条数为 0");
        assert_eq!(state.tree.len(), 2);
    }

    #[tokio::test]
    async fn multiple_messages_map_to_multiple_events() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "事件一")]).await;
        // 第二批未标记当前事件 → 顶层归并新建 e2
        model.push_group(&[0], false);
        archive(&handle, &model, &channel_key(), &[record_entry("2099-01-01 11:00:00", 2, "事件二")]).await;
        {
            let state = handle.state.lock().await;
            assert_eq!(state.tree.len(), 2);
            assert_eq!(state.current_events.get(&channel_key()).map(|id| id.as_str()), Some("e2"));
        }

        // 4 条消息分成三组：前两条延续当前事件 e2；第三条命中已有事件 e1；第四条是新话题
        model.push_groups(vec![
            MessageGroup { messages: vec![0, 1], is_current: true },
            MessageGroup { messages: vec![2], is_current: false },
            MessageGroup { messages: vec![3], is_current: false },
        ]);
        model.push_level(LevelDecision::Merge("e1".into())); // 第三条：顶层命中 e1（叶子）
        model.push_level(LevelDecision::New); // 第四条：顶层未命中 → 新话题
        let response = locate(&handle, &model, &[message("a"), message("b"), message("c"), message("d")]).await;

        assert_eq!(event_ids(&response), vec!["e2", "e1", "e3"], "涉及的事件按首次出现顺序去重（新话题建出 e3）");
        let flags: Vec<bool> = response.events.iter().map(|event| event.is_current).collect();
        assert_eq!(flags, vec![false, false, true], "指针指向本批最后一个组的事件 e3");
        assert_eq!(response.groups.len(), 3);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(response.groups[1].event_id.as_str(), "e1");
        assert!(!response.groups[0].is_current);
        assert!(!response.groups[1].is_current);
        assert_eq!(response.groups[2].event_id.as_str(), "e3", "新话题组建出新事件");
    }

    #[tokio::test]
    async fn level_descent_picks_child_or_creates_child() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "父事件内容")]).await;
        // 构造 e1（父）/ e2（子）结构
        {
            let mut state = handle.state.lock().await;
            state.tree.create_node("e2", Some("e1"), EventMode::Role, "t");
        }

        // 第三条消息：顶层命中 e1（非叶子）→ 下探子层命中 e2
        model.push_group(&[0], false);
        model.push_level(LevelDecision::Merge("e1".into()));
        model.push_level(LevelDecision::Merge("e2".into()));
        let response = locate(&handle, &model, &[message("子事件相关")]).await;
        assert_eq!(event_ids(&response), vec!["e2"]);

        // 第四条消息：顶层命中 e1（非叶子）→ 子层都未命中 → 在 e1 下建出新的空事件
        model.push_group(&[0], false);
        model.push_level(LevelDecision::Merge("e1".into()));
        model.push_level(LevelDecision::New);
        let response = locate(&handle, &model, &[message("父事件相关")]).await;
        assert_eq!(event_ids(&response), vec!["e3"]);
        let state = handle.state.lock().await;
        let created = state.tree.get("e3").unwrap();
        assert_eq!(created.parent_id.as_ref().map(|p| p.as_str()), Some("e1"), "建在命中的父事件下");
        assert_eq!(created.record_count, 0, "新事件记忆条数为 0");
    }

    #[tokio::test]
    async fn messages_on_different_channels_use_their_own_current_event() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev")).await.unwrap();
        let model = Arc::new(StubModel::new());
        let other = ChannelKey::new("web", "self2", "g2");
        archive(&handle, &model, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "通道一")]).await;
        model.push_group(&[0], false);
        archive(&handle, &model, &other, &[channel_in(&other, "2099-01-01 11:00:00", 2, "通道二")]).await;
        let (current_first, current_other) = {
            let state = handle.state.lock().await;
            (
                state.current_events.get(&channel_key()).cloned().unwrap(),
                state.current_events.get(&other).cloned().unwrap(),
            )
        };
        assert_ne!(current_first, current_other);

        // 两个通道各一条消息，各自标记为当前事件组 → 各命中自己的当前事件
        model.push_group(&[0], true);
        model.push_group(&[0], true);
        let response = locate(&handle, &model, &[message("通道一问"), message_in(&other, "通道二问")]).await;
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
        archive(&handle, &model, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")]).await;
        // 删掉原文副本目录：定位不读原文，仍应命中
        tokio::fs::remove_dir_all(handle.events_dir()).await.unwrap();

        model.push_group(&[0], true);
        let response = locate(&handle, &model, &[message("今天下雨吗")]).await;
        assert_eq!(event_ids(&response), vec!["e1"]);
    }

    #[tokio::test]
    async fn event_mode_returns_single_event_without_model() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev-e1")).await.unwrap();
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "事件模式内容")]).await;

        let response = locate(&handle, &model, &[message("任意消息"), message("又一条")]).await;
        assert_eq!(event_ids(&response), vec!["e1"]);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert!(model.prompts().is_empty(), "事件模式不触发定位判定");
    }

    /// 事件模式 + 没有事件（也没有记录）：定位会建出作用域事件（记忆条数 0），不调模型
    #[tokio::test]
    async fn event_mode_without_event_creates_scope_event() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = manager.open(ScopeId::new("0", "dev-e1")).await.unwrap();
        let model = Arc::new(StubModel::new());

        let response = locate(&handle, &model, &[message("任意消息")]).await;
        assert_eq!(event_ids(&response), vec!["e1"]);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].event_id.as_str(), "e1");
        assert!(model.prompts().is_empty(), "事件模式不调模型");
        let state = handle.state.lock().await;
        assert_eq!(state.tree.get("e1").unwrap().record_count, 0, "事件记忆条数为 0");
    }

    #[test]
    fn render_message_marks_self() {
        let mut msg = message("你好");
        assert_eq!(render_message(&msg), "[2099-01-01 14:00:00] alice: 你好");
        msg.is_self = 1;
        assert!(render_message(&msg).ends_with("(本人)"));
    }
}
