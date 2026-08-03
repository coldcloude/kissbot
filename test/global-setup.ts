import { execSync } from 'child_process';
import { fileURLToPath } from 'url';
import { dirname } from 'path';
import { killStrayServices } from './tests/helpers/server';

const __dirname = dirname(fileURLToPath(import.meta.url));

async function globalSetup() {
  // 清理残留服务进程（上次运行中断遗留的 agent/backend/memory-store/memory-ego），
  // 避免端口占用导致本测试服务 bind 失败、以及旧实例 cwd 被 resetWorkspace 删除后路径失效
  console.log('[global-setup] Killing stray service processes...');
  killStrayServices();
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
