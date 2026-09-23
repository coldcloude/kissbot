#!/bin/bash
# 启动 kissbot-memory-struct-abstract 服务（debug 模式，cargo run，cwd=workspace）
# 启动前按 memory_struct_abstract.llm.provider 从 key 文件注入 api key 到 workspace/config.json
# 用法: ./start-memory-struct-abstract.sh [key.local.json路径]
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
KEY_FILE="${1:-$PROJECT_DIR/key.local.json}"

echo "==> 清理旧进程..."
kill $(ps aux | grep "kissbot-memory-struct-abstract" | grep -v grep | awk '{print $2}') 2>/dev/null
sleep 1

if [ -f "$PROJECT_DIR/workspace/config.json" ]; then
  echo "==> 注入模型 api key..."
  node "$SCRIPT_DIR/inject-key.mjs" "$KEY_FILE" "-" "$PROJECT_DIR/workspace/config.json" \
    || echo "  ⚠ api key 注入失败，请手工填写 workspace/config.json 的 memory_struct_abstract 段"
fi

echo "==> 启动 memory-struct-abstract (debug)..."
cd "$PROJECT_DIR/workspace"
KISSBOT_CONFIG=config.json cargo run --manifest-path "$PROJECT_DIR/kissbot-memory-struct-abstract/Cargo.toml"
