#!/usr/bin/env bash
# Fetch Steam Audio (Valve, Apache-2.0) for the skate sounds' HRTF (crates/skate-game/src/binaural.rs):
# libphonon.so for the headset into the APK's runtime_libs, and for this machine (Linux x64, for the
# tests and desktop runs) with its header into vendor/steam-audio. Neither is committed; run this
# once per checkout. The app runs without it: sounds are then panned, not put through the HRTF.
set -euo pipefail
VERSION="${STEAM_AUDIO_VERSION:-4.8.1}"
SHA256="${STEAM_AUDIO_SHA256:-4a0aa5ec1176f38f0b0993a37c2259d9e86f27e22d5e24f83ec4c3cb9a1d5449}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APK="$ROOT/crates/skate-quest/runtime_libs/arm64-v8a"
DEV="$ROOT/vendor/steam-audio"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL="https://github.com/ValveSoftware/steam-audio/releases/download/v$VERSION/steamaudio_$VERSION.zip"
echo "Downloading Steam Audio $VERSION (about 170 MB)"
curl -fsSL "$URL" -o "$TMP/steamaudio.zip"
echo "$SHA256  $TMP/steamaudio.zip" | sha256sum -c - >/dev/null || { echo "Steam Audio $VERSION: checksum mismatch"; exit 1; }
unzip -q -o "$TMP/steamaudio.zip" \
  "steamaudio/lib/android-armv8/libphonon.so" "steamaudio/lib/linux-x64/libphonon.so" \
  "steamaudio/include/phonon.h" "steamaudio/include/phonon_version.h" "steamaudio/THIRDPARTY.md" -d "$TMP"
mkdir -p "$APK" "$DEV/lib/linux-x64" "$DEV/include"
cp "$TMP/steamaudio/lib/android-armv8/libphonon.so" "$APK/"
cp "$TMP/steamaudio/lib/linux-x64/libphonon.so" "$DEV/lib/linux-x64/"
cp "$TMP/steamaudio/include/"*.h "$DEV/include/"
cp "$TMP/steamaudio/THIRDPARTY.md" "$DEV/"
echo "Installed $APK/libphonon.so and $DEV"
