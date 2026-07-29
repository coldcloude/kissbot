# channel 组件自动化测试设计

日期：2026-07-29

## 背景与目标

`test/` 下原有 3 个手工执行的集成测试文档（Markdown 形式，靠人按步骤执行 curl / 浏览器 / cli 命令）：

- `kissbot-channel-web-api-integration-test.md`——channel-web 后端 HTTP API 测试（24 条 TC）
- `kissbot-channel-web-e2e-integration-test.md`——channel-web 前端 + 后端联调测试（24 条 TC）
- `kissbot-channel-web-combined-integration-test.md`——channel-web 前后端与 channel-client-cli 双向通信测试（15 条 TC）

目标：将三者合并为一套 `npx playwright test` 一键执行的自动化测试，浏览器操作使用 Playwright，CLI 进程用 Node `child_process` 交互，HTTP 接口用 Playwright 内置 `request`（APIRequestContext）。全部跑通后删除 3 个原 MD 文件。

## 技术选型

- **Playwright Test 框架**（`@playwright/test` 1.62，已安装）：负责用例发现、串行编排、断言（expect + 自动重试断言如 `toHaveText`/`toHaveCSS`）、失败截图/trace、HTML 报告
- **HTTP 请求**：内置 `request` fixture，支持 JSON、multipart 表单（附件上传）、二进制响应（附件下载），零新增依赖
- **浏览器**：仅 chromium（从 config 删除 firefox / webkit 项目）
- **CLI 交互**：Node `child_process.spawn`，stdin 写命令、stdout 缓冲断言

## 目录结构

```
test/
  playwright.config.ts          # 只留 chromium 项目，workers: 1，list+html reporter
  global-setup.ts               # cargo build 后端与 cli，确保二进制最新
  global-teardown.ts            # 兜底清理残留进程
  tests/
    helpers/
      server.ts                 # resetWorkspace() / startBackend() / stopBackend() / waitForPort()
      cli.ts                    # spawnCli() 子进程封装，waitForCliOutput(regex, timeout)
      api.ts                    # apiGet/apiPost 封装（X-Api-Key 头），返回 JSON
      assets.ts                 # 生成测试用 PNG（大图/小图）与 txt 附件文件
    channel-web-api.spec.ts     # 套件名：channel-web 后端 API 测试（24 条 TC）
    channel-web-ui.spec.ts      # 套件名：channel-web 前后端集成测试（24 条 TC）
    channel-web-client.spec.ts  # 套件名：channel-web 与 channel-client 通信测试（15 条 TC）
```

## 进程与服务生命周期

- **前端 dev server**：无状态，全局一次——config 的 `webServer` 声明（在 `kissbot-channel-web-ui` 目录执行 `npm run dev`，等 `:5173` 就绪，`reuseExistingServer: true`）。仅 ui / client 两个套件用到。
- **后端**：每个 spec 的 `beforeAll` 执行：重置 workspace（`rm -rf test/workspace` + 从 `test/workspace-template` 拷贝）→ 启动 `target/debug/kissbot-channel-web`（cwd 设为 workspace）→ 轮询 `127.0.0.1:8301/api/info` 就绪；`afterAll` 杀掉。套件间状态完全隔离，保留原文档的硬编码 ID 预期（如首个自建群组为 `g2`、首个自建用户为 `u3`）。client 套件 TC-15（SSE 断线重连）中途杀/起后端复用同一组 helper。
- **cli**：client 套件内按需 `spawnCli()`（cwd 设为 workspace，参数如 `web user-1 dev-team ./downloads`），test 结束 kill；TC-13 需要第二个 cli（user-2 绑定 project-x）时临时再起。
- **执行顺序**：`workers: 1` + 各 spec 内 `test.describe.serial`，套件内、套件间全部串行。
- **二进制构建**：`global-setup.ts` 先对 `kissbot-channel-web` 和 `kissbot-channel-client-cli` 执行 `cargo build`，随后测试直接启动编译产物，避免每个进程启动都触发 cargo 检查。

## 用例依赖与数据流

原文档的跨 TC 依赖（TC-03 的 msg_id、TC-06 的 GROUP_ID、TC-20 的 TRANSFER_ID / ATT_KEY、分页游标 date/line 等）用 describe 闭包内的共享变量传递。每条 MD 用例对应一个 `test('TC-XX 名称', ...)`，断言一一对应原文档"预期结果"。

## 原文档的"翻译"要点

- **curl + python 断言** → `request` fixture + `expect` 直接断言 JSON 字段
- **agent-browser 的 getComputedStyle 检查** → `toHaveCSS('border-color', 'rgb(74, 144, 217)')` / `toHaveCSS('background-color', 'rgb(236, 243, 250)')`
- **附件文件** → `assets.ts` 在 `beforeAll` 生成到临时目录；file input 用 `setInputFiles`
- **PNG 生成**（缩略图为 200×200，需验证两种路径）：
  - 大图 800×600（>200×200）→ 验证缩略图被**缩小**到 200×200 以内
  - 小图 100×80（<200×200）→ 验证缩略图**不放大**，按原尺寸返回
  - 实现：不引入新依赖，用 Node 内置 `zlib.deflateSync` 手写极简 PNG 编码器（IHDR/IDAT/IEND + CRC）生成任意尺寸 PNG
  - 缩略图尺寸断言：解析返回 JPEG 的 SOF 段读取宽高（纯 Buffer 操作）
- **ui 套件分页用例**（需 20+ 条历史消息）→ 前置用 API 快速发送 25 条再滚顶验证
- **文件下载**（ui TC-10、client TC-12）→ `page.waitForEvent('download')` 断言文件名与内容
- **消息落盘 4 秒缓冲** → 保留 `waitForTimeout(4000)`（后端固定行为）
- **SSE 断线重连**（client TC-15）→ 杀后端 → 等待 5 秒 → 重启后端与 cli → cli 发消息 → 断言页面出现该消息
- **cli 输出断言** → `waitForCliOutput(/>> sent msg_id=/)` 等正则等待，带超时

## 错误处理

- 端口就绪、cli 输出、SSE 消息等所有等待带超时，超时即测试失败并附上下文
- 后端 / cli 子进程 stdout 捕获，测试失败时附在报告中便于诊断
- `afterAll` / `global-teardown` 保证杀进程，测试中断也不留孤儿进程

## 收尾

全部测试跑通验证后，删除 3 个原 MD 文件：

- `test/kissbot-channel-web-api-integration-test.md`
- `test/kissbot-channel-web-e2e-integration-test.md`
- `test/kissbot-channel-web-combined-integration-test.md`

## 范围外（YAGNI）

- 不做 CI 集成
- 不做跨浏览器（firefox / webkit 从 config 删除）
- 不做用例并行化
