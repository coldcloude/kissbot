// 注入 api key：
// 1) nexus.json：从 key 文件（{"provider名":"key"}）按 provider 名注入 nexus.providers[].provider_config.api_key。
// 2) config.json（可选第三参数）：按 memory_struct_abstract.llm.provider 注入
//    memory_struct_abstract.provider(_config).api_key（摘要记忆组件自带 provider 配置）。
// 可被 test import（injectApiKeys），也可 CLI 调用：
//   node inject-key.mjs <key文件> <nexus.json路径|-> [config.json路径]
import { readFile, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export async function injectApiKeys(keyFile, nexusPath, configPath) {
  const keys = existsSync(keyFile) ? JSON.parse(await readFile(keyFile, 'utf8')) : null;
  if (!keys) {
    console.warn(`  ⚠ ${keyFile} 不存在，api_key 保持为空`);
  }

  if (nexusPath && existsSync(nexusPath)) {
    const nexus = JSON.parse(await readFile(nexusPath, 'utf8'));
    const providers = nexus.providers || {};
    if (keys) {
      for (const [name, key] of Object.entries(keys)) {
        if (providers[name]?.provider_config) {
          providers[name].provider_config.api_key = key;
          console.log(`  ✓ ${name}: api_key 已注入`);
        } else {
          console.warn(`  ⚠ ${name}: nexus.json 中没有名为 ${name} 的 provider，跳过`);
        }
      }
      for (const [name, provider] of Object.entries(providers)) {
        if (!provider.provider_config?.api_key) {
          console.warn(`  ⚠ provider ${name} 未配置 api_key（key 文件中无对应条目）`);
        }
      }
    }
    await writeFile(nexusPath, JSON.stringify(nexus, null, 2) + '\n', 'utf8');
  }

  if (configPath && existsSync(configPath)) {
    const config = JSON.parse(await readFile(configPath, 'utf8'));
    const section = config.memory_struct_abstract;
    // 兼容文档中的 `provider` 键名与 kissbot-llm 的 `provider_config` 键名
    const provider = section?.provider ?? section?.provider_config;
    if (provider) {
      const name = section?.llm?.provider;
      if (keys && name && keys[name]) {
        provider.api_key = keys[name];
        console.log(`  ✓ memory_struct_abstract(${name}): api_key 已注入`);
      } else {
        console.warn(`  ⚠ memory_struct_abstract: key 文件中没有 provider ${name}，api_key 保持为空`);
      }
      await writeFile(configPath, JSON.stringify(config, null, 2) + '\n', 'utf8');
    }
  }
}

// CLI 入口（被 import 时不执行）
if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [keyFile, nexusPath, configPath] = process.argv.slice(2);
  if (!keyFile || !nexusPath) {
    console.error('用法: node inject-key.mjs <key.local.json路径> <nexus.json路径|-> [config.json路径]');
    process.exit(1);
  }
  const nexus = nexusPath === '-' ? null : nexusPath;
  injectApiKeys(keyFile, nexus, configPath).catch((e) => { console.error('inject-key 失败:', e); process.exit(1); });
}
