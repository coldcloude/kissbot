#!/bin/bash
# 启动 kissbot-channel-client-cli
# 用法: ./start-cli.sh <user_id> <group_id> [download_dir]
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

USER_ID="${1:-user-1}"
GROUP_ID="${2:-dev-team}"
DOWNLOAD_DIR="${3:-$SCRIPT_DIR/downloads}"

echo "==> 编译 release 版本..."
cd "$PROJECT_DIR/kissbot-channel-client-cli"
cargo build --release 2>&1 | tail -3

echo "==> 启动 CLI (user=$USER_ID, group=$GROUP_ID)..."
cd "$SCRIPT_DIR"
"$PROJECT_DIR/kissbot-channel-client-cli/target/release/kissbot-channel-client-cli" web "$USER_ID" "$GROUP_ID" "$DOWNLOAD_DIR"
