import Header from './Header';
import Sidebar, { type SidebarConversation } from './Sidebar';
import MessageArea from './MessageArea';
import GroupManagement from './GroupManagement';
import UserManagement from './UserManagement';
import type { UserConfig, GroupConfig, GroupedMessages, AdminView } from '../types';

interface MainLayoutProps {
  adminName: string;
  users: UserConfig[];
  groups: GroupConfig[];
  groupedMessagesMap: Record<string, GroupedMessages[]>;
  unreadCounts: Map<string, number>;
  activeGroupId: string | null;
  adminView: AdminView;
  loadingMore: boolean;
  onSelectGroup: (groupId: string) => void;
  onAdminView: (view: AdminView) => void;
  onSendText: (groupId: string, text: string) => Promise<void>;
  onSendAttachment: (groupId: string, file: File) => Promise<void>;
  onLoadMore: () => void;
  onRenameAdmin: (name: string) => void;
  onCreateGroup: (name: string, memberIds: string[]) => void;
  onRenameGroup: (groupId: string, name: string) => void;
  onDeleteGroup: (groupId: string) => void;
  onManageMembers: (groupId: string, addIds: string[], removeIds: string[]) => void;
  onCreateUser: (userName: string) => void;
  onRenameUser: (userId: string, userName: string) => void;
  onDeleteUser: (userId: string) => void;
}

export default function MainLayout({
  adminName, users, groups, groupedMessagesMap,
  unreadCounts, activeGroupId, adminView, loadingMore,
  onSelectGroup, onAdminView,
  onSendText, onSendAttachment, onLoadMore, onRenameAdmin,
  onCreateGroup, onRenameGroup, onDeleteGroup, onManageMembers,
  onCreateUser, onRenameUser, onDeleteUser,
}: MainLayoutProps) {
  // 构建会话列表
  const conversations: SidebarConversation[] = [];

  // admin-user 单聊组
  for (const [gid, group] of Object.entries(
    Object.fromEntries(groups.filter(g => g.group_id.startsWith('a_')).map(g => [g.group_id, g]))
  )) {
    const userId = gid.replace(/^a_/, '');
    const user = users.find(u => u.user_id === userId);
    const msgs = groupedMessagesMap[gid] || [];
    const latestTime = msgs.length > 0
      ? msgs[msgs.length - 1].messages.slice(-1)[0]?.message.time || ''
      : '';
    conversations.push({
      groupId: gid,
      displayName: user?.user_name || group.group_name,
      isGroup: false,
      latestTime,
      unreadCount: unreadCounts.get(gid) || 0,
    });
  }

  // 多人群组
  for (const group of groups.filter(g => !g.group_id.startsWith('a_'))) {
    const msgs = groupedMessagesMap[group.group_id] || [];
    const latestTime = msgs.length > 0
      ? msgs[msgs.length - 1].messages.slice(-1)[0]?.message.time || ''
      : '';
    conversations.push({
      groupId: group.group_id,
      displayName: group.group_name,
      isGroup: true,
      latestTime,
      unreadCount: unreadCounts.get(group.group_id) || 0,
    });
  }

  const activeGroup = groups.find(g => g.group_id === activeGroupId);
  const isJoined = activeGroup ? activeGroup.members.includes('admin') : false;

  const msgs = activeGroupId ? (groupedMessagesMap[activeGroupId] || []) : [];

  const handleSendText = async (text: string) => {
    if (!activeGroupId) return;
    await onSendText(activeGroupId, text);
  };

  const handleSendAttachment = async (file: File) => {
    if (!activeGroupId) return;
    await onSendAttachment(activeGroupId, file);
  };

  return (
    <div className="chat-layout">
      <Header adminName={adminName} onRenameAdmin={onRenameAdmin} onNavigateAdmin={onAdminView} />
      <div className="body">
        <Sidebar
          conversations={conversations}
          activeGroupId={activeGroupId}
          onSelect={(gid) => { onSelectGroup(gid); onAdminView('none'); }}
        />
        <div className="main-content">
          {adminView !== 'none' ? (
            adminView === 'groups' ? (
              <GroupManagement
                groups={groups}
                users={users}
                onCreateGroup={onCreateGroup}
                onRenameGroup={onRenameGroup}
                onDeleteGroup={onDeleteGroup}
                onManageMembers={onManageMembers}
                onBack={() => onAdminView('none')}
              />
            ) : (
              <UserManagement
                users={users}
                onCreateUser={onCreateUser}
                onRenameUser={onRenameUser}
                onDeleteUser={onDeleteUser}
                onBack={() => onAdminView('none')}
              />
            )
          ) : activeGroupId ? (
            <MessageArea
              groupName={activeGroup?.group_name || ''}
              groupedMessages={msgs}
              loadingMore={loadingMore}
              canSend={isJoined}
              onSendText={handleSendText}
              onSendAttachment={handleSendAttachment}
              onLoadMore={onLoadMore}
            />
          ) : (
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '100%', color: 'var(--text-secondary)' }}>
              选择一个会话
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
