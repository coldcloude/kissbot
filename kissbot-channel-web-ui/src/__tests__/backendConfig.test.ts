import { describe, it, expect, beforeEach } from 'vitest';

function mockFetch(response: unknown, ok = true) {
  return async (_url: string) => ({
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
