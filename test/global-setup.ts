import { execSync } from 'child_process';
import { fileURLToPath } from 'url';
import { dirname } from 'path';

const __dirname = dirname(fileURLToPath(import.meta.url));

async function globalSetup() {
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
  console.log('[global-setup] Build complete.');
}

export default globalSetup;
