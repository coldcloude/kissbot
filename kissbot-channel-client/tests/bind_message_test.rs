mod mock;

use std::sync::{Arc, Weak};
use std::time::Duration;

use arcstr::ArcStr;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel::GroupChangeType;
use kissbot_channel_client::{ChannelClient, Terminal};
use mock::*;

#[tokio::test]
async fn test_bind_send_and_notify() {
    test_config_setup();
    let messenger = MockMessenger::new(make_messenger_info(ArcStr::from("m1"), ArcStr::from("u1"), ArcStr::from("g1")), b"download-not-used");
    let _manager = start_test_server(19101, messenger.clone()).await;

    let terminal = MockTerminal::new();
    let client = ChannelClient::new("m1".to_string(), Arc::downgrade(&terminal) as Weak<dyn Terminal>);
    client.clone().connect("ws://127.0.0.1:19101", "test-key").await.expect("connect failed");

    // 绑定
    client.bind(make_bind_request(ArcStr::from("m1"), ArcStr::from("u1"))).await.expect("bind failed");

    // messenger info 查询
    let info = client.get_info(ArcStr::from("m1")).await.expect("get_info failed");
    assert!(info.user_map.contains_key("u1"));

    // 发送文本消息 → mock messenger 收到
    let response = client.send_message(OutgoingMessage {
        messenger_id: ArcStr::from("m1"),
        user_id: ArcStr::from("u1"),
        group_id: ArcStr::from("g1"),
        content: Content::Text(ArcStr::from("hello")),
    }).await.expect("send_message failed");
    let sent = tokio::time::timeout(Duration::from_secs(2), messenger.sent_messages_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(sent.content, Content::Text(ArcStr::from("hello")));
    assert_eq!(response.content, Content::Text(ArcStr::from("hello")));

    // 上行消息 → terminal.incoming_message
    messenger.push_incoming(make_text_incoming(ArcStr::from("m1"), ArcStr::from("u1"), ArcStr::from("g1"), "hi"));
    let incoming = tokio::time::timeout(Duration::from_secs(2), terminal.incoming_rx().recv_async()).await.unwrap().unwrap();
    // 全链路透传：recipient_user_id 随 event 到达
    assert_eq!(incoming.recipient_user_id, "u1");
    assert_eq!(incoming.incoming_message.content, Content::Text(ArcStr::from("hi")));

    // 群组变化 join → terminal.join_group（同时会产生一条系统消息）
    messenger.push_group_change(GroupChangeType::Joined, ArcStr::from("u1"), ArcStr::from("g1"));
    let join = tokio::time::timeout(Duration::from_secs(2), terminal.joins_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(join.group_id, "g1");

    // 群组变化 leave → terminal.leave_group
    messenger.push_group_change(GroupChangeType::Left, ArcStr::from("u1"), ArcStr::from("g1"));
    let leave = tokio::time::timeout(Duration::from_secs(2), terminal.leaves_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(leave.group_id, "g1");

    // 用户删除 → terminal.user_removed
    messenger.push_user_remove(ArcStr::from("u1"));
    let removed = tokio::time::timeout(Duration::from_secs(2), terminal.removals_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(removed.user_id, "u1");

    // 重新绑定后解绑
    client.bind(make_bind_request(ArcStr::from("m1"), ArcStr::from("u1"))).await.expect("re-bind failed");
    client.unbind(make_bind_request(ArcStr::from("m1"), ArcStr::from("u1"))).await.expect("unbind failed");

    // 主动断开 → terminal.closed
    client.disconnect().await.expect("disconnect failed");
    tokio::time::timeout(Duration::from_secs(5), terminal.closed_rx().recv_async()).await.unwrap().unwrap();
}
