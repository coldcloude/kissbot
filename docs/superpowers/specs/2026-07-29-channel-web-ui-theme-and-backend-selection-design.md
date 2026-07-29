# channel-web-ui 换肤与登录页后端选择修正 — 设计文档

日期：2026-07-29
状态：已获用户批准
范围组件：`kissbot-channel-web-ui`（前端）、`test/` 测试文档、`docs/spec/` 约定文档

## 背景与问题

### 现状

1. `kissbot-channel-web-ui` 使用深色主题（`#1a1a2e` 背景、`#00d2ff` 强调色），与 UI 原型（`docs/design/components-design/ui-ux-design/kissbot-channel-web/`，浅色主题 `#4a90d9` 蓝色系）不一致。
2. 登录页预置后端硬编码在 `src/api/config.ts` 的 `DEFAULT_BACKEND_URLS` 中，被打包进 JS bundle，部署时无法替换；且无自定义 URL 输入能力（原型已有）。
3. 人工测试发现登录失败、预置后端"无法选中"，但自动化测试全部通过。

### 根因（已排查确认）

- `src/index.css` 中**完全没有** `.backend-url-list`、`.backend-url-item`、`.login-section`、`.login-label`、`.login-subtitle` 等样式。LoginPage 组件引用了这些 class，但样式从未从原型移植。
- React 状态与 DOM class（`selected`）切换**正确**，因此基于 DOM 断言的自动化测试通过；但用户看不到任何选中高亮（无边框、无 hover、无 pointer 光标），感知为"无法选中"。
- 默认选中项为"测试环境"，但选中态不可见，用户易在不知情下选中"生产环境"占位 URL（`https://api.kissbot.example.com`），连接必然失败。
- 测试盲区：
  - e2e/联合测试断言 DOM class 而非计算样式（computed style）或截图；
  - e2e/联合测试永远只点"测试环境"，从未尝试占位 URL 或自定义 URL；
  - vitest 单元测试未覆盖 LoginPage 组件。

## 需求

1. 整个 UI 与原型样式保持一致（浅色主题换肤）。
2. 登录页支持自定义后端 URL 输入；自定义与预置后端为互斥选择，选中项高亮。
3. 预置后端使用单独文件配置，部署时可替换，不打包进 JS。
4. 补强测试，使"DOM 对、视觉坏"类问题可被自动化发现。

## 设计

### 一、UI 整体换肤（对齐原型）

- **重写 `src/index.css`**：废弃深色 CSS 变量体系，按原型 `style.css` 建立浅色体系（保留 CSS 变量机制便于维护）：
  - 主色 `#4a90d9`，hover `#357abd`，active `#2a6cb0`；
  - 登录页：浅灰底 `#f0f0f0` + 白色卡片；后端选项 2px 边框、选中态蓝边 + `#ecf3fa` 底；
  - 主界面：蓝底白字 header、侧边栏 `#fafafa`、选中会话 `#d0e4f5`、hover `#eef3f8`；
  - 消息气泡：admin 自己蓝底白字靠右、对方 `#eee` 灰底靠左；系统消息灰色小胶囊；
  - 管理面板：`#fafafa` 区块 + `#e0e0e0` 边框；
  - 滚动条、输入框聚焦色等细节同步改为浅色系。
- **组件 class 名不动**，只重写 CSS 值；仅当原型有而组件缺失的视觉结构（如登录页自定义项、消息发送者名等）才微调组件 markup。
- 验收对照页面：`login.html`、`layout.html`、`group-management.html`、`user-management.html` 四个原型。

### 二、登录页后端选择修正

#### 预置后端配置文件

- 新增 `public/backends.json`（Vite 构建时原样拷入 `dist/`，部署时直接替换该文件即可改预置后端，无需重新构建）：

```json
{
  "backends": [
    { "name": "生产环境", "url": "https://api.kissbot.example.com" },
    { "name": "测试环境", "url": "http://localhost:8301" },
    { "name": "开发环境", "url": "http://192.168.1.100:8301" }
  ]
}
```

- 新增 `src/api/backendConfig.ts`：`loadBackendConfig(): Promise<BackendUrlOption[]>`，运行时 `fetch('/backends.json')`；加载失败或列表为空时返回 `[]`。
- 删除 `src/api/config.ts`（硬编码 `DEFAULT_BACKEND_URLS`）。

#### LoginPage 交互

- 选中模型：`selection = { kind: 'preset', url: string } | { kind: 'custom' }`。
- 自定义项固定显示在预置列表**上方**（对齐原型结构），含名称"自定义" + URL 输入框。
- 联动规则：
  - 聚焦或输入自定义 URL → 选中自定义项，预置项取消高亮；
  - 点击预置项 → 选中该项并高亮；自定义输入内容保留但取消选中；
  - 默认选中第一个预置项；无预置项时默认选中自定义。
- 连接校验：
  - 选中自定义但 URL 为空 → 提示"请输入后端 URL"，不发请求；
  - URL 非 `http://` / `https://` 开头 → 提示格式错误，不发请求；
  - 否则传出 `url.trim()` 给 `onConnect`。
- 配置加载中 / 加载失败时，自定义项始终可用。

### 三、测试补强

#### vitest（新增 `src/__tests__/LoginPage.test.tsx`）

mock `fetch` 提供 backends 数据，覆盖：

1. 预置项正确渲染（名称 + URL）；
2. 点击预置项 → `selected` class 切换；
3. 聚焦/输入自定义 URL → 自定义项选中、预置取消；
4. 连接传出正确 URL（预置、自定义各一例）；
5. 自定义为空点连接 → 显示错误提示且不调用 `onConnect`；
6. `fetch` 失败 → 降级为仅显示自定义项。

#### e2e 测试文档修订（`test/kissbot-channel-web-e2e-integration-test.md`）

- TC-01/TC-03 的"验证高亮"改为用 agent-browser 执行 JS 断言**计算样式**：选中项 `border-color === rgb(74, 144, 217)` 且 `background-color === rgb(236, 243, 250)`，而非仅断言 DOM class；
- 新增用例：自定义 URL（`http://localhost:8301`）登录成功；
- 新增用例：选择"生产环境"占位 URL，登录显示错误提示。

#### 联合测试文档修订（`test/kissbot-channel-web-combined-integration-test.md`）

- TC-01 登录步骤同步改为计算样式断言。

### 四、文档更新

- 实现细节约定写入 `docs/spec/`（新增或扩展 channel-web-ui 相关 spec 文件，记录 `backends.json` 的格式与部署替换约定）；
- 修订 `test/` 下两份测试文档（见上节）；
- 不改动 `docs/design/components-design/kissbot-channel-web.md`。

## 非目标（YAGNI）

- 不做主题切换（深色/浅色并存）；
- 不做后端 URL 的持久化记忆（localStorage）；
- 不改动后端 `kissbot-channel-web` 任何代码；
- 不为 SSE/消息等已有逻辑新增测试（本次只补登录页）。

## 验收标准

1. 四个页面视觉与原型一致（人工对照原型 HTML）；
2. `npm run build` 后 `dist/backends.json` 存在且独立于 JS bundle，修改后刷新页面生效；
3. 登录页自定义/预置互斥选中且高亮可见；
4. `npm test` 通过（含新增 LoginPage 测试）；
5. e2e/联合测试文档中的计算样式断言在真实浏览器中通过。
