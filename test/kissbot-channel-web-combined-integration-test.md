# kissbot-channel-web + channel-client-cli 联合集成测试

## 环境准备

### 1. 初始化 workspace

```bash
cd /home/admin/project/kissbot
rm -rf test/workspace && mkdir -p test/workspace
cp -r test/workspace-template/* test/workspace/
```

### 2. 启动 web 后端

```bash
cd /home/admin/project/kissbot/test/workspace
../../kissbot-channel-web/target/debug/kissbot-channel-web
```

确认输出包含：
```
kissbot-channel-web HTTP server listening on 127.0.0.1:8301
INFO ws server start: WS Server listening on: 127.0.0.1:8201
```

### 3. 启动 client-cli

新开一个终端：

```bash
cd /home/admin/project/kissbot/test/workspace
../../kissbot-channel-client-cli/target/debug/kissbot-channel-client-cli web user-1 dev-team ./downloads
```

确认输出包含：
```
>> bound. 输入行发送文本；/group <id> 切换群组；/download <key>；/upload <path>
```

### 4. 启动前端 dev server

新开一个终端：

```bash
cd /home/admin/project/kissbot/kissbot-channel-web-ui
npm run dev
```

确认输出包含 `http://localhost:5173`。

### 5. 浏览器访问

使用 agent-browser 技能打开 `http://localhost:5173`。

---

## 测试用例

### TC-01：web 登录

**前置**：浏览器已打开 `http://localhost:5173`

**步骤**：
1. 点击 "测试环境" 选项
2. 输入 Admin Key `admin-key-123`
3. 点击 "连接"

**预期**：
- 成功进入聊天主界面
- 左侧会话列表显示 `助手小A`、`助手小B`、`开发组 群组`、`项目X 群组`

### TC-02：web → cli 发送文本消息

**前置**：TC-01 通过，cli 已启动并绑定

**步骤**：
1. 点击左侧 `开发组 群组`
2. 在底部输入框输入 `web-to-cli 测试消息`
3. 点击发送按钮

**预期**：
- web 端：消息区域显示消息，靠右（蓝色气泡）
- cli 端：终端输出 `<< [admin:dev-team] {"Text":"web-to-cli 测试消息"}`

### TC-03：cli → web 发送文本消息

**前置**：TC-01 通过，cli 已启动并绑定

**步骤**：
1. 在 cli 终端输入 `cli-to-web 测试消息`，按 Enter

**预期**：
- cli 端：终端输出 `>> sent msg_id=...`
- web 端：消息区域显示消息，靠左（灰色气泡），发送者为 `user-1`
- web 端：会话列表 `开发组 群组` 右对齐显示未读数

### TC-04：web → cli 发送图片附件

**前置**：TC-01 通过，cli 已启动并绑定

**步骤**：
1. web 端选中 `开发组 群组`
2. 点击 📎 附件按钮
3. 选择一张图片文件（如 PNG）
4. 点击发送按钮

**预期**：
- web 端：消息区域显示图片缩略图，靠右
- cli 端：终端输出 `<< [admin:dev-team] {"AttachmentInfoResponse":{...}}`，其中 `key` 和 `transfer_id` 非空

### TC-05：cli 下载 web 上传的图片

**前置**：TC-04 通过，记录 `key` 值

**步骤**：
1. 在 cli 终端输入 `/download <key>`（将 `<key>` 替换为 TC-04 返回的实际 key），按 Enter

**预期**：
- cli 端：终端输出 `>> downloading photo.png (N bytes)`
- cli 端：下载完成后输出 `>> downloaded to ./downloads/photo.png`
- 验证文件存在：`ls -la test/workspace/downloads/photo.png`

### TC-06：cli → web 发送图片附件

**前置**：TC-01 通过，cli 已启动并绑定

**步骤**：
1. 在 cli 终端输入 `/upload /tmp/test-image.png`（准备一张测试图片），按 Enter

**预期**：
- cli 端：终端输出 `>> uploaded test-image.png key=dev-team/uuid`
- web 端：消息区域显示图片缩略图，靠左
- web 端：会话列表 `开发组 群组` 未读数增加

### TC-07：web 查看 cli 上传的图片

**前置**：TC-06 通过

**步骤**：
1. 点击 web 端消息区域中的图片缩略图

**预期**：
- 弹窗显示原图（大图）
- 点击弹窗背景关闭

### TC-08：web 下载 cli 上传的图片

**前置**：TC-06 通过

**步骤**：
1. 在 cli 终端查看上传输出的 `key`（如 `dev-team/uuid`）
2. 在 web 端点击消息区域中的图片缩略图（同 TC-07，但这是验证下载而非查看）
3. 或者在另一个终端用 curl 下载：

```bash
curl -s -H "X-Api-Key: admin-key-123" -o /tmp/downloaded.png \
  "http://127.0.0.1:8301/api/attachment/download?key=dev-team/uuid"
file /tmp/downloaded.png
```

**预期**：
- 下载的文件与原始文件一致
- 文件类型为 image/png

### TC-09：web → cli 发送文件附件

**前置**：TC-01 通过，cli 已启动并绑定

**步骤**：
1. web 端选中 `开发组 群组`
2. 点击 📎 附件按钮
3. 选择一个非图片文件（如 TXT 或 PDF）
4. 点击发送按钮

**预期**：
- web 端：消息区域显示文件链接，靠右
- cli 端：终端输出 `<< [admin:dev-team] {"AttachmentInfoResponse":{...}}`

### TC-10：cli 下载 web 上传的文件

**前置**：TC-09 通过

**步骤**：
1. 在 cli 终端输入 `/download <key>`，按 Enter

**预期**：
- cli 端：下载完成，文件保存在 `test/workspace/downloads/` 目录

### TC-11：cli → web 发送文件附件

**前置**：TC-01 通过，cli 已启动并绑定

**步骤**：
1. 在 cli 终端输入 `/upload /tmp/test-file.txt`，按 Enter

**预期**：
- cli 端：终端输出 `>> uploaded test-file.txt key=dev-team/uuid`
- web 端：消息区域显示文件链接，靠左
- web 端：文件链接显示文件名 `test-file.txt`

### TC-12：web 下载 cli 上传的文件

**前置**：TC-11 通过

**步骤**：
1. 点击 web 端消息区域中的文件链接

**预期**：浏览器触发文件下载

### TC-13：web 发消息 → cli 收到 GroupChange 通知

**前置**：TC-01 通过，cli 已启动并绑定

**步骤**：
1. web 端选中 `开发组 群组`
2. 在输入框输入 `group-change 测试`
3. 点击发送

**预期**：
- web 端：消息显示在消息区域
- cli 端：终端输出 `<< [admin:dev-team] {"Text":"group-change 测试"}`

### TC-14：群组管理 → cli 收到 JoinGroup 通知

**前置**：TC-01 通过，cli 已启动并绑定到 dev-team

**步骤**：
1. web 端点击 `管理员 ▼` → `群组管理`
2. 在 "管理成员" 区域选择 `dev-team`
3. 在成员选择区点击添加 `user-2`
4. 点击 "添加成员"

**预期**：
- web 端：dev-team 成员列表更新
- cli 端（user-1）：终端输出 `<< join group: dev-team @ web`
  （注：user-1 原本已在 dev-team 中，此通知可能不触发，取决于实现）

### TC-15：web 端消息历史持久化

**前置**：TC-02 ~ TC-12 已执行

**步骤**：
1. 刷新浏览器页面（F5 或 Ctrl+R）
2. 重新登录（选择测试环境 → 输入 admin-key-123 → 连接）
3. 点击 `开发组 群组`

**预期**：
- 消息区域显示之前的所有消息（TC-02 到 TC-12 的消息都在）
- 消息顺序正确

### TC-16：web 端 SSE 断线重连

**前置**：TC-01 通过

**步骤**：
1. 保持浏览器页面打开
2. 停止 web 后端（`pkill -f kissbot-channel-web`）
3. 等待 5 秒
4. 重新启动 web 后端
5. 在 cli 终端输入 `reconnect 测试`，按 Enter

**预期**：
- 浏览器自动重连 SSE
- web 端显示 `reconnect 测试` 消息

---

## 清理

```bash
# 停止 cli（在 cli 终端按 Ctrl+C）
# 停止 web 后端
pkill -f kissbot-channel-web
# 停止前端 dev server（在前端终端按 Ctrl+C）
# 清理 workspace
rm -rf test/workspace
```
