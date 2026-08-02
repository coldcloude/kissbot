#!/bin/bash
# 重启 channel 服务：reset-workspace（连带重置 channel-data）→ 编译 debug → 启动 channel-web + channel-web-ui
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "========================================"
echo "  Kissbot Channel — 重启全部"
echo "========================================"

# 清理旧进程
kill $(ps aux | grep "kissbot-channel-web" | grep -v grep | awk '{print $2}') 2>/dev/null
kill $(ps aux | grep "vite" | grep -v grep | awk '{print $2}') 2>/dev/null

# 重置 workspace（连带重置 channel-data）
echo ""
echo "[1/3] 重置 workspace (连带 channel-data)..."
bash "$SCRIPT_DIR/reset-workspace.sh"

# 启动 channel-web 后端
echo "[2/3] 启动 channel-web (http://127.0.0.1:8301)..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-channel-web/Cargo.toml" > /tmp/kissbot-channel-web.log 2>&1 &
BACKEND_PID=$!
echo "   PID: $BACKEND_PID"
sleep 4

# 验证后端
curl -s --max-time 3 -H "X-Api-Key: admin-key-123" http://127.0.0.1:8301/api/info > /dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "   ✅ channel-web 运行正常"
else
    tail -5 /tmp/kissbot-channel-web.log
    echo "   ❌ channel-web 启动失败，查看 /tmp/kissbot-channel-web.log"
    exit 1
fi

# 启动 channel-web-ui 前端
echo "[3/3] 启动 channel-web-ui (http://localhost:5173)..."
cd "$PROJECT_DIR/kissbot-channel-web-ui"
npm install 2>&1 | tail -1
npm run dev > /tmp/kissbot-channel-web-ui.log 2>&1 &
FRONTEND_PID=$!
echo "   PID: $FRONTEND_PID"
sleep 3

grep -q "ready in" /tmp/kissbot-channel-web-ui.log 2>/dev/null
if [ $? -eq 0 ]; then
    echo "   ✅ channel-web-ui 运行正常"
else
    echo "   ⚠️ channel-web-ui 可能未完全启动，查看 /tmp/kissbot-channel-web-ui.log"
fi

echo ""
echo "========================================"
echo "  channel-web:   http://127.0.0.1:8301"
echo "  channel-web-ui: http://localhost:5173"
echo "  CLI:           ./start-cli.sh <user_id> <group_id>"
echo "  Admin Key:     admin-key-123"
echo "========================================"
echo ""
echo "按 Ctrl+C 停止所有服务"

trap "echo '停止服务...'; kill $BACKEND_PID $FRONTEND_PID 2>/dev/null; exit 0" SIGINT SIGTERM
wait
