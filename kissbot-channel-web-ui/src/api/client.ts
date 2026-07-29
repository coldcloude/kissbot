import type {
  ApiResponse,
  MessengerAdminInfo,
  OutgoingMessage,
  OutgoingMessageResponse,
  GroupedMessages,
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
    content: { msg_type: 'Text', data: text },
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
    content: { msg_type: 'AttachmentInfo', data: info },
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
