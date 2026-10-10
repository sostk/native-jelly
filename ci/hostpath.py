#!/usr/bin/env python3
"""A build machine's directory layout inside a shipped file — the one scanner every gate shares.

`ci/check-package.py` (before publishing), `ci/gen-release-audit.py` (the audit's evidence) and
`ci/verify-published.sh` (after publishing) all ask the same question of the same bytes, and they
used to carry three copies of the answer. Two calibration bugs are designed around, both found by
running against a release KNOWN to be dirty:

  * per-BLOB matching with a "webos-ndk" allowance passes the file it was written for. FFmpeg
    records its whole configure invocation as ONE string, so the unavoidable
    `--cross-prefix=/…/webos-ndk/…` sits beside an offending `--prefix=/…/project/…` and one
    allowed token vouches for the other (v0.2.1's libraries pass that test). So: per PATH.
  * tokenising without the leading boundary makes plex.tv's `/api/v2/home/users` read as
    `/home/users`. So each path is extracted WITH its boundary character, which is then dropped.

Usage as a script: `ci/hostpath.py FILE...` prints each offending path and exits 1 if any.
"""
import re
import sys

# The account name must start with a LOWERCASE letter. Account names on the machines that build
# this are lowercase (macOS short names, Linux `NAME_REGEX`), and `ci/check-elf.sh` and
# `ci/verify-published.sh` already scanned that way. Without it, Jellyfin's `/Users/Me` endpoint
# was read as a Mac home directory whenever the linker put path-like bytes after it: Rust packs
# string literals with no separator, so `/Users/Me` + `/library/sections…` (#10) and
# `/Users/Me` + `Accept` (the v0.8.0 source rebuild) each failed a release. An allowance for
# `/Users/Me` followed by `/` or the end could not hold: what follows depends on the link.
# macOS's own `/Users/Shared` and `/Users/Public` fall outside it too, rightly.
HOSTPATH = re.compile(rb"(?:^|[^A-Za-z0-9/_.-])(/(?:Users|home)/[a-z][A-Za-z0-9_./+-]*)")
# The NDK's own location cannot be removed — `--cross-prefix` must be absolute (the wrapper gcc
# dies when invoked through PATH), so it rides in FFmpeg's recorded configure string. It is
# identical on every CI runner, which is the reason releases must be BUILT by CI.
ALLOWED_PATH = re.compile(rb"webos-ndk|^/home/runner/")


def build_machine_paths(blob: bytes) -> list:
    """Every build-machine path in `blob`, sorted and de-duplicated."""
    return sorted({m for m in HOSTPATH.findall(blob) if not ALLOWED_PATH.search(m)})


def main(argv) -> int:
    dirty = False
    for name in argv:
        with open(name, "rb") as f:
            for path in build_machine_paths(f.read())[:3]:
                print(path.decode(errors="replace"))
                dirty = True
    return 1 if dirty else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
