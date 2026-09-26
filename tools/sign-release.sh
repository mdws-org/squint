#!/bin/sh
# Sign a built Squint.app with a Developer ID, notarize it, and staple the ticket.
#
#   sh tools/sign-release.sh <Squint.app> <identity> <notary-profile> [keychain]
#
#   identity        the Developer ID Application certificate, by name or SHA-1
#   notary-profile  a profile stored with `xcrun notarytool store-credentials`
#   keychain        the keychain holding both, when it is not the login one
#
# Signing goes inside out. A signature covers the code it is attached to and the
# signatures of what that code contains, so the innermost pieces are signed first
# and the application last; anything signed after its container invalidates the
# container's seal. `codesign --deep` walks the same tree but has been
# discouraged by Apple for years and is rejected by notarization for apps that
# embed frameworks, which is why each of Sparkle's five pieces is named here.
#
# Every signature carries `--options runtime`. Notarization requires the
# hardened runtime, and Sparkle loads under it without any entitlement because
# it is signed by the same identity as the application: library validation is
# satisfied by the matching team, not by a permission.
#
# Nothing here touches the keychain's password. When the keychain is a
# dedicated one it must already be unlocked and its key's partition list must
# already admit `codesign`; that is set up once, when the certificate is
# installed, and `docs/RELEASING.md` records how.
set -eu

APP="$1"
IDENTITY="$2"
PROFILE="$3"
KEYCHAIN="${4:-}"

if [ ! -d "$APP/Contents" ]; then
    echo "not an application bundle: $APP" >&2
    exit 2
fi

KC=""
if [ -n "$KEYCHAIN" ]; then
    KC="--keychain $KEYCHAIN"
fi

sign() {
    # shellcheck disable=SC2086
    codesign --force --options runtime --timestamp $KC --sign "$IDENTITY" "$1"
}

SPARKLE="$APP/Contents/Frameworks/Sparkle.framework"
V="$SPARKLE/Versions/B"

sign "$V/XPCServices/Downloader.xpc"
sign "$V/XPCServices/Installer.xpc"
sign "$V/Autoupdate"
sign "$V/Updater.app"
sign "$SPARKLE"
sign "$APP"

# `--deep` is right for verification even though it is wrong for signing: here
# it asks that every nested seal be intact, which is the whole question.
codesign --verify --deep --strict --verbose=2 "$APP"

# What was actually applied, read back rather than assumed. `runtime` in the
# flags is the hardened runtime; the authority chain should end at Apple's
# Developer ID root, and the team identifier must be set.
codesign -dvv "$APP" 2>&1 | grep -E "^Authority=|^TeamIdentifier=|flags=|^Signature=" || true

# Signing can be proven without spending a notarization: a new certificate, a
# new build machine, or a keychain that may prompt are all checked here, and
# `spctl` on an unnotarized bundle answers `rejected` by design.
if [ "${SIGN_ONLY:-}" = "1" ]; then
    echo "SIGN_ONLY=1: signed and verified; not submitted, not stapled"
    exit 0
fi

# Apple's notary service takes an archive, not a bundle.
SUBMIT="$(mktemp -d)/Squint-notarize.zip"
ditto -c -k --keepParent "$APP" "$SUBMIT"

# shellcheck disable=SC2086
if ! xcrun notarytool submit "$SUBMIT" --keychain-profile "$PROFILE" $KC --wait --output-format json > "$SUBMIT.json"; then
    echo "notarization submission failed; the response follows" >&2
    cat "$SUBMIT.json" >&2 || true
    exit 1
fi
STATUS="$(/opt/homebrew/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("status",""))' "$SUBMIT.json")"
ID="$(/opt/homebrew/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id",""))' "$SUBMIT.json")"
echo "notarization $STATUS ($ID)"
if [ "$STATUS" != "Accepted" ]; then
    # The log names each file the service objected to and why. Without it a
    # refusal is a single word.
    # shellcheck disable=SC2086
    xcrun notarytool log "$ID" --keychain-profile "$PROFILE" $KC >&2 || true
    exit 1
fi
rm -rf "$(dirname "$SUBMIT")"

# The ticket is stapled to the bundle so Gatekeeper can verify it offline. The
# archives are then built FROM the stapled bundle by the caller; a zip made
# before this line ships without the ticket.
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"

# The answer a user's Mac will give. It must read "accepted" with
# "source=Notarized Developer ID"; anything else means the first launch is
# refused exactly as an unsigned build's would be.
spctl -a -vv -t exec "$APP"
