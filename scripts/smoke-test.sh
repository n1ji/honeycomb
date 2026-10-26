#!/usr/bin/env bash
# Run a built Honeycomb's self-test against generated audio files:
#   scripts/smoke-test.sh                 # the newest bundle in src-tauri/target
#   scripts/smoke-test.sh Honeycomb.dmg   # or a .dmg, .app, .AppImage or binary
#
# Makes one short, tagged file per format (MP3, FLAC, M4A, Ogg Vorbis, WAV)
# with ffmpeg, starts the app with HONEYCOMB_SMOKE pointing at them, and
# prints its report. The app uses a throwaway library, so the real one is
# never touched. Exits non-zero if any check failed.
#
# HONEYCOMB_SMOKE_NO_AUDIO=1 skips the playback checks (no sound device).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/honeycomb-smoke.XXXXXX")"
mounted=""
cleanup() {
  [[ -n "$mounted" ]] && hdiutil detach -quiet "$mounted" || true
  rm -rf "$work"
}
trap cleanup EXIT

command -v ffmpeg >/dev/null || { echo "ffmpeg is needed to make the test files." >&2; exit 1; }

# 1. Test files: 4 s of a tone each, a different pitch and sample rate per format.
music="$work/music"
mkdir -p "$music"
make() { # name, frequency, rate, codec args...
  local name="$1" freq="$2" rate="$3"; shift 3
  rm -f "$music/$name"
  ffmpeg -loglevel error -f lavfi -i "sine=frequency=$freq:sample_rate=$rate:duration=4" -ac 2 \
    -metadata title="Smoke ${name%.*}" -metadata artist="Honeycomb Test" -metadata album="Smoke Test" \
    -metadata track="$freq" "$@" "$music/$name"
}
make "tone.mp3" 440 44100 -c:a libmp3lame -b:a 192k
make "tone.flac" 494 96000 -c:a flac
make "tone.m4a" 523 48000 -c:a aac -b:a 160k
# ffmpeg builds without libvorbis have ffmpeg's own (experimental) encoder.
make "tone.ogg" 587 44100 -c:a libvorbis -q:a 4 2>/dev/null || make "tone.ogg" 587 44100 -c:a vorbis -strict -2
make "tone.wav" 698 44100 -c:a pcm_s16le

# 2. The build to test.
target="${1:-}"
if [[ -z "$target" ]]; then
  bundles="$root/src-tauri/target"
  target="$(ls -td "$bundles"/*/release/bundle/macos/*.app "$bundles"/release/bundle/macos/*.app \
    "$bundles"/release/bundle/appimage/*.AppImage "$bundles"/release/honeycomb.exe 2>/dev/null | head -1 || true)"
  [[ -n "$target" ]] || { echo "No build found; run 'npm run tauri build' or pass a path." >&2; exit 1; }
fi
[[ -e "$target" ]] || { echo "Not found: $target" >&2; exit 1; }
target="$(cd "$(dirname "$target")" && pwd)/$(basename "$target")"
case "$target" in
  *.dmg)
    mounted="$work/dmg"
    hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mounted" "$target"
    # Copy it out, as a user would, so the test runs the installed layout.
    cp -R "$mounted"/*.app "$work/"
    hdiutil detach -quiet "$mounted"; mounted=""
    app="$(ls -d "$work"/*.app | head -1)"
    xattr -cr "$app" 2>/dev/null || true
    bin="$app/Contents/MacOS/$(defaults read "$app/Contents/Info" CFBundleExecutable)"
    ;;
  *.app)
    bin="$target/Contents/MacOS/$(defaults read "$target/Contents/Info" CFBundleExecutable)"
    ;;
  *)
    bin="$target"
    chmod +x "$bin" 2>/dev/null || true
    ;;
esac
echo "Testing $target"

# 3. Run it.
report="$work/report.txt"
status=0
HONEYCOMB_SMOKE="$music" HONEYCOMB_SMOKE_REPORT="$report" HONEYCOMB_DATA_DIR="$work/data" \
  "$bin" >"$work/out.log" 2>&1 &
pid=$!
# Give up after 3 minutes; a hung app is a failure too.
( sleep 180; kill "$pid" 2>/dev/null ) &
watchdog=$!
wait "$pid" || status=$?
kill "$watchdog" 2>/dev/null || true

if [[ -f "$report" ]]; then
  cat "$report"
else
  echo "The app exited (status $status) without a report. Its output:"
  tail -40 "$work/out.log"
  status=1
fi
exit "$status"
