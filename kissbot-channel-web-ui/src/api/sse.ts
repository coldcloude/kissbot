import { fetchEventSource } from '@microsoft/fetch-event-source';
import { getApiKey, getApiBase } from './client';
import type { IncomingMessage } from '../types';

type MessageCallback = (message: IncomingMessage) => void;

class SSEService {
  private callbacks: MessageCallback[] = [];
  private abortController: AbortController | null = null;
  private connected = false;

  onMessage(callback: MessageCallback) {
    this.callbacks.push(callback);
  }

  removeCallback(callback: MessageCallback) {
    this.callbacks = this.callbacks.filter(cb => cb !== callback);
  }

  async connect() {
    if (this.connected) return;
    this.disconnect(); // 清理旧连接

    const apiKey = getApiKey();
    const apiBase = getApiBase();
    if (!apiKey || !apiBase) return;

    this.abortController = new AbortController();
    this.connected = true;

    try {
      await fetchEventSource(`${apiBase}/api/events`, {
        method: 'GET',
        headers: { 'X-Api-Key': apiKey },
        signal: this.abortController.signal,
        onmessage: (event) => {
          try {
            // 后端直接推送 IncomingMessage JSON
            const message = JSON.parse(event.data) as IncomingMessage;
            this.callbacks.forEach(cb => cb(message));
          } catch {
            // 忽略解析失败的 event（如 keep-alive）
          }
        },
        onerror: () => {
          this.connected = false;
          // fetch-event-source 会自动重连
        },
        onclose: () => {
          this.connected = false;
        },
      });
    } catch {
      this.connected = false;
    }
  }

  disconnect() {
    if (this.abortController) {
      this.abortController.abort();
      this.abortController = null;
    }
    this.connected = false;
  }
}

export const sseService = new SSEService();
