import type { ApiResponse, ConnectResponse, Group, User, SendMessageRequest } from '../types';

const API_BASE = '/api';

let apiKey: string = '';

export function setApiKey(key: string) {
  apiKey = key;
}

export function getApiKey(): string {
  return apiKey;
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

  const res = await fetch(`${API_BASE}${path}`, {
    method,
    headers,
    body: body instanceof FormData ? body : body ? JSON.stringify(body) : undefined,
  });

  return res.json();
}

// Connect
export async function connect(key: string): Promise<ApiResponse<ConnectResponse>> {
  setApiKey(key);
  return request<ConnectResponse>('GET', `/connect?api_key=${encodeURIComponent(key)}`);
}

// Messages
export async function sendMessage(req: SendMessageRequest): Promise<ApiResponse<{ msg_id: string; time: string }>> {
  return request('POST', '/message/send', req);
}

export async function getMessages(groupId: string, beforeId?: string, afterId?: string, time?: string): Promise<ApiResponse<unknown[]>> {
  let path = `/messages?group_id=${encodeURIComponent(groupId)}`;
  if (beforeId) path += `&before_id=${encodeURIComponent(beforeId)}`;
  if (afterId) path += `&after_id=${encodeURIComponent(afterId)}`;
  if (time) path += `&time=${encodeURIComponent(time)}`;
  return request('GET', path);
}

// Groups
export async function listGroups(): Promise<ApiResponse<Group[]>> {
  return request('GET', '/groups');
}

export async function createGroup(groupName: string, memberIds: string[]): Promise<ApiResponse<{ group_id: string; group_name: string }>> {
  return request('POST', '/groups/create', { group_name: groupName, member_ids: memberIds });
}

export async function renameGroup(groupId: string, groupName: string): Promise<ApiResponse<{ success: boolean }>> {
  return request('POST', '/groups/rename', { group_id: groupId, group_name: groupName });
}

export async function manageMembers(groupId: string, addIds: string[], removeIds: string[]): Promise<ApiResponse<{ success: boolean }>> {
  return request('POST', '/groups/manage-members', { group_id: groupId, add_ids: addIds, remove_ids: removeIds });
}

export async function deleteGroup(groupId: string): Promise<ApiResponse<{ success: boolean }>> {
  return request('POST', '/groups/delete', { group_id: groupId });
}

// Users
export async function listUsers(): Promise<ApiResponse<User[]>> {
  return request('GET', '/users');
}

export async function createUser(userId: string, userName: string): Promise<ApiResponse<{ user_id: string; user_name: string }>> {
  return request('POST', '/users/create', { user_id: userId, user_name: userName });
}

export async function deleteUser(userId: string): Promise<ApiResponse<{ success: boolean }>> {
  return request('POST', '/users/delete', { user_id: userId });
}

// Attachments
export async function uploadAttachment(file: File): Promise<ApiResponse<unknown>> {
  const formData = new FormData();
  formData.append('file', file);
  const headers: Record<string, string> = {
    'X-Api-Key': apiKey,
  };
  const res = await fetch(`${API_BASE}/attachment/upload`, {
    method: 'POST',
    headers,
    body: formData,
  });
  return res.json();
}

export function getDownloadUrl(key: string): string {
  return `${API_BASE}/attachment/download?key=${encodeURIComponent(key)}`;
}

export function getThumbnailUrl(key: string): string {
  return `${API_BASE}/attachment/thumbnail?key=${encodeURIComponent(key)}`;
}
