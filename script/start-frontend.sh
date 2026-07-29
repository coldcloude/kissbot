#!/bin/bash
# 启动 kissbot-channel-web-ui 前端 dev server
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> 清理旧进程..."
kill \$(ps aux | grep "vite" | grep -v grep | awk '{print \$2}') 2>/dev/null
sleep 1

echo "==> 安装前端依赖..."
cd "$PROJECT_DIR/kissbot-channel-web-ui"
npm install 2>&1 | tail -3

echo "==> 启动前端 dev server..."
npm run dev
