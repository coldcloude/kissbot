# kissbot-channel-web-ui 换肤与登录页后端选择修正 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** UI 整体换肤为原型浅色主题，登录页支持自定义 URL 输入、预置后端通过 `public/backends.json` 运行时加载部署可替换，补强测试覆盖。

**架构：** React 组件层不动 class 名，仅重写 `src/index.css` 浅色主题值；登录页状态改为 `<selectedUrl, isCustom>` 模型，预置列表首次挂载时 `fetch('/backends.json')` 加载。

**Tech Stack:** React 19 + TypeScript + Vite 8 + Vitest 4 + jsdom

**相关文档（执行前可读）：**
- 设计文档：`docs/superpowers/specs/2026-07-29-channel-web-ui-theme-and-backend-selection-design.md`
- UI 原型 HTML/CSS：`docs/design/components-design/ui-ux-design/kissbot-channel-web/`
- 现有代码：`src/components/LoginPage.tsx`、`src/api/config.ts`、`src/api/client.ts`

## 全局约束

- 使用中文 commit comment，包含所有改动的描述
- 禁止删除代码中的注释
- 所有文本文件 UTF-8 编码，\n 换行符
- 小组件 class 名保持不变，仅改 CSS 值和微调 markup
- `backends.json` 必须不进入 JS bundle（放在 `public/` 目录）

---

### Task 1: 创建后端配置文件与加载器

**Files:**
- Create: `public/backends.json`
- Create: `src/api/backendConfig.ts`
- Create: `src/__tests__/backendConfig.test.ts`
- Delete: `src/api/config.ts`

**Interfaces:**
- Consumes: `BackendUrlOption` from `src/types/index.ts`
- Produces: `loadBackendConfig(): Promise<BackendUrlOption[]>` exported from `src/api/backendConfig.ts`

- [ ] **Step 1: 创建 `public/backends.json`**

```json
{
  "backends": [
    { "name": "生产环境", "url": "https://api.kissbot.example.com" },
    { "name": "测试环境", "url": "http://localhost:8301" },
    { "name": "开发环境", "url": "http://192.168.1.100:8301" }
  ]
}
```

- [ ] **Step 2: 创建 `src/api/backendConfig.ts`**

```typescript
import type { BackendUrlOption } from '../types';

const CONFIG_PATH = '/backends.json';

export async function loadBackendConfig(): Promise<BackendUrlOption[]> {
  try {
    const res = await fetch(CONFIG_PATH);
    if (!res.ok) return [];
    const data = await res.json();
    if (!Array.isArray(data?.backends)) return [];
    // 过滤并校验每个条目
    const backends: BackendUrlOption[] = [];
    for (const item of data.backends) {
      if (typeof item.name === 'string' && typeof item.url === 'string') {
        backends.push({ name: item.name, url: item.url });
      }
    }
    return backends;
  } catch {
    return [];
  }
}
```

- [ ] **Step 3: 创建测试 `src/__tests__/backendConfig.test.ts`**

```typescript
import { describe, it, expect, beforeEach } from 'vitest';

// mock fetch 在测试文件中使用 vi.stubGlobal
function mockFetch(response: unknown, ok = true) {
  return async (url: string) => ({
    ok,
    status: ok ? 200 : 500,
    json: async () => response,
  });
}

describe('loadBackendConfig', () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
  });

  it('解析合法的 backends.json', async () => {
    vi.stubGlobal('fetch', mockFetch({
      backends: [
        { name: '测试', url: 'http://localhost:8301' },
      ],
    }));
    const { loadBackendConfig } = await import('../api/backendConfig');
    const result = await loadBackendConfig();
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ name: '测试', url: 'http://localhost:8301' });
  });

  it('缺少 backends 字段返回空数组', async () => {
    vi.stubGlobal('fetch', mockFetch({}));
    const { loadBackendConfig } = await import('../api/backendConfig');
    const result = await loadBackendConfig();
    expect(result).toEqual([]);
  });

  it('HTTP 错误返回空数组', async () => {
    vi.stubGlobal('fetch', mockFetch(null, false));
    const { loadBackendConfig } = await import('../api/backendConfig');
    const result = await loadBackendConfig();
    expect(result).toEqual([]);
  });

  it('网络异常返回空数组', async () => {
    vi.stubGlobal('fetch', () => Promise.reject(new Error('network')));
    const { loadBackendConfig } = await import('../api/backendConfig');
    const result = await loadBackendConfig();
    expect(result).toEqual([]);
  });

  it('过滤类型异常的条目', async () => {
    vi.stubGlobal('fetch', mockFetch({
      backends: [
        { name: '好', url: 'http://ok' },
        { name: null, url: 'http://bad' },
        { name: '坏', url: null },
        { name: '其实好', url: 'http://ok2' },
      ],
    }));
    const { loadBackendConfig } = await import('../api/backendConfig');
    const result = await loadBackendConfig();
    expect(result).toHaveLength(2);
  });
});
```

- [ ] **Step 4: 运行测试验证通过**

```bash
cd kissbot-channel-web-ui
npx vitest run src/__tests__/backendConfig.test.ts --reporter=verbose
```

预期：4 个 test 全部 PASS。

- [ ] **Step 5: 删除 `src/api/config.ts`**

```bash
rm src/api/config.ts
```

- [ ] **Step 6: 确认 `npm run build` 后 `dist/backends.json` 存在且不在 JS bundle 中**

```bash
npm run build
ls dist/backends.json
# 验证 JS bundle 不含 backends
grep -c "api.kissbot" dist/assets/*.js || echo "no hardcoded URLs in bundle"
```

预期：`dist/backends.json` 存在，JS bundle 中无预置 URL 字符串。

- [ ] **Step 7: 提交**

```bash
git add public/backends.json src/api/backendConfig.ts src/__tests__/backendConfig.test.ts
git add -u  # 包括删除 config.ts
git commit -m "feat: 预置后端改为 public/backends.json 运行时加载

- 新增 public/backends.json，构建时拷入 dist，部署可替换
- 新增 src/api/backendConfig.ts loadBackendConfig()，fetch 加载
- 新增 backendConfig.test.ts 覆盖正常/错误/异常/过滤四种场景
- 删除 src/api/config.ts（硬编码 DEFAULT_BACKEND_URLS）"
```

---

### Task 2: 重写 LoginPage（添加自定义 URL + 互斥选中）

**Files:**
- Modify: `src/components/LoginPage.tsx`

**Interfaces:**
- Consumes: `loadBackendConfig(): Promise<BackendUrlOption[]>` from `src/api/backendConfig`
- Consumes: `LoginPageProps { onConnect: (backendUrl: string, apiKey: string) => Promise<void> }`
- Produces: 渲染的 LoginPage 组件

- [ ] **Step 1: 重写 `LoginPage.tsx`**

完整替换为：

```tsx
import { useState, useEffect, useCallback } from 'react';
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

  const handleCustomFocus = useCallback(() => {
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

  const targetUrl = selection.kind === 'preset' ? selection.url : customUrl.trim();

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
            >
              <div className="backend-name">自定义</div>
              <input
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
```

- [ ] **Step 2: 确认 TypeScript 编译通过**

```bash
cd kissbot-channel-web-ui
npx tsc --noEmit 2>&1 | head -20
```

预期：无类型错误。

- [ ] **Step 3: 提交**

```bash
git add src/components/LoginPage.tsx
git commit -m "feat: 登录页支持自定义后端 URL 输入和互斥选中

- 自定义项固定在预置列表上方，聚焦/输入即选中
- 点击预置项选中该预置，自定义输入保留但取消选中
- 加载 backends.json 后默认选中第一个预置项
- 加载失败/空时降级为仅显示自定义项
- 连接前置 URL 校验：空值、非 http(s) 格式拦截
- 删除对已移除的 config.ts 的 import"
```

---

### Task 3: 重写 index.css 为浅色主题

**Files:**
- Modify: `src/index.css`

- [ ] **Step 1: 重写 `src/index.css` 为原型一致的浅色主题**

保留所有现有 CSS class 名不变，将全部色值/圆角/间距替换为原型 `style.css` 中的浅色值。完整内容如下：

```css
/* ========== 基础重置与全局 ========== */
* { margin: 0; padding: 0; box-sizing: border-box; }

:root {
  --bg-page: #f0f0f0;
  --bg-card: #ffffff;
  --bg-sidebar: #fafafa;
  --bg-sidebar-hover: #eef3f8;
  --bg-sidebar-active: #d0e4f5;
  --bg-chat-header: #ffffff;
  --bg-message-self: #4a90d9;
  --bg-message-other: #eeeeee;
  --bg-input: #ffffff;
  --bg-admin-panel: #fafafa;
  --bg-selected: #ecf3fa;
  --bg-header: #4a90d9;
  --bg-thumbnail: #e8e8e8;

  --text-primary: #333333;
  --text-secondary: #888888;
  --text-light: #ffffff;
  --text-link: #4a90d9;

  --accent: #4a90d9;
  --accent-hover: #357abd;
  --accent-active: #2a6cb0;
  --danger: #e74c3c;
  --danger-hover: #c0392b;
  --success: #27ae60;

  --border-color: #e0e0e0;
  --border-strong: #cccccc;

  --sidebar-width: 280px;
  --header-height: 56px;
  --input-height: 60px;

  --radius-sm: 6px;
  --radius-md: 8px;
  --radius-lg: 12px;
}

html, body, #root { height: 100%; width: 100%; }

body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
  background: var(--bg-page);
  color: var(--text-primary);
  line-height: 1.5;
  overflow: hidden;
}

/* ========== 连接页 ========== */
.login-page {
  display: flex; align-items: center; justify-content: center;
  height: 100%; background: var(--bg-page);
}

.login-card {
  width: 420px; background: var(--bg-card); border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg); padding: 40px; box-shadow: 0 2px 12px rgba(0,0,0,0.06);
}

.login-card h1 { font-size: 24px; text-align: center; color: var(--accent); margin-bottom: 4px; }
.login-subtitle { text-align: center; color: var(--text-secondary); font-size: 14px; margin-bottom: 28px; }
.login-section { margin-bottom: 20px; }
.login-label { display: block; font-size: 13px; color: var(--text-secondary); margin-bottom: 8px; font-weight: 500; }

.backend-url-list { display: flex; flex-direction: column; gap: 8px; }
.backend-url-item {
  padding: 14px 16px; border: 2px solid var(--border-color); border-radius: 10px;
  cursor: pointer; transition: border-color 0.15s, background 0.15s;
}
.backend-url-item:hover { border-color: #b0cfe6; background: #f5f9fc; }
.backend-url-item.selected { border-color: var(--accent); background: var(--bg-selected); }

.backend-url-custom input[type="url"] {
  width: 100%; margin-top: 6px; padding: 8px 12px; border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm); font-size: 14px; outline: none;
  transition: border-color 0.15s; box-sizing: border-box;
}
.backend-url-custom input[type="url"]:focus { border-color: var(--accent); }

.backend-name { font-size: 16px; font-weight: 600; color: var(--text-primary); }
.backend-url { font-size: 13px; color: var(--text-secondary); margin-top: 2px; word-break: break-all; }

.backend-url-loading { cursor: default; opacity: 0.6; }
.backend-url-loading:hover { border-color: var(--border-color); background: transparent; }

.login-card input[type="password"] {
  width: 100%; padding: 12px 16px; border: 1px solid var(--border-strong); border-radius: var(--radius-md);
  font-size: 15px; outline: none; transition: border-color 0.15s;
}
.login-card input[type="password"]:focus { border-color: var(--accent); }

.login-error { color: var(--danger); font-size: 14px; text-align: center; margin-top: 12px; }

.connect-btn {
  width: 100%; padding: 12px; border: none; border-radius: var(--radius-md);
  background: var(--accent); color: var(--text-light); font-size: 16px; font-weight: 600;
  cursor: pointer; transition: background 0.15s;
}
.connect-btn:hover { background: var(--accent-hover); }
.connect-btn:active { background: var(--accent-active); }
.connect-btn:disabled { opacity: 0.5; cursor: default; }

/* ========== 主界面布局 ========== */
.chat-layout { display: flex; height: 100%; }

/* ========== 顶部标题栏 ========== */
.header {
  background: var(--bg-header); color: var(--text-light); padding: 12px 16px;
  display: flex; justify-content: space-between; align-items: center; position: relative;
}
.app-name { font-size: 16px; font-weight: 600; }

.admin-dropdown { position: relative; cursor: pointer; }
.admin-trigger { font-size: 14px; padding: 4px 8px; border-radius: 4px; }
.admin-trigger:hover { background: rgba(255,255,255,0.15); }

.dropdown-menu {
  display: none;
  position: absolute; top: 100%; right: 0; margin-top: 4px;
  background: var(--bg-card); border: 1px solid #ddd; border-radius: var(--radius-md);
  box-shadow: 0 4px 12px rgba(0,0,0,0.1); min-width: 160px; padding: 4px; z-index: 10;
}
.admin-dropdown:hover .dropdown-menu { display: block; }
.dropdown-item {
  padding: 10px 14px; color: var(--text-primary); font-size: 14px;
  border-radius: var(--radius-sm); cursor: pointer;
  display: flex; align-items: center; gap: 8px;
}
.dropdown-item:hover { background: #f0f0f0; }

/* ========== 侧边栏 - 会话列表 ========== */
.sidebar {
  width: var(--sidebar-width); min-width: var(--sidebar-width);
  background: var(--bg-sidebar); border-right: 1px solid #eee;
  display: flex; flex-direction: column; height: 100%;
}

.conversation-list { flex: 1; overflow-y: auto; padding: 8px; }

.conversation-item {
  display: flex; align-items: center; padding: 10px 12px;
  border-radius: var(--radius-md); cursor: pointer; margin-bottom: 2px;
  transition: background 0.15s;
}
.conversation-item:hover { background: var(--bg-sidebar-hover); }
.conversation-item.active { background: var(--bg-sidebar-active); }
.conversation-item.disabled { opacity: 0.5; cursor: default; }

.conversation-name {
  font-size: 14px; font-weight: 500; white-space: nowrap; overflow: hidden;
  text-overflow: ellipsis; flex: 1;
}
.group-tag {
  font-size: 11px; color: var(--text-secondary); margin-left: 4px; font-weight: 400;
}
.conversation-badge {
  margin-left: 8px; background: var(--accent); color: var(--text-light);
  font-size: 11px; padding: 1px 6px; border-radius: 10px; min-width: 18px;
  text-align: center; white-space: nowrap;
}
.conversation-badge:empty { display: none; }

/* ========== 右侧主内容 ========== */
.main-content { flex: 1; display: flex; flex-direction: column; height: 100%; overflow: hidden; }

.chat-header {
  padding: 12px 20px; border-bottom: 1px solid #eee;
  display: flex; align-items: center; height: var(--header-height);
  background: var(--bg-chat-header);
}
.chat-header h3 { font-size: 16px; font-weight: 500; }
.chat-header-meta { margin-left: auto; display: flex; align-items: center; gap: 8px; }

.thinking-indicator {
  color: var(--accent); font-size: 13px; animation: pulse 1.5s infinite;
}
@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.5; }
}

/* ========== 消息列表 ========== */
.message-list {
  flex: 1; overflow-y: auto; padding: 16px 20px;
  display: flex; flex-direction: column; gap: 8px;
}

.message-load-more {
  text-align: center; padding: 8px; color: var(--text-secondary);
  font-size: 13px; cursor: pointer;
}
.message-load-more:hover { color: var(--accent); }

.message { max-width: 80%; display: flex; flex-direction: column; }
.message.self { align-self: flex-end; align-items: flex-end; }
.message.other { align-self: flex-start; align-items: flex-start; }

.message-bubble {
  padding: 10px 14px; border-radius: var(--radius-lg);
  font-size: 14px; line-height: 1.5; word-break: break-word;
}
.message.self .message-bubble { background: var(--bg-message-self); color: var(--text-light); border-bottom-right-radius: 4px; }
.message.other .message-bubble { background: var(--bg-message-other); color: var(--text-primary); border-bottom-left-radius: 4px; }

.message-time {
  font-size: 11px; color: var(--text-secondary); margin-top: 4px; padding: 0 4px;
}

.message-content .image-attachment {
  max-width: 200px; max-height: 200px; border-radius: var(--radius-md);
  cursor: pointer; display: block; margin: 4px 0;
}

.message-content .file-attachment {
  display: flex; align-items: center; gap: 8px;
  padding: 8px 12px; background: var(--bg-thumbnail); border-radius: var(--radius-md);
  color: var(--text-link); text-decoration: none; font-size: 13px; margin: 4px 0;
}
.message-content .file-attachment:hover { background: #ddd; }

.file-attachment {
  display: inline-flex; align-items: center; gap: 6px;
  padding: 6px 12px; background: var(--bg-thumbnail); border-radius: var(--radius-md);
  color: var(--text-link); font-size: 13px; text-decoration: none; margin-top: 4px; cursor: pointer;
}
.file-attachment:hover { background: #ddd; }

.message-content .text-content { white-space: pre-wrap; }

/* 系统消息 */
.msg-system { text-align: center; margin: 8px 0; }
.msg-system-text {
  display: inline-block; font-size: 12px; color: var(--text-secondary);
  background: #f5f5f5; padding: 4px 12px; border-radius: 10px;
}

/* Multi 消息 */
.msg-multi { padding: 0; overflow: hidden; }
.multi-item { padding: 8px 14px; }
.multi-item + .multi-item { border-top: 1px solid #ddd; }
.multi-text { white-space: pre-wrap; }
.multi-thumb { max-width: 180px; max-height: 120px; border-radius: var(--radius-sm); display: block; cursor: pointer; }
.multi-attachment { padding: 6px 14px 10px; }

/* ========== 输入区域 ========== */
.input-area {
  padding: 10px 16px; border-top: 1px solid var(--border-color);
  background: var(--bg-card); display: flex; align-items: flex-end; gap: 8px;
}
.input-area.disabled { opacity: 0.5; }

.input-wrapper {
  flex: 1; display: flex; align-items: flex-end; gap: 8px;
  background: var(--bg-input); border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg); padding: 4px 12px;
}
.input-wrapper:focus-within { border-color: var(--accent); }

.input-wrapper input {
  flex: 1; border: none; background: none; color: var(--text-primary);
  font-size: 14px; outline: none; padding: 8px 0; line-height: 1.4;
}

.input-actions { display: flex; align-items: center; gap: 4px; }
.input-actions button {
  background: none; border: none; color: var(--text-secondary);
  cursor: pointer; padding: 6px; border-radius: var(--radius-sm);
  font-size: 18px; display: flex; align-items: center; justify-content: center;
  transition: color 0.15s, background 0.15s;
}
.input-actions button:hover { color: var(--accent); background: #f0f0f0; }

.send-button {
  padding: 8px 20px; border: none; border-radius: 10px;
  background: var(--accent); color: var(--text-light);
  font-size: 14px; font-weight: 600; cursor: pointer;
  transition: background 0.15s; white-space: nowrap;
}
.send-button:hover { background: var(--accent-hover); }
.send-button:disabled { opacity: 0.5; cursor: default; }

/* ========== 附件预览 ========== */
.attachment-preview {
  display: flex; flex-wrap: wrap; gap: 6px; padding: 8px 16px 0;
}
.attachment-preview-item {
  display: flex; align-items: center; gap: 4px;
  padding: 4px 8px; background: #f0f0f0; border-radius: var(--radius-sm);
  font-size: 12px;
}
.attachment-preview-item button {
  background: none; border: none; color: var(--danger); cursor: pointer;
  font-size: 14px; padding: 0 2px;
}

/* ========== 管理面板 ========== */
.admin-panel { flex: 1; padding: 20px; overflow-y: auto; }
.admin-panel-title {
  font-size: 18px; color: var(--text-primary); margin-bottom: 20px;
  display: flex; align-items: center; gap: 8px;
}
.back-btn {
  background: none; border: none; font-size: 18px; color: var(--accent);
  cursor: pointer; padding: 4px 8px; border-radius: 4px;
}
.back-btn:hover { background: #f0f0f0; }

.admin-panel-section {
  margin-bottom: 24px; background: var(--bg-admin-panel);
  border: 1px solid var(--border-color); border-radius: 10px; padding: 16px;
}
.admin-panel-section h4 {
  font-size: 14px; color: var(--text-secondary); margin-bottom: 12px; font-weight: 500;
}

.admin-panel-section input[type="text"],
.admin-panel-section select {
  width: 100%; padding: 8px 12px; border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm); font-size: 14px; margin-bottom: 8px;
  outline: none; background: var(--bg-card); color: var(--text-primary);
}
.admin-panel-section input[type="text"]:focus,
.admin-panel-section select:focus { border-color: var(--accent); }

.admin-select { cursor: pointer; }

.admin-panel-section button {
  padding: 8px 16px; border: none; border-radius: var(--radius-sm);
  background: var(--accent); color: var(--text-light); cursor: pointer;
  font-size: 13px; font-weight: 500; margin-right: 8px;
  transition: background 0.15s;
}
.admin-panel-section button:hover { background: var(--accent-hover); }
.admin-panel-section button.danger { background: var(--danger); }
.admin-panel-section button.danger:hover { background: var(--danger-hover); }

.hint-text { font-size: 12px; color: var(--text-secondary); margin-top: 8px; }

.admin-item {
  display: flex; align-items: center; justify-content: space-between;
  padding: 10px 12px; border-bottom: 1px solid #eee; font-size: 14px;
}
.admin-item:last-child { border-bottom: none; }
.admin-item.disabled { opacity: 0.5; }

.item-tag { font-size: 11px; color: var(--text-secondary); font-weight: 400; }
.disabled-note { font-size: 12px; color: #bbb; }

.admin-item-info { display: flex; flex-direction: column; gap: 2px; }
.admin-item-name { font-weight: 500; }
.admin-item-meta { font-size: 12px; color: var(--text-secondary); }

.admin-item-actions { display: flex; gap: 4px; flex-shrink: 0; }
.admin-item-actions button { padding: 4px 10px; font-size: 12px; }

.member-select { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 8px; }
.member-tag {
  padding: 5px 12px; background: #eee; border: 1px solid #ddd;
  border-radius: 16px; font-size: 13px; cursor: pointer; transition: background 0.1s;
}
.member-tag:hover { background: #e4e4e4; }
.member-tag.selected { background: var(--accent); color: var(--text-light); border-color: var(--accent); }

/* ========== 图片全屏预览 ========== */
.image-overlay {
  position: fixed; top: 0; left: 0; right: 0; bottom: 0;
  background: rgba(0,0,0,0.85); display: flex; align-items: center;
  justify-content: center; z-index: 100; cursor: pointer;
}
.image-overlay img { max-width: 90%; max-height: 90%; border-radius: var(--radius-md); }

/* ========== 滚动条 ========== */
::-webkit-scrollbar { width: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--border-color); border-radius: 3px; }
::-webkit-scrollbar-thumb:hover { background: var(--text-secondary); }
```

- [ ] **Step 2: 验证 `npm run build` 编译通过**

```bash
cd kissbot-channel-web-ui
npm run build 2>&1 | tail -5
```

预期：build 成功退出，无异常。

- [ ] **Step 3: 提交**

```bash
git add src/index.css
git commit -m "feat: index.css 重写为浅色主题对齐原型

- 全文件色值/间距/圆角替换为原型 style.css 的浅色体系
- 主色 #4a90d9，页面背景 #f0f0f0，卡片 #ffffff
- 侧边栏 #fafafa，选中态 #d0e4f5
- 消息气泡：admin 蓝底白字，对方灰底深字
- 保留 CSS 变量机制，class 名完全不变"
```

---

### Task 4: LoginPage 组件单元测试

**Files:**
- Create: `src/__tests__/LoginPage.test.tsx`

- [ ] **Step 1: 创建 `src/__tests__/LoginPage.test.tsx`**（mock `fetch` 和 `loadBackendConfig`，测试八大场景）

```tsx
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import LoginPage from '../components/LoginPage';

const mockOnConnect = vi.fn<[string, string], Promise<void>>();

function mockFetchBackends(backends: Array<{name: string; url: string}>) {
  vi.stubGlobal('fetch', () =>
    Promise.resolve({
      ok: true,
      json: () => Promise.resolve({ backends }),
    })
  );
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  mockOnConnect.mockReset();
});

describe('LoginPage', () => {
  it('渲染预置后端列表并默认选中第一项', async () => {
    mockFetchBackends([
      { name: 'EnvA', url: 'http://a.com' },
      { name: 'EnvB', url: 'http://b.com' },
    ]);
    render(<LoginPage onConnect={mockOnConnect} />);

    await waitFor(() => {
      expect(screen.getByText('EnvA')).toBeInTheDocument();
    });

    // 默认选中第一项
    const items = screen.getByText('EnvA').closest('.backend-url-item')!;
    expect(items.classList.contains('selected')).toBe(true);
  });

  it('点击预置项切换选中态', async () => {
    mockFetchBackends([
      { name: 'EnvA', url: 'http://a.com' },
      { name: 'EnvB', url: 'http://b.com' },
    ]);
    render(<LoginPage onConnect={mockOnConnect} />);

    await waitFor(() => {
      expect(screen.getByText('EnvA')).toBeInTheDocument();
    });

    const itemB = screen.getByText('EnvB').closest('.backend-url-item')!;
    fireEvent.click(itemB);
    expect(itemB.classList.contains('selected')).toBe(true);

    const itemA = screen.getByText('EnvA').closest('.backend-url-item')!;
    expect(itemA.classList.contains('selected')).toBe(false);
  });

  it('聚焦自定义 URL 取消预置选中并选中自定义', async () => {
    mockFetchBackends([
      { name: 'EnvA', url: 'http://a.com' },
    ]);
    render(<LoginPage onConnect={mockOnConnect} />);

    await waitFor(() => {
      expect(screen.getByText('EnvA')).toBeInTheDocument();
    });

    const customInput = screen.getByPlaceholderText(/自定义后端 URL/);
    fireEvent.focus(customInput);

    const customItem = customInput.closest('.backend-url-item')!;
    expect(customItem.classList.contains('selected')).toBe(true);

    const presetItem = screen.getByText('EnvA').closest('.backend-url-item')!;
    expect(presetItem.classList.contains('selected')).toBe(false);
  });

  it('输入自定义 URL 选中自定义项', async () => {
    mockFetchBackends([
      { name: 'EnvA', url: 'http://a.com' },
    ]);
    render(<LoginPage onConnect={mockOnConnect} />);

    await waitFor(() => {
      expect(screen.getByText('EnvA')).toBeInTheDocument();
    });

    const customInput = screen.getByPlaceholderText(/自定义后端 URL/);
    fireEvent.change(customInput, { target: { value: 'http://my.host:9999' } });

    const customItem = customInput.closest('.backend-url-item')!;
    expect(customItem.classList.contains('selected')).toBe(true);
  });

  it('连接传出预置后端的 URL', async () => {
    mockFetchBackends([
      { name: 'EnvA', url: 'http://a.com' },
      { name: 'EnvB', url: 'http://b.com/foo' },
    ]);
    mockOnConnect.mockResolvedValue();
    render(<LoginPage onConnect={mockOnConnect} />);

    await waitFor(() => {
      expect(screen.getByText('EnvA')).toBeInTheDocument();
    });

    // 点击 EnvB
    fireEvent.click(screen.getByText('EnvB').closest('.backend-url-item')!);

    // 输入 Key
    const keyInput = screen.getByPlaceholderText('输入 Admin API Key');
    fireEvent.change(keyInput, { target: { value: 'test-key' } });

    // 点击连接
    fireEvent.click(screen.getByText('连接'));
    await waitFor(() => {
      expect(mockOnConnect).toHaveBeenCalledWith('http://b.com/foo', 'test-key');
    });
  });

  it('自定义 URL 连接传出正确值', async () => {
    mockFetchBackends([
      { name: 'EnvA', url: 'http://a.com' },
    ]);
    mockOnConnect.mockResolvedValue();
    render(<LoginPage onConnect={mockOnConnect} />);

    await waitFor(() => {
      expect(screen.getByText('EnvA')).toBeInTheDocument();
    });

    const customInput = screen.getByPlaceholderText(/自定义后端 URL/);
    fireEvent.change(customInput, { target: { value: 'http://custom.host:8888' } });

    const keyInput = screen.getByPlaceholderText('输入 Admin API Key');
    fireEvent.change(keyInput, { target: { value: 'test-key' } });

    fireEvent.click(screen.getByText('连接'));
    await waitFor(() => {
      expect(mockOnConnect).toHaveBeenCalledWith('http://custom.host:8888', 'test-key');
    });
  });

  it('自定义 URL 为空时点连接显示错误', async () => {
    mockFetchBackends([]);  // 无预置，默认选中自定义
    render(<LoginPage onConnect={mockOnConnect} />);

    await waitFor(() => {
      expect(screen.getByPlaceholderText(/自定义后端 URL/)).toBeInTheDocument();
    });

    // 聚焦自定义（默认已选中无预置时）
    const customInput = screen.getByPlaceholderText(/自定义后端 URL/);
    fireEvent.focus(customInput);

    const keyInput = screen.getByPlaceholderText('输入 Admin API Key');
    fireEvent.change(keyInput, { target: { value: 'test-key' } });

    fireEvent.click(screen.getByText('连接'));
    await waitFor(() => {
      expect(screen.getByText('请输入后端 URL')).toBeInTheDocument();
    });
    expect(mockOnConnect).not.toHaveBeenCalled();
  });

  it('fetch 失败时降级为仅显示自定义项', async () => {
    vi.stubGlobal('fetch', () => Promise.reject(new Error('network')));
    render(<LoginPage onConnect={mockOnConnect} />);

    await waitFor(() => {
      expect(screen.getByPlaceholderText(/自定义后端 URL/)).toBeInTheDocument();
    });

    // 不应出现预置项
    expect(screen.queryByText('生产环境')).toBeNull();
  });
});
```

- [ ] **Step 2: 运行测试确认全部通过**

```bash
cd kissbot-channel-web-ui
npx vitest run src/__tests__/LoginPage.test.tsx --reporter=verbose
```

预期：8 个 test 全部 PASS。如果某项失败，检查 className 选择器与 LoginPage 渲染结果是否一致。

- [ ] **Step 3: 运行全部测试确认不影响已有测试**

```bash
npx vitest run --reporter=verbose
```

预期：backendConfig + LoginPage + 其他现有测试全部 PASS。

- [ ] **Step 4: 提交**

```bash
git add src/__tests__/LoginPage.test.tsx
git commit -m "test: 新增 LoginPage 组件单元测试八大场景

- 预置项渲染与默认选中、点击切换
- 自定义/预置互斥选中联动
- 连接传出预置 URL 和自定义 URL
- 自定义空值校验拦截
- fetch 失败降级为仅自定义"
```

---

### Task 5: 更新测试文档和 spec 约定

**Files:**
- Modify: `test/kissbot-channel-web-e2e-integration-test.md`
- Modify: `test/kissbot-channel-web-combined-integration-test.md`
- Create: `docs/spec/channel-web-ui.md`

- [ ] **Step 1: 修订 e2e 测试文档 `test/kissbot-channel-web-e2e-integration-test.md`**

修改 TC-01 步骤 4 和 TC-03 步骤 1 的"确认高亮"断言方式。将：

```
4. 验证 "测试环境" 选项有高亮边框
```

改为：

```markdown
4. 验证 "测试环境" 选项有高亮边框（前端校验选中样式的计算值）：
   - agent-browser 执行 `getComputedStyle` 检查选中项的 `border-color === 'rgb(74, 144, 217)'` 且 `background-color === 'rgb(236, 243, 250)'`
```

在 TC-03 后新增：

```markdown
### TC-03b：自定义 URL 登录

**前置**：TC-01 通过

**步骤**：
1. 聚焦自定义 URL 输入框
2. 输入 `http://localhost:8301`
3. 确认自定义项显示高亮边框（border-color === rgb(74, 144, 217)）
4. 输入 Admin Key `admin-key-123`
5. 点击"连接"

**预期**：成功进入聊天主界面

### TC-03c：占位 URL 登录失败

**前置**：TC-01 通过

**步骤**：
1. 点击"生产环境"选项（预置列表第一项）
2. 输入 Admin Key `admin-key-123`
3. 点击"连接"

**预期**：
- 页面显示错误提示（连接失败）
- 不进入聊天主界面

### TC-03d：自定义空 URL 登录拦截

**前置**：TC-01 通过

**步骤**：
1. 聚焦自定义 URL 输入框（不输入）
2. 确保预置项未选中
3. 输入 Admin Key `admin-key-123`
4. 点击"连接"

**预期**：页面显示错误提示"请输入后端 URL"
```

- [ ] **Step 2: 修订联合测试文档 `test/kissbot-channel-web-combined-integration-test.md`**

修改 TC-01 步骤 1 的"确认选中状态"为计算样式断言，同 e2e TC-01 的改动。

- [ ] **Step 3: 创建 `docs/spec/channel-web-ui.md`**（记录 backends.json 部署约定）

```markdown
# channel-web-ui 技术细节约定

## 预置后端配置（backends.json）

`kissbot-channel-web-ui` 使用 `public/backends.json` 文件存储预置后端列表，运行时通过 `fetch('/backends.json')` 加载，**不进入 JS bundle**。

### 文件格式

```json
{
  "backends": [
    { "name": "环境名称", "url": "https://api.example.com" }
  ]
}
```

- `name`（string）：显示在登录页的选项名称
- `url`（string）：后端 HTTP 地址，必须以 `http://` 或 `https://` 开头

### 部署替换

```bash
# 将生产环境的配置文件拷入构建产物覆盖
cp /path/to/production-backends.json dist/backends.json
```

- 替换后刷新页面即可生效，**无需重新构建**
- 至少保留一个条目，否则登录页降级为仅显示自定义输入
- 若不需要预置后端，可提供空数组 `{ "backends": [] }`
```

- [ ] **Step 4: 提交**

```bash
git add test/kissbot-channel-web-e2e-integration-test.md test/kissbot-channel-web-combined-integration-test.md docs/spec/channel-web-ui.md
git commit -m "docs: 更新测试文档和新增 channel-web-ui 部署约定

- e2e/联合测试登录用例改为计算样式断言，堵住 DOM 对视觉坏漏检
- 新增自定义 URL 登录、占位 URL 失败、空 URL 拦截三组 e2e 用例
- 新增 docs/spec/channel-web-ui.md 记录 backends.json 格式与部署替换约定"
```
