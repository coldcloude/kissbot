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
