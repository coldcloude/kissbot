#!/bin/bash
# 重置 agent 数据：从 template 生成 nexus.json/station.json 到 workspace/agent-data，
# 并从 key.local.json 注入 api key（不入库的本地密钥文件）
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "==> 重置 agent 数据..."
node "$SCRIPT_DIR/agent-reset.mjs"
echo "==> 完成（agent 启动前请确认 script/key.local.json 已配置）"
