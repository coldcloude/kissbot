# 设计：memory API 测试 + 启动脚本调整

日期：2026-08-03

## 目标

1. 在 `test` 中为 memory-store、memory-ego 增加 API 测试（HTTP 请求覆盖各 REST 接口，只测正常情况），测试过程中发现问题则修复。
2. 调整 `script` 目录的启动脚本命名与职责，并统一数据目录布局。

## 背景事实（已确认）

- 测试模式参照 `test/tests/channel-web-api.spec.ts`：直接启动 debug 二进制（cwd=`test/workspace`）+ `X-Api-Key` 请求头 + Playwright `APIRequestContext`。
- memory 组件认证用 `security.api_key`（`user-key-456`），与 channel-web 的 `admin-key-123` 不同。
- memory-store 端口 8082（`memory.store.listen_port`），路由 8 个（4 追加 + 4 查询）。
- memory-ego 端口 3001（`memory.ego.listen_port`），路由约 35 个（agent / individual / role 三组）。
- 追加记录请求带 `force` 字段（`force: 1` 可避免乱序拒绝）。
- `role_name`、`individual_name` 仅允许 `[A-Za-z0-9_]+`（代号限制），测试数据须用合法代号。
- memory-store / memory-ego 会自动创建数据目录（`ensure_dir_exists` / `ensure_agent_store_dir` / `create_dir_all`），channel-web 会自动创建 attachments / messages 目录，但 repo 文件父目录需由 reset 脚本保证。
- `script/` 下不存在 `start-all.sh`；用户确认删除 `restart-all.sh`（其"一键启动全部"角色由 restart-channel-all.sh 替代）。

## 第 1 部分：API 测试

### 新增 `test/tests/memory-store-api.spec.ts`（serial）

- beforeAll：`resetWorkspace()` → 启动 memory-store 二进制（cwd=`test/workspace`）→ `waitForPort(8082)`。
- 认证头：`X-Api-Key: user-key-456`。
- 覆盖 8 个路由，只测正常路径：
  - 追加：`POST /store/channel`、`/store/think`、`/store/tool-call`、`/store/tool-result`（`force: 1`）。
  - 查询：`POST /store/query/channel`、`/store/query/think`、`/store/query/tool-call`、`/store/query/tool-result`，断言返回记录与追加内容匹配。
- 测试数据时间使用当前日期（记录按日期分文件），query 的 `start_time`/`end_time` 使用日期字符串。

### 新增 `test/tests/memory-ego-api.spec.ts`（serial）

- beforeAll：`resetWorkspace()` → 启动 memory-ego 二进制（cwd=`test/workspace`）→ `waitForPort(3001)`。
- 认证头：`X-Api-Key: user-key-456`。
- 覆盖全部路由，按三组流程走正常路径：
  - Agent：create → list → get → update-name → update-description → copy → search-name → search-description → retrieve → name-completion。
  - Individual：replace（插入个体）→ get-all → get → rename → replace-identifiers → replace-relations。
  - Role：create → list → get → update-description → update-full-name → create-from → rename → search-name → search-description → retrieve → name-completion。
  - Role-other：replace-other-roles → get → rename → update-individual-name → update-description → update-relation → replace-relations。
  - remove-role（DELETE）。
- 测试数据用合法代号（如 `agent_a`、`alice`、`admin`）。

### 配套改动

- `test/tests/helpers/server.ts`：新增 memory-store / memory-ego 二进制路径与启动/停止 helper（复用 `waitForPort`）。
- `test/global-setup.ts`：追加编译 `kissbot-memory-store`、`kissbot-memory-ego`。
- `test/global-teardown.ts`：追加清理 memory-store / memory-ego 进程。

## 第 2 部分：数据目录调整

### channel-web 数据 → `workspace/channel-data/`

- `script/config.json`：`messenger_repo`、`attachment_dir`、`message_dir` 改为 `../workspace/channel-data/{channel-web-repo.json,attachments,messages}`。
- `test/workspace-template/config.json`：对应改为 `channel-data/{...}`（相对 test/workspace）。
- 模板文件移动：
  - `script/template/channel-web-repo.json` → `script/template/channel-data/channel-web-repo.json`
  - `test/workspace-template/channel-web-repo.json` → `test/workspace-template/channel-data/channel-web-repo.json`
- `script/reset-workspace.sh`：重建 workspace → 创建 `channel-data/` 子目录 → 从 `template/channel-data/` 复制 repo 文件（reset-workspace 连带 reset-channel）。
- `test/tests/helpers/server.ts` 的 `resetWorkspace()`：整体 `cp -r template → workspace`，自动带上 `channel-data/`。

### memory 系列数据 → `memory-data/`

- `script/config.json` 与 `test/workspace-template/config.json`：`memory.root_dir` 由 `"data"` 改为 `"memory-data"`。
  - script 场景落在 `script/memory-data/`，test 场景落在 `test/workspace/memory-data/`。

## 第 3 部分：启动脚本

### 重命名（内容不变）

- `script/start-backend.sh` → `script/start-channel-web.sh`（前台 cargo run）。
- `script/start-frontend.sh` → `script/start-channel-web-ui.sh`（前台 npm run dev）。

### 删除

- `script/restart-all.sh`。

### 新增（单组件，前台模式，风格与现有 start-*.sh 一致）

- `script/start-memory-store.sh`：清理 memory-store 旧进程 + `cargo run --manifest-path kissbot-memory-store/Cargo.toml`（`KISSBOT_CONFIG=config.json`）。
- `script/start-memory-ego.sh`：清理 memory-ego 旧进程 + `cargo run --manifest-path kissbot-memory-ego/Cargo.toml`（`KISSBOT_CONFIG=config.json`）。

### 新增（组合，后台 + 日志 + 端口验证 + trap 停止）

- `script/restart-channel-all.sh`：
  1. 清理 channel-web / vite 旧进程；
  2. 调用 `reset-workspace.sh`（重置 workspace 并连带重置 channel-data）；
  3. 启动 channel-web 后端（日志 `/tmp/kissbot-channel-web.log`），验证 8301；
  4. 启动 channel-web-ui 前端（日志 `/tmp/kissbot-channel-web-ui.log`），验证 5173；
  5. Ctrl+C 一起停止。
- `script/restart-memory-all.sh`：
  1. 清理 memory-store / memory-ego 旧进程；
  2. 清理 `script/memory-data/`；
  3. 启动 memory-store（日志 `/tmp/kissbot-memory-store.log`），验证 8082；
  4. 启动 memory-ego（日志 `/tmp/kissbot-memory-ego.log`），验证 3001；
  5. Ctrl+C 一起停止。

## 验证方式

- `cd test && npx playwright test memory-store-api memory-ego-api`（全部通过）。
- 原有 channel-web 相关测试不受目录调整影响（`npx playwright test` 全量通过）。
- 手动执行 `script/restart-channel-all.sh` 与 `script/restart-memory-all.sh` 验证启动、数据目录生成与 Ctrl+C 停止。

## 风险与注意

- memory-store 追加记录的乱序判定：同批记录时间若早于文件最后写入时间会被拒绝，测试用 `force: 1` 规避。
- memory-ego 的 `individual_name` / `role_name` 只允许 `[A-Za-z0-9_]+`，测试数据不可用中文。
- 修改 config 中路径后需同步检查 channel-web 相关 spec（channel-web-api / channel-web-ui / channel-web-client）不依赖旧路径。
