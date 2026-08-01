#!/bin/bash
# 启动 kissbot-agent（debug，KISSBOT_CONFIG=script/config.json）
# 提示：首次或重置后启动前，先执行 ./reset-agent.sh 生成 nexus.json/station.json
#       并确认根目录 key.local.json 已配置
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> 清理旧进程..."
kill $(ps aux | grep "kissbot-agent" | grep -v grep | awk '{print $2}') 2>/dev/null
sleep 1

echo "==> 启动 agent (debug)..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-agent/Cargo.toml"
