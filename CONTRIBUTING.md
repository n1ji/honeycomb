# Contributing to Honeycomb

Thanks for wanting to help! Bug reports, ideas and pull requests are all welcome.

- **Bugs and ideas:** open an issue on GitHub. For a bug, say what you did, what happened, and your OS and Honeycomb version.
- **Security problems:** please don't open an issue. Follow [SECURITY.md](SECURITY.md) instead.
- **Pull requests:** base them on the `dev` branch (`main` only moves at releases). Keep each one to a single change, and run the checks below first.

## Checks

```bash
cd src-tauri && cargo test    # backend tests
npx tsc --noEmit              # type-check the UI
```

Some engine tests use ffmpeg. Never run tests that write tags against your real music library; set `HONEYCOMB_DATA_DIR` to a throwaway folder when running the app for testing.

## Contributor License Agreement

Most of Honeycomb takes contributions under its normal license, the GPL (version 3 or later), with nothing to sign.

A few parts are shared with Honeycomb apps for other platforms. Pull requests that change them need the [Contributor License Agreement](CLA.md). You keep your copyright, and you only sign once, with a comment in your pull request. `CLA.md` explains how.

**Covered code:**

| Now | After the planned crate split (`docs/ARCHITECTURE.md`, phase 1) |
| --- | --- |
| `src-tauri/src/kaseki.rs`, `src-tauri/src/convert.rs` | `crates/kaseki/` |
| `src-tauri/src/models.rs`, `src-tauri/src/db.rs`, `src-tauri/src/smart.rs`, `src-tauri/src/meta.rs`, `src-tauri/src/scanner.rs`, `src-tauri/src/matching.rs`, `src-tauri/src/lyrics.rs`, `src-tauri/src/net.rs` | `crates/honeycomb-core/` |
| (not written yet) | the shared OpenSubsonic client and sync crate, once it exists |

Everything else, including the UI (`src/`), the rest of `src-tauri/`, plugins, scripts and docs, is **not** covered.

A pull request that touches covered code can't be merged until its author has signed. If you'd rather not sign, say so in the pull request. A change to the rest of Honeycomb is still very welcome.

**For the maintainer:** before moving existing code into a covered crate, check with `git log --format='%an <%ae>' -- <file>` that everyone who changed it has signed.
