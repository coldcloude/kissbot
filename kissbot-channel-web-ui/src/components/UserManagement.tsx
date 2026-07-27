import { useState, useRef, useEffect } from 'react';
import type { UserConfig } from '../types';

interface UserManagementProps {
  users: UserConfig[];
  onCreateUser: (userName: string) => void;
  onRenameUser: (userId: string, userName: string) => void;
  onDeleteUser: (userId: string) => void;
  onBack: () => void;
}

function RenameDialog({ userId, currentName, onConfirm, onCancel }: {
  userId: string;
  currentName: string;
  onConfirm: (userId: string, name: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(currentName);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (inputRef.current) { inputRef.current.focus(); inputRef.current.select(); }
  }, []);

  return (
    <div className="image-overlay" onClick={onCancel}>
      <div className="login-card" onClick={e => e.stopPropagation()} style={{ width: 320 }}>
        <h1>重命名用户</h1>
        <div className="login-section">
          <input
            ref={inputRef}
            type="text" placeholder="新名称"
            value={value}
            onChange={e => setValue(e.target.value)}
            onKeyDown={e => e.key === 'Enter' && value.trim() && onConfirm(userId, value.trim())}
          />
        </div>
        <button className="connect-btn" onClick={() => value.trim() && onConfirm(userId, value.trim())}>确认</button>
      </div>
    </div>
  );
}

export default function UserManagement({
  users, onCreateUser, onRenameUser, onDeleteUser, onBack,
}: UserManagementProps) {
  const [newName, setNewName] = useState('');
  const [renameTarget, setRenameTarget] = useState<{ id: string; name: string } | null>(null);

  const handleCreate = () => {
    if (!newName.trim()) return;
    onCreateUser(newName.trim());
    setNewName('');
  };

  const handleRename = (userId: string, userName: string) => {
    onRenameUser(userId, userName);
    setRenameTarget(null);
  };

  return (
    <div className="admin-panel">
      <h3 className="admin-panel-title">
        <button className="back-btn" onClick={onBack}>←</button>
        用户管理
      </h3>

      {/* 新建用户 */}
      <div className="admin-panel-section">
        <h4>新建用户</h4>
        <input type="text" placeholder="用户名称" value={newName} onChange={e => setNewName(e.target.value)}
          onKeyDown={e => e.key === 'Enter' && handleCreate()} />
        <button className="action-btn" onClick={handleCreate}>创建</button>
        <p className="hint-text">用户 ID 由系统自动生成，创建后自动建立与 admin 的单聊群组</p>
      </div>

      {/* 用户列表 */}
      <div className="admin-panel-section">
        <h4>用户列表</h4>
        {users.map(u => (
          <div key={u.user_id} className="admin-item">
            <div className="admin-item-info">
              <span className="admin-item-name">{u.user_name}</span>
              <span className="admin-item-meta">ID: {u.user_id}</span>
            </div>
            <div className="admin-item-actions">
              <button className="action-btn" onClick={() => setRenameTarget({ id: u.user_id, name: u.user_name })}>
                ✏️ 重命名
              </button>
              <button className="action-btn danger" onClick={() => {
                if (window.confirm(`确认删除用户 ${u.user_name}？`)) onDeleteUser(u.user_id);
              }}>删除</button>
            </div>
          </div>
        ))}
        {users.length === 0 && <div style={{ color: 'var(--text-secondary)', fontSize: 13 }}>暂无用户</div>}
      </div>

      {renameTarget && (
        <RenameDialog
          userId={renameTarget.id}
          currentName={renameTarget.name}
          onConfirm={handleRename}
          onCancel={() => setRenameTarget(null)}
        />
      )}
    </div>
  );
}
