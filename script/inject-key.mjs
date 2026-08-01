// 注入 api key 到 nexus.json：
// 从 key 文件（{"provider名":"key"}）按 provider 名注入 nexus.providers[].api_key，就地写回。
// 可被 test import（injectApiKeys），也可 CLI 调用：node inject-key.mjs <key文件> <nexus.json路径>
import { readFile, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export async function injectApiKeys(keyFile, nexusPath) {
  const nexus = JSON.parse(await readFile(nexusPath, 'utf8'));
  const providers = nexus.providers || {};
  if (existsSync(keyFile)) {
    const keys = JSON.parse(await readFile(keyFile, 'utf8'));
    for (const [name, key] of Object.entries(keys)) {
      if (providers[name]) {
        providers[name].api_key = key;
        console.log(`  ✓ ${name}: api_key 已注入`);
      } else {
        console.warn(`  ⚠ ${name}: nexus.json 中没有名为 ${name} 的 provider，跳过`);
      }
    }
    for (const [name, provider] of Object.entries(providers)) {
      if (!provider.api_key) {
        console.warn(`  ⚠ provider ${name} 未配置 api_key（key 文件中无对应条目）`);
      }
    }
  } else {
    console.warn(`  ⚠ ${keyFile} 不存在，api_key 保持为空`);
  }
  await writeFile(nexusPath, JSON.stringify(nexus, null, 2) + '\n', 'utf8');
}

// CLI 入口（被 import 时不执行）
if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [keyFile, nexusPath] = process.argv.slice(2);
  if (!keyFile || !nexusPath) {
    console.error('用法: node inject-key.mjs <key.local.json路径> <nexus.json路径>');
    process.exit(1);
  }
  injectApiKeys(keyFile, nexusPath).catch((e) => { console.error('inject-key 失败:', e); process.exit(1); });
}
