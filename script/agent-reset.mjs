// 重置 agent 数据：
// 用法: node agent-reset.mjs <key.local.json路径> <nexus.json输出路径>
// 1. 将 script/template/nexus.json、station.json 复制到 <nexus.json输出路径> 所在目录
// 2. 从 <key路径> 读取 api key（{"名称":"key",...}），按名称（= provider 名）
//    注入 nexus.json 的 providers[名称].api_key（provider 级密钥）
import { readFile, writeFile, mkdir, copyFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const templateDir = path.join(scriptDir, 'template');

const [keyFile, nexusPath] = process.argv.slice(2);
if (!keyFile || !nexusPath) {
  console.error('用法: node agent-reset.mjs <key.local.json路径> <nexus.json输出路径>');
  process.exit(1);
}
const dataDir = path.dirname(path.resolve(nexusPath));

async function main() {
  // 1. 准备数据目录，复制模板（station.json 与 nexus.json 同目录）
  await mkdir(dataDir, { recursive: true });
  await copyFile(path.join(templateDir, 'station.json'), path.join(dataDir, 'station.json'));
  const nexus = JSON.parse(await readFile(path.join(templateDir, 'nexus.json'), 'utf8'));

  // 2. 注入 api key（名称 = provider 名）
  const providers = nexus.providers || {};
  if (existsSync(keyFile)) {
    const keys = JSON.parse(await readFile(keyFile, 'utf8'));
    for (const [name, key] of Object.entries(keys)) {
      if (providers[name]) {
        providers[name].api_key = key;
        console.log(`  ✓ ${name}: api_key 已注入`);
      } else {
        console.warn(`  ⚠ ${name}: nexus.json 中没有名为 ${name} 的 provider，跳过（可先在 template/nexus.json 中添加）`);
      }
    }
    // 提示没有注入 key 的 provider
    for (const [name, provider] of Object.entries(providers)) {
      if (!provider.api_key) {
        console.warn(`  ⚠ provider ${name} 未配置 api_key（key.local.json 中无对应条目）`);
      }
    }
  } else {
    console.warn(`  ⚠ ${keyFile} 不存在，api_key 将保持为空`);
  }

  // 3. 写回
  await writeFile(nexusPath, JSON.stringify(nexus, null, 2) + '\n', 'utf8');
  console.log(`==> 已写入 ${nexusPath}、${path.join(dataDir, 'station.json')}`);
}

main().catch((e) => {
  console.error('agent-reset 失败:', e);
  process.exit(1);
});
