# kissbot-channel-web 后端 API 集成测试

## 环境准备

### 1. 初始化 workspace

```bash
# 在项目根目录下执行
rm -rf test/workspace && mkdir -p test/workspace
cp -r test/workspace-template/* test/workspace/
```

### 2. 启动后端服务

```bash
cd ./test/workspace
cargo run --manifest-path ../../kissbot-channel-web/Cargo.toml
```

看到以下输出表示启动成功：
```
kissbot-channel-web HTTP server listening on 127.0.0.1:8301
INFO ws server start: WS Server listening on: 127.0.0.1:8201
```

### 3. 验证基本连接

```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json; d=json.load(sys.stdin); print('OK' if d['success'] else 'FAIL')"
```

预期输出：`OK`

---

## 测试用例

### TC-01：获取管理员信息

**前置**：服务已启动

```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info
```

**预期结果**：
- `success: true`
- `data.messenger_id` = `"web"`
- `data.admin_name` = `"管理员"`
- `data.users` 对象包含 `"user-1"` 和 `"user-2"`，各含 `user_id` 和 `user_name`
- `data.groups` 对象包含 `"dev-team"` 和 `"project-x"`，各含 `group_id`、`group_name`、`members`

### TC-02：错误 API Key

```bash
curl -s -H "X-Api-Key: wrong-key" http://127.0.0.1:8301/api/info
```

**预期结果**：返回非 2xx 状态码或 `success: false`

### TC-03：发送文本消息

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{
    "messenger_id": "web",
    "user_id": "admin",
    "group_id": "dev-team",
    "msg_type": "text",
    "content": {"Text": "你好！"}
  }' \
  http://127.0.0.1:8301/api/message/send
```

**预期结果**：
- `success: true`
- `data.msg_id` 非空
- `data.time` 为 ISO 时间格式
- `data.content` = `{"Text": "你好！"}`

### TC-04：发送消息到不存在的群组

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{
    "messenger_id": "web",
    "user_id": "admin",
    "group_id": "nonexistent",
    "msg_type": "text",
    "content": {"Text": "你好"}
  }' \
  http://127.0.0.1:8301/api/message/send
```

**预期结果**：`success: false`，`error` 非空

### TC-05：获取最近消息

**前置**：TC-03 已执行（消息存储有 3 秒缓冲延迟，查询前等待 4 秒）

```bash
sleep 4
curl -s -H "X-Api-Key: admin-key-123" \
  "http://127.0.0.1:8301/api/messages/recent?group_id=dev-team&n=5"
```

**预期结果**：
- `success: true`
- `data` 为非空数组
- 至少含一条消息，`msg_id` 与 TC-03 的返回一致

### TC-06：创建群组

```bash
RESP=$(curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{"group_name": "新群组", "member_ids": ["user-1"]}' \
  http://127.0.0.1:8301/api/groups/create)
echo "$RESP"
GROUP_ID=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['group_id'])")
echo "GROUP_ID=$GROUP_ID"
```

**预期结果**：
- `success: true`
- `data.group_id` 非空（首次创建为 `"g2"`）

记录 `GROUP_ID` 值用于后续测试。

### TC-07：创建群组后自动出现在会话列表

**前置**：TC-06 已执行，记录 `GROUP_ID`（如 `g2`）

```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json,os; d=json.load(sys.stdin); gid=os.environ['GROUP_ID']; g=d['data']['groups'][gid]; print(f'group_name={g[\"group_name\"]}, members={g[\"members\"]}')"
```

**预期结果**：
- `data.groups` 对象包含新增的 `group_id`（如 `"g2"`）
- 该 group 的 `group_name` = `"新群组"`
- `members` 包含 `"user-1"`

### TC-08：重命名群组

**前置**：TC-06 已执行，记录 `GROUP_ID`（如 `g2`）

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d "{\"group_id\": \"$GROUP_ID\", \"group_name\": \"重命名后的群组\"}" \
  http://127.0.0.1:8301/api/groups/rename
```

**预期结果**：`success: true`

验证：
```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json,os; d=json.load(sys.stdin); gid=os.environ['GROUP_ID']; print(d['data']['groups'][gid]['group_name'])"
```

预期输出：`重命名后的群组`

### TC-09：管理成员——添加成员

**前置**：TC-06 已执行，记录 `GROUP_ID`（如 `g2`）

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d "{\"group_id\": \"$GROUP_ID\", \"add_ids\": [\"user-2\"], \"remove_ids\": []}" \
  http://127.0.0.1:8301/api/groups/manage-members
```

**预期结果**：`success: true`

验证：
```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json,os; d=json.load(sys.stdin); gid=os.environ['GROUP_ID']; print(sorted(d['data']['groups'][gid]['members']))"
```

预期输出：`['user-1', 'user-2']`

### TC-10：管理成员——移除成员

**前置**：TC-09 已执行，记录 `GROUP_ID`（如 `g2`）

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d "{\"group_id\": \"$GROUP_ID\", \"add_ids\": [], \"remove_ids\": [\"user-2\"]}" \
  http://127.0.0.1:8301/api/groups/manage-members
```

**预期结果**：`success: true`

验证：
```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json,os; d=json.load(sys.stdin); gid=os.environ['GROUP_ID']; print(sorted(d['data']['groups'][gid]['members']))"
```

预期输出：`['user-1']`

### TC-11：创建用户

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{"user_name": "助手小C"}' \
  http://127.0.0.1:8301/api/users/create
```

**预期结果**：
- `success: true`
- `data.user_id` 非空（格式如 `"u3"`）

### TC-12：创建用户后自动生成单聊群组

**前置**：TC-11 已执行

```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json; d=json.load(sys.stdin); uid=d['data']['users']['u3']['user_id']; print([g for g in d['data']['groups'] if uid in d['data']['groups'][g]['members']])"
```

预期输出：列表中包含 `a_u3`（或对应的单聊群组 ID）

### TC-13：重命名用户

**前置**：TC-11 已执行

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{"user_id": "u3", "user_name": "助手小C（改）"}' \
  http://127.0.0.1:8301/api/users/rename
```

**预期结果**：`success: true`

验证：
```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['data']['users']['u3']['user_name'])"
```

预期输出：`助手小C（改）`

### TC-14：管理员改名

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{"admin_name": "超级管理员"}' \
  http://127.0.0.1:8301/api/admin/rename
```

**预期结果**：`success: true`

验证：
```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['data']['admin_name'])"
```

预期输出：`超级管理员`

### TC-15：删除群组

**前置**：TC-06 已执行，记录 `GROUP_ID`（如 `g2`）

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d "{\"group_id\": \"$GROUP_ID\"}" \
  http://127.0.0.1:8301/api/groups/delete
```

**预期结果**：`success: true`

验证：
```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json,os; d=json.load(sys.stdin); gid=os.environ['GROUP_ID']; print(gid in d['data']['groups'])"
```

预期输出：`False`

### TC-16：删除用户

**前置**：TC-11 已执行

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{"user_id": "u3"}' \
  http://127.0.0.1:8301/api/users/delete
```

**预期结果**：`success: true`

验证：
```bash
curl -s -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info | python3 -c "import sys,json; d=json.load(sys.stdin); print('u3' in d['data']['users'])"
```

预期输出：`False`

### TC-17：删除不存在的群组

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{"group_id": "nonexistent"}' \
  http://127.0.0.1:8301/api/groups/delete
```

**预期结果**：`success: false`

### TC-18：删除不存在的用户

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{"user_id": "nonexistent"}' \
  http://127.0.0.1:8301/api/users/delete
```

**预期结果**：`success: false`

### TC-19：admin-user 单聊群组不可操作

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{"group_id": "a_user-1", "group_name": "改名"}' \
  http://127.0.0.1:8301/api/groups/rename
```

**预期结果**：`success: false`

```bash
curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{"group_id": "a_user-1"}' \
  http://127.0.0.1:8301/api/groups/delete
```

**预期结果**：`success: false`

### TC-20：附件上传——发消息获取 transfer_id

```bash
RESP=$(curl -s -H "X-Api-Key: admin-key-123" -H "Content-Type: application/json" \
  -X POST -d '{
    "messenger_id": "web",
    "user_id": "admin",
    "group_id": "dev-team",
    "msg_type": "attachment",
    "content": {"AttachmentInfo": {"file_name": "photo.png", "mime_type": "image/png", "size_bytes": 4}}
  }' \
  http://127.0.0.1:8301/api/message/send)
echo "$RESP"
TRANSFER_ID=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['content']['AttachmentInfoResponse']['transfer_id'])")
ATT_KEY=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['content']['AttachmentInfoResponse']['key'])")
echo "TRANSFER_ID=$TRANSFER_ID, KEY=$ATT_KEY"
```

**预期结果**：
- `success: true`
- `data.content.AttachmentInfoResponse.key` 非空
- `data.content.AttachmentInfoResponse.transfer_id` 为数字

记录 `TRANSFER_ID` 和 `ATT_KEY` 值用于后续测试。

### TC-21：附件上传——上传文件数据

**前置**：TC-20 已执行，记录 `TRANSFER_ID`

```bash
echo "test" > /tmp/testfile.txt
curl -s -H "X-Api-Key: admin-key-123" \
  -F "transfer_id=$TRANSFER_ID" \
  -F "file=@/tmp/testfile.txt" \
  http://127.0.0.1:8301/api/attachment/upload
```

**预期结果**：`success: true`

### TC-22：附件下载

**前置**：TC-20 已执行，记录 `ATT_KEY`（格式如 `dev-team/uuid`）

```bash
curl -s -H "X-Api-Key: admin-key-123" \
  "http://127.0.0.1:8301/api/attachment/download?key=$ATT_KEY"
```

**预期结果**：返回文件内容（本例中应为 `test`）

### TC-23：附件缩略图（图片）

**前置**：TC-20 已执行，记录 `ATT_KEY`

```bash
curl -s -H "X-Api-Key: admin-key-123" -o /tmp/thumb.jpg \
  "http://127.0.0.1:8301/api/attachment/thumbnail?key=$ATT_KEY"
file /tmp/thumb.jpg
```

**预期结果**：返回 JPEG 图片

### TC-24：分页加载历史消息

**前置**：在 dev-team 群组中已有多条消息

```bash
# 先获取一条消息的 date 和 line 作为游标
curl -s -H "X-Api-Key: admin-key-123" \
  "http://127.0.0.1:8301/api/messages/recent?group_id=dev-team&n=1"
# 从返回中取 date 和 line，例如 date="2026-07-27", line=1

curl -s -H "X-Api-Key: admin-key-123" \
  "http://127.0.0.1:8301/api/messages/before?group_id=dev-team&date=2026-07-27&line=1&n=10"
```

**预期结果**：
- `success: true`
- 返回消息数组（可能为空，表示更早的消息）

---

## 清理

```bash
pkill -f kissbot-channel-web
rm -rf test/workspace
```
