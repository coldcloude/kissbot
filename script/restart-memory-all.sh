#!/bin/bash
# 重启 memory 服务：清理 memory-data → 编译 debug → 启动 memory-store + memory-ego
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "========================================"
echo "  Kissbot Memory — 重启全部"
echo "========================================"

# 清理旧进程
kill $(ps aux | grep "kissbot-memory-store" | grep -v grep | awk '{print $2}') 2>/dev/null
kill $(ps aux | grep "kissbot-memory-ego" | grep -v grep | awk '{print $2}') 2>/dev/null

# 清理 memory 数据目录
echo ""
echo "[1/3] 清理 memory-data..."
rm -rf "$SCRIPT_DIR/memory-data"
mkdir -p "$SCRIPT_DIR/memory-data"

# 启动 memory-store
echo "[2/3] 启动 memory-store (http://127.0.0.1:8082)..."
cd "$SCRIPT_DIR"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-store/Cargo.toml" > /tmp/kissbot-memory-store.log 2>&1 &
STORE_PID=$!
echo "   PID: $STORE_PID"
sleep 4

# 验证 memory-store（TCP 端口检查）
timeout 2 bash -c "echo > /dev/tcp/127.0.0.1/8082" 2>/dev/null
if [ $? -eq 0 ]; then
    echo "   ✅ memory-store 运行正常"
else
    tail -5 /tmp/kissbot-memory-store.log
    echo "   ❌ memory-store 启动失败，查看 /tmp/kissbot-memory-store.log"
    exit 1
fi

# 启动 memory-ego
echo "[3/3] 启动 memory-ego (http://127.0.0.1:3001)..."
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-ego/Cargo.toml" > /tmp/kissbot-memory-ego.log 2>&1 &
EGO_PID=$!
echo "   PID: $EGO_PID"
sleep 4

# 验证 memory-ego（调用 /agent/list）
curl -s --max-time 3 -X POST -H "X-Api-Key: user-key-456" -H "Content-Type: application/json" http://127.0.0.1:3001/agent/list > /dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "   ✅ memory-ego 运行正常"
else
    tail -5 /tmp/kissbot-memory-ego.log
    echo "   ❌ memory-ego 启动失败，查看 /tmp/kissbot-memory-ego.log"
    exit 1
fi

echo ""
echo "========================================"
echo "  memory-store: http://127.0.0.1:8082"
echo "  memory-ego:   http://127.0.0.1:3001"
echo "  Api Key:      user-key-456"
echo "========================================"
echo ""
echo "按 Ctrl+C 停止所有服务"

trap "echo '停止服务...'; kill $STORE_PID $EGO_PID 2>/dev/null; exit 0" SIGINT SIGTERM
wait
