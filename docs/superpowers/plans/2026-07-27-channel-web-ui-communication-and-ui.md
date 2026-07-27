# channel-web-ui 前后端通信适配与 UI 对齐 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 kissbot-channel-web-ui 前端与 kissbot-channel-web 后端对接，修复全部 API 通信断裂，按最新设计稿调整 UI。

**Architecture:** Frontend (React + TypeScript + Vite) → HTTP API + SSE → Backend (Rust + Axum)。Vite 代理 `/api` → `http://127.0.0.1:8301`。认证通过 `X-Api-Key` header。

**Tech Stack:** React 18, TypeScript, Vite, Vitest, @microsoft/fetch-event-source

## Global Constraints

- 所有 API 路径以 `/api/` 开头
- 认证方式：`X-Api-Key` header（不是 query param）
- 后端 `/api/info` 返回的 `users`/`groups` 是对象（key-value map），前端需转数组
- SSE 推送原始 `IncomingMessage` JSON，无 `{type,data}` 包装
- 消息 `content` 是 `Content` 枚举 JSON（`{"Text":"..."}` / `{"AttachmentInfoResponse":{...}}` / `{"GroupChange":{...}}` 等）
- 附件上传两步流程：先发消息（Content::AttachmentInfo）→ 取 transfer_id → 上传文件数据
- 前端不构造 `Content::Multi` 消息
- 运行测试需启动后端和前端 dev server

---

### Task 1: 更新类型定义（types/index.ts）

**Files:**
- Modify: `kissbot-channel-web-ui/src/types/index.ts`

**Interfaces:**
- Consumes: 后端 `MessengerAdminInfo`、`UserConfig`、`GroupConfig`、`IncomingMessage`、`OutgoingMessage`、`Content` 枚举
- Produces: 供 `client.ts`、`App.tsx` 使用的完整类型

- [ ] **Step 1: 检查当前类型文件**

```bash
cat kissbot-channel-web-ui/src/types/index.ts
```

- [ ] **Step 2: 用后端数据契约替换所有类型**

删除旧类型，写入新类型定义：

```typescript
// 后端返回的管理员信息
export interface MessengerAdminInfo {
  messenger_id: string;
  admin_name: string;
  // 注意：后端返回的是 key-value 对象
  users: Record<string, UserConfig>;
  groups: Record<string, GroupConfig>;
}

export interface UserConfig {
  user_id: string;
  user_name: string;
}

export interface GroupConfig {
  group_id: string;
  group_name: string;
  members: string[];
}

// 消息类型
export interface IncomingMessage {
  msg_id: string;
  messenger_id: string;
  user_id: string;
  group_id: string;
  is_self: number; // 1=admin, 0=user
  msg_type: string;
  content: Content; // Content 枚举
  time: string;
}

// Content 枚举——serde untagged 格式
export type Content =
  | { Text: string }
  | { AttachmentInfoResponse: AttachmentInfoResponse }
  | { GroupChange: GroupChangeNotification }
  | { UserRemove: UserRemoveNotification }
  | { Multi: MessageItem[] };

export interface AttachmentInfo {
  file_name: string;
  mime_type: string;
  size_bytes: number;
}

export interface AttachmentInfoResponse {
  key: string;
  info: AttachmentInfo;
  transfer_id: number;
}

export interface GroupChangeNotification {
  messenger_id: string;
  group_id: string;
  user_id: string;
}

export interface UserRemoveNotification {
  messenger_id: string;
  user_id: string;
}

export interface MessageItem {
  msg_type: string;
  content: Content;
}

// 消息历史
export interface LineMessage {
  line: number;
  message: IncomingMessage;
}

export interface GroupedMessages {
  key: MsgKey;
  messages: LineMessage[];
}

export interface MsgKey {
  group_id: string;
  date: string;
}

// API 请求/响应
export interface ApiResponse<T> {
  success: boolean;
  data?: T;
  error?: string;
}

export interface OutgoingMessage {
  messenger_id: string;
  user_id: string;
  group_id: string;
  msg_type: string;
  content: Content;
}

export interface OutgoingMessageResponse {
  msg_id: string;
  time: string;
  msg_type: string;
  content: Content;
}

// 管理请求
export interface CreateGroupRequest {
  group_name: string;
  member_ids: string[];
}

export interface RenameGroupRequest {
  group_id: string;
  group_name: string;
}

export interface ManageMembersRequest {
  group_id: string;
  add_ids: string[];
  remove_ids: string[];
}

export interface DeleteGroupRequest {
  group_id: string;
}

export interface CreateUserRequest {
  user_name: string;
}

export interface RenameUserRequest {
  user_id: string;
  user_name: string;
}

export interface DeleteUserRequest {
  user_id: string;
}

export interface RenameAdminRequest {
  admin_name: string;
}

// 管理面板视图
export type AdminView = 'none' | 'groups' | 'users';

// 预置后端 URL
export interface BackendUrlOption {
  name: string;
  url: string;
}
```

- [ ] **Step 3: 运行 tsc 检查类型**

```bash
cd kissbot-channel-web-ui && npx tsc --noEmit
```

- [ ] **Step 4: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-channel-web-ui/src/types/index.ts && git commit -m "channel-web-ui types: 按后端数据契约重写类型定义"
```

---

### Task 2: 重写 API 客户端（api/client.ts）

**Files:**
- Modify: `kissbot-channel-web-ui/src/api/client.ts`
- Create: `kissbot-channel-web-ui/src/api/config.ts`

**Interfaces:**
- Consumes: `types/index.ts` 的类型
- Produces: `connect()`、`sendMessage()`、`uploadAttachment()`、管理类 API 等方法，供 `App.tsx` 调用

- [ ] **Step 1: 创建预置后端 URL 配置文件**

```typescript
// src/api/config.ts
import type { BackendUrlOption } from '../types';

export const DEFAULT_BACKEND_URLS: BackendUrlOption[] = [
  { name: '生产环境', url: 'https://api.kissbot.example.com' },
  { name: '测试环境', url: 'http://localhost:8301' },
  { name: '开发环境', url: 'http://192.168.1.100:8301' },
];
```

- [ ] **Step 2: 重写 client.ts**

用以下内容替换整个文件：

```typescript
import type {
  ApiResponse,
  MessengerAdminInfo,
  OutgoingMessage,
  OutgoingMessageResponse,
  GroupedMessages,
  Content,
  AttachmentInfo,
} from '../types';

// 动态后端 URL，登录时设置
let apiBase = '';
// API Key，登录时设置
let apiKey = '';

export function setApiKey(key: string) {
  apiKey = key;
}

export function getApiKey(): string {
  return apiKey;
}

export function setApiBase(url: string) {
  apiBase = url.replace(/\/+$/, '');
}

export function getApiBase(): string {
  return apiBase;
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<ApiResponse<T>> {
  const headers: Record<string, string> = {
    'X-Api-Key': apiKey,
  };
  if (body && !(body instanceof FormData)) {
    headers['Content-Type'] = 'application/json';
  }

  const url = `${apiBase}/api${path}`;
  const res = await fetch(url, {
    method,
    headers,
    body: body instanceof FormData ? body : body ? JSON.stringify(body) : undefined,
  });

  return res.json();
}

// ===== 连接 =====

/** 验证 API Key 并获取管理员和 messenger 信息 */
export async function connect(backendUrl: string, key: string): Promise<ApiResponse<MessengerAdminInfo>> {
  setApiBase(backendUrl);
  setApiKey(key);
  return request<MessengerAdminInfo>('GET', '/info');
}

// ===== 消息 =====

/** 发送消息（纯文本） */
export async function sendTextMessage(
  messengerId: string,
  groupId: string,
  text: string,
): Promise<ApiResponse<OutgoingMessageResponse>> {
  const msg: OutgoingMessage = {
    messenger_id: messengerId,
    user_id: 'admin',
    group_id: groupId,
    msg_type: 'text',
    content: { Text: text },
  };
  return request<OutgoingMessageResponse>('POST', '/message/send', msg);
}

/** 发送附件消息（注册附件，返回含 transfer_id） */
export async function sendAttachmentMessage(
  messengerId: string,
  groupId: string,
  info: AttachmentInfo,
): Promise<ApiResponse<OutgoingMessageResponse>> {
  const msg: OutgoingMessage = {
    messenger_id: messengerId,
    user_id: 'admin',
    group_id: groupId,
    msg_type: 'attachment',
    content: { AttachmentInfo: info },
  };
  return request<OutgoingMessageResponse>('POST', '/message/send', msg);
}

/** 上传附件数据（拿到 transfer_id 后调用） */
export async function uploadAttachmentData(
  transferId: number,
  file: File,
): Promise<ApiResponse<unknown>> {
  const formData = new FormData();
  formData.append('transfer_id', String(transferId));
  formData.append('file', file);

  const headers: Record<string, string> = {
    'X-Api-Key': apiKey,
  };
  const res = await fetch(`${apiBase}/api/attachment/upload`, {
    method: 'POST',
    headers,
    body: formData,
  });
  return res.json();
}

// ===== 消息历史 =====

/** 获取最近 N 条消息 */
export async function getMessagesRecent(
  groupId: string,
  n: number = 20,
): Promise<ApiResponse<GroupedMessages[]>> {
  return request<GroupedMessages[]>(
    'GET',
    `/messages/recent?group_id=${encodeURIComponent(groupId)}&n=${n}`,
  );
}

/** 获取指定位置之前的 N 条消息 */
export async function getMessagesBefore(
  groupId: string,
  date: string,
  line: number,
  n: number = 10,
): Promise<ApiResponse<GroupedMessages[]>> {
  return request<GroupedMessages[]>(
    'GET',
    `/messages/before?group_id=${encodeURIComponent(groupId)}&date=${encodeURIComponent(date)}&line=${line}&n=${n}`,
  );
}

// ===== 群组管理 =====

export async function createGroup(
  groupName: string,
  memberIds: string[],
): Promise<ApiResponse<{ group_id: string }>> {
  return request<{ group_id: string }>('POST', '/groups/create', {
    group_name: groupName,
    member_ids: memberIds,
  });
}

export async function renameGroup(
  groupId: string,
  groupName: string,
): Promise<ApiResponse<{ success: boolean }>> {
  return request<{ success: boolean }>('POST', '/groups/rename', {
    group_id: groupId,
    group_name: groupName,
  });
}

export async function manageMembers(
  groupId: string,
  addIds: string[],
  removeIds: string[],
): Promise<ApiResponse<{ success: boolean }>> {
  return request<{ success: boolean }>('POST', '/groups/manage-members', {
    group_id: groupId,
    add_ids: addIds,
    remove_ids: removeIds,
  });
}

export async function deleteGroup(
  groupId: string,
): Promise<ApiResponse<{ success: boolean }>> {
  return request<{ success: boolean }>('POST', '/groups/delete', {
    group_id: groupId,
  });
}

// ===== 用户管理 =====

export async function createUser(
  userName: string,
): Promise<ApiResponse<{ user_id: string }>> {
  return request<{ user_id: string }>('POST', '/users/create', {
    user_name: userName,
  });
}

export async function renameUser(
  userId: string,
  userName: string,
): Promise<ApiResponse<{ success: boolean }>> {
  return request<{ success: boolean }>('POST', '/users/rename', {
    user_id: userId,
    user_name: userName,
  });
}

export async function deleteUser(
  userId: string,
): Promise<ApiResponse<{ success: boolean }>> {
  return request<{ success: boolean }>('POST', '/users/delete', {
    user_id: userId,
  });
}

// ===== 管理员改名 =====

export async function renameAdmin(
  adminName: string,
): Promise<ApiResponse<{ success: boolean }>> {
  return request<{ success: boolean }>('POST', '/admin/rename', {
    admin_name: adminName,
  });
}

// ===== 附件 =====

export function getDownloadUrl(backendBase: string, key: string): string {
  return `${backendBase}/api/attachment/download?key=${encodeURIComponent(key)}`;
}

export function getThumbnailUrl(backendBase: string, key: string): string {
  return `${backendBase}/api/attachment/thumbnail?key=${encodeURIComponent(key)}`;
}
```

- [ ] **Step 3: 运行 tsc 检查**

```bash
cd kissbot-channel-web-ui && npx tsc --noEmit
```

- [ ] **Step 4: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-channel-web-ui/src/api/ && git commit -m "channel-web-ui api: 重写客户端，对齐后端 API 契约"
```

---

### Task 3: 更新 SSE 服务（api/sse.ts）

**Files:**
- Modify: `kissbot-channel-web-ui/src/api/sse.ts`

**Interfaces:**
- Consumes: `IncomingMessage` 类型
- Produces: 接收 SSE 事件 → 透传原始 `IncomingMessage`

- [ ] **Step 1: 重写 sse.ts**

后端推送原始 `IncomingMessage` JSON，前端直接透传（无 `{type,data}` 包装）：

```typescript
import { fetchEventSource } from '@microsoft/fetch-event-source';
import { getApiKey, getApiBase } from './client';
import type { IncomingMessage } from '../types';

type MessageCallback = (message: IncomingMessage) => void;

class SSEService {
  private callbacks: MessageCallback[] = [];
  private abortController: AbortController | null = null;
  private connected = false;

  onMessage(callback: MessageCallback) {
    this.callbacks.push(callback);
  }

  removeCallback(callback: MessageCallback) {
    this.callbacks = this.callbacks.filter(cb => cb !== callback);
  }

  async connect() {
    if (this.connected) return;
    this.disconnect(); // 清理旧连接

    const apiKey = getApiKey();
    const apiBase = getApiBase();
    if (!apiKey || !apiBase) return;

    this.abortController = new AbortController();
    this.connected = true;

    try {
      await fetchEventSource(`${apiBase}/api/events`, {
        method: 'GET',
        headers: { 'X-Api-Key': apiKey },
        signal: this.abortController.signal,
        onmessage: (event) => {
          try {
            // 后端直接推送 IncomingMessage JSON
            const message = JSON.parse(event.data) as IncomingMessage;
            this.callbacks.forEach(cb => cb(message));
          } catch {
            // 忽略解析失败的 event（如 keep-alive）
          }
        },
        onerror: () => {
          this.connected = false;
          // fetch-event-source 会自动重连
        },
        onclose: () => {
          this.connected = false;
        },
      });
    } catch {
      this.connected = false;
    }
  }

  disconnect() {
    if (this.abortController) {
      this.abortController.abort();
      this.abortController = null;
    }
    this.connected = false;
  }
}

export const sseService = new SSEService();
```

- [ ] **Step 2: tsc 检查**

```bash
cd kissbot-channel-web-ui && npx tsc --noEmit
```

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-channel-web-ui/src/api/sse.ts && git commit -m "channel-web-ui sse: 透传原始 IncomingMessage JSON"
```

---

### Task 4: 重写 CSS（index.css）

**Files:**
- Modify: `kissbot-channel-web-ui/src/index.css`

**Interfaces:**
- Produces: 全量样式，供 `App.tsx` 使用

- [ ] **Step 1: 将设计稿 style.css 适配到前端**

用设计稿 `docs/design/components-design/ui-ux-design/kissbot-channel-web/style.css` 的内容替换 `index.css`，保留 CSS 变量方案。注意设计稿是浅色主题，前端当前是深色主题——保留前端原有的深色主题 CSS 变量，只调整布局类名和结构。

- [ ] **Step 2: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-channel-web-ui/src/index.css && git commit -m "channel-web-ui css: 按设计稿重写样式"
```

---

### Task 5: 重构前端组件

**Files:**
- Create: `kissbot-channel-web-ui/src/components/LoginPage.tsx`
- Create: `kissbot-channel-web-ui/src/components/MainLayout.tsx`
- Create: `kissbot-channel-web-ui/src/components/Header.tsx`
- Create: `kissbot-channel-web-ui/src/components/Sidebar.tsx`
- Create: `kissbot-channel-web-ui/src/components/MessageArea.tsx`
- Create: `kissbot-channel-web-ui/src/components/MessageBubble.tsx`
- Create: `kissbot-channel-web-ui/src/components/AttachmentPreview.tsx`
- Create: `kissbot-channel-web-ui/src/components/ImageOverlay.tsx`
- Create: `kissbot-channel-web-ui/src/components/GroupManagement.tsx`
- Create: `kissbot-channel-web-ui/src/components/UserManagement.tsx`
- Create: `kissbot-channel-web-ui/src/hooks/useUnreadCounts.ts`
- Modify: `kissbot-channel-web-ui/src/App.tsx`（精简为状态切换）

**Interfaces:**
- Consumes: `types/index.ts` 类型、`api/client.ts` 方法、`api/sse.ts`、`api/config.ts` 预置 URL
- Produces: 完整 UI

按职责拆分，每个文件一个关注点。建议分步完成：

- [ ] **Step 5.1: 创建 types/index.ts（已在 Task 1 完成）**

- [ ] **Step 5.2: 创建 LoginPage.tsx**

```typescript
// 状态：connecting / apiKeyInput / selectedBackendUrl / connectError
// Props: onConnect(backendUrl, apiKey) → Promise<void>
// 渲染：
// - 应用名称 "Kissbot Web Chat" + "管理后台"副标题
// - 后端 URL 列表（从 config.ts 取，选中高亮，大字名称+小字URL）
// - Admin Key 密码输入框
// - 连接按钮
```

- [ ] **Step 5.3: 创建 Header.tsx**

```typescript
// Props: adminName, onRenameAdmin(name) → void, onNavigateAdmin(view) → void
// 渲染：
// - 左侧："Kissbot Web Chat"
// - 右侧：管理员名称 + "▼" 下拉菜单
// - 下拉项：重命名管理员 / 群组管理 / 用户管理
// - 重命名管理员：弹窗输入新名称 → onRenameAdmin(name)
```

- [ ] **Step 5.4: 创建 Sidebar.tsx**

```typescript
// Props: conversations（排序后的列表）, activeGroupId, onSelect(groupId), unreadCounts
// 每个 conversation 对象：
//   - group_id, display_name, is_admin_user_group, latest_time, unread_count
// 渲染：
// - 列表项：显示名称 + 右对齐未读数（>99 显示 "..."）
// - active 高亮
// - admin-user 单聊组显示用户名，多人群组显示群组名 + "群组"标记
```

- [ ] **Step 5.5: 创建 MessageBubble.tsx**

```typescript
// Props: message（IncomingMessage）
// 按 Content 枚举渲染：
// - Text → 直接文本
// - AttachmentInfoResponse(mime_type=image/*) → 缩略图，点击弹窗原图
// - AttachmentInfoResponse(非图片) → 文件链接
// - GroupChange → 居中 "XX 加入了群组"/"XX 离开了群组"
// - UserRemove → 居中 "XX 已被删除"
// - 其他 → 忽略不显示
```

- [ ] **Step 5.6: 创建 AttachmentPreview.tsx**

```typescript
// Props: files（File[]）, onRemove(index) → void
// 渲染：已选文件的名称列表，每项可移除
```

- [ ] **Step 5.7: 创建 ImageOverlay.tsx**

```typescript
// Props: src（string）, onClose() → void
// 渲染：全屏半透明背景 + 居中图片，点击关闭
```

- [ ] **Step 5.8: 创建 MessageArea.tsx**

```typescript
// Props:
//   - groupName, messages（按 date 分组的 GroupedMessages[]）
//   - loadingMore（是否正在加载更多）
//   - canSend（是否可发送，admin 是否在群组中）
//   - onSendText(text) → Promise<void>
//   - onSendAttachment(file) → Promise<void>
//   - onLoadMore() → void（滚动到顶部时调用）
// 渲染：
// - 顶部标题（groupName）
// - 消息列表（MessageBubble 渲染，is_self 靠左/靠右）
// - 附件预览（AttachmentPreview）
// - 输入区：文本输入 + 📎 + 发送按钮（canSend 为 false 时禁用）
```

- [ ] **Step 5.9: 创建 GroupManagement.tsx**

```typescript
// Props:
//   - groups, users
//   - onCreateGroup(name, memberIds) → void
//   - onRenameGroup(groupId, name) → void
//   - onDeleteGroup(groupId) → void
//   - onManageMembers(groupId, addIds, removeIds) → void
//   - onBack() → void
// 渲染：
// - 新建群组：输入名称 + 选成员 → onCreateGroup
// - 重命名群组：选群组 + 新名称 → onRenameGroup
// - 管理成员：选群组 + 选加/删成员 → onManageMembers
// - 群组列表：admin-user 单聊组禁用（显示 "仅可查看消息"），多人群组可删除
```

- [ ] **Step 5.10: 创建 UserManagement.tsx**

```typescript
// Props:
//   - users
//   - onCreateUser(userName) → void
//   - onRenameUser(userId, userName) → void
//   - onDeleteUser(userId) → void
//   - onBack() → void
// 渲染：
// - 新建用户：只输入用户名 → onCreateUser
// - 用户列表：每项有重命名（弹编辑框）和删除按钮
```

- [ ] **Step 5.11: 创建 useUnreadCounts.ts**

```typescript
// 管理未读计数 Map<string, number>
// - increment(groupId) → void
// - clear(groupId) → void
// - get(groupId) → number（格式化：>99 → "..."）
// 初始状态从 messages map 派生
```

- [ ] **Step 5.12: 创建 MainLayout.tsx**

```typescript
// Props:
//   - adminName, groups, users, messages, unreadCounts
//   - activeGroupId, onSelectGroup(groupId), adminView, onAdminView(view)
//   - onSendText, onSendAttachment, onLoadMore
//   - onCreateGroup, onRenameGroup, onDeleteGroup, onManageMembers
//   - onCreateUser, onRenameUser, onDeleteUser
//   - onRenameAdmin
// 渲染：
// - Header（adminName, 下拉菜单）
// - Sidebar（会话列表）
// - Main：adminView='none' → MessageArea，'groups' → GroupManagement，'users' → UserManagement
```

- [ ] **Step 5.13: 精简 App.tsx**

```typescript
// 状态：connected / adminName / messengerId / groups / users / messages / unreadCounts
// 渲染：
//   - 未连接 → LoginPage（onConnect 调用 connect()）
//   - 已连接 → MainLayout
// 管理：
//   - connect() → 解析 MessengerAdminInfo → 填充 groups/users 状态
//   - SSE 消息 → 按 msg_id 去重，存入 messages map，更新未读
//   - 所有管理 API 调用 → 成功后更新本地状态
```

- [ ] **Step 5.14: 运行 tsc 检查并启动 dev server 验证**

```bash
cd kissbot-channel-web-ui && npx tsc --noEmit && npm run dev
```

- [ ] **Step 5.15: 提交**

```bash
cd /home/admin/project/kissbot && git add kissbot-channel-web-ui/src/ && git commit -m "channel-web-ui: 按职责拆分组件，对接后端 API 和设计稿"
```

---

### Task 6: 前端单元测试

**Files:**
- Create: `kissbot-channel-web-ui/src/__tests__/client.test.ts`
- Create: `kissbot-channel-web-ui/src/__tests__/sse.test.ts`
- Create: `kissbot-channel-web-ui/src/__tests__/content.test.tsx`
- Create: `kissbot-channel-web-ui/src/__tests__/sidebar.test.ts`

- [ ] **Step 6.1: 安装 Vitest + React Testing Library**

```bash
cd kissbot-channel-web-ui && npm install -D vitest @testing-library/react @testing-library/jest-dom jsdom
```

package.json 添加 test script:
```json
"scripts": {
  "test": "vitest run",
  "test:watch": "vitest"
}
```

vite.config.ts 添加 test 配置:
```typescript
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: { '/api': { target: 'http://127.0.0.1:8301', changeOrigin: true } },
  },
  test: {
    environment: 'jsdom',
    globals: true,
  },
})
```

- [ ] **Step 6.2: 编写 client.test.ts**

测试 `sendTextMessage` 请求体格式、`connect` 调用、`sendAttachmentMessage` 的 Content 枚举格式等。

- [ ] **Step 6.3: 编写 sse.test.ts**

测试 SSE 事件流的原始 JSON 解析。

- [ ] **Step 6.4: 编写 content.test.tsx**

测试各 `Content` 变体的渲染逻辑（Text→文本、AttachmentInfoResponse图片→缩略图、GroupChange→系统消息等）。

- [ ] **Step 6.5: 编写 sidebar.test.ts**

测试会话列表排序、未读计数、admin-user 单聊组命名等。

- [ ] **Step 6.6: 运行全部测试并提交**

```bash
cd kissbot-channel-web-ui && npm test && cd /home/admin/project/kissbot && git add -A && git commit -m "channel-web-ui tests: 前端单元测试"
```

---

### Task 7: 集成测试验证

**Files:**
- Already created: `test/kissbot-channel-web-integration-test.md`

- [ ] **Step 7.1: 按集成测试文档（TC-01 ~ TC-24）逐条验证**

启动后端 + 前端 dev server，使用 agent-browser 技能打开浏览器模拟操作。

```bash
# 终端 1：启动后端
cd kissbot-channel-web && cargo run

# 终端 2：启动前端
cd kissbot-channel-web-ui && npm run dev
```

- [ ] **Step 7.2: 修复测试中发现的问题并提交**

```bash
git add -A && git commit -m "fix: 集成测试修复"
```
