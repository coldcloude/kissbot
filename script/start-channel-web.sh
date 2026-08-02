#!/bin/bash
# 启动 kissbot-channel-web 后端服务（debug 模式，cargo run）
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> 清理旧进程..."
kill $(ps aux | grep "kissbot-channel-web" | grep -v grep | awk '{print $2}') 2>/dev/null
sleep 1

echo "==> 启动 channel-web 后端 (debug)..."
mkdir -p "$PROJECT_DIR/workspace"
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-channel-web/Cargo.toml"
