#!/bin/bash
# 重置 workspace：从 template 恢复初始数据、清空工作目录
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_DIR="$(cd "$SCRIPT_DIR/../workspace" && pwd)"

echo "==> 重置 workspace ($WORKSPACE_DIR)..."

# 恢复 messenger 数据
cp "$SCRIPT_DIR/template/channel-web-repo.json" "$WORKSPACE_DIR/channel-web-repo.json"
echo "   ✅ channel-web-repo.json 已重置"

# 清空并重建工作目录
rm -rf "$WORKSPACE_DIR/attachments" "$WORKSPACE_DIR/messages" "$WORKSPACE_DIR/downloads"
mkdir -p "$WORKSPACE_DIR/attachments" "$WORKSPACE_DIR/messages" "$WORKSPACE_DIR/downloads"
echo "   ✅ attachments/ messages/ downloads/ 已清空"

echo "==> workspace 已就绪，可以启动服务"
