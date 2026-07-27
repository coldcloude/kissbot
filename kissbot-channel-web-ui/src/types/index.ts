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

// Content 枚举——serde external tagging 格式
export type Content =
  | { Text: string }
  | { AttachmentInfo: AttachmentInfo }
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
