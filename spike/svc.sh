#!/bin/sh
S="$HOME/squint-sandbox-spike/spike"
IMG="$HOME/squint-assets/img"
D="$HOME/Pictures/squint-spike"
cp "$IMG/sample.jpg" "$D/svc2-shrink.jpg"
cp "$IMG/sample.jpg" "$D/svc2-strip.jpg"
cp "$IMG/sample.jpg" "$D/fast-inode.jpg"
"$S/inspect" --tag Red "$D/svc2-shrink.jpg"
echo "inode before: $(stat -f %i "$D/fast-inode.jpg")"
open -a "$HOME/Applications/Squint Spike.app" "$D/fast-inode.jpg"
"$S/perform-service" "Spike:Shrink" "$D/svc2-shrink.jpg"
"$S/perform-service" "Spike:Remove Location Data" "$D/svc2-strip.jpg"
/bin/sleep 12
echo "inode after:  $(stat -f %i "$D/fast-inode.jpg")"
"$S/inspect" "$D/svc2-shrink.jpg" "$D/svc2-strip.jpg" "$D/fast-inode.jpg"
ls -A "$D"
echo "== Squint messages mentioning writer/NSIRD/TemporaryItems/SPIKE"
/usr/bin/log show --last 3m --info --debug --style compact --predicate 'process == "Squint"' 2>&1 | grep -iE "spike|NSIRD|TemporaryItems|writer" | cut -c1-260 | head -20
echo "== new denials"
/usr/bin/log show --last 3m --style compact --predicate 'eventMessage CONTAINS "Sandbox" AND eventMessage CONTAINS "Squint"' 2>&1 | grep deny | grep -v disk-space | cut -c1-260 | head -20
