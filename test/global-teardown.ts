import { execSync } from 'child_process';

async function globalTeardown() {
  try {
    execSync('pkill -f kissbot-channel-web 2>/dev/null; pkill -f kissbot-channel-client-cli 2>/dev/null; pkill -f kissbot-memory-store 2>/dev/null; pkill -f kissbot-memory-ego 2>/dev/null', {
      stdio: 'ignore',
    });
  } catch { /* ok */ }
}

export default globalTeardown;
