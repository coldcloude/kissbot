import { useState } from 'react';
import { DEFAULT_BACKEND_URLS } from '../api/config';

interface LoginPageProps {
  onConnect: (backendUrl: string, apiKey: string) => Promise<void>;
}

export default function LoginPage({ onConnect }: LoginPageProps) {
  const [selectedUrl, setSelectedUrl] = useState(DEFAULT_BACKEND_URLS[1].url);
  const [apiKey, setApiKey] = useState('');
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState('');

  const handleConnect = async () => {
    if (!apiKey.trim()) { setError('请输入 Admin Key'); return; }
    setConnecting(true);
    setError('');
    try {
      await onConnect(selectedUrl, apiKey.trim());
    } catch {
      setError('连接失败');
    } finally {
      setConnecting(false);
    }
  };

  return (
    <div className="login-page">
      <div className="login-card">
        <h1>Kissbot Web Chat</h1>
        <p className="login-subtitle">管理后台</p>

        <div className="login-section">
          <label className="login-label">选择后端</label>
          <div className="backend-url-list">
            {DEFAULT_BACKEND_URLS.map(opt => (
              <div
                key={opt.url}
                className={`backend-url-item${selectedUrl === opt.url ? ' selected' : ''}`}
                onClick={() => setSelectedUrl(opt.url)}
              >
                <div className="backend-name">{opt.name}</div>
                <div className="backend-url">{opt.url}</div>
              </div>
            ))}
          </div>
        </div>

        <div className="login-section">
          <label className="login-label">Admin Key</label>
          <input
            type="password"
            placeholder="输入 Admin API Key"
            value={apiKey}
            onChange={e => setApiKey(e.target.value)}
            onKeyDown={e => e.key === 'Enter' && handleConnect()}
          />
        </div>

        {error && <div className="login-error">{error}</div>}

        <button className="connect-btn" onClick={handleConnect} disabled={connecting}>
          {connecting ? '连接中...' : '连接'}
        </button>
      </div>
    </div>
  );
}
