#!/bin/sh
echo "== SPIKE messages (any process, info level)"
/usr/bin/log show --last 20m --info --style compact --predicate 'eventMessage CONTAINS "SPIKE"' 2>&1 | grep -v '^Timestamp' | cut -c1-280 | head -40
echo "== Squint process messages, count"
/usr/bin/log show --last 20m --info --style compact --predicate 'process == "Squint"' 2>&1 | wc -l
/usr/bin/log show --last 20m --info --style compact --predicate 'process == "Squint"' 2>&1 | grep -v disk-space | cut -c1-200 | head -15
echo "== non-disk-space denials"
/usr/bin/log show --last 20m --style compact --predicate 'eventMessage CONTAINS "Sandbox" AND eventMessage CONTAINS "Squint"' 2>&1 | grep deny | grep -v disk-space | cut -c1-260 | head -20
echo "== pbs registration"
/System/Library/CoreServices/pbs -dump_pboard 2>/dev/null | grep -n -i "spike" | head -20
