import { execSync } from 'child_process';

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
  console.log('[global-setup] Build complete.');
}

export default globalSetup;
