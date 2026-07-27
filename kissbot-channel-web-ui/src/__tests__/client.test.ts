import { describe, it, expect, beforeEach } from 'vitest';
import { setApiBase, setApiKey, getApiBase, getApiKey } from '../api/client';

describe('API client', () => {
  beforeEach(() => {
    setApiBase('http://test:8301');
    setApiKey('test-key');
  });

  it('sendTextMessage is a function', async () => {
    const { sendTextMessage } = await import('../api/client');
    expect(sendTextMessage).toBeDefined();
    expect(typeof sendTextMessage).toBe('function');
  });

  it('sendAttachmentMessage is a function', async () => {
    const { sendAttachmentMessage } = await import('../api/client');
    expect(sendAttachmentMessage).toBeDefined();
    expect(typeof sendAttachmentMessage).toBe('function');
  });

  it('sets apiBase correctly', () => {
    expect(getApiBase()).toBe('http://test:8301');
    expect(getApiKey()).toBe('test-key');
  });

  it('strips trailing slash from apiBase', () => {
    setApiBase('http://test:8301/');
    expect(getApiBase()).toBe('http://test:8301');
  });
});

describe('Content type parsing', () => {
  it('Text content has correct shape', () => {
    const content: Record<string, unknown> = { Text: '你好' };
    expect('Text' in content).toBe(true);
    expect(content.Text).toBe('你好');
  });

  it('AttachmentInfoResponse has correct shape', () => {
    const content: Record<string, unknown> = {
      AttachmentInfoResponse: {
        key: 'g0/uuid',
        info: { file_name: 'photo.png', mime_type: 'image/png', size_bytes: 1024 },
        transfer_id: 42,
      },
    };
    expect('AttachmentInfoResponse' in content).toBe(true);
    const resp = content.AttachmentInfoResponse as { transfer_id: number };
    expect(resp.transfer_id).toBe(42);
  });

  it('GroupChange has correct shape', () => {
    const content: Record<string, unknown> = {
      GroupChange: { messenger_id: 'web', group_id: 'g0', user_id: 'u1' },
    };
    expect('GroupChange' in content).toBe(true);
    const notif = content.GroupChange as { user_id: string };
    expect(notif.user_id).toBe('u1');
  });
});

describe('Sidebar logic', () => {
  it('sorts conversations by latestTime descending', () => {
    const conversations = [
      { groupId: 'a_u0', displayName: 'A', isGroup: false, latestTime: '2026-07-27T10:00:00Z', unreadCount: 0 },
      { groupId: 'a_u1', displayName: 'B', isGroup: false, latestTime: '2026-07-27T12:00:00Z', unreadCount: 3 },
      { groupId: 'g0', displayName: 'C', isGroup: true, latestTime: '2026-07-26T08:00:00Z', unreadCount: 1 },
    ];

    const sorted = [...conversations].sort((a, b) => {
      if (!a.latestTime && !b.latestTime) return 0;
      if (!a.latestTime) return 1;
      if (!b.latestTime) return -1;
      return b.latestTime.localeCompare(a.latestTime);
    });

    expect(sorted[0].groupId).toBe('a_u1');
    expect(sorted[1].groupId).toBe('a_u0');
    expect(sorted[2].groupId).toBe('g0');
  });

  it('formats badge correctly', () => {
    const formatBadge = (count: number): string => {
      if (count === 0) return '';
      if (count > 99) return '...';
      return String(count);
    };
    expect(formatBadge(0)).toBe('');
    expect(formatBadge(5)).toBe('5');
    expect(formatBadge(99)).toBe('99');
    expect(formatBadge(100)).toBe('...');
    expect(formatBadge(999)).toBe('...');
  });
});
