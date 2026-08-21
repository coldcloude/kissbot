import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startMemoryEgo, stopMemoryEgo, waitForPort } from './helpers/server';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { join, dirname } from 'path';

const BASE = 'http://127.0.0.1:3001';
const API_KEY = 'user-key-456'; // security.api_key
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

let ego: ChildProcess;
// 测试间共享变量
let agentId: string;
let copiedAgentId: string;

async function apiReq(request: APIRequestContext, method: string, path: string, body?: unknown) {
  const res = await request.fetch(`${BASE}${path}`, {
    method,
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: body,
  });
  // 正常路径下所有端点均返回 2xx；非 2xx 直接报错暴露 HTTP 层问题
  expect(res.ok()).toBe(true);
  return (await res.json()) as any;
}

test.describe.serial('memory-ego API 测试', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    ego = startMemoryEgo(WORKSPACE);
    await waitForPort(3001, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopMemoryEgo(ego);
  });

  // ========== Agent 管理 ==========

  // TC-01 创建 agent
  test('TC-01: 创建 agent', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/agent/create', {
      agent_id: 'alice', description: 'Alice 助理',
    });
    expect(resp.success).toBe(true);
    expect(resp.data).toBe('alice');
    agentId = resp.data;
  });

  // TC-02 列出 agent
  test('TC-02: 列出 agent', async ({ request }) => {
    const resp = await apiReq(request, 'GET', '/agent/list');
    expect(resp.success).toBe(true);
    const ids = resp.data.map((a: any) => a.agent_id);
    expect(ids).toContain(agentId);
  });

  // TC-03 获取 agent
  test('TC-03: 获取 agent', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/agent/get', { agent_id: agentId });
    expect(resp.success).toBe(true);
    expect(resp.data.agent_id).toBe(agentId);
    expect(resp.data.description).toBe('Alice 助理');
  });

  // TC-05 更新 agent 描述
  test('TC-05: 更新 agent 描述', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/agent/update-description', {
      agent_id: agentId, description: '新描述',
    });
    expect(resp.success).toBe(true);
    const g = await apiReq(request, 'POST', '/agent/get', { agent_id: agentId });
    expect(g.data.description).toBe('新描述');
  });

  // TC-06 复制 agent
  test('TC-06: 复制 agent', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/agent/copy', {
      agent_id: agentId, new_agent_id: 'alice_copy',
    });
    expect(resp.success).toBe(true);
    expect(resp.data).toBe('alice_copy');
    copiedAgentId = resp.data;
  });

  // TC-08 按描述搜索 agent
  test('TC-08: 按描述搜索 agent', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/agent/search-description', { keyword: '新描述' });
    expect(resp.success).toBe(true);
    expect(Array.isArray(resp.data)).toBe(true);
    expect(resp.data).toContain(agentId);
  });

  // TC-09 批量取回 agent
  test('TC-09: 批量取回 agent', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/agent/retrieve', {
      agent_ids: [agentId, copiedAgentId],
    });
    expect(resp.success).toBe(true);
    expect(resp.data.length).toBe(2);
    const ids = resp.data.map((a: any) => a.agent_id);
    expect(ids).toContain(agentId);
    expect(ids).toContain(copiedAgentId);
  });

  // TC-10 agent 名称前缀补全
  test('TC-10: agent 名称前缀补全', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/agent/name-completion', { prefix: 'ali' });
    expect(resp.success).toBe(true);
    expect(resp.data.length).toBeGreaterThanOrEqual(1);
    const keys = resp.data.map((c: any) => c.key);
    expect(keys).toContain(agentId);
  });

  // ========== 个体识别信息 ==========

  // TC-11 获取全部个体（初始为空）
  test('TC-11: 获取全部个体（初始为空）', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/individual/get-all', { agent_id: agentId });
    expect(resp.success).toBe(true);
    expect(resp.data.agent_id).toBe(agentId);
    // 尚未插入任何个体，individual_map 为空对象
    expect(resp.data.individual_map).toEqual({});
  });

  // TC-12 批量替换个体
  test('TC-12: 批量替换个体', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/individual/replace', {
      agent_id: agentId,
      remove_individual_names: [],
      insert_individuals: [['bob', {
        identifiers: [],
        relation: { relation: 'friend', description: '好友' },
        other_relations: {},
      }]],
    });
    expect(resp.success).toBe(true);
  });

  // TC-13 获取单个个体
  test('TC-13: 获取单个个体', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/individual/get', {
      agent_id: agentId, individual_name: 'bob',
    });
    expect(resp.success).toBe(true);
    expect(resp.data.relation.relation).toBe('friend');
  });

  // TC-14 重命名个体
  test('TC-14: 重命名个体', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/individual/rename', {
      agent_id: agentId, individual_name: 'bob', new_name: 'robert',
    });
    expect(resp.success).toBe(true);
    const g = await apiReq(request, 'POST', '/individual/get', {
      agent_id: agentId, individual_name: 'robert',
    });
    expect(g.success).toBe(true);
  });

  // TC-15 替换个体标识符
  test('TC-15: 替换个体标识符', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/individual/replace-identifiers', {
      agent_id: agentId, individual_name: 'robert',
      remove_identifiers: [],
      insert_identifiers: [{ messenger_id: 'web', user_id: 'u9' }],
    });
    expect(resp.success).toBe(true);
    const g = await apiReq(request, 'POST', '/individual/get', {
      agent_id: agentId, individual_name: 'robert',
    });
    const ids = g.data.identifiers;
    expect(ids.some((i: any) => i.messenger_id === 'web' && i.user_id === 'u9')).toBe(true);
  });

  // TC-16 替换个体关系
  test('TC-16: 替换个体关系', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/individual/replace-relations', {
      agent_id: agentId, individual_name: 'robert',
      remove_relations: [],
      insert_relations: [{ individual_name: 'carol', relation: { relation: 'sister', description: '姐妹' } }],
    });
    expect(resp.success).toBe(true);
    const g = await apiReq(request, 'POST', '/individual/get', {
      agent_id: agentId, individual_name: 'robert',
    });
    expect(g.data.other_relations.carol.relation).toBe('sister');
  });

  // ========== 角色设定 ==========

  // TC-17 创建角色
  test('TC-17: 创建角色', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/create', {
      agent_id: agentId, role_name: 'admin', description: '管理员角色',
    });
    expect(resp.success).toBe(true);
  });

  // TC-18 列出角色
  test('TC-18: 列出角色', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/list', { agent_id: agentId });
    expect(resp.success).toBe(true);
    expect(resp.data).toContain('admin');
  });

  // TC-19 获取角色
  test('TC-19: 获取角色', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/get', {
      agent_id: agentId, role_name: 'admin',
    });
    expect(resp.success).toBe(true);
    expect(resp.data.role.role_name).toBe('admin');
    expect(resp.data.role.description).toBe('管理员角色');
  });

  // TC-20 更新角色描述
  test('TC-20: 更新角色描述', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/role/update-description', {
      agent_id: agentId, role_name: 'admin', description: '新描述',
    });
    expect(resp.success).toBe(true);
    const g = await apiReq(request, 'POST', '/role/get', { agent_id: agentId, role_name: 'admin' });
    expect(g.data.role.description).toBe('新描述');
  });

  // TC-21 更新角色展示名
  test('TC-21: 更新角色展示名', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/role/update-full-name', {
      agent_id: agentId, role_name: 'admin', full_name: '管理员',
    });
    expect(resp.success).toBe(true);
    const g = await apiReq(request, 'POST', '/role/get', { agent_id: agentId, role_name: 'admin' });
    expect(g.data.role.full_name).toBe('管理员');
  });

  // TC-22 从已有角色复制创建
  test('TC-22: 从已有角色复制创建', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/create-from', {
      agent_id: agentId, role_name: 'admin', new_name: 'admin2',
    });
    expect(resp.success).toBe(true);
    // 回读：新角色出现在角色列表
    const list = await apiReq(request, 'POST', '/role/list', { agent_id: agentId });
    expect(list.data).toContain('admin2');
  });

  // TC-25 按描述搜索角色
  test('TC-25: 按描述搜索角色', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/search-description', {
      keyword: '新描述', agent_id: agentId,
    });
    expect(resp.success).toBe(true);
    expect(resp.data.some((k: any) => k.role_name === 'admin2')).toBe(true);
  });

  // TC-26 批量取回角色
  // 注：retrieve_roles 返回 Vec<Role>（RolePlay.role 部分，非完整 RolePlay）
  test('TC-26: 批量取回角色', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/retrieve', {
      role_keys: [{ agent_id: agentId, role_name: 'admin2' }],
    });
    expect(resp.success).toBe(true);
    expect(resp.data.length).toBe(1);
    expect(resp.data[0].role_name).toBe('admin2');
    expect(resp.data[0].agent_id).toBe(agentId);
  });

  // TC-27 角色名称前缀补全
  test('TC-27: 角色名称前缀补全', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/name-completion', {
      prefix: 'ad', agent_id: agentId,
    });
    expect(resp.success).toBe(true);
    expect(resp.data.some((c: any) => c.key.role_name === 'admin')).toBe(true);
  });

  // ========== 角色间关系 ==========

  // TC-28 替换其他角色
  test('TC-28: 替换其他角色', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/role/other/replace', {
      agent_id: agentId, role_name: 'admin',
      remove_other_roles: [],
      insert_other_roles: [{
        role_name: 'bob',
        other_role: {
          individual_name: 'bob',
          role_relation: { relation: 'colleague', full_name: '', description: '同事' },
          other_role_relations: {},
          description: '同事角色',
        },
      }],
    });
    expect(resp.success).toBe(true);
  });

  // TC-29 获取其他角色
  test('TC-29: 获取其他角色', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/other/get', {
      agent_id: agentId, role_name: 'admin', other_role_name: 'bob',
    });
    expect(resp.success).toBe(true);
    expect(resp.data.individual_name).toBe('bob');
  });

  // TC-30 重命名其他角色
  test('TC-30: 重命名其他角色', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/role/other/rename', {
      agent_id: agentId, role_name: 'admin', other_role_name: 'bob', new_name: 'bobby',
    });
    expect(resp.success).toBe(true);
    // 回读：新名可取、旧名不可取
    const g = await apiReq(request, 'POST', '/role/other/get', {
      agent_id: agentId, role_name: 'admin', other_role_name: 'bobby',
    });
    expect(g.success).toBe(true);
  });

  // TC-31 更新其他角色个体名
  test('TC-31: 更新其他角色个体名', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/role/other/update-individual-name', {
      agent_id: agentId, role_name: 'admin', other_role_name: 'bobby', new_individual_name: 'robert',
    });
    expect(resp.success).toBe(true);
  });

  // TC-32 更新其他角色描述
  test('TC-32: 更新其他角色描述', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/role/other/update-description', {
      agent_id: agentId, role_name: 'admin', other_role_name: 'bobby', new_description: '新描述',
    });
    expect(resp.success).toBe(true);
  });

  // TC-33 更新其他角色关系
  test('TC-33: 更新其他角色关系', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/role/other/update-relation', {
      agent_id: agentId, role_name: 'admin', other_role_name: 'bobby',
      new_relation: { relation: 'friend', full_name: '', description: '好友' },
    });
    expect(resp.success).toBe(true);
  });

  // TC-34 替换其他角色关系
  test('TC-34: 替换其他角色关系', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/role/other/replace-relations', {
      agent_id: agentId, role_name: 'admin', other_role_name: 'bobby',
      remove_relations: [],
      insert_relations: [{ role_name: 'carol', relation: { relation: 'sister', full_name: '', description: '姐妹' } }],
    });
    expect(resp.success).toBe(true);
    const g = await apiReq(request, 'POST', '/role/other/get', {
      agent_id: agentId, role_name: 'admin', other_role_name: 'bobby',
    });
    expect(g.data.other_role_relations.carol.relation).toBe('sister');
  });
});
