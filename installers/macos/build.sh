#!/usr/bin/env bash
# DevToolkit macOS 构建脚本（Apple Silicon / Intel 通用）
# 用法：在 Mac 上执行  ./installers/macos/build.sh
# 产物：.app 与 .dmg（原生 arm64 / x86_64，M 芯片原生运行，无需 Rosetta）

set -euo pipefail

# 回到项目根目录
cd "$(dirname "$0")/../.."
ROOT="$(pwd)"

echo "== DevToolkit macOS 构建 =="

# 1. 环境检查
command -v cargo >/dev/null 2>&1 || { echo "❌ 未找到 cargo，请先安装 Rust: https://rustup.rs"; exit 1; }
command -v npm   >/dev/null 2>&1 || { echo "❌ 未找到 npm，请先安装 Node.js"; exit 1; }

# 2. 前端
echo "[1/4] 构建前端..."
npm install --registry=https://registry.npmmirror.com
npm run build

# 3. tauri-cli
if ! cargo tauri --version >/dev/null 2>&1; then
  echo "[2/4] 安装 tauri-cli（首次约 3-5 分钟）..."
  cargo install tauri-cli --locked
else
  echo "[2/4] 已存在 tauri-cli，跳过安装"
fi

# 4. 生成图标（含 .icns，需要一张 ≥1024 的源 PNG）
echo "[3/4] 生成图标..."
if [ -f src-tauri/icons/icon.png ]; then
  cargo tauri icon src-tauri/icons/icon.png
else
  echo "      跳过（缺少 src-tauri/icons/icon.png 源图，可在 Windows 上运行 tools/make_icons.py 生成）"
fi

# 5. 构建 .app + .dmg
echo "[4/4] 构建应用..."
# 可选：构建通用二进制（同时含 arm64 + x86_64）
if [ "${UNIVERSAL:-0}" = "1" ]; then
  rustup target add x86_64-apple-darwin aarch64-apple-darwin
  cargo tauri build --target universal-apple-darwin
else
  cargo tauri build
fi

echo ""
echo "✅ 构建完成，产物位置："
echo "   .app : $ROOT/src-tauri/target/release/bundle/macos/DevToolkit.app"
echo "   .dmg : $ROOT/src-tauri/target/release/bundle/dmg/"
echo ""
echo "安装：把 DevToolkit.app 拖入 /Applications；或双击 .dmg 后拖入。"
