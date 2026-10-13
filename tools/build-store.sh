#!/bin/sh
# Build the Mac App Store edition, sign it for the store, and package it.
#
#   sh tools/build-store.sh <profile> <app identity> <installer identity> [keychain]
#
#   profile             the Mac App Store provisioning profile for the SquintStore bundle
#   app identity        the Apple Distribution certificate, by name or SHA-1
#   installer identity  the Mac Installer Distribution certificate, by name or SHA-1
#   keychain            the keychain holding both, when it is not the login one
#
# The result is app/build/Squint-<version>-appstore.pkg. With ASC_KEY_ID and
# ASC_ISSUER_ID set, the package is also validated by App Store Connect; the
# key's AuthKey_<id>.p8 must be in a directory altool searches, such as the one
# named by API_PRIVATE_KEYS_DIR. With UPLOAD=1 as well, it is uploaded. Building
# and uploading are separate decisions, so uploading is never the default.
#
# The edition on the releases page is signed by tools/sign-release.sh instead.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="$1"
APP_IDENTITY="$2"
INSTALLER_IDENTITY="$3"
KEYCHAIN="${4:-}"

KC=""
if [ -n "$KEYCHAIN" ]; then
    KC="--keychain $KEYCHAIN"
fi

export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"
PB=/usr/libexec/PlistBuddy

cd "$ROOT/app"
mkdir -p build
xcodegen generate >/dev/null
if ! xcodebuild -scheme SquintStore -configuration Release -derivedDataPath build build > build/store-build.log 2>&1; then
    tail -40 build/store-build.log
    exit 1
fi
APP="$ROOT/app/build/Build/Products/Release-AppStore/Squint.app"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# A provisioning profile is a signed message; its content says which
# application and team it was issued for. A profile for another bundle would
# sign cleanly and be refused only at upload, so the match is checked here.
security cms -D -i "$PROFILE" > "$WORK/profile.plist"
APPLICATION_ID="$("$PB" -c 'Print :Entitlements:com.apple.application-identifier' "$WORK/profile.plist")"
TEAM_ID="$("$PB" -c 'Print :TeamIdentifier:0' "$WORK/profile.plist")"
BUNDLE_ID="$("$PB" -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")"
if [ "$APPLICATION_ID" != "$TEAM_ID.$BUNDLE_ID" ]; then
    echo "the profile is for $APPLICATION_ID, but the application is $TEAM_ID.$BUNDLE_ID" >&2
    exit 1
fi
cp "$PROFILE" "$APP/Contents/embedded.provisionprofile"

# The store requires the signature to carry the application and team
# identifiers that the profile grants, beside the sandbox entitlements.
cp "$ROOT/app/SquintStore.entitlements" "$WORK/entitlements.plist"
"$PB" -c "Add :com.apple.application-identifier string $APPLICATION_ID" "$WORK/entitlements.plist"
"$PB" -c "Add :com.apple.developer.team-identifier string $TEAM_ID" "$WORK/entitlements.plist"

# The store edition embeds no framework, so the application is the only code
# to sign.
# shellcheck disable=SC2086
codesign --force --options runtime --timestamp $KC --sign "$APP_IDENTITY" \
    --entitlements "$WORK/entitlements.plist" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -d --entitlements - "$APP" 2>/dev/null | grep -E "app-sandbox|application-identifier|team-identifier"

VERSION="$("$PB" -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
PKG="$ROOT/app/build/Squint-$VERSION-appstore.pkg"
rm -f "$PKG"
# shellcheck disable=SC2086
productbuild --component "$APP" /Applications --sign "$INSTALLER_IDENTITY" $KC "$PKG"
pkgutil --check-signature "$PKG"

if [ -n "${ASC_KEY_ID:-}" ] && [ -n "${ASC_ISSUER_ID:-}" ]; then
    xcrun altool --validate-app "$PKG" -t macos --api-key "$ASC_KEY_ID" --api-issuer "$ASC_ISSUER_ID"
    if [ "${UPLOAD:-}" = "1" ]; then
        xcrun altool --upload-package "$PKG" -t macos --api-key "$ASC_KEY_ID" --api-issuer "$ASC_ISSUER_ID"
    fi
fi

echo "$PKG"
