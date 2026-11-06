#!/bin/bash

# macOSアプリバンドルを作成するスクリプト

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This script builds a macOS app bundle; run it on macOS." >&2
    exit 1
fi

cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

APP_NAME="RawHID Rust"
APP_DIR="${APP_NAME}.app"
CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"
BUILD_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
BUILD_TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
case "${BUILD_TARGET}" in
    *-apple-darwin) ;;
    *) echo "A macOS Rust target is required: ${BUILD_TARGET}" >&2; exit 1 ;;
esac

# リリースビルド
echo "Building Rust application..."
cargo build --release --locked --target "${BUILD_TARGET}" --target-dir "${BUILD_TARGET_DIR}"

# アプリバンドルディレクトリを作成
echo "Creating app bundle structure..."
rm -rf "${APP_DIR}"
mkdir -p "${MACOS_DIR}"
mkdir -p "${RESOURCES_DIR}"

# バイナリをコピー
echo "Copying binary..."
cp "${BUILD_TARGET_DIR}/${BUILD_TARGET}/release/rawhid-rust" "${MACOS_DIR}/${APP_NAME}"

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
echo "${APP_DIR} を Applications フォルダに移動して起動してください。"
echo "アプリ名の取得に画面収録・アクセシビリティ権限は不要です。"
