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
