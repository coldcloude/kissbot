//! 定位结果汇总：归档器对「待归档消息 + 只用于定位的新消息」分组并逐层判定所属 case 后，
//! 本模块把各组合并成对外响应（[`merge_located`]：同一 case 的多组合并成一组）。
//!
//! 定位复用归档的判定：每个组都会命中已有 case 或建出 case（只含新消息的组也一样，
//! 新话题建出的 case 记忆条数为 0），见 [`crate::archive`]。

use std::{collections::{HashMap, hash_map::Entry}};

use arcstr::ArcStr;
use kissbot_api::{LocatedGroup, LocateResponse};

/// 汇总定位分组：同一 case 的多个组按消息 `msg_id` 的并集合并成一组；分组顺序不构成约定。
pub(crate) fn merge_located(groups: impl IntoIterator<Item = LocatedGroup>) -> LocateResponse {
    let mut merged: HashMap<ArcStr, LocatedGroup> = HashMap::new();
    for group in groups.into_iter() {
        match merged.entry(group.case_name.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().messages.extend(group.messages);
            }
            Entry::Vacant(entry) => {
                entry.insert(group);
            }
        }
    }
    LocateResponse {
        groups: merged.into_iter().map(|(_, group)| group).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use super::*;
    use crate::archive::Archiver;
    use crate::records::{RecordEntry, StoreMessage};
    use crate::agent_role::{AgentRole, AgentRoleHandle, AgentRoleManager};
    use crate::stub::StubModel;
    use kissbot_api::{ChannelKey, ChannelRecord, Content, LocateMessage, MemoryMeta, RoleEvent};

    fn role_event(role_name: &str, event_id: Option<&str>) -> RoleEvent {
        RoleEvent {
            role_name: ArcStr::from(role_name),
            event_id: event_id.map(ArcStr::from),
        }
    }

    fn record_entry(time: &str, sn: u64, text: &str) -> RecordEntry {
        channel_in(&channel_key(), time, sn, text)
    }

    /// 某通道的记录（身份取通道三元组）
    fn channel_in(channel: &ChannelKey, time: &str, sn: u64, text: &str) -> RecordEntry {
        Arc::new(ChannelRecord {
            meta: None,
            user_id: ArcStr::from("u1"),
            self_user_id: channel.self_user_id.clone(),
            messenger_id: channel.messenger_id.clone(),
            group_id: channel.group_id.clone(),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::from("alice"),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from(text)),
            time: ArcStr::from(time),
            sn,
        })
    }

    fn channel_key() -> ChannelKey {
        ChannelKey::new(ArcStr::from("web"), ArcStr::from("self1"), ArcStr::from("g1"))
    }

    /// 测试用定位消息：用文本充当 msg_id（分组结果按 msg_id 标识消息）
    fn message(text: &str) -> LocateMessage {
        LocateMessage {
            msg_id: ArcStr::from(text),
            time: ArcStr::from("2099-01-01 14:00:00"),
            user_id: ArcStr::from("u1"),
            user_name: ArcStr::from("alice"),
            is_self: 0,
            content: Content::Text(ArcStr::from(text)),
        }
    }

    /// 组内消息的 msg_id 集合
    fn ids(values: &[&str]) -> HashSet<ArcStr> {
        values.iter().map(|value| ArcStr::from(*value)).collect()
    }

    /// 按 case 名取分组（分组顺序不构成约定）
    fn group_of<'a>(response: &'a LocateResponse, case_name: &str) -> &'a LocatedGroup {
        response.groups.iter().find(|group| group.case_name.as_str() == case_name).expect("分组应存在")
    }

    /// 打开某记忆后缀所属的角色句柄（case 树按 AgentRole）
    async fn open(manager: &AgentRoleManager, role_event: &RoleEvent) -> Arc<AgentRoleHandle> {
        manager.open(AgentRole::new(ArcStr::from("0"), role_event.role_name.clone())).await.unwrap()
    }

    /// 归档一批记录（带 meta 直接落到指定 case；不带新消息 = 原来的归档）
    async fn archive(
        handle: &AgentRoleHandle,
        model: &Arc<StubModel>,
        role_event: &RoleEvent,
        channel: &ChannelKey,
        entries: &[RecordEntry],
        case_name: &str,
    ) {
        let pending: Vec<StoreMessage> = entries
            .iter()
            .map(|record| {
                let mut record = record.as_ref().clone();
                record.meta = Some(Arc::new(MemoryMeta::with_case_name(ArcStr::from(case_name))));
                let record = Arc::new(record);
                StoreMessage { channel: ChannelKey::from(record.as_ref()), record }
            })
            .collect();
        let archiver = Archiver::new(model.clone(), 200);
        let mut state = handle.state.lock().await;
        archiver.archive(handle, &mut state, role_event.event_id.clone(), channel, &pending, &[]).await.unwrap();
    }

    /// 定位一批消息：走归档入口但只传消息（不落盘；新话题会建出记忆条数为 0 的 case）
    async fn locate(
        handle: &AgentRoleHandle,
        model: &Arc<StubModel>,
        role_event: &RoleEvent,
        channel: &ChannelKey,
        messages: &[LocateMessage],
    ) -> LocateResponse {
        let archiver = Archiver::new(model.clone(), 200);
        let mut state = handle.state.lock().await;
        let channel_messages: Vec<&LocateMessage> = messages.iter().collect();
        let result =
            archiver.archive(handle, &mut state, role_event.event_id.clone(), channel, &[], &channel_messages).await.unwrap();
        let groups: Vec<LocatedGroup> = result
            .groups
            .into_iter()
            .filter(|group| !group.messages.is_empty())
            .map(|group| LocatedGroup { messages: group.messages.into_iter().collect(), case_name: group.case_name })
            .collect();
        merge_located(groups)
    }

    fn case_names(response: &LocateResponse) -> Vec<String> {
        response.groups.iter().map(|group| group.case_name.as_str().to_string()).collect()
    }

    #[tokio::test]
    async fn empty_messages_returns_nothing_and_empty_tree_creates_named_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());

        // 没有消息：不建 case
        let response = locate(&handle, &model, &role, &channel_key(), &[]).await;
        assert!(response.groups.is_empty());
        assert_eq!(model.prompts().len(), 0, "没有消息不调用模型");

        // 空树 + 消息：整批一组；当层没有候选也调一次逐层判定，由它给出新名称
        let response = locate(&handle, &model, &role, &channel_key(), &[message("你好"), message("在吗")]).await;
        assert_eq!(case_names(&response), vec!["case"]);
        assert_eq!(response.groups.len(), 1, "同一通道归一组");
        assert_eq!(response.groups[0].case_name.as_str(), "case");
        assert_eq!(model.prompts().len(), 2, "分组轮 + 一次逐层判定（当层为空也要问名称）");
        assert!(model.prompts()[0].contains("\"groups\""));
        assert!(model.prompts()[1].contains("当层 case"), "第二次是逐层判定");
        let (tree_len, record_count, summary) = {
            let state = handle.state.lock().await;
            let node = state.tree.get("case").unwrap();
            (state.tree.len(), node.record_count, node.summary.as_str().to_string())
        };
        assert_eq!(tree_len, 1, "定位为新话题建出 case");
        assert_eq!(record_count, 0, "case 记忆条数为 0");
        assert_eq!(summary, "", "定位阶段不生成摘要");
    }

    #[tokio::test]
    async fn current_case_group_hits_current_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")], "case").await;

        // 第 1 轮把整批标记为当前 case 组 → 命中该通道当前 case
        model.push_group(&[0, 1], true);
        let response = locate(&handle, &model, &role, &channel_key(), &[message("今天下雨吗"), message("带伞吗")]).await;
        assert_eq!(case_names(&response), vec!["case"]);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, ids(&["今天下雨吗", "带伞吗"]));
        assert_eq!(response.groups[0].case_name.as_str(), "case");
    }

    #[tokio::test]
    async fn new_topic_group_creates_empty_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")], "case").await;

        // 组未标记当前 case → 逐层判定给出新名称 → 建出记忆条数为 0 的新 case
        model.push_group(&[0], false);
        model.push_level("topic");
        let response = locate(&handle, &model, &role, &channel_key(), &[message("换个话题")]).await;
        assert_eq!(case_names(&response), vec!["topic"], "新话题也建出 case");
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].case_name.as_str(), "topic");
        let state = handle.state.lock().await;
        let created = state.tree.get("topic").unwrap();
        assert_eq!(created.record_count, 0, "新 case 记忆条数为 0");
        assert_eq!(created.summary.as_str(), "", "定位阶段不生成摘要");
        assert_eq!(state.tree.len(), 2);
    }

    #[tokio::test]
    async fn multiple_messages_map_to_multiple_cases() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "case 一")], "case").await;
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 11:00:00", 2, "case 二")], "case2").await;
        {
            let state = handle.state.lock().await;
            assert_eq!(state.tree.len(), 2);
            assert_eq!(state.current_case(&role, &channel_key()).as_deref(), Some("case2"));
        }

        // 4 条消息分成三组：前两条延续当前 case case2；第三条命中已有 case case；第四条是新话题
        model.push_groups(vec![
            (vec![0, 1], true),
            (vec![2], false),
            (vec![3], false),
        ]);
        model.push_level("case"); // 第三条：顶层命中已有 case（叶子）
        model.push_level("third"); // 第四条：顶层未命中 → 新话题
        let response = locate(&handle, &model, &role, &channel_key(), &[message("a"), message("b"), message("c"), message("d")]).await;

        assert_eq!(response.groups.len(), 3);
        let mut names = case_names(&response);
        names.sort_unstable();
        assert_eq!(names, vec!["case", "case2", "third"], "涉及三个 case（新话题建出 third）");
        assert_eq!(group_of(&response, "case2").messages, ids(&["a", "b"]));
        assert_eq!(group_of(&response, "case").messages, ids(&["c"]));
        assert_eq!(group_of(&response, "third").messages, ids(&["d"]), "新话题组建出新 case");
    }

    #[tokio::test]
    async fn level_descent_picks_child_or_creates_child() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "父 case 内容")], "case").await;
        // 构造 case（父）/ case/child（子）结构
        {
            let mut state = handle.state.lock().await;
            state.tree.create_role_case(Some("case"), "child", "t");
        }

        // 第 3 条消息：顶层命中 case（非叶子）→ 下探子层命中 case/child
        model.push_group(&[0], false);
        model.push_level("case");
        model.push_level("child");
        let response = locate(&handle, &model, &role, &channel_key(), &[message("子 case 相关")]).await;
        assert_eq!(case_names(&response), vec!["case/child"]);

        // 第 4 条消息：顶层命中 case（非叶子）→ 子层都未命中 → 在 case 下建出新的空 case
        model.push_group(&[0], false);
        model.push_level("case");
        model.push_level("new_child");
        let response = locate(&handle, &model, &role, &channel_key(), &[message("父 case 相关")]).await;
        assert_eq!(case_names(&response), vec!["case/new_child"]);
        let state = handle.state.lock().await;
        let created = state.tree.get("case/new_child").unwrap();
        assert_eq!(created.parent_id.as_ref().map(|p| p.as_str()), Some("case"), "建在命中的父 case 下");
        assert_eq!(created.record_count, 0, "新 case 记忆条数为 0");
        assert_eq!(created.summary.as_str(), "", "定位阶段不生成摘要");
    }

    #[tokio::test]
    async fn same_leaf_name_under_different_subtrees_are_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());
        {
            let mut state = handle.state.lock().await;
            state.tree.ensure_path("a/x", "t");
            state.tree.ensure_path("b/x", "t");
        }

        model.push_group(&[0], false);
        model.push_level("a");
        model.push_level("x");
        let response = locate(&handle, &model, &role, &channel_key(), &[message("甲")]).await;
        assert_eq!(case_names(&response), vec!["a/x"]);

        model.push_group(&[0], false);
        model.push_level("b");
        model.push_level("x");
        let response = locate(&handle, &model, &role, &channel_key(), &[message("乙")]).await;
        assert_eq!(case_names(&response), vec!["b/x"], "不同子树同名是不同 case");
    }

    #[tokio::test]
    async fn same_new_name_in_same_level_reuses_one_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());

        // 两组都给出同一个新名称：同父同名 → 同一个 case，返回前合并
        model.push_groups(vec![(vec![0], false), (vec![1], false)]);
        model.push_level("same");
        model.push_level("same");
        let response = locate(&handle, &model, &role, &channel_key(), &[message("一"), message("二")]).await;
        assert_eq!(case_names(&response), vec!["same"]);
        assert_eq!(response.groups[0].messages, ids(&["一", "二"]));
        let state = handle.state.lock().await;
        assert_eq!(state.tree.len(), 1, "同名兄弟只建一个节点");
    }

    #[tokio::test]
    async fn messages_on_different_channels_use_their_own_current_case() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());
        let other = ChannelKey::new(ArcStr::from("web"), ArcStr::from("self2"), ArcStr::from("g2"));
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "通道一")], "case").await;
        archive(&handle, &model, &role, &other, &[channel_in(&other, "2099-01-01 11:00:00", 2, "通道二")], "case2").await;
        let (current_first, current_other) = {
            let state = handle.state.lock().await;
            (
                state.current_case(&role, &channel_key()).unwrap(),
                state.current_case(&role, &other).unwrap(),
            )
        };
        assert_ne!(current_first, current_other);

        // 两个通道各定位一次（一次只针对一个通道），各自标记为当前 case 组 → 各命中自己的当前 case
        model.push_group(&[0], true);
        let response = locate(&handle, &model, &role, &channel_key(), &[message("通道一问")]).await;
        assert_eq!(case_names(&response), vec![current_first.as_str()]);
        assert_eq!(response.groups[0].messages, ids(&["通道一问"]));
        model.push_group(&[0], true);
        let response = locate(&handle, &model, &role, &other, &[message("通道二问")]).await;
        assert_eq!(case_names(&response), vec![current_other.as_str()]);
        assert_eq!(response.groups[0].messages, ids(&["通道二问"]));
    }

    /// 同一 case 的多个组在返回前合并：msg_id 取并集
    #[tokio::test]
    async fn groups_of_same_case_merge_into_one() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")], "case").await;

        // 两组都标记为当前 case 的延续，中间隔着一条别的话题 → 两组落点相同
        model.push_groups(vec![
            (vec![0], true),
            (vec![1], false),
            (vec![2], true),
        ]);
        model.push_level("other");
        let response = locate(
            &handle,
            &model,
            &role,
            &channel_key(),
            &[message("继续"), message("别的"), message("又继续")],
        )
        .await;

        assert_eq!(response.groups.len(), 2, "同 case 的两组在返回前合并成一组");
        assert_eq!(group_of(&response, "case").messages, ids(&["继续", "又继续"]), "合并后 msg_id 取并集");
        assert_eq!(group_of(&response, "other").messages, ids(&["别的"]));
    }

    #[tokio::test]
    async fn locate_does_not_read_records() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "聊天气")], "case").await;
        // 删掉原文副本目录：定位不读原文，仍应命中
        tokio::fs::remove_dir_all(handle.cases_dir()).await.unwrap();

        model.push_group(&[0], true);
        let response = locate(&handle, &model, &role, &channel_key(), &[message("今天下雨吗")]).await;
        assert_eq!(case_names(&response), vec!["case"]);
    }

    #[tokio::test]
    async fn event_mode_returns_single_case_without_model() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", Some("e1"))).await;
        let role = role_event("dev", Some("e1"));
        let model = Arc::new(StubModel::new());
        archive(&handle, &model, &role, &channel_key(), &[record_entry("2099-01-01 10:00:00", 1, "事件模式内容")], "[event]e1").await;

        let response = locate(&handle, &model, &role, &channel_key(), &[message("任意消息"), message("又一条")]).await;
        assert_eq!(case_names(&response), vec!["[event]e1"], "事件模式固定归入事件根");
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].messages, ids(&["任意消息", "又一条"]));
        assert!(model.prompts().is_empty(), "事件模式不触发定位判定");
        let state = handle.state.lock().await;
        let node = state.tree.get("[event]e1").unwrap();
        assert!(node.parent_id.is_none(), "事件根固定在顶层");
        assert!(node.is_leaf(), "事件根是顶层叶子");
        assert!(!node.summary.is_empty(), "有记录的 event case 由摘要重算生成摘要");
    }

    /// 事件模式 + 没有 case（也没有记录）：定位会建出事件根（记忆条数 0），不调模型
    #[tokio::test]
    async fn event_mode_without_case_creates_leaf() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", Some("e1"))).await;
        let role = role_event("dev", Some("e1"));
        let model = Arc::new(StubModel::new());

        let response = locate(&handle, &model, &role, &channel_key(), &[message("任意消息")]).await;
        assert_eq!(case_names(&response), vec!["[event]e1"]);
        assert_eq!(response.groups.len(), 1);
        assert_eq!(response.groups[0].case_name.as_str(), "[event]e1");
        assert!(model.prompts().is_empty(), "事件模式不调模型");
        {
            let state = handle.state.lock().await;
            let node = state.tree.get("[event]e1").unwrap();
            assert_eq!(node.record_count, 0, "case 记忆条数为 0");
            assert_eq!(node.summary.as_str(), "", "定位阶段不生成摘要");
        }
        assert!(model.summary_inputs().is_empty(), "定位阶段不调用摘要生成");
        assert!(!handle.case_dir("[event]e1").exists(), "没有记录就不落原文副本");
        // dev 与 dev-e1 是同一个角色：同一个句柄、同一棵 case 树
        let shared = open(&manager, &role_event("dev", None)).await;
        assert!(Arc::ptr_eq(&shared, &handle), "角色模式与事件模式共用同一个句柄");
        assert_eq!(shared.id, AgentRole::new(ArcStr::from("0"), ArcStr::from("dev")), "记忆后缀 dev-e1 的角色是 dev");
        assert!(shared.state.lock().await.tree.contains("[event]e1"), "case 在角色共享的 case 树上");
    }

    #[tokio::test]
    async fn new_case_name_allows_underscore() {
        let temp = tempfile::tempdir().unwrap();
        let manager = AgentRoleManager::new(temp.path());
        let handle = open(&manager, &role_event("dev", None)).await;
        let role = role_event("dev", None);
        let model = Arc::new(StubModel::new());

        model.push_group(&[0], false);
        model.push_level("topic_weather_chat");
        let response = locate(&handle, &model, &role, &channel_key(), &[message("天气")]).await;
        assert_eq!(case_names(&response), vec!["topic_weather_chat"]);
    }
}
