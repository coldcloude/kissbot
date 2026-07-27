import { useState, useCallback } from 'react';

export function useUnreadCounts() {
  const [unreadCounts, setUnreadCounts] = useState<Map<string, number>>(new Map());

  const increment = useCallback((groupId: string) => {
    setUnreadCounts(prev => {
      const next = new Map(prev);
      next.set(groupId, (next.get(groupId) || 0) + 1);
      return next;
    });
  }, []);

  const clear = useCallback((groupId: string) => {
    setUnreadCounts(prev => {
      if (!prev.has(groupId)) return prev;
      const next = new Map(prev);
      next.set(groupId, 0);
      return next;
    });
  }, []);

  const get = useCallback((groupId: string): number => {
    return unreadCounts.get(groupId) || 0;
  }, [unreadCounts]);

  return { unreadCounts, increment, clear, get };
}
