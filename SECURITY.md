# Security

## Reporting a problem

Please report security problems privately, not in a public issue: use
**Report a vulnerability** on the repository's Security tab on GitHub, or
email n1ji2work@gmail.com. Say what you found, how to reproduce it, and which
version and operating system you used. You'll get an answer within a week.

## Supported versions

Honeycomb is in alpha. Fixes go into the next release only, so please check
the newest release before reporting.

## Checking a download

Every release has a `SHA256SUMS.txt` with a checksum for each file. To check
one:

```bash
shasum -a 256 -c SHA256SUMS.txt --ignore-missing     # macOS
sha256sum -c SHA256SUMS.txt --ignore-missing         # Linux
```

On Windows, compare the output of `certutil -hashfile <file> SHA256` with the
line for that file.

Builds aren't code-signed yet, so the checksums are the way to tell that a
download is the one GitHub's build produced.

## What Honeycomb talks to

Honeycomb only connects to the internet for:

- GitHub's API, to check for a newer release.
- LRCLIB (lrclib.net), for lyrics, when online lookup is on.
- Last.fm and ListenBrainz, for scrobbling, once you sign in.

It never uploads your music files. It only changes them when you edit tags,
and then writes the change to a copy that replaces the original once the write
is complete.
