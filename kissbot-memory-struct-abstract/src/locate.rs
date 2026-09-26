//! 定位结果汇总：归档器对「记录 + 只用于定位的新消息」分组并逐层判定所属 case 后，
//! 本模块把各通道的分组按请求下标合并成对外响应（[`merge_located`]），并提供按通道切分
//! 消息下标的 [`group_indices_by_channel`] 与新消息的文本渲染 [`render_message`]。
//!
//! 定位复用归档的判定：每个组都会命中已有 case 或建出 case（只含新消息的组也一样，
//! 新话题建出的 case 记忆条数为 0），见 [`crate::archive`]。

use std::collections::BTreeMap;
use std::sync::Arc;

use kissbot_api::{ChannelKey, LocateMessage, LocateResponse, LocatedCaseRef, LocatedGroup};

use crate::records::render_content;

/// 汇总各通道的定位分组：`is_current` 按该记忆后缀下各通道的最终当前 case 判定，
/// 分组按各组首条消息下标升序，case 按首次出现顺序去重。
pub(crate) fn merge_located(
    mut located: Vec<(usize, ChannelKey, LocatedGroup)>,
    current_cases: &BTreeMap<ChannelKey, String>,
) -> LocateResponse {
    located.sort_by_key(|(first, _, _)| *first);

    let mut groups: Vec<Arc<LocatedGroup>> = Vec::with_capacity(located.len());
    let mut cases: Vec<Arc<LocatedCaseRef>> = Vec::new();
    for (_, channel, group) in located {
        let is_current = current_cases
            .get(&channel)
            .map(|current| current.as_str() == group.case_name.as_str())
            .unwrap_or(false);
        let group = Arc::new(LocatedGroup { messages: group.messages, case_name: group.case_name, is_current });
        match cases.iter_mut().find(|case| case.case_name == group.case_name) {
            Some(case) => {
                if is_current {
                    *case = Arc::new(LocatedCaseRef { case_name: group.case_name.clone(), is_current: true });
                }
            }
            None => cases.push(Arc::new(LocatedCaseRef { case_name: group.case_name.clone(), is_current })),
        }
        groups.push(group);
    }
    LocateResponse { cases, groups }
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
    use crate::scope::{AgentRole, RoleEvent, ScopeHandle, ScopeManager};
    use crate::stub::StubModel;
    use kissbot_api::{ChannelRecord, Content, CaseMode, RecordKind};

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

    /// 打开某记忆后缀所属的角色句柄（角色树按角色共享）
    async fn open(manager: &ScopeManager, suffix: &str) -> Arc<ScopeHandle> {
        manager.open(RoleEvent::parse(suffix).agent_role("0")).await.unwrap()
    }

    /// 归档一批记录（不带新消息 = 原来的归档）
    async fn archive(
        handle: &ScopeHandle,
        model: &Arc<StubModel>,
        role_event: &RoleEvent,
        channel: &ChannelKey,
        entries: &[RecordEntry],
    ) {
        let archiver = Archiver::new(model.clone(), 200);
        let mut state = handle.state.lock().await;
        archiver.archive(handle, &mut state, role_event, channel, entries, &[]).await.unwrap();
    }

    /// 定位一批消息：逐通道走归档入口，但只传消息（不落盘；新话题会建出记忆条数为 0 的 case）
    async fn locate(handle: &ScopeHandle, model: &Arc<StubModel>, role_event: &RoleEvent, messages: &[LocateMessage]) -> LocateResponse {
        let archiver = Archiver::new(model.clone(), 200);
        let mut state = handle.state.lock().await;
        let mut located = Vec::new();
        for (channel, indices) in group_indices_by_channel(messages) {
            let channel_messages: Vec<&LocateMessage> = indices.iter().map(|index| &messages[*index]).collect();
            let result = archiver.archive(handle, &mut state, role_event, &channel, &[], &channel_messages).await.unwrap();
            for group in result.groups {
                let global: Vec<usize> = group.messages.iter().map(|local| indices[*local]).collect();
                let first = *global.first().expect("分组非空（parse_groups 保证）");
                located.push((
                    first,
                    channel.clone(),
                    LocatedGroup { messages: global, case_name: Arc::new(group.case_name), is_current: false },
                ));
            }
        }
        merge_located(located, &state.current_of(role_event))
    }

    fn case_names(response: &LocateResponse) -> Vec<String> {
        response.cases.iter().map(|case| case.case_name.as_str().to_string()).collect()
    }

    #[tokio::test]
    async fn empty_messages_returns_nothing_and_empty_tree_creates_named_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = open(&manager, "dev").await;
        let role = RoleEvent::parse("dev");
        let model = Arc::new(StubModel::new());

        // 没有消息：不建 case
        let response = locate(&handle, &model, &role, &[]).await;
        assert!(response.cases.is_empty());
        assert!(response.groups.is_empty());
        assert_eq!(model.prompts().len(), 0, "没有消息不调用模型");

        // 空树 + 消息：整批一组；当层没有候选也调一次逐层判定，由它给出名称与摘要
        let response = locate(&handle, &model, &role, &[message("你好"), message("在吗")]).await;
        assert_eq!(case_names(&response), vec!["role-case"]);
        assert_eq!(response.groups.len(), 1, "同一通道归一组");
        assert_eq!(response.groups[0].case_name.as_str(), "role-case");
        assert_eq!(model.prompts().len(), 2, "分组轮 + 一次逐层判定（当层为空也要问名称与摘要）");
        assert!(model.prompts()[0].contains("\"groups\""));
        assert!(model.prompts()[1].contains("当层 case"), "第二次是逐层判定");
        let (tree_len, record_count, summary) = {
            let state = handle.state.lock().await;
            let node = state.tree.get("role-case").unwrap();
            (state.tree.len(), node.record_count, node.summary.as_str().to_string())
        };
        assert_eq!(tree_len, 1, "定位为新话题建出 case");
        assert_eq!(record_count, 0, "case 记忆条数为 0");
        assert_eq!(summary, "新 case 摘要", "case 建立当次就带上模型给出的摘要");
    }

    #[tokio::test]
    async fn current_case_group_hits_current_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = open(&manager, "dev").await;
        let role = RoleEvent::parse("dev");
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")]).await;

        // 第 1 轮把整批标记为当前 case 组 → 命中该通道当前 case
        model.push_group(&[0, 1], true);
        let response = locate(&handle, &model, &role, &[message("今天下雨吗"), message("带伞吗")]).await;
        assert_eq!(case_names(&response), vec!["role-case"]);
        assert!(response.cases[0].is_current);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(response.groups[0].case_name.as_str(), "role-case");
    }

    #[tokio::test]
    async fn new_topic_group_creates_empty_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = open(&manager, "dev").await;
        let role = RoleEvent::parse("dev");
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")]).await;

        // 组未标记当前 case → 逐层判定给出名称与摘要 → 建出记忆条数为 0 的新 case
        model.push_group(&[0], false);
        model.push_level(LevelDecision::new_case("topic", "话题摘要"));
        let response = locate(&handle, &model, &role, &[message("换个话题")]).await;
        assert_eq!(case_names(&response), vec!["role-topic"], "新话题也建出 case");
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].case_name.as_str(), "role-topic");
        assert!(response.cases[0].is_current, "指针指向本批最后一个组的 case");
        let state = handle.state.lock().await;
        let created = state.tree.get("role-topic").unwrap();
        assert_eq!(created.record_count, 0, "新 case 记忆条数为 0");
        assert_eq!(created.summary.as_str(), "话题摘要", "新 case 摘要来自逐层判定");
        assert_eq!(state.tree.len(), 2);
    }

    #[tokio::test]
    async fn multiple_messages_map_to_multiple_cases() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = open(&manager, "dev").await;
        let role = RoleEvent::parse("dev");
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "case 一")]).await;
        // 第二批未标记当前 case → 顶层归并新建 case-2
        model.push_group(&[0], false);
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 11:00:00", 2, "case 二")]).await;
        {
            let state = handle.state.lock().await;
            assert_eq!(state.tree.len(), 2);
            assert_eq!(state.current_case(&role, &channel_key()).map(|id| id.as_str()), Some("role-case-2"));
        }

        // 4 条消息分成三组：前两条延续当前 case case-2；第三条命中已有 case case；第四条是新话题
        model.push_groups(vec![
            MessageGroup { messages: vec![0, 1], is_current: true },
            MessageGroup { messages: vec![2], is_current: false },
            MessageGroup { messages: vec![3], is_current: false },
        ]);
        model.push_level(LevelDecision::Merge("role-case".into())); // 第三条：顶层命中 case（叶子）
        model.push_level(LevelDecision::new_case("third", "第三摘要")); // 第四条：顶层未命中 → 新话题
        let response = locate(&handle, &model, &role, &[message("a"), message("b"), message("c"), message("d")]).await;

        assert_eq!(case_names(&response), vec!["role-case-2", "role-case", "role-third"], "涉及的 case 按首次出现顺序去重（新话题建出 third）");
        let flags: Vec<bool> = response.cases.iter().map(|event| event.is_current).collect();
        assert_eq!(flags, vec![false, false, true], "指针指向本批最后一个组的 case third");
        assert_eq!(response.groups.len(), 3);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert_eq!(response.groups[1].case_name.as_str(), "role-case");
        assert!(!response.groups[0].is_current);
        assert!(!response.groups[1].is_current);
        assert_eq!(response.groups[2].case_name.as_str(), "role-third", "新话题组建出新 case");
    }

    #[tokio::test]
    async fn level_descent_picks_child_or_creates_child() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = open(&manager, "dev").await;
        let role = RoleEvent::parse("dev");
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "父 case 内容")]).await;
        // 构造 case（父）/ role-2（子）结构
        {
            let mut state = handle.state.lock().await;
            state.tree.create_node("role-2", Some("role-case"), CaseMode::Role, "t");
        }

        // 第三条消息：顶层命中 case（非叶子）→ 下探子层命中 role-2
        model.push_group(&[0], false);
        model.push_level(LevelDecision::Merge("role-case".into()));
        model.push_level(LevelDecision::Merge("role-2".into()));
        let response = locate(&handle, &model, &role, &[message("子 case 相关")]).await;
        assert_eq!(case_names(&response), vec!["role-2"]);

        // 第四条消息：顶层命中 case（非叶子）→ 子层都未命中 → 在 case 下建出新的空 case
        model.push_group(&[0], false);
        model.push_level(LevelDecision::Merge("role-case".into()));
        model.push_level(LevelDecision::new_case("child", "子摘要"));
        let response = locate(&handle, &model, &role, &[message("父 case 相关")]).await;
        assert_eq!(case_names(&response), vec!["role-child"]);
        let state = handle.state.lock().await;
        let created = state.tree.get("role-child").unwrap();
        assert_eq!(created.parent_id.as_ref().map(|p| p.as_str()), Some("role-case"), "建在命中的父 case 下");
        assert_eq!(created.record_count, 0, "新 case 记忆条数为 0");
    }

    #[tokio::test]
    async fn messages_on_different_channels_use_their_own_current_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = open(&manager, "dev").await;
        let role = RoleEvent::parse("dev");
        let model = Arc::new(StubModel::new());
        let other = ChannelKey::new("web", "self2", "g2");
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "通道一")]).await;
        model.push_group(&[0], false);
        archive(&handle, &model, &role, &other, &[channel_in(&other, "2099-01-01 11:00:00", 2, "通道二")]).await;
        let (current_first, current_other) = {
            let state = handle.state.lock().await;
            (
                state.current_case(&role, &channel_key()).cloned().unwrap(),
                state.current_case(&role, &other).cloned().unwrap(),
            )
        };
        assert_ne!(current_first, current_other);

        // 两个通道各一条消息，各自标记为当前 case 组 → 各命中自己的当前 case
        model.push_group(&[0], true);
        model.push_group(&[0], true);
        let response = locate(&handle, &model, &role, &[message("通道一问"), message_in(&other, "通道二问")]).await;
        assert_eq!(case_names(&response), vec![current_first.as_str(), current_other.as_str()]);
        assert_eq!(response.groups[0].messages, vec![0]);
        assert_eq!(response.groups[1].messages, vec![1]);
        assert!(response.groups.iter().all(|group| group.is_current));
    }

    #[tokio::test]
    async fn locate_does_not_read_records() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = open(&manager, "dev").await;
        let role = RoleEvent::parse("dev");
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")]).await;
        // 删掉原文副本目录：定位不读原文，仍应命中
        tokio::fs::remove_dir_all(handle.cases_dir()).await.unwrap();

        model.push_group(&[0], true);
        let response = locate(&handle, &model, &role, &[message("今天下雨吗")]).await;
        assert_eq!(case_names(&response), vec!["role-case"]);
    }

    #[tokio::test]
    async fn case_mode_returns_single_case_without_model() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = open(&manager, "dev-e1").await;
        let role = RoleEvent::parse("dev-e1");
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "事件模式内容")]).await;

        let response = locate(&handle, &model, &role, &[message("任意消息"), message("又一条")]).await;
        assert_eq!(case_names(&response), vec!["event-e1"], "事件模式固定归入事件标识本身");
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, vec![0, 1]);
        assert!(model.prompts().is_empty(), "事件模式不触发定位判定");
        let state = handle.state.lock().await;
        let node = state.tree.get("event-e1").unwrap();
        assert!(node.parent_id.is_none(), "case 固定在顶层");
        assert!(node.is_leaf(), "case 是顶层叶子");
        assert_eq!(node.mode, CaseMode::Event);
        assert!(!node.summary.is_empty(), "有记录的 event case 由摘要重算生成摘要");
    }

    /// 事件模式 + 没有 case（也没有记录）：定位会建出角色 case（记忆条数 0），不调模型
    #[tokio::test]
    async fn case_mode_without_event_creates_scope_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScopeManager::new(temp.path());
        let handle = open(&manager, "dev-e1").await;
        let role = RoleEvent::parse("dev-e1");
        let model = Arc::new(StubModel::new());

        let response = locate(&handle, &model, &role, &[message("任意消息")]).await;
        assert_eq!(case_names(&response), vec!["event-e1"]);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].case_name.as_str(), "event-e1");
        assert!(model.prompts().is_empty(), "事件模式不调模型");
        {
            let state = handle.state.lock().await;
            let node = state.tree.get("event-e1").unwrap();
            assert_eq!(node.record_count, 0, "case 记忆条数为 0");
            assert!(!node.summary.is_empty(), "纯定位（只有消息）也当场生成摘要");
        }
        assert_eq!(model.summary_inputs().len(), 1, "纯定位由 summarize 生成摘要");
        assert!(!handle.case_dir("event-e1").exists(), "没有记录就不落原文副本");
        // dev 与 dev-e1 是同一个角色：同一个句柄、同一棵 case 树
        let shared = open(&manager, "dev").await;
        assert!(Arc::ptr_eq(&shared, &handle), "角色模式与事件模式共用同一个句柄");
        assert_eq!(shared.id, AgentRole::new("0", "dev"), "记忆后缀 dev-e1 的角色是 dev");
        assert!(shared.state.lock().await.tree.contains("event-e1"), "case 在角色共享的 case 树上");
    }

    #[test]
    fn render_message_marks_self() {
        let mut msg = message("你好");
        assert_eq!(render_message(&msg), "[2099-01-01 14:00:00] alice: 你好");
        msg.is_self = 1;
        assert!(render_message(&msg).ends_with("(本人)"));
    }
}
