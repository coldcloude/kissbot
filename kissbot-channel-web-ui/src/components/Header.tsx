import { useState, useRef, useEffect } from 'react';
import type { AdminView } from '../types';

interface HeaderProps {
  adminName: string;
  onRenameAdmin: (name: string) => void;
  onNavigateAdmin: (view: AdminView) => void;
}

export default function Header({ adminName, onRenameAdmin, onNavigateAdmin }: HeaderProps) {
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameValue, setRenameValue] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (renameOpen && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [renameOpen]);

  const handleRename = () => {
    if (renameValue.trim()) {
      onRenameAdmin(renameValue.trim());
    }
    setRenameOpen(false);
  };

  return (
    <div className="header">
      <span className="app-name">Kissbot Web Chat</span>
      <div className="admin-dropdown">
        <span className="admin-trigger">{adminName} ▼</span>
        <div className="dropdown-menu">
          <div className="dropdown-item" onClick={() => { setRenameValue(adminName); setRenameOpen(true); }}>
            ✏️ 重命名管理员
          </div>
          <div className="dropdown-item" onClick={() => onNavigateAdmin('groups')}>
            ☰ 群组管理
          </div>
          <div className="dropdown-item" onClick={() => onNavigateAdmin('users')}>
            ☰ 用户管理
          </div>
        </div>
      </div>

      {renameOpen && (
        <div className="image-overlay" onClick={() => setRenameOpen(false)}>
          <div className="login-card" onClick={e => e.stopPropagation()} style={{ width: 320 }}>
            <h1>重命名管理员</h1>
            <div className="login-section">
              <input
                ref={inputRef}
                type="text"
                placeholder="新名称"
                value={renameValue}
                onChange={e => setRenameValue(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && handleRename()}
              />
            </div>
            <button className="connect-btn" onClick={handleRename}>确认</button>
          </div>
        </div>
      )}
    </div>
  );
}
