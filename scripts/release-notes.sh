#!/usr/bin/env bash
# Print one version's section of CHANGELOG.md (without its heading):
#   scripts/release-notes.sh 0.1.1
#   scripts/release-notes.sh Unreleased
# A leading "v" is ignored, so CI can pass the tag name.
set -euo pipefail

cd "$(dirname "$0")/.."
want="${1#v}"
awk -v want="$want" '
  /^## / {
    if (found) exit
    head = substr($0, 4)
    # "## 0.1.1, 2026-11-02" or "## 0.1.0 (alpha-1), 2026-10-22" or "## Unreleased"
    split(head, parts, /[ ,]/)
    if (parts[1] == want) { found = 1; next }
  }
  found { print }
' CHANGELOG.md | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}'
