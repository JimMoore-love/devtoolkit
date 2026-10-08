#!/usr/bin/env bash
# DevToolkit macOS 卸载脚本
# 用法：chmod +x uninstall.sh && ./uninstall.sh
# 删除应用本体与用户配置（项目/历史数据也会一并删除）

set -euo pipefail

APP="/Applications/DevToolkit.app"
CFG="$HOME/Library/Application Support/cn.devtoolkit.app"

echo "== DevToolkit 卸载 =="

if [ -d "$APP" ]; then
  rm -rf "$APP"
  echo "✅ 已删除 $APP"
else
  echo "· 未找到 $APP（可能未安装或已移动）"
fi

if [ -d "$CFG" ]; then
  rm -rf "$CFG"
  echo "✅ 已删除用户配置 $CFG"
else
  echo "· 无残留配置"
fi

echo ""
echo "DevToolkit 已卸载完成。"
