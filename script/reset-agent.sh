#!/bin/bash
# 重置 agent 数据：从模板生成 nexus.json/station.json 到 workspace/agent-data，
# 并从根目录 key.local.json 注入 api key（不入库的本地密钥文件）
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> 重置 agent 数据..."
node "$SCRIPT_DIR/agent-reset.mjs" "$PROJECT_DIR/key.local.json" "$PROJECT_DIR/workspace/agent-data/nexus.json"
echo "==> 完成（agent 启动前请确认根目录 key.local.json 已配置）"
