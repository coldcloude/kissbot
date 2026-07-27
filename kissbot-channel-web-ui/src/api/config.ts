import type { BackendUrlOption } from '../types';

export const DEFAULT_BACKEND_URLS: BackendUrlOption[] = [
  { name: '生产环境', url: 'https://api.kissbot.example.com' },
  { name: '测试环境', url: 'http://localhost:8301' },
  { name: '开发环境', url: 'http://192.168.1.100:8301' },
];
