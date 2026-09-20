import { execSync } from 'child_process';
import { mkdirSync, writeFileSync } from 'fs';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { killStrayServices } from './tests/helpers/server';

const __dirname = dirname(fileURLToPath(import.meta.url));

async function globalSetup() {
  // 清理残留服务进程（上次运行中断遗留的 agent/backend/memory-store/memory-ego），
  // 避免端口占用导致本测试服务 bind 失败、以及旧实例 cwd 被 resetWorkspace 删除后路径失效
  console.log('[global-setup] Killing stray service processes...');
  killStrayServices();
  // 越界测试目录：位于各服务 cwd（workspace）之外，供 read 工具的路径越界校验使用；
  // 整目录已在 .gitignore 中，样例文件每次运行重新生成
  const outOfCwd = join(__dirname, 'out-of-cwd');
  mkdirSync(outOfCwd, { recursive: true });
  writeFileSync(join(outOfCwd, 'sample.txt'), 'workspace 之外的样例文件，read 工具应拒绝读取\n', 'utf8');
  console.log(`[global-setup] Out-of-cwd sample ready: ${outOfCwd}`);
  console.log('[global-setup] Building channel-web...');
  execSync('cargo build --manifest-path ../kissbot-channel-web/Cargo.toml', {
    stdio: 'inherit',
    cwd: __dirname,
  });
  console.log('[global-setup] Building channel-client-cli...');
  execSync('cargo build --manifest-path ../kissbot-channel-client-cli/Cargo.toml', {
    stdio: 'inherit',
    cwd: __dirname,
  });
  console.log('[global-setup] Building kissbot-agent...');
  execSync('cargo build --manifest-path ../kissbot-agent/Cargo.toml', {
    stdio: 'inherit',
    cwd: __dirname,
  });
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
  console.log('[global-setup] Build complete.');
}

export default globalSetup;
