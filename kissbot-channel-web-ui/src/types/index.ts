// API 类型定义

export interface User {
  user_id: string;
  user_name: string;
}

export interface Group {
  group_id: string;
  group_name: string;
  members: string[];
  is_admin_user_group: boolean;
}

export interface ConnectResponse {
  user_id: string;
  user_name: string;
  is_admin: boolean;
  messenger: {
    messenger_id: string;
    messenger_name: string;
    users: User[];
    groups: Group[];
  };
}

export interface MessageData {
  msg_id: string;
  group_id: string;
  user_id: string;
  is_self: number;
  msg_type: string;
  content: string;
  time: string;
}

export interface SSEEvent {
  type: 'message';
  data: MessageData;
}

export interface ApiResponse<T> {
  success: boolean;
  data?: T;
  error?: string;
}

export interface SendMessageRequest {
  group_id: string;
  content: string;
  attachments?: AttachmentRef[];
}

export interface AttachmentRef {
  filename: string;
  key: string;
}

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
  user_id: string;
  user_name: string;
}

export interface DeleteUserRequest {
  user_id: string;
}
