#!/bin/sh
# SANDBOX SPIKE: put fresh copies through the sandboxed app and report what the
# sandbox allowed. Run on the Mac build box after spike/build.sh.
#
# The mode for files opened through Launch Services comes from the filename
# prefix (fast-, quality-, strip-); Services entries carry their own.
set -u
S="$(cd "$(dirname "$0")" && pwd)"
IMG="$HOME/squint-assets/img"
APP="$HOME/Applications/Squint Spike.app"

pkill -x Squint 2>/dev/null
/bin/sleep 1
START="$(date '+%Y-%m-%d %H:%M:%S')"

for D in "$HOME/Pictures/squint-spike" "$HOME/Downloads/squint-spike"; do
    rm -rf "$D"
    mkdir -p "$D"
    cp "$IMG/sample.jpg" "$D/fast-a.jpg"
    cp "$IMG/sample.jpg" "$D/quality-a.jpg"
    cp "$IMG/sample.jpg" "$D/strip-a.jpg"
    cp "$IMG/orig.png" "$D/fast-b.png"
    sips -s format heic "$IMG/sample.jpg" --out "$D/strip-c.heic" >/dev/null
    cp "$IMG/sample.jpg" "$D/svc-shrink.jpg"
    cp "$IMG/sample.jpg" "$D/svc-strip.jpg"
    for F in "$D"/*; do "$S/inspect" --tag Red "$F"; done
    echo "== before: $D"
    "$S/inspect" "$D"/*
done

echo "== open -a (Launch Services intake)"
for D in "$HOME/Pictures/squint-spike" "$HOME/Downloads/squint-spike"; do
    open -a "$APP" "$D/fast-a.jpg" "$D/quality-a.jpg" "$D/strip-a.jpg" "$D/fast-b.png" "$D/strip-c.heic"
done
/bin/sleep 40

echo "== Services intake"
for D in "$HOME/Pictures/squint-spike" "$HOME/Downloads/squint-spike"; do
    "$S/perform-service" "Spike: Shrink" "$D/svc-shrink.jpg"
    "$S/perform-service" "Spike: Remove Location Data" "$D/svc-strip.jpg"
done
/bin/sleep 15

for D in "$HOME/Pictures/squint-spike" "$HOME/Downloads/squint-spike"; do
    echo "== after: $D"
    "$S/inspect" "$D"/*
    echo "-- directory listing (leftover temporaries?)"
    ls -A "$D"
done

echo "== sandboxed?"
ls -d "$HOME/Library/Containers/net.mdws.squint.spike" 2>&1
codesign -d --entitlements - "$APP" 2>/dev/null | grep -E "app-sandbox|user-selected"

echo "== SPIKE log"
log show --start "$START" --style compact --predicate 'process == "Squint" AND eventMessage CONTAINS "SPIKE"' 2>&1 | grep SPIKE
echo "== sandbox denials naming Squint"
log show --start "$START" --style compact --predicate 'eventMessage CONTAINS "Sandbox" AND eventMessage CONTAINS "Squint"' 2>&1 | grep -i deny | cut -c1-300 | head -30
