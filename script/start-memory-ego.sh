#!/bin/bash
# 启动 kissbot-memory-ego 服务（debug 模式，cargo run）
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> 清理旧进程..."
kill $(ps aux | grep "kissbot-memory-ego" | grep -v grep | awk '{print $2}') 2>/dev/null
sleep 1

echo "==> 启动 memory-ego (debug)..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-ego/Cargo.toml"
