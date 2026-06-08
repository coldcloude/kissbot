import { fetchEventSource } from '@microsoft/fetch-event-source';
import { getApiKey } from './client';
import type { MessageData } from '../types';

type MessageCallback = (message: MessageData) => void;

class SSEService {
  private callbacks: MessageCallback[] = [];
  private connected = false;

  onMessage(callback: MessageCallback) {
    this.callbacks.push(callback);
  }

  removeCallback(callback: MessageCallback) {
    this.callbacks = this.callbacks.filter(cb => cb !== callback);
  }

  async connect() {
    if (this.connected) return;

    const apiKey = getApiKey();
    if (!apiKey) return;

    try {
      this.connected = true;
      await fetchEventSource('/api/events', {
        method: 'GET',
        headers: {
          'X-Api-Key': apiKey,
        },
        onopen: async () => {
          console.log('SSE connected');
        },
        onmessage: (event) => {
          try {
            const parsed = JSON.parse(event.data);
            if (parsed.type === 'message') {
              const messageData = parsed.data as MessageData;
              this.callbacks.forEach(cb => cb(messageData));
            }
          } catch (e) {
            console.error('Failed to parse SSE message', e);
          }
        },
        onerror: (error) => {
          console.error('SSE error:', error);
          this.connected = false;
          // 自动重连由库处理
        },
        onclose: () => {
          console.log('SSE connection closed');
          this.connected = false;
        },
      });
    } catch (e) {
      console.error('SSE connection failed:', e);
      this.connected = false;
    }
  }

  disconnect() {
    this.connected = false;
  }
}

export const sseService = new SSEService();
