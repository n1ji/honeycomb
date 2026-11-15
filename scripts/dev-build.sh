#!/usr/bin/env bash
# A local test build labelled as a pre-release of the next version, e.g.
# 0.3.0-dev.4 while 0.2.0 is the newest release. The number counts up per
# checkout and starts again when the version moves. Nothing is committed.
#
#   scripts/dev-build.sh            # macOS .app
#   scripts/dev-build.sh --bundles dmg
set -euo pipefail
cd "$(dirname "$0")/.."

current=$(node -p 'require("./src-tauri/tauri.conf.json").version')
IFS=. read -r major minor _ <<<"$current"
next="$major.$((minor + 1)).0"

counter="$(git rev-parse --git-common-dir)/honeycomb-dev-build"
n=1
if [[ -f "$counter" ]]; then
  read -r last_version last_n <"$counter" || true
  [[ "${last_version:-}" == "$next" ]] && n=$((last_n + 1))
fi
version="$next-dev.$n"

args=("$@")
[[ ${#args[@]} -eq 0 ]] && args=(--bundles app)
echo "Building Honeycomb $version"
npm run tauri build -- "${args[@]}" --config "{\"version\":\"$version\"}"
echo "$next $n" >"$counter"

app=src-tauri/target/release/bundle/macos/Honeycomb.app
if [[ -d "$app" ]]; then
  xattr -cr "$app" 2>/dev/null || true
  echo "Honeycomb $version: $app"
fi
