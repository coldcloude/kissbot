#!/bin/bash
# 启动 kissbot-channel-web 后端服务
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

mkdir -p "$SCRIPT_DIR/attachments" "$SCRIPT_DIR/messages"

echo "==> 编译 release 版本..."
cd "$PROJECT_DIR/kissbot-channel-web"
cargo build --release 2>&1 | tail -3

echo "==> 启动后端..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json "$PROJECT_DIR/kissbot-channel-web/target/release/kissbot-channel-web"
