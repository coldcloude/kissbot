import { useState, useEffect, useCallback, useRef } from 'react';
import type { UserConfig, GroupConfig, GroupedMessages, IncomingMessage, AdminView } from './types';
import * as api from './api/client';
import { sseService } from './api/sse';
import { useUnreadCounts } from './hooks/useUnreadCounts';
import LoginPage from './components/LoginPage';
import MainLayout from './components/MainLayout';

function entriesToArray<T>(obj: Record<string, T>): T[] {
  return Object.values(obj);
}

export default function App() {
  const [connected, setConnected] = useState(false);
  // connectError 在 handleConnect 中抛出，LoginPage 自己处理错误显示

  const [adminName, setAdminName] = useState('');
  const [messengerId, setMessengerId] = useState('');
  const [users, setUsers] = useState<UserConfig[]>([]);
  const [groups, setGroups] = useState<GroupConfig[]>([]);
  const [groupedMessagesMap, setGroupedMessagesMap] = useState<Record<string, GroupedMessages[]>>({});
  const [loadingMore, setLoadingMore] = useState(false);
  const [activeGroupId, setActiveGroupId] = useState<string | null>(null);
  const [adminView, setAdminView] = useState<AdminView>('none');

  // 游标追踪：{ groupId: { date, line } } 用于分页
  const cursorMapRef = useRef<Record<string, { date: string; line: number }>>({});
  // 所有已加载的 msg_id 集合用于去重
  const loadedMsgIdsRef = useRef<Set<string>>(new Set());

  const { unreadCounts, increment: incUnread, clear: clearUnread } = useUnreadCounts();

  // ===== 消息去重添加 =====
  const addMessages = useCallback((groupId: string, newGrouped: GroupedMessages[]) => {
    setGroupedMessagesMap(prev => {
      const existing = prev[groupId] || [];
      // 合并同 date 分组
      const merged = [...existing];
      for (const ng of newGrouped) {
        const existingGroupIdx = merged.findIndex(g => g.key.date === ng.key.date);
        if (existingGroupIdx >= 0) {
          // 按 line 去重合并
          const existingLines = new Set(merged[existingGroupIdx].messages.map(m => m.line));
          for (const lm of ng.messages) {
            if (!existingLines.has(lm.line)) {
              merged[existingGroupIdx].messages.push(lm);
            }
          }
          // 按 line 排序
          merged[existingGroupIdx].messages.sort((a, b) => a.line - b.line);
        } else {
          merged.push(ng);
        }
      }
      // 按 date 排序
      merged.sort((a, b) => a.key.date.localeCompare(b.key.date));
      return { ...prev, [groupId]: merged };
    });
  }, []);

  // ===== 连接 =====
  const handleConnect = async (backendUrl: string, key: string) => {
    const res = await api.connect(backendUrl, key);
    if (res.success && res.data) {
      const info = res.data;
      setAdminName(info.admin_name);
      setMessengerId(info.messenger_id);
      setUsers(entriesToArray(info.users));
      setGroups(entriesToArray(info.groups));
      setConnected(true);
      sseService.onMessage(handleSSEMessage);
      sseService.connect();
    } else {
      throw new Error(res.error || '连接失败');
    }
  };

  // ===== SSE 消息处理 =====
  const handleSSEMessage = useCallback((msg: IncomingMessage) => {
    if (loadedMsgIdsRef.current.has(msg.msg_id)) return;
    loadedMsgIdsRef.current.add(msg.msg_id);

    // 构建单条 GroupedMessages
    const date = msg.time.split('T')[0];
    const grouped: GroupedMessages = {
      key: { group_id: msg.group_id, date },
      messages: [{ line: 0, message: msg }],
    };
    addMessages(msg.group_id, [grouped]);

    // 更新未读（活跃群组不清零，非活跃群组增加）
    if (msg.group_id !== activeGroupId) {
      incUnread(msg.group_id);
    }
  }, [activeGroupId, addMessages, incUnread]);

  // ===== 发送文本消息 =====
  const handleSendText = async (groupId: string, text: string) => {
    const res = await api.sendTextMessage(messengerId, groupId, text);
    if (res.success && res.data) {
      // 本地添加已发送消息
      const msg: IncomingMessage = {
        msg_id: res.data.msg_id,
        messenger_id: messengerId,
        user_id: 'admin',
        group_id: groupId,
        is_self: 1,
        msg_type: 'text',
        content: { Text: text },
        time: res.data.time,
      };
      loadedMsgIdsRef.current.add(msg.msg_id);
      const date = msg.time.split('T')[0];
      addMessages(groupId, [{
        key: { group_id: groupId, date },
        messages: [{ line: 0, message: msg }],
      }]);
    }
  };

  // ===== 发送附件 =====
  const handleSendAttachment = async (groupId: string, file: File) => {
    // Step 1: send attachment message
    const res = await api.sendAttachmentMessage(messengerId, groupId, {
      file_name: file.name,
      mime_type: file.type,
      size_bytes: file.size,
    });
    if (!res.success || !res.data) return;

    // 从响应提取 transfer_id
    const content = res.data.content;
    let transferId = 0;
    if ('AttachmentInfoResponse' in content) {
      transferId = content.AttachmentInfoResponse.transfer_id;
    }

    // 本地添加已发送消息
    const msg: IncomingMessage = {
      msg_id: res.data.msg_id,
      messenger_id: messengerId,
      user_id: 'admin',
      group_id: groupId,
      is_self: 1,
      msg_type: 'attachment',
      content: res.data.content,
      time: res.data.time,
    };
    loadedMsgIdsRef.current.add(msg.msg_id);
    const date = msg.time.split('T')[0];
    addMessages(groupId, [{
      key: { group_id: groupId, date },
      messages: [{ line: 0, message: msg }],
    }]);

    // Step 2: upload file data
    if (transferId > 0) {
      await api.uploadAttachmentData(transferId, file);
    }
  };

  // ===== 加载历史消息 =====
  const loadMessages = useCallback(async (groupId: string, initial: boolean) => {
    if (initial) {
      setLoadingMore(false);
      const res = await api.getMessagesRecent(groupId, 20);
      if (res.success && res.data) {
        for (const g of res.data) {
          for (const lm of g.messages) {
            loadedMsgIdsRef.current.add(lm.message.msg_id);
          }
        }
        addMessages(groupId, res.data);
        // 记录游标
        const gms = res.data;
        if (gms.length > 0 && gms[0].messages.length > 0) {
          cursorMapRef.current[groupId] = {
            date: gms[0].key.date,
            line: gms[0].messages[0].line,
          };
        }
      }
    } else {
      // 加载更早消息
      const cursor = cursorMapRef.current[groupId];
      if (!cursor) return;
      setLoadingMore(true);
      const res = await api.getMessagesBefore(groupId, cursor.date, cursor.line, 10);
      if (res.success && res.data && res.data.length > 0) {
        for (const g of res.data) {
          for (const lm of g.messages) {
            loadedMsgIdsRef.current.add(lm.message.msg_id);
          }
        }
        addMessages(groupId, res.data);
        // 更新游标
        const firstGroup = res.data[0];
        if (firstGroup.messages.length > 0) {
          cursorMapRef.current[groupId] = {
            date: firstGroup.key.date,
            line: firstGroup.messages[0].line,
          };
        }
      }
      setLoadingMore(false);
    }
  }, [addMessages]);

  // ===== 选择群组 =====
  const handleSelectGroup = useCallback((groupId: string) => {
    setActiveGroupId(groupId);
    clearUnread(groupId);
    // 如果该群组尚无消息，首次加载
    if (!groupedMessagesMap[groupId] || groupedMessagesMap[groupId].length === 0) {
      loadMessages(groupId, true);
    }
  }, [groupedMessagesMap, loadMessages, clearUnread]);

  // ===== 加载更多（滚动到顶部） =====
  const handleLoadMore = useCallback(() => {
    if (!activeGroupId || loadingMore) return;
    loadMessages(activeGroupId, false);
  }, [activeGroupId, loadingMore, loadMessages]);

  // ===== 管理操作 =====

  const handleRenameAdmin = async (name: string) => {
    const res = await api.renameAdmin(name);
    if (res.success) setAdminName(name);
  };

  const handleCreateGroup = async (name: string, memberIds: string[]) => {
    const res = await api.createGroup(name, [...memberIds, 'admin']);
    if (res.success && res.data) {
      const newGroup: GroupConfig = {
        group_id: res.data.group_id,
        group_name: name,
        members: [...memberIds, 'admin'],
      };
      setGroups(prev => [...prev, newGroup]);
    }
  };

  const handleRenameGroup = async (groupId: string, name: string) => {
    const res = await api.renameGroup(groupId, name);
    if (res.success) {
      setGroups(prev => prev.map(g => g.group_id === groupId ? { ...g, group_name: name } : g));
    }
  };

  const handleDeleteGroup = async (groupId: string) => {
    const res = await api.deleteGroup(groupId);
    if (res.success) {
      setGroups(prev => prev.filter(g => g.group_id !== groupId));
      if (activeGroupId === groupId) setActiveGroupId(null);
    }
  };

  const handleManageMembers = async (groupId: string, addIds: string[], removeIds: string[]) => {
    await api.manageMembers(groupId, addIds, removeIds);
    // 管理成员成功后刷新本地 groups
    // 简单做法：直接从 /api/info 重新获取
  };

  const handleCreateUser = async (userName: string) => {
    const res = await api.createUser(userName);
    if (res.success && res.data) {
      const newUser: UserConfig = {
        user_id: res.data.user_id,
        user_name: userName,
      };
      setUsers(prev => [...prev, newUser]);
      // 添加对应的 admin-user 单聊组
      const gid = `a_${res.data.user_id}`;
      const newGroup: GroupConfig = {
        group_id: gid,
        group_name: userName,
        members: ['admin', res.data.user_id],
      };
      setGroups(prev => [...prev, newGroup]);
    }
  };

  const handleRenameUser = async (userId: string, userName: string) => {
    const res = await api.renameUser(userId, userName);
    if (res.success) {
      setUsers(prev => prev.map(u => u.user_id === userId ? { ...u, user_name: userName } : u));
      // 同步更新对应的单聊组名称
      setGroups(prev => prev.map(g =>
        g.group_id === `a_${userId}` ? { ...g, group_name: userName } : g
      ));
    }
  };

  const handleDeleteUser = async (userId: string) => {
    const res = await api.deleteUser(userId);
    if (res.success) {
      setUsers(prev => prev.filter(u => u.user_id !== userId));
      setGroups(prev => prev.filter(g => g.group_id !== `a_${userId}`));
      if (activeGroupId === `a_${userId}`) setActiveGroupId(null);
    }
  };

  // ===== 清理 =====
  useEffect(() => {
    return () => { sseService.disconnect(); };
  }, []);

  if (!connected) {
    return <LoginPage onConnect={handleConnect} />;
  }

  return (
    <MainLayout
      adminName={adminName}
      messengerId={messengerId}
      users={users}
      groups={groups}
      groupedMessagesMap={groupedMessagesMap}
      unreadCounts={unreadCounts}
      activeGroupId={activeGroupId}
      adminView={adminView}
      loadingMore={loadingMore}
      onSelectGroup={handleSelectGroup}
      onAdminView={setAdminView}
      onSendText={handleSendText}
      onSendAttachment={handleSendAttachment}
      onLoadMore={handleLoadMore}
      onRenameAdmin={handleRenameAdmin}
      onCreateGroup={handleCreateGroup}
      onRenameGroup={handleRenameGroup}
      onDeleteGroup={handleDeleteGroup}
      onManageMembers={handleManageMembers}
      onCreateUser={handleCreateUser}
      onRenameUser={handleRenameUser}
      onDeleteUser={handleDeleteUser}
    />
  );
}
