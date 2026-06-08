import { useState, useEffect, useRef, useCallback } from 'react';
import type { MessageData, Group, User } from './types';
import * as api from './api/client';
import { sseService } from './api/sse';

// 解析消息内容中的附件 JSON
interface ParsedContent {
  text: string;
  attachments: Array<{
    filename: string;
    key: string;
    msg_type: string;
  }>;
}

function parseMessageContent(content: string): ParsedContent {
  try {
    const parsed = JSON.parse(content);
    if (parsed && typeof parsed === 'object' && 'attachments' in parsed) {
      return {
        text: parsed.text || '',
        attachments: parsed.attachments || [],
      };
    }
  } catch {}
  return { text: content, attachments: [] };
}

// 本地消息存储
interface LocalMessage extends MessageData {
  user_name: string;
}

// 管理面板类型
type AdminView = 'none' | 'groups' | 'users';

function App() {
  const [connected, setConnected] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [apiKeyInput, setApiKeyInput] = useState('');
  const [connectError, setConnectError] = useState('');

  const [userName, setUserName] = useState('');
  const [groups, setGroups] = useState<Group[]>([]);
  const [users, setUsers] = useState<User[]>([]);

  const [activeGroupId, setActiveGroupId] = useState<string | null>(null);
  const [messages, setMessages] = useState<Map<string, LocalMessage[]>>(new Map());
  const [messageInput, setMessageInput] = useState('');
  const [attachments, setAttachments] = useState<File[]>([]);

  const [adminView, setAdminView] = useState<AdminView>('none');
  const [thinking, setThinking] = useState(false);

  const messageListRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // 连接
  const handleConnect = async () => {
    if (!apiKeyInput.trim()) return;
    setConnecting(true);
    setConnectError('');

    try {
      const res = await api.connect(apiKeyInput.trim());
      if (res.success && res.data) {
        setConnected(true);
        setUserName(res.data.user_name);
        setGroups(res.data.messenger.groups);
        setUsers(res.data.messenger.users.filter(u => u.user_id !== res.data?.user_id));

        // 建立 SSE 连接
        sseService.onMessage(handleSSEMessage);
        sseService.connect();
      } else {
        setConnectError(res.error || '连接失败');
      }
    } catch (e) {
      setConnectError('网络错误');
    } finally {
      setConnecting(false);
    }
  };

  // SSE 消息处理
  const handleSSEMessage = useCallback((msg: MessageData) => {
    setMessages(prev => {
      const updated = new Map(prev);
      const existing = updated.get(msg.group_id) || [];
      // 去重
      if (!existing.find(m => m.msg_id === msg.msg_id)) {
        updated.set(msg.group_id, [...existing, { ...msg, user_name: '' }]);
      }
      return updated;
    });
    setThinking(false);
  }, []);

  // 发送消息
  const handleSendMessage = async () => {
    if (!messageInput.trim() && attachments.length === 0) return;
    if (!activeGroupId) return;

    const attachmentRefs = await Promise.all(
      attachments.map(async (file) => {
        const res = await api.uploadAttachment(file);
        if (res.success && Array.isArray(res.data)) {
          return {
            filename: file.name,
            key: res.data[0]?.key as string,
          };
        }
        return null;
      })
    );

    const validRefs = attachmentRefs.filter(Boolean) as { filename: string; key: string }[];

    setThinking(true);
    setAttachments([]);

    const res = await api.sendMessage({
      group_id: activeGroupId,
      content: messageInput,
      attachments: validRefs.length > 0 ? validRefs : undefined,
    });

    if (res.success) {
      // 添加本地消息
      const localMsg: LocalMessage = {
        msg_id: res.data!.msg_id,
        group_id: activeGroupId,
        user_id: '',
        is_self: 1,
        msg_type: attachments.length > 0 ? 'mixed' : 'text',
        content: messageInput,
        time: res.data!.time,
        user_name: userName,
      };
      setMessages(prev => {
        const updated = new Map(prev);
        const existing = updated.get(activeGroupId) || [];
        updated.set(activeGroupId, [...existing, localMsg]);
        return updated;
      });
      setMessageInput('');
    } else {
      setThinking(false);
    }
  };

  // 上传附件
  const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (files) {
      setAttachments(prev => [...prev, ...Array.from(files)]);
    }
  };

  const removeAttachment = (index: number) => {
    setAttachments(prev => prev.filter((_, i) => i !== index));
  };

  // 获取活跃群组的消息
  const activeMessages = activeGroupId ? messages.get(activeGroupId) || [] : [];
  const activeGroup = activeGroupId ? groups.find(g => g.group_id === activeGroupId) : null;
  const isJoinedGroup = activeGroup ? activeGroup.members.includes('admin') : false;

  // 自动滚动到底部
  useEffect(() => {
    if (messageListRef.current) {
      messageListRef.current.scrollTop = messageListRef.current.scrollHeight;
    }
  }, [activeMessages]);

  // 管理面板操作
  const handleCreateGroup = async (name: string, memberIds: string[]) => {
    const res = await api.createGroup(name, memberIds);
    if (res.success) {
      const newGroup: Group = {
        group_id: res.data!.group_id,
        group_name: res.data!.group_name,
        members: [...memberIds, 'admin'],
        is_admin_user_group: false,
      };
      setGroups(prev => [...prev, newGroup]);
    }
  };

  const handleRenameGroup = async (groupId: string, name: string) => {
    const res = await api.renameGroup(groupId, name);
    if (res.success) {
      setGroups(prev => prev.map(g =>
        g.group_id === groupId ? { ...g, group_name: name } : g
      ));
    }
  };

  const handleDeleteGroup = async (groupId: string) => {
    if (!window.confirm('确认删除该群组？')) return;
    const res = await api.deleteGroup(groupId);
    if (res.success) {
      setGroups(prev => prev.filter(g => g.group_id !== groupId));
      if (activeGroupId === groupId) setActiveGroupId(null);
    }
  };

  const handleCreateUser = async (userId: string, userName: string) => {
    const res = await api.createUser(userId, userName);
    if (res.success) {
      setUsers(prev => [...prev, { user_id: userId, user_name: userName }]);
      // 刷新群组列表
      const groupsRes = await api.listGroups();
      if (groupsRes.success && groupsRes.data) {
        setGroups(groupsRes.data);
      }
    }
  };

  const handleDeleteUser = async (userId: string) => {
    if (!window.confirm('确认删除该用户及其群组？')) return;
    const res = await api.deleteUser(userId);
    if (res.success) {
      setUsers(prev => prev.filter(u => u.user_id !== userId));
      const groupsRes = await api.listGroups();
      if (groupsRes.success && groupsRes.data) {
        setGroups(groupsRes.data);
      }
    }
  };

  const handleManageMembers = async (groupId: string, addIds: string[], removeIds: string[]) => {
    const res = await api.manageMembers(groupId, addIds, removeIds);
    if (res.success) {
      const groupsRes = await api.listGroups();
      if (groupsRes.success && groupsRes.data) {
        setGroups(groupsRes.data);
      }
    }
  };

  // 按键处理
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSendMessage();
    }
  };

  // 登录页面
  if (!connected) {
    return (
      <div className="login-page">
        <div className="login-card">
          <h1>Kissbot Web Chat</h1>
          <p>输入 API Key 连接</p>
          <input
            type="password"
            placeholder="API Key"
            value={apiKeyInput}
            onChange={e => setApiKeyInput(e.target.value)}
            onKeyDown={e => e.key === 'Enter' && handleConnect()}
          />
          <button onClick={handleConnect} disabled={connecting}>
            {connecting ? '连接中...' : '连接'}
          </button>
          {connectError && <div className="login-error">{connectError}</div>}
        </div>
      </div>
    );
  }

  // 按最后消息时间排序的群组列表
  const sortedGroups = [...groups].sort((a, b) => {
    const msgsA = messages.get(a.group_id) || [];
    const msgsB = messages.get(b.group_id) || [];
    if (msgsA.length === 0 && msgsB.length === 0) return 0;
    if (msgsA.length === 0) return 1;
    if (msgsB.length === 0) return -1;
    return msgsB[msgsB.length - 1].time.localeCompare(msgsA[msgsA.length - 1].time);
  });

  return (
    <div className="chat-layout">
      {/* 左侧边栏 */}
      <div className="sidebar">
        <div className="sidebar-header">
          <h2>消息</h2>
          <div className="user-info">
            <span className="user-name">{userName}</span>
          </div>
        </div>

        <div className="conversation-list">
          {sortedGroups.map(group => (
            <div
              key={group.group_id}
              className={`conversation-item ${activeGroupId === group.group_id ? 'active' : ''} ${!group.members.includes('admin') ? 'disabled' : ''}`}
              onClick={() => {
                setActiveGroupId(group.group_id);
                setAdminView('none');
              }}
            >
              <div className="conversation-name">
                {group.group_name}
                {group.is_admin_user_group ? '' : ' (群组)'}
              </div>
              <div className="conversation-badge">
                {messages.get(group.group_id)?.length || 0}
              </div>
            </div>
          ))}
        </div>

        <div className="sidebar-bottom">
          <button onClick={() => { setAdminView('groups'); setActiveGroupId(null); }}>
            ☰ 群组管理
          </button>
          <button onClick={() => { setAdminView('users'); setActiveGroupId(null); }}>
            ☰ 用户管理
          </button>
        </div>
      </div>

      {/* 右侧主区域 */}
      <div className="main-content">
        {adminView !== 'none' ? (
          adminView === 'groups' ? (
            <GroupManagementPanel
              groups={groups}
              users={users}
              onCreateGroup={handleCreateGroup}
              onRenameGroup={handleRenameGroup}
              onDeleteGroup={handleDeleteGroup}
              onManageMembers={handleManageMembers}
              onBack={() => setAdminView('none')}
            />
          ) : (
            <UserManagementPanel
              users={users}
              onCreateUser={handleCreateUser}
              onDeleteUser={handleDeleteUser}
              onBack={() => setAdminView('none')}
            />
          )
        ) : activeGroup ? (
          <>
            <div className="chat-header">
              <h3>{activeGroup.group_name}</h3>
              <div className="chat-header-meta">
                {thinking && <span className="thinking-indicator">思考中...</span>}
              </div>
            </div>

            <div className="message-list" ref={messageListRef}>
              {activeMessages.map(msg => {
                const parsed = parseMessageContent(msg.content);
                return (
                  <div key={msg.msg_id} className={`message ${msg.is_self === 1 ? 'self' : 'other'}`}>
                    <div className="message-bubble">
                      <div className="message-content">
                        {msg.msg_type === 'system_join' && (
                          <span style={{ color: 'var(--success)', fontSize: '13px' }}>用户加入了群组</span>
                        )}
                        {msg.msg_type === 'system_leave' && (
                          <span style={{ color: 'var(--danger)', fontSize: '13px' }}>用户离开了群组</span>
                        )}
                        {msg.msg_type !== 'system_join' && msg.msg_type !== 'system_leave' && (
                          <>
                            <div className="text-content">{parsed.text}</div>
                            {parsed.attachments.map((att, i) => (
                              <div key={i}>
                                {att.msg_type === 'image' ? (
                                  <img
                                    className="image-attachment"
                                    src={api.getThumbnailUrl(att.key)}
                                    alt={att.filename}
                                    onClick={() => window.open(api.getDownloadUrl(att.key), '_blank')}
                                  />
                                ) : (
                                  <a className="file-attachment" href={api.getDownloadUrl(att.key)} target="_blank" rel="noopener noreferrer">
                                    📎 {att.filename}
                                  </a>
                                )}
                              </div>
                            ))}
                          </>
                        )}
                      </div>
                    </div>
                    <span className="message-time">{formatTime(msg.time)}</span>
                  </div>
                );
              })}
            </div>

            {/* 附件预览 */}
            {attachments.length > 0 && (
              <div className="attachment-preview">
                {attachments.map((file, i) => (
                  <div key={i} className="attachment-preview-item">
                    <span>📎 {file.name}</span>
                    <button onClick={() => removeAttachment(i)}>×</button>
                  </div>
                ))}
              </div>
            )}

            {/* 输入区域 */}
            <div className={`input-area ${!isJoinedGroup ? 'disabled' : ''}`}>
              <div className="input-wrapper">
                <input
                  type="text"
                  placeholder={isJoinedGroup ? '输入消息...' : '你未加入该群组，无法发送消息'}
                  value={messageInput}
                  onChange={e => setMessageInput(e.target.value)}
                  onKeyDown={handleKeyDown}
                  disabled={!isJoinedGroup}
                />
                <div className="input-actions">
                  <button
                    title="上传附件"
                    onClick={() => fileInputRef.current?.click()}
                    disabled={!isJoinedGroup}
                  >
                    📎
                  </button>
                </div>
              </div>
              <button
                className="send-button"
                onClick={handleSendMessage}
                disabled={!isJoinedGroup || (!messageInput.trim() && attachments.length === 0)}
              >
                发送
              </button>
              <input
                type="file"
                ref={fileInputRef}
                style={{ display: 'none' }}
                multiple
                onChange={handleFileSelect}
              />
            </div>
          </>
        ) : (
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '100%', color: 'var(--text-secondary)' }}>
            选择一个会话
          </div>
        )}
      </div>
    </div>
  );
}

// 格式化时间
function formatTime(timeStr: string): string {
  try {
    const d = new Date(timeStr);
    const now = new Date();
    const isToday = d.toDateString() === now.toDateString();
    if (isToday) {
      return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
    }
    return d.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
  } catch {
    return timeStr;
  }
}

// ===== 群组管理面板 =====
interface GroupManagementProps {
  groups: Group[];
  users: User[];
  onCreateGroup: (name: string, memberIds: string[]) => void;
  onRenameGroup: (groupId: string, name: string) => void;
  onDeleteGroup: (groupId: string) => void;
  onManageMembers: (groupId: string, addIds: string[], removeIds: string[]) => void;
  onBack: () => void;
}

function GroupManagementPanel({ groups, users, onCreateGroup, onRenameGroup, onDeleteGroup, onManageMembers, onBack }: GroupManagementProps) {
  const [newGroupName, setNewGroupName] = useState('');
  const [newGroupMembers, setNewGroupMembers] = useState<string[]>([]);
  const [renameId, setRenameId] = useState('');
  const [renameName, setRenameName] = useState('');
  const [manageGroupId, setManageGroupId] = useState('');
  const [addMemberIds, setAddMemberIds] = useState<string[]>([]);

  const handleCreate = () => {
    if (!newGroupName.trim()) return;
    onCreateGroup(newGroupName.trim(), newGroupMembers);
    setNewGroupName('');
    setNewGroupMembers([]);
  };

  const handleRename = () => {
    if (!renameId || !renameName.trim()) return;
    onRenameGroup(renameId, renameName.trim());
    setRenameId('');
    setRenameName('');
  };

  const handleAddMembers = () => {
    if (!manageGroupId) return;
    onManageMembers(manageGroupId, addMemberIds, []);
    setAddMemberIds([]);
  };

  const toggleMember = (id: string, list: string[], setList: (ids: string[]) => void) => {
    if (list.includes(id)) {
      setList(list.filter(m => m !== id));
    } else {
      setList([...list, id]);
    }
  };

  const manageableGroups = groups.filter(g => !g.is_admin_user_group);

  return (
    <div className="admin-panel">
      <h3>
        <button onClick={onBack} style={{ background: 'none', border: 'none', color: 'var(--accent)', cursor: 'pointer', marginRight: 8, fontSize: 16 }}>
          ←
        </button>
        群组管理
      </h3>

      {/* 新建群组 */}
      <div className="admin-panel-section">
        <h4>新建群组</h4>
        <input
          placeholder="群组名称"
          value={newGroupName}
          onChange={e => setNewGroupName(e.target.value)}
        />
        <div className="member-select">
          {users.map(u => (
            <div
              key={u.user_id}
              className={`member-tag ${newGroupMembers.includes(u.user_id) ? 'selected' : ''}`}
              onClick={() => toggleMember(u.user_id, newGroupMembers, setNewGroupMembers)}
            >
              {u.user_name}
            </div>
          ))}
        </div>
        <button onClick={handleCreate}>创建</button>
      </div>

      {/* 重命名群组 */}
      <div className="admin-panel-section">
        <h4>重命名群组</h4>
        <select
          value={renameId}
          onChange={e => {
            setRenameId(e.target.value);
            const group = groups.find(g => g.group_id === e.target.value);
            setRenameName(group?.group_name || '');
          }}
          style={{
            width: '100%', padding: '8px 12px', marginBottom: 8,
            border: '1px solid var(--border)', borderRadius: 6,
            background: 'var(--bg-primary)', color: 'var(--text-primary)',
            fontSize: 14,
          }}
        >
          <option value="">选择群组</option>
          {manageableGroups.map(g => (
            <option key={g.group_id} value={g.group_id}>{g.group_name}</option>
          ))}
        </select>
        <input
          placeholder="新名称"
          value={renameName}
          onChange={e => setRenameName(e.target.value)}
        />
        <button onClick={handleRename}>重命名</button>
      </div>

      {/* 管理成员 */}
      <div className="admin-panel-section">
        <h4>添加成员</h4>
        <select
          value={manageGroupId}
          onChange={e => setManageGroupId(e.target.value)}
          style={{
            width: '100%', padding: '8px 12px', marginBottom: 8,
            border: '1px solid var(--border)', borderRadius: 6,
            background: 'var(--bg-primary)', color: 'var(--text-primary)',
            fontSize: 14,
          }}
        >
          <option value="">选择群组</option>
          {manageableGroups.map(g => (
            <option key={g.group_id} value={g.group_id}>{g.group_name}</option>
          ))}
        </select>
        <div className="member-select">
          {users.map(u => (
            <div
              key={u.user_id}
              className={`member-tag ${addMemberIds.includes(u.user_id) ? 'selected' : ''}`}
              onClick={() => toggleMember(u.user_id, addMemberIds, setAddMemberIds)}
            >
              {u.user_name}
            </div>
          ))}
        </div>
        <button onClick={handleAddMembers}>添加成员</button>
      </div>

      {/* 群组列表 */}
      <div className="admin-panel-section">
        <h4>群组列表</h4>
        {manageableGroups.map(group => (
          <div key={group.group_id} className="admin-item">
            <div className="admin-item-info">
              <span className="admin-item-name">{group.group_name}</span>
              <span className="admin-item-meta">
                ID: {group.group_id} | 成员: {group.members.join(', ')}
              </span>
            </div>
            <div className="admin-item-actions">
              <button className="danger" onClick={() => onDeleteGroup(group.group_id)}>删除</button>
            </div>
          </div>
        ))}
        {manageableGroups.length === 0 && (
          <div style={{ color: 'var(--text-secondary)', fontSize: 13 }}>暂无群组</div>
        )}
      </div>
    </div>
  );
}

// ===== 用户管理面板 =====
interface UserManagementProps {
  users: User[];
  onCreateUser: (userId: string, userName: string) => void;
  onDeleteUser: (userId: string) => void;
  onBack: () => void;
}

function UserManagementPanel({ users, onCreateUser, onDeleteUser, onBack }: UserManagementProps) {
  const [newUserId, setNewUserId] = useState('');
  const [newUserName, setNewUserName] = useState('');

  const handleCreate = () => {
    if (!newUserId.trim() || !newUserName.trim()) return;
    onCreateUser(newUserId.trim(), newUserName.trim());
    setNewUserId('');
    setNewUserName('');
  };

  return (
    <div className="admin-panel">
      <h3>
        <button onClick={onBack} style={{ background: 'none', border: 'none', color: 'var(--accent)', cursor: 'pointer', marginRight: 8, fontSize: 16 }}>
          ←
        </button>
        用户管理
      </h3>

      {/* 新建用户 */}
      <div className="admin-panel-section">
        <h4>新建用户</h4>
        <input
          placeholder="用户 ID（唯一标识）"
          value={newUserId}
          onChange={e => setNewUserId(e.target.value)}
        />
        <input
          placeholder="用户名称"
          value={newUserName}
          onChange={e => setNewUserName(e.target.value)}
        />
        <button onClick={handleCreate}>创建</button>
      </div>

      {/* 用户列表 */}
      <div className="admin-panel-section">
        <h4>用户列表</h4>
        {users.map(user => (
          <div key={user.user_id} className="admin-item">
            <div className="admin-item-info">
              <span className="admin-item-name">{user.user_name}</span>
              <span className="admin-item-meta">ID: {user.user_id}</span>
            </div>
            <div className="admin-item-actions">
              <button className="danger" onClick={() => onDeleteUser(user.user_id)}>删除</button>
            </div>
          </div>
        ))}
        {users.length === 0 && (
          <div style={{ color: 'var(--text-secondary)', fontSize: 13 }}>暂无用户</div>
        )}
      </div>
    </div>
  );
}

export default App;
