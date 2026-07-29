#!/bin/bash
# 启动 kissbot-channel-web 后端服务（debug 模式，cargo run）
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> 启动后端 (debug)..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-channel-web/Cargo.toml"
