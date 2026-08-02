# memory API 测试与脚本调整 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 memory-store / memory-ego 增加 HTTP API 测试（只测正常情况），并调整数据目录布局与 script 启动脚本。

**Architecture:** 测试复用现有 `channel-web-api.spec.ts` 模式（Playwright + 直接启动 debug 二进制 + `X-Api-Key` 认证），新增两个 spec 文件。数据目录改为 channel 数据进 `workspace/channel-data/`、memory 数据进 `memory-data/`。脚本命名改为 `start-channel-web` / `start-channel-web-ui`，组合启动改为 `restart-channel-all` / `restart-memory-all`（含清理数据步骤）。

**Tech Stack:** Playwright（TypeScript）、Rust（axum + kissbot-security）、bash 脚本、serde_json。

## Global Constraints

- 认证头：memory 组件用 `X-Api-Key: user-key-456`（配置 `security.api_key`；channel-web 用 `admin-key-123` 不变）。
- memory-store 端口 8082（`memory.store.listen_port`）、memory-ego 端口 3001（`memory.ego.listen_port`）。
- `role_name` / `individual_name` 仅允许 `^[A-Za-z0-9_]+$`（见 `kissbot-memory-ego/src/code.rs`），测试数据只能用 ASCII 代号。
- 追加记忆请求一律带 `force: 1`（避免乱序拒绝）。
- 追加后等待 ≥1s 再查询（memory-store 的 FileObjectAppender 有 100ms 批量落盘延迟）。
- 查询时间范围用 `"YYYY-MM-DD 00:00:00"` ~ `"YYYY-MM-DD 23:59:59"`（当天，`kai_date::get_date_time_segments` 同日返回单段）。
- 不要删除代码中的注释（项目 CLAUDE.md 要求）。
- 文本文件 UTF-8、`\n` 换行。
- 提交 comment 用中文，且包含本次提交的所有改动内容。

---

### Task 1: 数据目录调整（channel-data / memory-data）

**Files:**
- Modify: `script/config.json`
- Modify: `test/workspace-template/config.json`
- Move: `script/template/channel-web-repo.json` → `script/template/channel-data/channel-web-repo.json`
- Move: `test/workspace-template/channel-web-repo.json` → `test/workspace-template/channel-data/channel-web-repo.json`
- Modify: `script/reset-workspace.sh`
- Verify: `test/tests/helpers/server.ts`（resetWorkspace 无需改代码，整体 `cp -r template → workspace` 已自动带上 channel-data；运行测试验证）

**Interfaces:**
- Consumes: 现有 `workspace-template/config.json` 结构、`template/channel-web-repo.json`
- Produces: 新的配置路径约定（channel-web 数据在 `<cwd>/channel-data/`、memory 数据在 `<cwd>/memory-data/`），后续 Task 2/3/4/5 依赖此路径

- [ ] **Step 1: 移动模板文件**

```bash
cd /home/admin/project/kissbot
mkdir -p script/template/channel-data test/workspace-template/channel-data
git mv script/template/channel-web-repo.json script/template/channel-data/channel-web-repo.json
git mv test/workspace-template/channel-web-repo.json test/workspace-template/channel-data/channel-web-repo.json
```

- [ ] **Step 2: 修改 `script/config.json` 的 channel-web 路径与 memory.root_dir**

`script/config.json` 中：
- `channel-web` 段：`messenger_repo` → `"../workspace/channel-data/channel-web-repo.json"`、`attachment_dir` → `"../workspace/channel-data/attachments"`、`message_dir` → `"../workspace/channel-data/messages"`
- `memory` 段：`root_dir` → `"memory-data"`

用 Edit 工具精确修改（禁止 sed/python 改文件）：

```json
  "memory": {
    "root_dir": "memory-data",
    "store": {
      "listen_addr": "127.0.0.1",
      "listen_port": 8082
    },
    "ego": {
      "listen_addr": "127.0.0.1",
      "listen_port": 3001
    }
  },
```

```json
  "channel-web": {
    "messenger_id": "web",
    "admin_name": "管理员",
    "messenger_repo": "../workspace/channel-data/channel-web-repo.json",
    "attachment_dir": "../workspace/channel-data/attachments",
    "message_dir": "../workspace/channel-data/messages",
    "ws_listen_addr": "127.0.0.1:8201",
    "http_listen_addr": "127.0.0.1:8301"
  },
```

- [ ] **Step 3: 修改 `test/workspace-template/config.json`（同样两处）**

```json
  "memory": {
    "root_dir": "memory-data",
    "store": {
      "listen_addr": "127.0.0.1",
      "listen_port": 8082
    },
    "ego": {
      "listen_addr": "127.0.0.1",
      "listen_port": 3001
    }
  },
```

```json
  "channel-web": {
    "messenger_id": "web",
    "admin_name": "管理员",
    "messenger_repo": "channel-data/channel-web-repo.json",
    "attachment_dir": "channel-data/attachments",
    "message_dir": "channel-data/messages",
    "ws_listen_addr": "127.0.0.1:8201",
    "http_listen_addr": "127.0.0.1:8301"
  },
```

- [ ] **Step 4: 更新 `script/reset-workspace.sh`（连带重置 channel-data）**

完整替换文件内容：

```bash
#!/bin/bash
# 重置 workspace：删除并重建，连带重置 channel-data
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_DIR="$(dirname "$SCRIPT_DIR")/workspace"

echo "==> 重置 workspace ($WORKSPACE_DIR)..."

rm -rf "$WORKSPACE_DIR"
mkdir -p "$WORKSPACE_DIR/channel-data"
cp "$SCRIPT_DIR/template/channel-data/channel-web-repo.json" "$WORKSPACE_DIR/channel-data/"

echo "==> workspace 已就绪"
```

- [ ] **Step 5: 验证 channel-web 相关测试不受影响**

```bash
cd /home/admin/project/kissbot/test
npx playwright test tests/channel-web-api.spec.ts
```

Expected: 全部通过（channel-web 会自动创建 `channel-data/attachments`、`channel-data/messages` 目录）。

- [ ] **Step 6: 提交**

```bash
cd /home/admin/project/kissbot
git add script/config.json test/workspace-template/config.json script/template test/workspace-template script/reset-workspace.sh
git commit -m "chore: channel 数据目录改为 workspace/channel-data，memory 数据目录改为 memory-data，reset-workspace 连带重置 channel-data"
```

---

### Task 2: 启动脚本调整

**Files:**
- Rename: `script/start-backend.sh` → `script/start-channel-web.sh`（注释同步更新）
- Rename: `script/start-frontend.sh` → `script/start-channel-web-ui.sh`（注释同步更新）
- Delete: `script/restart-all.sh`
- Create: `script/start-memory-store.sh`
- Create: `script/start-memory-ego.sh`
- Create: `script/restart-channel-all.sh`
- Create: `script/restart-memory-all.sh`

**Interfaces:**
- Consumes: Task 1 的路径约定（`reset-workspace.sh` 重置 channel-data、`memory-data` 目录）
- Produces: 可手动执行的启动/重启脚本

- [ ] **Step 1: 重命名 start-backend.sh → start-channel-web.sh 并更新注释**

```bash
cd /home/admin/project/kissbot
git mv script/start-backend.sh script/start-channel-web.sh
```

修改文件内注释（第 1、4 行）为：

```bash
#!/bin/bash
# 启动 kissbot-channel-web 后端服务（debug 模式，cargo run）
...
echo "==> 启动 channel-web 后端 (debug)..."
```

- [ ] **Step 2: 重命名 start-frontend.sh → start-channel-web-ui.sh 并更新注释**

```bash
cd /home/admin/project/kissbot
git mv script/start-frontend.sh script/start-channel-web-ui.sh
```

修改文件内注释（第 1 行）为 `# 启动 kissbot-channel-web-ui 前端 dev server`，其余内容不变。

- [ ] **Step 3: 删除 restart-all.sh**

```bash
cd /home/admin/project/kissbot
git rm script/restart-all.sh
```

- [ ] **Step 4: 新建 `script/start-memory-store.sh`**

```bash
#!/bin/bash
# 启动 kissbot-memory-store 服务（debug 模式，cargo run）
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> 清理旧进程..."
kill $(ps aux | grep "kissbot-memory-store" | grep -v grep | awk '{print $2}') 2>/dev/null
sleep 1

echo "==> 启动 memory-store (debug)..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-store/Cargo.toml"
```

- [ ] **Step 5: 新建 `script/start-memory-ego.sh`**

```bash
#!/bin/bash
# 启动 kissbot-memory-ego 服务（debug 模式，cargo run）
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> 清理旧进程..."
kill $(ps aux | grep "kissbot-memory-ego" | grep -v grep | awk '{print $2}') 2>/dev/null
sleep 1

echo "==> 启动 memory-ego (debug)..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-ego/Cargo.toml"
```

- [ ] **Step 6: 新建 `script/restart-channel-all.sh`**

```bash
#!/bin/bash
# 重启 channel 服务：reset-workspace（连带重置 channel-data）→ 编译 debug → 启动 channel-web + channel-web-ui
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "========================================"
echo "  Kissbot Channel — 重启全部"
echo "========================================"

# 清理旧进程
kill $(ps aux | grep "kissbot-channel-web" | grep -v grep | awk '{print $2}') 2>/dev/null
kill $(ps aux | grep "vite" | grep -v grep | awk '{print $2}') 2>/dev/null

# 重置 workspace（连带重置 channel-data）
echo ""
echo "[1/3] 重置 workspace (连带 channel-data)..."
bash "$SCRIPT_DIR/reset-workspace.sh"

# 启动 channel-web 后端
echo "[2/3] 启动 channel-web (http://127.0.0.1:8301)..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-channel-web/Cargo.toml" > /tmp/kissbot-channel-web.log 2>&1 &
BACKEND_PID=$!
echo "   PID: $BACKEND_PID"
sleep 4

# 验证后端
curl -s --max-time 3 -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info > /dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "   ✅ channel-web 运行正常"
else
    tail -5 /tmp/kissbot-channel-web.log
    echo "   ❌ channel-web 启动失败，查看 /tmp/kissbot-channel-web.log"
    exit 1
fi

# 启动 channel-web-ui 前端
echo "[3/3] 启动 channel-web-ui (http://localhost:5173)..."
cd "$PROJECT_DIR/kissbot-channel-web-ui"
npm install 2>&1 | tail -1
npm run dev > /tmp/kissbot-channel-web-ui.log 2>&1 &
FRONTEND_PID=$!
echo "   PID: $FRONTEND_PID"
sleep 3

grep -q "ready in" /tmp/kissbot-channel-web-ui.log 2>/dev/null
if [ $? -eq 0 ]; then
    echo "   ✅ channel-web-ui 运行正常"
else
    echo "   ⚠️ channel-web-ui 可能未完全启动，查看 /tmp/kissbot-channel-web-ui.log"
fi

echo ""
echo "========================================"
echo "  channel-web:   http://127.0.0.1:8301"
echo "  channel-web-ui: http://localhost:5173"
echo "  CLI:           ./start-cli.sh <user_id> <group_id>"
echo "  Admin Key:     admin-key-123"
echo "========================================"
echo ""
echo "按 Ctrl+C 停止所有服务"

trap "echo '停止服务...'; kill $BACKEND_PID $FRONTEND_PID 2>/dev/null; exit 0" SIGINT SIGTERM
wait
```

- [ ] **Step 7: 新建 `script/restart-memory-all.sh`**

```bash
#!/bin/bash
# 重启 memory 服务：清理 memory-data → 编译 debug → 启动 memory-store + memory-ego
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "========================================"
echo "  Kissbot Memory — 重启全部"
echo "========================================"

# 清理旧进程
kill $(ps aux | grep "kissbot-memory-store" | grep -v grep | awk '{print $2}') 2>/dev/null
kill $(ps aux | grep "kissbot-memory-ego" | grep -v grep | awk '{print $2}') 2>/dev/null

# 清理 memory 数据目录
echo ""
echo "[1/3] 清理 memory-data..."
rm -rf "$SCRIPT_DIR/memory-data"
mkdir -p "$SCRIPT_DIR/memory-data"

# 启动 memory-store
echo "[2/3] 启动 memory-store (http://127.0.0.1:8082)..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-store/Cargo.toml" > /tmp/kissbot-memory-store.log 2>&1 &
STORE_PID=$!
echo "   PID: $STORE_PID"
sleep 4

# 验证 memory-store（TCP 端口检查）
timeout 2 bash -c "echo > /dev/tcp/127.0.0.1/8082" 2>/dev/null
if [ $? -eq 0 ]; then
    echo "   ✅ memory-store 运行正常"
else
    tail -5 /tmp/kissbot-memory-store.log
    echo "   ❌ memory-store 启动失败，查看 /tmp/kissbot-memory-store.log"
    exit 1
fi

# 启动 memory-ego
echo "[3/3] 启动 memory-ego (http://127.0.0.1:3001)..."
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-ego/Cargo.toml" > /tmp/kissbot-memory-ego.log 2>&1 &
EGO_PID=$!
echo "   PID: $EGO_PID"
sleep 4

# 验证 memory-ego（调用 /agent/list）
curl -s --max-time 3 -X POST -H "X-Api-Key: user-key-456" -H "Content-Type: application/json" http://127.0.0.1:3001/agent/list > /dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "   ✅ memory-ego 运行正常"
else
    tail -5 /tmp/kissbot-memory-ego.log
    echo "   ❌ memory-ego 启动失败，查看 /tmp/kissbot-memory-ego.log"
    exit 1
fi

echo ""
echo "========================================"
echo "  memory-store: http://127.0.0.1:8082"
echo "  memory-ego:   http://127.0.0.1:3001"
echo "  Api Key:      user-key-456"
echo "========================================"
echo ""
echo "按 Ctrl+C 停止所有服务"

trap "echo '停止服务...'; kill $STORE_PID $EGO_PID 2>/dev/null; exit 0" SIGINT SIGTERM
wait
```

- [ ] **Step 8: 语法检查**

```bash
cd /home/admin/project/kissbot
for f in script/*.sh; do bash -n "$f" || echo "FAIL: $f"; done
echo "syntax ok"
```

Expected: 无 FAIL 输出。

- [ ] **Step 9: 提交**

```bash
cd /home/admin/project/kissbot
git add -A script/
git commit -m "chore: 脚本调整——start-backend/start-frontend 改名 start-channel-web/start-channel-web-ui，删除 restart-all，新增 start-memory-store/start-memory-ego 与 restart-channel-all/restart-memory-all（含清理数据目录步骤）"
```

---

### Task 3: 测试基础设施（helpers / global-setup / global-teardown）

**Files:**
- Modify: `test/tests/helpers/server.ts`
- Modify: `test/global-setup.ts`
- Modify: `test/global-teardown.ts`

**Interfaces:**
- Consumes: Task 1 的数据目录约定
- Produces: `startMemoryStore(cwd): ChildProcess`、`stopMemoryStore(proc?)`、`startMemoryEgo(cwd): ChildProcess`、`stopMemoryEgo(proc?)` —— Task 4/5 使用

- [ ] **Step 1: 在 `test/tests/helpers/server.ts` 增加二进制路径常量**

在 `BACKEND_BINARY` 定义后追加：

```ts
const MEMORY_STORE_BINARY = join(REPO_ROOT, 'kissbot-memory-store', 'target', 'debug', 'kissbot-memory-store');
const MEMORY_EGO_BINARY = join(REPO_ROOT, 'kissbot-memory-ego', 'target', 'debug', 'kissbot-memory-ego');
```

- [ ] **Step 2: 在 `test/tests/helpers/server.ts` 增加 memory-store 启动/停止函数**

在 `stopBackend` 函数后追加（复制 startBackend 的 spawn 模式）：

```ts
// 启动 kissbot-memory-store（debug 二进制），cwd 需含 config.json（memory.root_dir 为相对路径）
// 调用方用 waitForPort(8082) 等待就绪
export function startMemoryStore(cwd: string): ChildProcess {
  const proc = spawn(MEMORY_STORE_BINARY, [], {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'info' },
  });
  proc.stdout?.on('data', (d) => process.stdout.write(`[memory-store] ${d}`));
  proc.stderr?.on('data', (d) => process.stderr.write(`[memory-store:err] ${d}`));
  return proc;
}

export function stopMemoryStore(proc?: ChildProcess): void {
  if (proc && !proc.killed) {
    proc.kill('SIGTERM');
  }
}

// 启动 kissbot-memory-ego（debug 二进制），cwd 需含 config.json
// 调用方用 waitForPort(3001) 等待就绪
export function startMemoryEgo(cwd: string): ChildProcess {
  const proc = spawn(MEMORY_EGO_BINARY, [], {
    cwd,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'info' },
  });
  proc.stdout?.on('data', (d) => process.stdout.write(`[memory-ego] ${d}`));
  proc.stderr?.on('data', (d) => process.stderr.write(`[memory-ego:err] ${d}`));
  return proc;
}

export function stopMemoryEgo(proc?: ChildProcess): void {
  if (proc && !proc.killed) {
    proc.kill('SIGTERM');
  }
}
```

- [ ] **Step 3: `test/global-setup.ts` 增加 memory 组件编译**

在 `kissbot-agent` 编译后追加：

```ts
  console.log('[global-setup] Building memory-store...');
  execSync('cargo build --manifest-path ../kissbot-memory-store/Cargo.toml', {
    stdio: 'inherit',
    cwd: __dirname,
  });
  console.log('[global-setup] Building memory-ego...');
  execSync('cargo build --manifest-path ../kissbot-memory-ego/Cargo.toml', {
    stdio: 'inherit',
    cwd: __dirname,
  });
```

- [ ] **Step 4: `test/global-teardown.ts` 增加 memory 进程清理**

将 pkill 命令追加 memory 组件：

```ts
    execSync('pkill -f kissbot-channel-web 2>/dev/null; pkill -f kissbot-channel-client-cli 2>/dev/null; pkill -f kissbot-memory-store 2>/dev/null; pkill -f kissbot-memory-ego 2>/dev/null', {
      stdio: 'ignore',
    });
```

- [ ] **Step 5: 验证编译**

```bash
cd /home/admin/project/kissbot
cargo build --manifest-path kissbot-memory-store/Cargo.toml
cargo build --manifest-path kissbot-memory-ego/Cargo.toml
```

Expected: 两个 crate 编译成功，生成 `target/debug/kissbot-memory-store` 与 `target/debug/kissbot-memory-ego`。

- [ ] **Step 6: 提交**

```bash
cd /home/admin/project/kissbot
git add test/tests/helpers/server.ts test/global-setup.ts test/global-teardown.ts
git commit -m "chore: 测试基础设施增加 memory-store/memory-ego 编译、启动 helper 与进程清理"
```

---

### Task 4: memory-store API 测试

**Files:**
- Create: `test/tests/memory-store-api.spec.ts`

**Interfaces:**
- Consumes: Task 1 的 workspace 模板（`memory.root_dir: "memory-data"`）、Task 3 的 `startMemoryStore` / `stopMemoryStore` / `waitForPort`
- Produces: 可独立运行的 memory-store API 测试文件

**背景**：memory-store 8 个路由（4 追加 + 4 查询），认证 `user-key-456`。追加带 `force: 1`；追加后需等待 FileObjectAppender 落盘（≥1s）再查询。查询请求 `start_time`/`end_time` 用当天 `"YYYY-MM-DD 00:00:00"` ~ `"YYYY-MM-DD 23:59:59"`。查询响应 `data` 为 `[[key, [[line, record], ...]], ...]`（tuple 序列化为数组）。

- [ ] **Step 1: 写测试文件 `test/tests/memory-store-api.spec.ts`**

```ts
import { test, expect } from '@playwright/test';
import { APIRequestContext } from '@playwright/test';
import { resetWorkspace, startMemoryStore, stopMemoryStore, waitForPort } from './helpers/server';
import { fileURLToPath } from 'url';
import { ChildProcess } from 'child_process';
import { join, dirname } from 'path';

const BASE = 'http://127.0.0.1:8082';
const API_KEY = 'user-key-456'; // security.api_key
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const WORKSPACE = join(__dirname, '..', 'workspace');

// 测试常量（role_name 仅允许 [A-Za-z0-9_]+）
const AGENT = 'agent_a';
const ROLE = 'admin';
const MESSENGER = 'web';
const USER = 'u1';
const GROUP = 'g1';

let store: ChildProcess;

function pad(n: number): string {
  return String(n).padStart(2, '0');
}

function nowTime(): string {
  const d = new Date();
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function todayDate(): string {
  const d = new Date();
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function apiPost(request: APIRequestContext, path: string, body: unknown) {
  return (await request.post(`${BASE}${path}`, {
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: body,
  })).json();
}

test.describe.serial('memory-store API 测试', () => {

  test.beforeAll(async () => {
    resetWorkspace();
    store = startMemoryStore(WORKSPACE);
    await waitForPort(8082, '127.0.0.1', 30000);
  });

  test.afterAll(() => {
    stopMemoryStore(store);
  });

  // TC-01 追加 channel 记录并查询
  test('TC-01: 追加并查询 channel 记录', async ({ request }) => {
    const time = nowTime();
    const resp = await apiPost(request, '/store/channel', {
      requests: [{
        agent_id: AGENT, role_name: ROLE, messenger_id: MESSENGER,
        user_id: USER, group_id: GROUP, is_self: 0,
        messenger_name: 'Web', user_name: '用户1', group_name: '群组1',
        content: { msg_type: 'Text', data: '你好' }, time,
      }],
      force: 1,
    });
    expect(resp.success).toBe(true);

    // 等待追加器落盘（FileObjectAppender 100ms 批量）
    await sleep(1000);

    const q = await apiPost(request, '/store/query/channel', {
      agent_id: AGENT, role_name: ROLE, messenger_id: MESSENGER,
      user_id: USER, group_id: GROUP,
      start_time: `${todayDate()} 00:00:00`, end_time: `${todayDate()} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(Array.isArray(q.data)).toBe(true);
    expect(q.data.length).toBeGreaterThanOrEqual(1);
    const records = q.data[0][1]; // [[line, record], ...]
    expect(records.length).toBeGreaterThanOrEqual(1);
    // key 中含 messenger_id/user_id/group_id（记录本身不含）
    expect(q.data[0][0].user_id).toBe(USER);
    expect(q.data[0][0].group_id).toBe(GROUP);
    const record = records[records.length - 1][1];
    expect(record.user_id).toBe(USER);
    expect(record.content).toEqual({ msg_type: 'Text', data: '你好' });
    expect(record.time).toBe(time);
  });

  // TC-02 追加 think 记录并查询
  test('TC-02: 追加并查询 think 记录', async ({ request }) => {
    const time = nowTime();
    const resp = await apiPost(request, '/store/think', {
      requests: [{
        agent_id: AGENT, role_name: ROLE,
        content: '思考内容', key: 'think_key_1', time,
      }],
      force: 1,
    });
    expect(resp.success).toBe(true);

    await sleep(1000);

    const q = await apiPost(request, '/store/query/think', {
      agent_id: AGENT, role_name: ROLE,
      start_time: `${todayDate()} 00:00:00`, end_time: `${todayDate()} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(q.data.length).toBeGreaterThanOrEqual(1);
    const records = q.data[0][1];
    expect(records.length).toBeGreaterThanOrEqual(1);
    const record = records[records.length - 1][1];
    expect(record.content).toBe('思考内容');
    expect(record.key).toBe('think_key_1');
    expect(record.time).toBe(time);
  });

  // TC-03 追加 tool-call 记录并查询
  test('TC-03: 追加并查询 tool-call 记录', async ({ request }) => {
    const time = nowTime();
    const resp = await apiPost(request, '/store/tool-call', {
      requests: [{
        agent_id: AGENT, role_name: ROLE,
        tool_name: 'get_weather', tool_params: { city: 'Beijing' }, key: 'tool_call_key_1', time,
      }],
      force: 1,
    });
    expect(resp.success).toBe(true);

    await sleep(1000);

    const q = await apiPost(request, '/store/query/tool-call', {
      agent_id: AGENT, role_name: ROLE,
      start_time: `${todayDate()} 00:00:00`, end_time: `${todayDate()} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(q.data.length).toBeGreaterThanOrEqual(1);
    const records = q.data[0][1];
    expect(records.length).toBeGreaterThanOrEqual(1);
    const record = records[records.length - 1][1];
    expect(record.tool_name).toBe('get_weather');
    expect(record.tool_params).toEqual({ city: 'Beijing' });
    expect(record.key).toBe('tool_call_key_1');
    expect(record.time).toBe(time);
  });

  // TC-04 追加 tool-result 记录并查询
  test('TC-04: 追加并查询 tool-result 记录', async ({ request }) => {
    const time = nowTime();
    const resp = await apiPost(request, '/store/tool-result', {
      requests: [{
        agent_id: AGENT, role_name: ROLE,
        tool_result: { temp: 25 }, key: 'tool_result_key_1', time,
      }],
      force: 1,
    });
    expect(resp.success).toBe(true);

    await sleep(1000);

    const q = await apiPost(request, '/store/query/tool-result', {
      agent_id: AGENT, role_name: ROLE,
      start_time: `${todayDate()} 00:00:00`, end_time: `${todayDate()} 23:59:59`,
    });
    expect(q.success).toBe(true);
    expect(q.data.length).toBeGreaterThanOrEqual(1);
    const records = q.data[0][1];
    expect(records.length).toBeGreaterThanOrEqual(1);
    const record = records[records.length - 1][1];
    expect(record.tool_result).toEqual({ temp: 25 });
    expect(record.key).toBe('tool_result_key_1');
    expect(record.time).toBe(time);
  });
});
```

- [ ] **Step 2: 运行测试**

```bash
cd /home/admin/project/kissbot/test
npx playwright test tests/memory-store-api.spec.ts
```

Expected: 4 个用例全部通过。

- [ ] **Step 3: 若有失败，调试并修复**

常见失败点与修复位置（按需处理，修复后回到 Step 2 重跑直到全绿）：

| 症状 | 可能原因 | 修复位置 |
|------|---------|---------|
| 追加返回 success=false（乱序错误） | 同批记录时间早于文件最后写入时间 | 确认请求 `force: 1`；或检查 `kissbot-memory-store/src/record.rs` 乱序判定 |
| 查询返回空数组 | 落盘未完成 | 增大 sleep；或检查 `kissbot-memory-store/src/record.rs` FileObjectAppender 批量落盘 |
| 查询 500 | `query_by_time_range` 读不到文件/路径 | 检查 `kissbot-memory/src/data.rs` 的 `ensure_year_role_dir` 与 `FilePathGenerator` |
| Content 反序列化失败 | 序列化格式不匹配 | 检查请求 `content` 是否为 `{"msg_type": "Text", "data": "..."}` |

修复原则：优先修 Rust 代码缺陷（不要删除注释），测试断言如确属测试写法问题才改测试。

- [ ] **Step 4: 提交**

```bash
cd /home/admin/project/kissbot
git add -A
git commit -m "test: 新增 memory-store API 测试（追加与查询 4+4 路由，HTTP 正常路径）及修复发现的问题"
```

---

### Task 5: memory-ego API 测试

**Files:**
- Create: `test/tests/memory-ego-api.spec.ts`

**Interfaces:**
- Consumes: Task 1 的 workspace 模板、Task 3 的 `startMemoryEgo` / `stopMemoryEgo` / `waitForPort`
- Produces: 可独立运行的 memory-ego API 测试文件

**背景**：memory-ego 端口 3001，认证 `user-key-456`。全部路由按 agent / individual / role 三组流程覆盖。`individual_name`/`role_name` 仅允许 `[A-Za-z0-9_]+`；`description`/`full_name` 可为中文。`data` 中 `individual_map`/`other_roles`/`other_role_relations` 为对象；`CompletionResult` 形如 `{completion, key}`；`RoleKey` 形如 `{agent_id, role_name}`。

- [ ] **Step 1: 写测试文件 `test/tests/memory-ego-api.spec.ts`**

```ts
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
  return (await request.fetch(`${BASE}${path}`, {
    method,
    headers: { 'X-Api-Key': API_KEY, 'Content-Type': 'application/json' },
    data: body,
  })).json();
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
      individual_name: 'alice', description: 'Alice 助理',
    });
    expect(resp.success).toBe(true);
    expect(resp.data).toBeTruthy();
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
    expect(resp.data.individual_name).toBe('alice');
    expect(resp.data.description).toBe('Alice 助理');
  });

  // TC-04 更新 agent 名称
  test('TC-04: 更新 agent 名称', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/agent/update-name', {
      agent_id: agentId, individual_name: 'alice2',
    });
    expect(resp.success).toBe(true);
    const g = await apiReq(request, 'POST', '/agent/get', { agent_id: agentId });
    expect(g.data.individual_name).toBe('alice2');
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
    const resp = await apiReq(request, 'POST', '/agent/copy', { agent_id: agentId });
    expect(resp.success).toBe(true);
    expect(resp.data).toBeTruthy();
    expect(resp.data).not.toBe(agentId);
    copiedAgentId = resp.data;
  });

  // TC-07 按名称搜索 agent
  // 注：TC-06 copy 会复制 individual_name，两个 agent 同名 alice2，全匹配索引只保留一个，故断言包含关系
  test('TC-07: 按名称搜索 agent', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/agent/search-name', { keyword: 'alice2' });
    expect(resp.success).toBe(true);
    expect([agentId, copiedAgentId]).toContain(resp.data);
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
  });

  // TC-23 重命名角色
  test('TC-23: 重命名角色', async ({ request }) => {
    const resp = await apiReq(request, 'PUT', '/role/rename', {
      agent_id: agentId, role_name: 'admin2', new_name: 'mod',
    });
    expect(resp.success).toBe(true);
  });

  // TC-24 按名称搜索角色
  test('TC-24: 按名称搜索角色', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/search-name', {
      keyword: 'mod', agent_id: agentId,
    });
    expect(resp.success).toBe(true);
    expect(resp.data.some((k: any) => k.role_name === 'mod' && k.agent_id === agentId)).toBe(true);
  });

  // TC-25 按描述搜索角色
  test('TC-25: 按描述搜索角色', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/search-description', {
      keyword: '新描述', agent_id: agentId,
    });
    expect(resp.success).toBe(true);
    expect(resp.data.some((k: any) => k.role_name === 'mod')).toBe(true);
  });

  // TC-26 批量取回角色
  test('TC-26: 批量取回角色', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/retrieve', {
      role_keys: [{ agent_id: agentId, role_name: 'mod' }],
    });
    expect(resp.success).toBe(true);
    expect(resp.data.length).toBe(1);
    expect(resp.data[0].role.role_name).toBe('mod');
  });

  // TC-27 角色名称前缀补全
  test('TC-27: 角色名称前缀补全', async ({ request }) => {
    const resp = await apiReq(request, 'POST', '/role/name-completion', {
      prefix: 'mo', agent_id: agentId,
    });
    expect(resp.success).toBe(true);
    expect(resp.data.some((c: any) => c.key.role_name === 'mod')).toBe(true);
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

  // TC-35 删除角色
  test('TC-35: 删除角色', async ({ request }) => {
    const resp = await apiReq(request, 'DELETE', '/role/remove', {
      agent_id: agentId, role_name: 'mod',
    });
    expect(resp.success).toBe(true);
    const list = await apiReq(request, 'POST', '/role/list', { agent_id: agentId });
    expect(list.data).not.toContain('mod');
  });
});
```

- [ ] **Step 2: 运行测试**

```bash
cd /home/admin/project/kissbot/test
npx playwright test tests/memory-ego-api.spec.ts
```

Expected: 35 个用例全部通过。

- [ ] **Step 3: 若有失败，调试并修复**

常见失败点与修复位置（按需处理，修复后回到 Step 2 重跑直到全绿）：

| 症状 | 可能原因 | 修复位置 |
|------|---------|---------|
| 搜索（search/name-completion）返回空 | 脏索引未同步 | 检查 `kissbot-memory-ego/src/search.rs` 的 `sync_identity` / `sync_all_identity` 是否在搜索入口调用；`mark_identity_dirty` 是否在 create/update 后调用 |
| 重命名/删除后搜索返回旧数据 | 索引移除逻辑 | 检查 `search.rs` 的 remove 逻辑 |
| 接口返回 500 | 目录/文件未创建、代号校验失败 | 检查 `kissbot-memory-ego/src/agent.rs`、`individual_recognition.rs`、`role_play.rs` |
| 复制 agent 后元数据缺失 | copy 未复制完整 | 检查 `agent.rs copy_agent` |

修复原则：优先修 Rust 代码缺陷（不要删除注释），测试断言如确属测试写法问题才改测试。

- [ ] **Step 4: 提交**

```bash
cd /home/admin/project/kissbot
git add -A
git commit -m "test: 新增 memory-ego API 测试（agent/individual/role 全部路由，HTTP 正常路径）及修复发现的问题"
```

---

### Task 6: 全量回归

**Files:** 无（验证）

**Interfaces:**
- Consumes: Task 1-5 的全部改动

- [ ] **Step 1: 运行全部测试**

```bash
cd /home/admin/project/kissbot/test
npx playwright test
```

Expected: 全部 spec（channel-web-api、memory-store-api、memory-ego-api、agent-commands、channel-cli、channel-web-client、channel-web-ui、nexus-chat 等）通过，无新增失败。

- [ ] **Step 2: 手动验证重启脚本（可选但建议）**

```bash
cd /home/admin/project/kissbot
# 快速验证脚本语法与 memory 启动（Ctrl+C 停止）
bash -n script/restart-memory-all.sh && echo ok
```

Expected: `ok`。若环境允许，运行 `bash script/restart-memory-all.sh` 并确认输出两个 ✅ 后 Ctrl+C 停止。

- [ ] **Step 3: 确认无遗漏改动并收尾**

```bash
cd /home/admin/project/kissbot
git status --short
git log --oneline -8
```

Expected: 工作区干净（或仅有预期内未跟踪文件），提交历史包含 Task 1-5 的提交。
