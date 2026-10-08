#!/usr/bin/env bash
# DevToolkit · macOS 一键构建脚本（Apple Silicon / Intel 均支持）
# 用法: 解压源码后进入目录执行  bash build-macos.sh
set -e
cd "$(dirname "$0")"

echo "════════════════════════════════════════════"
echo "  DevToolkit · macOS 构建器"
echo "════════════════════════════════════════════"

# ---------- 1. Rust 工具链 ----------
if ! command -v cargo >/dev/null 2>&1; then
  echo "[1/5] 未检测到 Rust，通过国内镜像安装（约 2 分钟）..."
  export RUSTUP_DIST_SERVER=https://rsproxy.cn
  export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
  curl --proto '=https' --tlsv1.2 -sSf https://rsproxy.cn/rustup-init.sh | sh -s -- -y --default-toolchain stable
  source "$HOME/.cargo/env"
else
  echo "[1/5] Rust 已就绪: $(rustc --version)"
fi

# cargo 国内镜像
mkdir -p "$HOME/.cargo"
if ! grep -q rsproxy "$HOME/.cargo/config.toml" 2>/dev/null; then
  cat >> "$HOME/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = 'rsproxy-sparse'
[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
[net]
git-fetch-with-cli = true
EOF
fi

# ---------- 2. Node ----------
if ! command -v node >/dev/null 2>&1; then
  echo "[2/5] 未检测到 Node.js，请先安装（brew install node 或 https://nodejs.org）"
  exit 1
else
  echo "[2/5] Node 已就绪: $(node --version)"
fi

# ---------- 3. 前端依赖 ----------
# ⚠️ package.json 在**仓库根目录**（不在 src/）。这里原先写的是 `cd src && npm install`，
# 而 src/ 下没有 package.json，脚本会在这一步直接失败。前端命令必须在根目录跑。
echo "[3/5] 安装前端依赖（国内镜像）..."
npm install --registry=https://registry.npmmirror.com --no-fund --no-audit
npm run build

# ---------- 4. 编译 Rust ----------
echo "[4/5] 编译 Rust 后端（首次约 5-10 分钟）..."
cargo build --release --manifest-path src-tauri/Cargo.toml

# ---------- 5. 打包 .app ----------
echo "[5/5] 打包 DevToolkit.app ..."
APP="DevToolkit.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp src-tauri/target/release/devtoolkit "$APP/Contents/MacOS/DevToolkit"
chmod +x "$APP/Contents/MacOS/DevToolkit"

# 图标: PNG → iconset → icns
ICON_SRC="src-tauri/icons/icon.png"
if [ -f "$ICON_SRC" ]; then
  mkdir -p "$APP/Contents/Resources/icon.iconset"
  for size in 16 32 64 128 256 512; do
    sips -z $size $size "$ICON_SRC" --out "$APP/Contents/Resources/icon.iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size*2)) $((size*2)) "$ICON_SRC" --out "$APP/Contents/Resources/icon.iconset/icon_${size}x${size}@2x.png" >/dev/null
  done
  cp "$ICON_SRC" "$APP/Contents/Resources/icon.iconset/icon_512x512@2x.png"
  iconutil -c icns "$APP/Contents/Resources/icon.iconset" -o "$APP/Contents/Resources/icon.icns" 2>/dev/null || true
  rm -rf "$APP/Contents/Resources/icon.iconset"
fi

# 版本号从 package.json 现取，不再写死 —— 原先这里硬编码 1.2.0，
# 项目已经到 1.5.0 而 plist 里还印着旧版本（同一个版本号在仓库里有 5 处副本，必然漂移）。
VER="$(node -p "require('./package.json').version")"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>DevToolkit</string>
  <key>CFBundleDisplayName</key><string>DevToolkit</string>
  <key>CFBundleIdentifier</key><string>cn.devtoolkit.app</string>
  <key>CFBundleVersion</key><string>${VER}</string>
  <key>CFBundleShortVersionString</key><string>${VER}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>DevToolkit</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
EOF

echo ""
echo "════════════════════════════════════════════"
echo "  ✅ 构建完成: $(pwd)/$APP"
echo "  运行方式: 双击 $APP，或 open ./$APP"
echo "  安装方式: mv $APP /Applications/"
echo "════════════════════════════════════════════"
open "$APP"
