#!/bin/bash

# macOSアプリバンドルを作成するスクリプト

set -e

APP_NAME="RawHID Rust"
APP_DIR="${APP_NAME}.app"
CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"

# クリーンビルド
echo "Building Rust application..."
cargo build --release

# アプリバンドルディレクトリを作成
echo "Creating app bundle structure..."
rm -rf "${APP_DIR}"
mkdir -p "${MACOS_DIR}"
mkdir -p "${RESOURCES_DIR}"

# バイナリをコピー
echo "Copying binary..."
cp target/release/rawhid-rust "${MACOS_DIR}/${APP_NAME}"

# Info.plistを作成
echo "Creating Info.plist..."
cat > "${CONTENTS_DIR}/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>com.rawhid.rust</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleVersion</key>
    <string>0.1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.13</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright © 2024</string>
    <key>NSAppleEventsUsageDescription</key>
    <string>このアプリはアクティブなアプリケーション名を取得するためにアクセシビリティ権限が必要です。</string>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>LSUIElement</key>
    <true/>
</dict>
</plist>
EOF

# 実行権限を付与
chmod +x "${MACOS_DIR}/${APP_NAME}"

echo "App bundle created: ${APP_DIR}"
echo ""
echo "次の手順でアクセシビリティ権限を付与してください:"
echo "1. ${APP_DIR} を Applications フォルダに移動"
echo "2. システム設定 > プライバシーとセキュリティ > アクセシビリティ"
echo "3. ${APP_NAME} を追加してチェックを入れる"
