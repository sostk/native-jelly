#!/usr/bin/env python3
"""ci/hostpath.py: what counts as a build machine's path inside a shipped binary."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from hostpath import build_machine_paths  # noqa: E402


def found(blob: bytes) -> list:
    return [p.decode() for p in build_machine_paths(blob)]


class BuildMachinePaths(unittest.TestCase):
    def test_a_source_path_inside_a_home_directory_is_found(self):
        for path in ["/Users/synthetic_builder/work/src/main.rs", "/home/synthetic_builder/proj/x.c",
                     "/Users/me/project/ffmpeg"]:
            with self.subTest(path=path):
                self.assertEqual(found(b"\0" + path.encode() + b"\0"), [path])

    def test_ffmpegs_configure_blob_cannot_vouch_for_its_own_prefix(self):
        blob = (b"--cross-prefix=/Users/synthetic_builder/webos-ndk/bin/arm- "
                b"--prefix=/Users/synthetic_builder/project/build")
        self.assertEqual(found(blob), ["/Users/synthetic_builder/project/build"])

    def test_an_api_path_ending_in_home_users_is_not_a_home_directory(self):
        self.assertEqual(found(b"https://plex.tv/api/v2/home/users"), [])

    def test_the_runner_and_the_ndk_are_allowed(self):
        self.assertEqual(found(b"\0/home/runner/work/x/y.rs\0"), [])

    def test_jellyfins_signed_in_user_endpoint_is_not_a_home_directory(self):
        # Rust packs string literals with no separator, so what follows `/Users/Me` is whatever the
        # linker placed next: a path (`/library/…`, seen in #10) or a header name (`Accept`, seen in
        # the v0.8.0 source rebuild, which failed on `/Users/MeAccept`).
        for neighbour in [b"", b"\0", b"/library/sections", b"Accept", b"AcceptX-Emby-Authorization",
                          b"application/json"]:
            with self.subTest(neighbour=neighbour):
                self.assertEqual(found(b"\0/Users/Me" + neighbour), [])

    def test_only_a_lowercase_account_name_is_a_home_directory(self):
        # Build machines' account names are lowercase; macOS's own shared folders are not accounts.
        self.assertEqual(found(b"\0/Users/Shared/x\0/Users/Public/y\0"), [])


if __name__ == "__main__":
    unittest.main()
