#!/usr/bin/env bash
# Cut a release: scripts/release.sh 0.1.1
#
# 1. Checks the tree is clean, on main, and CHANGELOG.md has something under
#    "## Unreleased".
# 2. Sets the version in package.json, package-lock.json, tauri.conf.json,
#    Cargo.toml and Cargo.lock.
# 3. Renames "## Unreleased" to "## <version>, <date>" and opens a fresh,
#    empty Unreleased section above it.
# 4. Commits "Release <version>" and tags it v<version>.
#
# Nothing is pushed. Run the smoke test on the build (scripts/smoke-test.sh)
# before pushing the tag; the tag push is what starts the CI build.
set -euo pipefail

cd "$(dirname "$0")/.."
version="${1:-}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: scripts/release.sh X.Y.Z (numbers only: Windows installers reject 'alpha' suffixes)" >&2
  exit 1
fi
tag="v$version"

[[ "$(git rev-parse --abbrev-ref HEAD)" == main ]] || { echo "Not on main." >&2; exit 1; }
[[ -z "$(git status --porcelain --untracked-files=no)" ]] || { echo "Uncommitted changes; commit or stash them first." >&2; exit 1; }
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && { echo "Tag $tag already exists." >&2; exit 1; }

notes="$(scripts/release-notes.sh Unreleased)"
if [[ -z "${notes//[[:space:]]/}" ]]; then
  echo "CHANGELOG.md has nothing under '## Unreleased'." >&2
  exit 1
fi

node - "$version" <<'EOF'
const fs = require("fs");
const v = process.argv[2];
for (const f of ["package.json", "src-tauri/tauri.conf.json"]) {
  const j = JSON.parse(fs.readFileSync(f, "utf8"));
  j.version = v;
  fs.writeFileSync(f, JSON.stringify(j, null, 2) + "\n");
}
const lock = JSON.parse(fs.readFileSync("package-lock.json", "utf8"));
lock.version = v;
lock.packages[""].version = v;
fs.writeFileSync("package-lock.json", JSON.stringify(lock, null, 2) + "\n");

const cargo = fs.readFileSync("src-tauri/Cargo.toml", "utf8");
fs.writeFileSync("src-tauri/Cargo.toml", cargo.replace(/^version = ".*"$/m, `version = "${v}"`));
const cl = fs.readFileSync("src-tauri/Cargo.lock", "utf8");
fs.writeFileSync("src-tauri/Cargo.lock", cl.replace(/(\[\[package\]\]\nname = "honeycomb"\nversion = )".*"/, `$1"${v}"`));

const date = new Date().toISOString().slice(0, 10);
const log = fs.readFileSync("CHANGELOG.md", "utf8");
if (!log.includes("\n## Unreleased\n")) throw new Error("CHANGELOG.md: no '## Unreleased' heading");
fs.writeFileSync("CHANGELOG.md", log.replace("\n## Unreleased\n", `\n## Unreleased\n\n## ${v}, ${date}\n`));
EOF

git add package.json package-lock.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock CHANGELOG.md
git commit -q -m "Release $version"
git tag -a "$tag" -m "Honeycomb $version"

echo "Tagged $tag. Release notes:"
echo
scripts/release-notes.sh "$version"
echo
echo "Next: build and smoke-test (scripts/smoke-test.sh), then"
echo "  git push origin main $tag"
echo "CI builds every platform into a draft pre-release; publish it from GitHub."
