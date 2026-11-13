#!/usr/bin/env bash
# Bring the sound lab's tuning from the headset into the source tree, where the
# next build starts from it (crates/skate-game/clean-audio/tuning.json).
#
#   scripts/quest/pull-sound-tuning.sh [adb serial]
set -euo pipefail
cd "$(dirname "$0")/../.."
PKG="${PKG:-com.andrewnakas.skate3ar}"
serial=()
[ -n "${1:-}" ] && serial=(-s "$1")
out=crates/skate-game/clean-audio/tuning.json
tmp="$(mktemp)"
adb "${serial[@]}" pull "/sdcard/Android/data/$PKG/files/sound-tuning.json" "$tmp" >/dev/null
python3 -c 'import json,sys; t=json.load(open(sys.argv[1])); print("tuned:", ", ".join(sorted(t.get("sounds", {}))) or "nothing")' "$tmp"
mv "$tmp" "$out"
echo "wrote $out"
