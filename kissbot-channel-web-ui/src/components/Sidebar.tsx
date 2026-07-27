export interface SidebarConversation {
  groupId: string;
  displayName: string;
  isGroup: boolean;
  latestTime: string;
  unreadCount: number;
}

interface SidebarProps {
  conversations: SidebarConversation[];
  activeGroupId: string | null;
  onSelect: (groupId: string) => void;
}

function formatBadge(count: number): string {
  if (count === 0) return '';
  if (count > 99) return '...';
  return String(count);
}

export default function Sidebar({ conversations, activeGroupId, onSelect }: SidebarProps) {
  const sorted = [...conversations].sort((a, b) => {
    if (!a.latestTime && !b.latestTime) return 0;
    if (!a.latestTime) return 1;
    if (!b.latestTime) return -1;
    return b.latestTime.localeCompare(a.latestTime);
  });

  return (
    <div className="sidebar">
      <div className="conversation-list">
        {sorted.map(c => (
          <div
            key={c.groupId}
            className={`conversation-item${activeGroupId === c.groupId ? ' active' : ''}`}
            onClick={() => onSelect(c.groupId)}
          >
            <span className="conversation-name">
              {c.displayName}
              {c.isGroup && <span className="group-tag">群组</span>}
            </span>
            <span className="conversation-badge">{formatBadge(c.unreadCount)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
