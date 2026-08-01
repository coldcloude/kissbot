#!/bin/bash
# 重置 agent 数据：从模板生成 nexus.json/station.json 到数据目录，并从 key 文件注入 api key
# 用法: ./reset-agent.sh [数据目录] [key.local.json路径]
#       默认数据目录 <项目根>/workspace/agent-data，默认 key 文件 <项目根>/key.local.json
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DATA_DIR="${1:-$PROJECT_DIR/workspace/agent-data}"
KEY_FILE="${2:-$PROJECT_DIR/key.local.json}"

echo "==> 重置 agent 数据..."
mkdir -p "$DATA_DIR"
cp "$SCRIPT_DIR/template/nexus.json" "$DATA_DIR/nexus.json"
cp "$SCRIPT_DIR/template/station.json" "$DATA_DIR/station.json"
node "$SCRIPT_DIR/inject-key.mjs" "$KEY_FILE" "$DATA_DIR/nexus.json"
echo "==> 完成"
