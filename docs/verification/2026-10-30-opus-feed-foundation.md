# C3 Opus feed foundation — 30 October 2026

Gated source: `d377a00a234eec011afd99d8c91cb551726fd08e`.
The working tree was clean before and after the gates. The following documentation
commit changes no Rust/C source, dependency, configuration or build recipe.

This verifies the [staged feed foundation](../opus.md), not enabled Opus direct play.
No TV was deployed, driven or captured, and no household server settings were changed.

## Regression and packet evidence

The declaration regression was observed failing before implementation:
`audio_payload_codec("opus")` returned `None` instead of `Some(("OPUS", 6))`.

Focused tests on the committed source, with the host build of the project's bundled
FFmpeg selected through `NJ_FFMPEG_DIR`:

```text
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 5224 filtered out; finished in 0.78s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 231 filtered out; finished in 0.01s
```

The first group includes the normally ignored real-demux integration test; the second
is the platform rollout guard. The fixture uses the local ffmpeg CLI, a loopback HTTP
server, the bundled libavformat and production AU queues. It compares concatenated
received packet bytes with the original copied audio packets, not just liveness:

| source | declared channels | result | audio bytes received |
|---|---:|---|---:|
| Opus mono | 1 | exact raw bytes | 9,559 |
| Opus stereo | 2 | exact raw bytes | 14,864 |
| Opus mono | 2 | refused | 0 |
| Opus stereo | 1 | refused | 0 |
| Opus 5.1 | 2 | refused | 0 |
| AAC stereo | 2 | refused | 0 |

All refused cases also assert zero video bytes queued. Unit checks cover exact header
bytes, truncated/malformed headers, timing/gain mismatch, unknown/surround widths,
one JSON node, unchanged video codec declarations, carried-track identity and withholding
Opus from normal/forced routing and server profiles. The raw packet integration check
is explicitly run here; it remains ignored in the ordinary host gate because it needs
the host bundled libraries and ffmpeg CLI.

## Complete host and ARM checks

```text
test result: ok. 5204 passed; 0 failed; 27 ignored; 0 measured; 0 filtered out; finished in 60.90s
test result: ok. 5300 passed; 0 failed; 27 ignored; 0 measured; 0 filtered out; finished in 57.06s
```

These are the default and hostsim application suites inside `make check`. The shared
crate suites, lint, structural and Python gates also passed. End-to-end checks:

| check | result | seconds |
|---|---|---:|
| `make check` | pass | 486.35 |
| shipping `--no-default-features` | pass | 19.52 |
| debug ARM build | pass | 74.93 |
| ELF assertions | pass | 14.54 |

Link evidence plus firmware compatibility passed for all eight shipped application,
helper and library ELFs against the offline firmware inventories from 4.4.2 onward.
The stripped debug application is 13,476,800 bytes, within the 13,810,000-byte budget.
Unstripped ARM application SHA-256:
`980c3fe2eb1cb6f9a7b3f5ed9f285f82bd009f20399165e0b9225b68ce552c09`.
This is an absolute budget check, not a comparative performance measurement.

Manual seam review: no native symbol, dependency or struct-layout changes; existing
FFmpeg fields are used with a nonnull, bounded header slice. No build benchmark is
required because build inputs/module layout did not change. Privacy review found no
new findings: added lines scan clean, and the two full-file token-template findings
are byte-identical to `origin/main`. Prose audit updates the implementation vocabulary
while retaining the normal route eligibility set.

## Limits

The TV decoder, sound, panel continuity, native seek and track switching are untested
for Opus. Real Jellyfin negotiation/recovery, firmware/rank restrictions, persistent
refusal learning and per-attempt capability projection are still pending. The source
header subset is deliberately narrower than upstream's surround header synthesis.
No new direct-play capability should be advertised from this foundation alone.
