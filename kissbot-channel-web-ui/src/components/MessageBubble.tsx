import { useState } from 'react';
import type { IncomingMessage, Content } from '../types';
import { getDownloadUrl, getThumbnailUrl, getApiBase } from '../api/client';
import ImageOverlay from './ImageOverlay';

interface MessageBubbleProps {
  message: IncomingMessage;
}

function formatTime(timeStr: string): string {
  try {
    const d = new Date(timeStr);
    const now = new Date();
    const isToday = d.toDateString() === now.toDateString();
    if (isToday) {
      return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
    }
    return d.toLocaleDateString('zh-CN', {
      month: '2-digit', day: '2-digit',
      hour: '2-digit', minute: '2-digit',
    });
  } catch {
    return timeStr;
  }
}

function renderContent(content: Content, apiBase: string): React.ReactNode {
  switch (content.msg_type) {
    case 'Text':
      return <div className="text-content">{content.data}</div>;

    case 'AttachmentInfoResponse': {
      const resp = content.data;
      return renderAttachment(resp.key, resp.info.file_name, resp.info.mime_type, apiBase);
    }

    case 'GroupJoin':
      return (
        <div className="msg-system-text">📋 用户 {content.data.user_id} 加入了群组</div>
      );

    case 'GroupLeave':
      return (
        <div className="msg-system-text">📋 用户 {content.data.user_id} 离开了群组</div>
      );

    case 'UserRemove':
      return (
        <div className="msg-system-text">📋 用户 {content.data.user_id} 已被删除</div>
      );

    case 'Multi':
      return (
        <div className="msg-multi">
          {content.data.map((item, i) => (
            <div key={i} className="multi-item">
              {renderContent(item, apiBase)}
            </div>
          ))}
        </div>
      );

    default:
      return null;
  }
}

function renderAttachment(key: string, fileName: string, mimeType: string, apiBase: string) {
  if (mimeType.startsWith('image/')) {
    return <ImageAttachment keyStr={key} fileName={fileName} apiBase={apiBase} />;
  }
  return (
    <a
      className="file-attachment"
      href={getDownloadUrl(apiBase, key)}
      target="_blank"
      rel="noopener noreferrer"
      onClick={e => e.stopPropagation()}
    >
      📎 {fileName}
    </a>
  );
}

function ImageAttachment({ keyStr, fileName, apiBase }: { keyStr: string; fileName: string; apiBase: string }) {
  const [overlayOpen, setOverlayOpen] = useState(false);
  return (
    <>
      <img
        className="image-attachment"
        src={getThumbnailUrl(apiBase, keyStr)}
        alt={fileName}
        onClick={() => setOverlayOpen(true)}
      />
      {overlayOpen && (
        <ImageOverlay
          src={getDownloadUrl(apiBase, keyStr)}
          onClose={() => setOverlayOpen(false)}
        />
      )}
    </>
  );
}

export default function MessageBubble({ message }: MessageBubbleProps) {
  const apiBase = getApiBase();
  const rendered = renderContent(message.content, apiBase);

  // 系统消息（GroupJoin / GroupLeave / UserRemove）居中显示
  if (message.content.msg_type === 'GroupJoin' || message.content.msg_type === 'GroupLeave' || message.content.msg_type === 'UserRemove') {
    return <div className="msg msg-system">{rendered}</div>;
  }

  return (
    <div className={`message ${message.is_self === 1 ? 'self' : 'other'}`}>
      {message.is_self === 0 && <div className="message-sender">{message.user_id}</div>}
      <div className="message-bubble">{rendered}</div>
      <span className="message-time">{formatTime(message.time)}</span>
    </div>
  );
}
