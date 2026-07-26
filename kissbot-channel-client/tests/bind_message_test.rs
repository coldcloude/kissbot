mod mock;

use std::sync::Arc;
use std::time::Duration;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel::GroupChangeType;
use kissbot_channel_client::ChannelClient;
use mock::*;

#[tokio::test]
async fn test_bind_send_and_notify() {
    test_config_setup();
    let messenger = MockMessenger::new(make_messenger_info("m1", "u1", "g1"), b"download-not-used");
    let _manager = start_test_server(19101, messenger.clone()).await;

    let terminal = MockTerminal::new();
    let client = ChannelClient::new();
    let terminal = client.connect("ws://127.0.0.1:19101", "test-key", MockTerminalCreator { terminal })
        .await.expect("connect failed");

    // 绑定
    terminal.bind_handler().bind(make_bind_request("m1", "u1")).await.expect("bind failed");

    // messenger info 查询
    let info = terminal.messenger_info_handler().get_info(Arc::new("m1".to_string())).await.expect("get_info failed");
    assert!(info.user_map.contains_key("u1"));

    // 发送文本消息 → mock messenger 收到
    let response = terminal.outgoing_message_handler().send_message(OutgoingMessage {
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
        group_id: Arc::new("g1".to_string()),
        msg_type: Arc::new(MSG_TYPE_TEXT.to_string()),
        content: Content::Text(Arc::new("hello".to_string())),
    }).await.expect("send_message failed");
    let sent = tokio::time::timeout(Duration::from_secs(2), messenger.sent_messages_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(sent.content, Content::Text(Arc::new("hello".to_string())));
    assert_eq!(response.content, Content::Text(Arc::new("hello".to_string())));

    // 上行消息 → terminal.incoming_message
    messenger.push_incoming(make_text_incoming("m1", "u1", "g1", "hi"));
    let incoming = tokio::time::timeout(Duration::from_secs(2), terminal.incoming_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(incoming.content, Content::Text(Arc::new("hi".to_string())));

    // 群组变化 join → terminal.join_group（同时会产生一条系统消息）
    messenger.push_group_change(GroupChangeType::Joined, "u1", "g1");
    let join = tokio::time::timeout(Duration::from_secs(2), terminal.joins_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(*join.group_id, "g1");

    // 群组变化 leave → terminal.leave_group
    messenger.push_group_change(GroupChangeType::Left, "u1", "g1");
    let leave = tokio::time::timeout(Duration::from_secs(2), terminal.leaves_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(*leave.group_id, "g1");

    // 用户删除 → terminal.user_removed
    messenger.push_user_remove("u1");
    let removed = tokio::time::timeout(Duration::from_secs(2), terminal.removals_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!(*removed.user_id, "u1");

    // 重新绑定后解绑
    terminal.bind_handler().bind(make_bind_request("m1", "u1")).await.expect("re-bind failed");
    terminal.bind_handler().unbind(make_bind_request("m1", "u1")).await.expect("unbind failed");

    // 主动断开 → terminal.closed
    client.disconnect().await.expect("disconnect failed");
    tokio::time::timeout(Duration::from_secs(5), terminal.closed_rx().recv_async()).await.unwrap().unwrap();
}
