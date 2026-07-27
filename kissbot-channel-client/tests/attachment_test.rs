mod mock;

use std::sync::{Arc, Weak};
use std::time::Duration;
use bytes::Bytes;
use kissbot_api::channel::*;
use kissbot_api::message::*;
use kissbot_channel_client::{ChannelClient, Terminal};
use mock::*;

#[tokio::test]
async fn test_attachment_upload_download() {
    test_config_setup();
    let download_data = b"abcdefghij"; // 10 字节，mock 按 4 字节分块 → 3 块
    let messenger = MockMessenger::new(make_messenger_info("m1", "u1", "g1"), download_data);
    let _manager = start_test_server(19102, messenger.clone()).await;

    let terminal = MockTerminal::new();
    let client = ChannelClient::new("m1".to_string(), Arc::downgrade(&terminal) as Weak<dyn Terminal>);
    client.connect("ws://127.0.0.1:19102", "test-key").await.expect("connect failed");
    client.bind(make_bind_request("m1", "u1")).await.expect("bind failed");

    // ===== 上传 =====
    let upload_data = b"0123456789";
    let response = client.send_message(OutgoingMessage {
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
        group_id: Arc::new("g1".to_string()),
        msg_type: Arc::new(MSG_TYPE_ATTACHMENT.to_string()),
        content: Content::AttachmentInfo(Arc::new(AttachmentInfo {
            file_name: Arc::new("upload.bin".to_string()),
            mime_type: Arc::new("application/octet-stream".to_string()),
            size_bytes: upload_data.len() as u64,
        })),
    }).await.expect("send attachment message failed");

    // 响应 content 中取出 transfer_id
    let Content::AttachmentInfoResponse(att) = &response.content else {
        panic!("expected AttachmentInfoResponse, got {:?}", response.content);
    };
    assert_eq!(*att.key, "key-upload.bin");

    // 分两块上传
    let r1 = client.send_upload_chunk(att.transfer_id, 0, Bytes::copy_from_slice(&upload_data[..5]))
        .await.expect("upload chunk 1 failed");
    assert_eq!(r1.error_code, PAYLOAD_ERRCODE_OK);
    let r2 = client.send_upload_chunk(att.transfer_id, 5, Bytes::copy_from_slice(&upload_data[5..]))
        .await.expect("upload chunk 2 failed");
    assert_eq!(r2.error_code, PAYLOAD_ERRCODE_OK);

    // mock messenger 收到两块且数据正确
    let (tid1, pos1, data1) = tokio::time::timeout(Duration::from_secs(2), messenger.upload_chunks_rx().recv_async()).await.unwrap().unwrap();
    let (tid2, pos2, data2) = tokio::time::timeout(Duration::from_secs(2), messenger.upload_chunks_rx().recv_async()).await.unwrap().unwrap();
    assert_eq!((tid1, pos1, data1.as_ref()), (att.transfer_id, 0, &upload_data[..5]));
    assert_eq!((tid2, pos2, data2.as_ref()), (att.transfer_id, 5, &upload_data[5..]));

    // ===== 下载 =====
    let header = client.request_download(AttachmentDownloadRequest {
        messenger_id: Arc::new("m1".to_string()),
        user_id: Arc::new("u1".to_string()),
        group_id: Arc::new("g1".to_string()),
        key: Arc::new("download-key".to_string()),
    }).await.expect("request_download failed");
    assert_eq!(header.info.size_bytes, download_data.len() as u64);
    assert_eq!(*header.info.file_name, "download.bin");

    // 收 3 块并重组
    let mut received = Vec::new();
    // mock 按 4 字节分块，位置依次为 0, 4, 8
    for expect_pos in [0u64, 4, 8] {
        let (info, pos, data) = tokio::time::timeout(Duration::from_secs(2), terminal.chunks_rx().recv_async()).await.unwrap().unwrap();
        assert_eq!(pos, expect_pos);
        assert_eq!(info.transfer_id, header.transfer_id);
        received.extend_from_slice(&data);
    }
    assert_eq!(received, download_data);

    // 断开
    client.disconnect().await.expect("disconnect failed");
    tokio::time::timeout(Duration::from_secs(5), terminal.closed_rx().recv_async()).await.unwrap().unwrap();
}
