#!/bin/sh
# SANDBOX SPIKE: build the app, sign it with the App Sandbox entitlements, and
# install it as ~/Applications/Squint Spike.app. Run on the Mac build box.
#
#   sh spike/build.sh            Developer ID signature from mdws-signing
#   SIGN=adhoc sh spike/build.sh ad-hoc signature, no hardened runtime
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH=/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH

cd "$ROOT/app"
xcodegen generate
if ! xcodebuild -scheme Squint -configuration Release -derivedDataPath build build > build.log 2>&1; then
    tail -40 build.log
    exit 1
fi
APP="$ROOT/app/build/Build/Products/Release/Squint.app"

if [ "${SIGN:-developerid}" = "adhoc" ]; then
    sign() { codesign --force --sign - "$@"; }
else
    KC="$HOME/Library/Keychains/mdws-signing.keychain-db"
    ID=A61A2109DE17F150ED001F2B227DC4AA81EEA1FB
    sign() { codesign --force --options runtime --timestamp --keychain "$KC" --sign "$ID" "$@"; }
fi
V="$APP/Contents/Frameworks/Sparkle.framework/Versions/B"
sign "$V/XPCServices/Downloader.xpc"
sign "$V/XPCServices/Installer.xpc"
sign "$V/Autoupdate"
sign "$V/Updater.app"
sign "$APP/Contents/Frameworks/Sparkle.framework"
sign --entitlements "$ROOT/app/Spike.entitlements" "$APP"
codesign --verify --deep --strict "$APP"
codesign -d --entitlements - "$APP" 2>/dev/null | grep -E "app-sandbox|user-selected" || true

DEST="$HOME/Applications/Squint Spike.app"
mkdir -p "$HOME/Applications"
rm -rf "$DEST"
ditto "$APP" "$DEST"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST"

cd "$ROOT/spike"
swiftc -O perform-service.swift -o perform-service
swiftc -O inspect.swift -o inspect
echo "installed: $DEST"
