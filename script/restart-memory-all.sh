#!/bin/bash
# 重启 memory 服务：清理 memory-data → 编译 debug → 启动 memory-store + memory-ego + memory-struct-abstract
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "========================================"
echo "  Kissbot Memory — 重启全部"
echo "========================================"

# 清理旧进程
kill $(ps aux | grep "kissbot-memory-store" | grep -v grep | awk '{print $2}') 2>/dev/null
kill $(ps aux | grep "kissbot-memory-ego" | grep -v grep | awk '{print $2}') 2>/dev/null
kill $(ps aux | grep "kissbot-memory-struct-abstract" | grep -v grep | awk '{print $2}') 2>/dev/null

# 清理 memory 数据目录
echo ""
echo "[1/4] 清理 memory-data..."
rm -rf "$PROJECT_DIR/workspace/memory-data"
mkdir -p "$PROJECT_DIR/workspace/memory-data"

# 启动 memory-store
echo "[2/4] 启动 memory-store (http://127.0.0.1:8082, notify ws://127.0.0.1:8182)..."
cd "$PROJECT_DIR/workspace"
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
echo "[3/4] 启动 memory-ego (http://127.0.0.1:3001)..."
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-ego/Cargo.toml" > /tmp/kissbot-memory-ego.log 2>&1 &
EGO_PID=$!
echo "   PID: $EGO_PID"
sleep 4

# 验证 memory-ego（GET /agent/list 须返回 200；curl 对 4xx/5xx 不置错误码，故判状态码）
if [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 -H "X-Api-Key: user-key-456" http://127.0.0.1:3001/agent/list)" = "200" ]; then
    echo "   ✅ memory-ego 运行正常"
else
    tail -5 /tmp/kissbot-memory-ego.log
    echo "   ❌ memory-ego 启动失败，查看 /tmp/kissbot-memory-ego.log"
    exit 1
fi

# 注入摘要记忆组件的模型 api key（key.local.json 缺失时保持空，仅模型判定不可用）
echo "[4/4] 启动 memory-struct-abstract (http://127.0.0.1:8083)..."
node "$SCRIPT_DIR/inject-key.mjs" "$PROJECT_DIR/key.local.json" "-" "$PROJECT_DIR/workspace/config.json" 2>/dev/null
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-struct-abstract/Cargo.toml" > /tmp/kissbot-memory-struct-abstract.log 2>&1 &
ABSTRACT_PID=$!
echo "   PID: $ABSTRACT_PID"
sleep 5

if [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 -H "X-Api-Key: user-key-456" -H "Content-Type: application/json" -d '{"agent_id":"0","role":"dev"}' http://127.0.0.1:8083/abstract/tree/get)" = "200" ]; then
    echo "   ✅ memory-struct-abstract 运行正常"
else
    tail -5 /tmp/kissbot-memory-struct-abstract.log
    echo "   ❌ memory-struct-abstract 启动失败，查看 /tmp/kissbot-memory-struct-abstract.log"
    exit 1
fi

echo ""
echo "========================================"
echo "  memory-store:              http://127.0.0.1:8082"
echo "  memory-store notify (WS):  ws://127.0.0.1:8182"
echo "  memory-ego:                http://127.0.0.1:3001"
echo "  memory-struct-abstract:    http://127.0.0.1:8083"
echo "  Api Key:                   user-key-456"
echo "========================================"
echo ""
echo "按 Ctrl+C 停止所有服务"

trap "echo '停止服务...'; kill $STORE_PID $EGO_PID $ABSTRACT_PID 2>/dev/null; exit 0" SIGINT SIGTERM
wait
