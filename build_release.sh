#!/bin/bash
set -euo pipefail

# macOS release build script for Rustle
# Builds the app, creates .app bundle, and packages into .dmg

APP_NAME="Rustle"
VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
BUNDLE_ID="com.rustle.app"
ARCH=$(uname -m)

echo "[build] Building ${APP_NAME} v${VERSION} for macOS (${ARCH})"

# Clean if requested
if [[ "${1:-}" == "--clean" ]]; then
    echo "[build] Cleaning build artifacts..."
    cargo clean
fi

# Build release
echo "[build] Running cargo build --release..."
cargo build --release

# Paths
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps | grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4)
BINARY="${TARGET_DIR}/release/rustle"
APP_BUNDLE="${TARGET_DIR}/release/${APP_NAME}.app"
DMG_DIR="${TARGET_DIR}/release/dmg"
DMG_NAME="${APP_NAME}-${VERSION}-${ARCH}.dmg"

if [[ ! -f "${BINARY}" ]]; then
    echo "[build] ERROR: Binary not found at ${BINARY}"
    exit 1
fi

echo "[build] Binary found: ${BINARY}"

# Create .app bundle structure
echo "[build] Creating .app bundle..."
rm -rf "${APP_BUNDLE}"
mkdir -p "${APP_BUNDLE}/Contents/MacOS"
mkdir -p "${APP_BUNDLE}/Contents/Resources"

# Copy binary
cp "${BINARY}" "${APP_BUNDLE}/Contents/MacOS/rustle"
chmod +x "${APP_BUNDLE}/Contents/MacOS/rustle"

# Create Info.plist
cat > "${APP_BUNDLE}/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleExecutable</key>
    <string>rustle</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# Copy icon if available
if [[ -f "rustle.icns" ]]; then
    cp "rustle.icns" "${APP_BUNDLE}/Contents/Resources/AppIcon.icns"
    echo "CFBundleIconName" >> "${APP_BUNDLE}/Contents/Info.plist.tmp"
    echo "<string>AppIcon</string>" >> "${APP_BUNDLE}/Contents/Info.plist.tmp"
    echo "[build] Icon copied"
fi

echo "[build] .app bundle created: ${APP_BUNDLE}"

# Create DMG
echo "[build] Creating DMG installer..."
rm -rf "${DMG_DIR}"
mkdir -p "${DMG_DIR}"

# Copy .app to DMG staging directory
cp -R "${APP_BUNDLE}" "${DMG_DIR}/"

# Create symbolic link to Applications
ln -s /Applications "${DMG_DIR}/Applications"

# Create DMG using hdiutil
DMG_PATH="${TARGET_DIR}/release/${DMG_NAME}"
rm -f "${DMG_PATH}"

echo "[build] Packaging ${DMG_NAME}..."
hdiutil create \
    -volname "${APP_NAME}" \
    -srcfolder "${DMG_DIR}" \
    -ov \
    -format UDZO \
    -imagekey zlib-level=9 \
    "${DMG_PATH}"

# Cleanup staging
rm -rf "${DMG_DIR}"

echo "[build] Done!"
echo "[build] App bundle: ${APP_BUNDLE}"
echo "[build] DMG installer: ${DMG_PATH}"
