// 重置 agent 数据：
// 1. 将 script/template/nexus.json、station.json 复制到 <project>/workspace/agent-data/
// 2. 从 script/key.local.json（{"名称":"key",...}）读取 api key，
//    按名称（= nexus.json providers 的配置名）注入对应 provider 的 api_key 字段
// 用法：node agent-reset.mjs（由 reset-agent.sh 调用）
import { readFile, writeFile, mkdir, copyFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const projectDir = path.resolve(scriptDir, '..');
const dataDir = path.join(projectDir, 'workspace', 'agent-data');
const keyFile = path.join(scriptDir, 'key.local.json');

async function main() {
  // 1. 准备数据目录，复制模板
  await mkdir(dataDir, { recursive: true });
  await copyFile(path.join(scriptDir, 'template', 'station.json'), path.join(dataDir, 'station.json'));
  const nexus = JSON.parse(await readFile(path.join(scriptDir, 'template', 'nexus.json'), 'utf8'));

  // 2. 注入 api key（名称 = provider 配置名）
  if (existsSync(keyFile)) {
    const keys = JSON.parse(await readFile(keyFile, 'utf8'));
    for (const [name, key] of Object.entries(keys)) {
      if (nexus.providers[name]) {
        nexus.providers[name].api_key = key;
        console.log(`  ✓ ${name}: api_key 已注入`);
      } else {
        console.warn(`  ⚠ ${name}: nexus.json 中没有名为 ${name} 的 provider，跳过（可先在 template/nexus.json 中添加）`);
      }
    }
    // 提示没有注入 key 的 provider
    for (const [name, provider] of Object.entries(nexus.providers)) {
      if (!provider.api_key) {
        console.warn(`  ⚠ provider ${name} 未配置 api_key（key.local.json 中无对应条目）`);
      }
    }
  } else {
    console.warn(`  ⚠ ${keyFile} 不存在，api_key 将保持为空`);
  }

  // 3. 写回
  await writeFile(path.join(dataDir, 'nexus.json'), JSON.stringify(nexus, null, 2) + '\n', 'utf8');
  console.log(`==> 已写入 ${dataDir}/nexus.json、station.json`);
}

main().catch((e) => {
  console.error('agent-reset 失败:', e);
  process.exit(1);
});
