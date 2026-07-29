import { useState, useEffect, useCallback, useRef } from 'react';
import { loadBackendConfig } from '../api/backendConfig';
import type { BackendUrlOption } from '../types';

interface LoginPageProps {
  onConnect: (backendUrl: string, apiKey: string) => Promise<void>;
}

type Selection =
  | { kind: 'preset'; url: string }
  | { kind: 'custom'; url: string };

export default function LoginPage({ onConnect }: LoginPageProps) {
  const [presetBackends, setPresetBackends] = useState<BackendUrlOption[]>([]);
  const [configLoading, setConfigLoading] = useState(true);
  const [selection, setSelection] = useState<Selection>({ kind: 'custom', url: '' });
  const [customUrl, setCustomUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState('');

  // 加载预置后端配置
  useEffect(() => {
    (async () => {
      const backends = await loadBackendConfig();
      setPresetBackends(backends);
      setConfigLoading(false);
      // 默认选中第一个预置项；无预置时默认选中自定义
      if (backends.length > 0) {
        setSelection({ kind: 'preset', url: backends[0].url });
      }
    })();
  }, []);

  const customInputRef = useRef<HTMLInputElement>(null);

  const handleCustomFocus = useCallback(() => {
    setSelection({ kind: 'custom', url: customUrl.trim() });
  }, [customUrl]);

  const handleCustomClick = useCallback(() => {
    // 点击自定义项外部区域时，聚焦 input 并选中自定义
    customInputRef.current?.focus();
    setSelection({ kind: 'custom', url: customUrl.trim() });
  }, [customUrl]);

  const handleCustomInput = useCallback((value: string) => {
    setCustomUrl(value);
    setSelection({ kind: 'custom', url: value.trim() });
  }, []);

  const handlePresetClick = useCallback((url: string) => {
    setSelection({ kind: 'preset', url });
  }, []);

  const handleConnect = async () => {
    setError('');

    // 校验：自定义选中但 URL 为空
    if (selection.kind === 'custom') {
      const trimmed = customUrl.trim();
      if (!trimmed) {
        setError('请输入后端 URL');
        return;
      }
      if (!/^https?:\/\//i.test(trimmed)) {
        setError('URL 必须以 http:// 或 https:// 开头');
        return;
      }
    }

    if (!apiKey.trim()) {
      setError('请输入 Admin Key');
      return;
    }

    setConnecting(true);
    try {
      const url = selection.kind === 'custom' ? customUrl.trim() : selection.url;
      await onConnect(url, apiKey.trim());
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
            {/* 自定义项 — 始终显示在最上方 */}
            <div
              className={`backend-url-item backend-url-custom${selection.kind === 'custom' ? ' selected' : ''}`}
              onClick={handleCustomClick}
            >
              <div className="backend-name">自定义</div>
              <input
                ref={customInputRef}
                type="url"
                placeholder="输入自定义后端 URL，如 https://api.example.com"
                value={customUrl}
                onFocus={handleCustomFocus}
                onChange={e => handleCustomInput(e.target.value)}
              />
            </div>

            {/* 加载中状态 */}
            {configLoading && (
              <div className="backend-url-item backend-url-loading">
                <div className="backend-name">加载配置中...</div>
              </div>
            )}

            {/* 预置后端列表 */}
            {!configLoading && presetBackends.map(opt => (
              <div
                key={opt.url}
                className={`backend-url-item${selection.kind === 'preset' && selection.url === opt.url ? ' selected' : ''}`}
                onClick={() => handlePresetClick(opt.url)}
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
            onKeyDown={e => e.key === 'Enter' && !connecting && handleConnect()}
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
