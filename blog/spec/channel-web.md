# Channel Web

## 目标

- 同时实现 Web 通信工具及 Channel Messenger 适配层
- 支持管理员和多个 agent 的单聊、群聊

## 设计

- 只有一个管理员，user_id 固定为 admin，其他用户均称为 user，user_id 为 u<数字>
- 每个 user 自动生成和管理员的单聊，group_id 为 a_<user_id>
- Web 页面为管理员的界面，通过 SSE 接收 Channel Web 推送的消息
- user 均通过 WS 实现 Channel 接口接入
- Web 页面上管理员可以看到所有群组的消息，即使管理员不在群组中
- 附件保存到 Channel Web 本地，图片上传时自动生成缩略图，SSE 推送时显示缩略图，点击缩略图显示原图
