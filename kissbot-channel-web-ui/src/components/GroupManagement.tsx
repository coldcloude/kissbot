import { useState } from 'react';
import type { UserConfig, GroupConfig } from '../types';

interface GroupManagementProps {
  groups: GroupConfig[];
  users: UserConfig[];
  onCreateGroup: (name: string, memberIds: string[]) => void;
  onRenameGroup: (groupId: string, name: string) => void;
  onDeleteGroup: (groupId: string) => void;
  onManageMembers: (groupId: string, addIds: string[], removeIds: string[]) => void;
  onBack: () => void;
}

export default function GroupManagement({
  groups, users, onCreateGroup, onRenameGroup,
  onDeleteGroup, onManageMembers, onBack,
}: GroupManagementProps) {
  const [newName, setNewName] = useState('');
  const [newMembers, setNewMembers] = useState<string[]>([]);
  const [renameId, setRenameId] = useState('');
  const [renameName, setRenameName] = useState('');
  const [manageId, setManageId] = useState('');
  const [addIds, setAddIds] = useState<string[]>([]);

  const handleCreate = () => {
    if (!newName.trim()) return;
    onCreateGroup(newName.trim(), newMembers);
    setNewName('');
    setNewMembers([]);
  };

  const handleRename = () => {
    if (!renameId || !renameName.trim()) return;
    onRenameGroup(renameId, renameName.trim());
    setRenameId('');
    setRenameName('');
  };

  const handleAddMembers = () => {
    if (!manageId) return;
    onManageMembers(manageId, addIds, []);
    setAddIds([]);
  };

  const toggleMember = (id: string, list: string[], setList: (ids: string[]) => void) => {
    setList(list.includes(id) ? list.filter(m => m !== id) : [...list, id]);
  };

  const adminUserGroups = groups.filter(g => g.group_id.startsWith('a_'));
  const multiGroups = groups.filter(g => !g.group_id.startsWith('a_'));

  return (
    <div className="admin-panel">
      <h3 className="admin-panel-title">
        <button className="back-btn" onClick={onBack}>←</button>
        群组管理
      </h3>

      {/* 新建群组 */}
      <div className="admin-panel-section">
        <h4>新建群组</h4>
        <input type="text" placeholder="群组名称" value={newName} onChange={e => setNewName(e.target.value)} />
        <div className="member-select">
          {users.map(u => (
            <div
              key={u.user_id}
              className={`member-tag${newMembers.includes(u.user_id) ? ' selected' : ''}`}
              onClick={() => toggleMember(u.user_id, newMembers, setNewMembers)}
            >
              {u.user_name}
            </div>
          ))}
        </div>
        <button className="action-btn" onClick={handleCreate}>创建</button>
      </div>

      {/* 重命名群组 */}
      <div className="admin-panel-section">
        <h4>重命名群组</h4>
        <select className="admin-select" value={renameId} onChange={e => {
          setRenameId(e.target.value);
          const g = groups.find(gr => gr.group_id === e.target.value);
          setRenameName(g?.group_name || '');
        }}>
          <option value="">选择群组</option>
          {multiGroups.map(g => (
            <option key={g.group_id} value={g.group_id}>{g.group_name}</option>
          ))}
        </select>
        <input type="text" placeholder="新名称" value={renameName} onChange={e => setRenameName(e.target.value)} />
        <button className="action-btn" onClick={handleRename}>重命名</button>
      </div>

      {/* 管理成员 */}
      <div className="admin-panel-section">
        <h4>管理成员</h4>
        <select className="admin-select" value={manageId} onChange={e => setManageId(e.target.value)}>
          <option value="">选择群组</option>
          {multiGroups.map(g => (
            <option key={g.group_id} value={g.group_id}>{g.group_name}</option>
          ))}
        </select>
        <div className="member-select">
          {users.map(u => (
            <div
              key={u.user_id}
              className={`member-tag${addIds.includes(u.user_id) ? ' selected' : ''}`}
              onClick={() => toggleMember(u.user_id, addIds, setAddIds)}
            >
              {u.user_name}
            </div>
          ))}
        </div>
        <button className="action-btn" onClick={handleAddMembers}>添加成员</button>
      </div>

      {/* 群组列表 */}
      <div className="admin-panel-section">
        <h4>群组列表</h4>
        {adminUserGroups.map(g => (
          <div key={g.group_id} className="admin-item disabled">
            <div className="admin-item-info">
              <span className="admin-item-name">{g.group_name} <span className="item-tag">单聊</span></span>
              <span className="admin-item-meta">ID: {g.group_id}</span>
            </div>
            <div className="admin-item-actions">
              <span className="disabled-note">仅可查看消息</span>
            </div>
          </div>
        ))}
        {multiGroups.map(g => (
          <div key={g.group_id} className="admin-item">
            <div className="admin-item-info">
              <span className="admin-item-name">{g.group_name}</span>
              <span className="admin-item-meta">ID: {g.group_id} | 成员: {g.members.join(', ')}</span>
            </div>
            <div className="admin-item-actions">
              <button className="action-btn danger" onClick={() => onDeleteGroup(g.group_id)}>删除</button>
            </div>
          </div>
        ))}
        {groups.length === 0 && <div style={{ color: 'var(--text-secondary)', fontSize: 13 }}>暂无群组</div>}
      </div>
    </div>
  );
}
