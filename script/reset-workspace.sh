#!/bin/bash
# 重置 workspace：删除并重建，连带重置 channel-data
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_DIR="$(dirname "$SCRIPT_DIR")/workspace"

echo "==> 重置 workspace ($WORKSPACE_DIR)..."

rm -rf "$WORKSPACE_DIR"
mkdir -p "$WORKSPACE_DIR/channel-data"
cp "$SCRIPT_DIR/template/channel-data/channel-web-repo.json" "$WORKSPACE_DIR/channel-data/"

echo "==> workspace 已就绪"
