import { useRef, useEffect, useState, useCallback } from 'react';
import type { GroupedMessages } from '../types';
import MessageBubble from './MessageBubble';
import AttachmentPreview from './AttachmentPreview';

interface MessageAreaProps {
  groupName: string;
  groupedMessages: GroupedMessages[];
  loadingMore: boolean;
  canSend: boolean;
  onSendText: (text: string) => Promise<void>;
  onSendAttachment: (file: File) => Promise<void>;
  onLoadMore: () => void;
}

export default function MessageArea({
  groupName, groupedMessages, loadingMore, canSend,
  onSendText, onSendAttachment, onLoadMore,
}: MessageAreaProps) {
  const [text, setText] = useState('');
  const [attachments, setAttachments] = useState<File[]>([]);
  const listRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const prevMsgCount = useRef(0);

  // 自动滚动到底部（仅首次加载或新消息追加时）
  useEffect(() => {
    const total = groupedMessages.reduce((s, g) => s + g.messages.length, 0);
    if (total > prevMsgCount.current && total - prevMsgCount.current < 5) {
      const el = listRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    }
    prevMsgCount.current = total;
  }, [groupedMessages]);

  // 滚动到顶部触发加载更多
  const handleScroll = useCallback(() => {
    const el = listRef.current;
    if (!el || loadingMore) return;
    if (el.scrollTop < 50) {
      onLoadMore();
    }
  }, [loadingMore, onLoadMore]);

  const handleSend = async () => {
    if (text.trim()) {
      await onSendText(text.trim());
      setText('');
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (!files || files.length === 0) return;
    setAttachments(prev => [...prev, ...Array.from(files)]);
  };

  const handleSendAttachments = async () => {
    for (const file of attachments) {
      await onSendAttachment(file);
    }
    setAttachments([]);
  };

  return (
    <>
      <div className="chat-header">
        <h3>{groupName}</h3>
      </div>

      <div className="message-list" ref={listRef} onScroll={handleScroll}>
        {loadingMore && <div className="message-load-more">加载更多...</div>}
        {groupedMessages.map(group => (
          group.messages.map(lm => (
            <MessageBubble key={lm.message.msg_id} message={lm.message} />
          ))
        ))}
      </div>

      <AttachmentPreview files={attachments} onRemove={i => setAttachments(prev => prev.filter((_, j) => j !== i))} />

      <div className={`input-area${!canSend ? ' disabled' : ''}`}>
        <div className="input-wrapper">
          <input
            type="text"
            placeholder={canSend ? '输入消息...' : '你未加入该群组，无法发送消息'}
            value={text}
            onChange={e => setText(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={!canSend}
          />
          <div className="input-actions">
            <button
              title="上传附件"
              onClick={() => fileInputRef.current?.click()}
              disabled={!canSend}
            >
              📎
            </button>
          </div>
        </div>
        {attachments.length > 0 ? (
          <button className="send-button" onClick={handleSendAttachments}>
            上传附件 ({attachments.length})
          </button>
        ) : (
          <button
            className="send-button"
            onClick={handleSend}
            disabled={!canSend || !text.trim()}
          >
            发送
          </button>
        )}
        <input type="file" ref={fileInputRef} style={{ display: 'none' }} multiple onChange={handleFileChange} />
      </div>
    </>
  );
}
