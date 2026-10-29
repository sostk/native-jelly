# C2 AV1 Dolby Vision Profile 10 — 30 October 2026

Implemented on `feat/av1-dolby-vision`, stacked on C1 `b976caee` and U5.
The clean committed source checked below is `c876814fb8e9fc958318d662d24ee4f3b53a6e99`. The later evidence commit changes
documentation only. No main merge, push, version bump or release occurred.

**TV acceptance remains pending at the user's request.** No TV commands ran for C2.
Neither picture quality, Dolby Vision panel mode, playback stability nor performance is
claimed as device-verified. C1 Profile 7 acceptance also remains pending independently.

## Behavior and regression coverage

This adapts upstream PlxNative `3ac9e03f` / #838: AV1 keeps its AV1 codec declaration and
receives a single-layer, clear `DolbyHdrInfo` Profile 10 node on confirmed DV hardware.
An AV1 decoder capability is independently required. Existing AV1 temporal-unit framing
already preserves metadata OBUs; no AV1 bitstream converter or new dependency is introduced.

Native Jelly additionally matches HEVC with Profiles 5/7/8 and AV1 with Profile 10 at planning,
installation and payload construction. An AV1 Profile 7 mismatch cannot arm the HEVC RPU
converter. Dual-layer AV1 and unsupported codec/profile declarations are rejected or left
undeclared under the existing base-layer policy. Unknown/unsupported DV capability cannot
emit a node; a cross-compatible Profile 10 base may play without it, whereas a source without
a usable base requires video encoding. Decoder size limits continue to apply.

Jellyfin range-only AV1 Dolby metadata implies Profile 10 while explicit profile and EL facts
are preserved for refusal checks. Confirmed panel capabilities determine the compatible
HDR10/SDR/HLG range offers. Generic `DOVI` requires confirmed DV support; AV1 offers neither
EL ranges nor `DOVIInvalid`. An audio-only conversion preserves copied AV1 metadata and the
frozen decision; re-encoded video clears it. HLS video-copy restrictions remain in force.

The first Profile 10 payload regression was observed failing without the DV node before the
implementation, then passing. Focused checks passed 67 framing, 35 Dolby, 16 payload and
7 capability tests, with zero failures/ignored tests in those scoped runs. The complete gate
also covers the existing AV1/Jellyfin negotiation, recovery and decoder-bound tests.

The first full gate found one stale AV1 range-list expectation (5197 passed, 1 failed,
26 ignored). It expected the pre-port range list. A separate test commit updates that exact
expectation to include compatible DV bases. No acceptance threshold was relaxed.
The next full run passed default tests but hostsim found an unrelated
existing diagnostics fixture reading a leftover `seeking` flag (5290 passed, 1 failed,
26 ignored). Its separate test-only correction saves, clears and restores that flag under
the existing serial lock; the Idle assertion remains intact. The final full gate includes
that correction. Private failure evidence for both attempts is retained outside the repository.

## Committed host and ARM gates

| Check | Result | Elapsed |
|---|---|---|
| host | exit 0 | 574.9 s |
| shipping | exit 0 | 30.05 s |
| arm | exit 0 | 99.04 s |
| elf | exit 0 | 23.17 s |

Exact application results from default and hostsim:

```text
test result: ok. 5198 passed; 0 failed; 26 ignored; 0 measured; 0 filtered out; finished in 74.95s
test result: ok. 5291 passed; 0 failed; 26 ignored; 0 measured; 0 filtered out; finished in 65.96s
```

Ignored tests are not executed coverage. Shared-crate, C, Python, lint and structural checks
also passed. The shipping-feature check used `--no-default-features`; the ARM artifact is a
debug build, not a stable release.

## Compatibility and claim review

All eight shipped ELFs (application, storage helper, crash helper and five bundled libraries)
passed SHA-256/linker-input evidence and firmware inventory checks from 4.4.2 through 11.2.0.
Older inventory rows remain non-gating. ELF architecture and unchanged-linkage checks passed.
The stripped private debug copy is **13,542,352 / 13,810,000 bytes**, within the existing budget.
Dependency graphs remain 84 packages under the 87-package ARM budgets. C2 changes no Cargo,
build configuration, FFI declaration, firmware call, library candidate or FFmpeg layout.
A build benchmark is not required for this source-only port; no runtime speedup is claimed.
Loader compatibility does not establish AV1 Dolby Vision support on any individual firmware.

Manual seam review found no new ABI/import risk. The existing node's JSON types/keys are
retained; codec/profile consistency is independently checked before splicing it. A prose
contradiction audit updated the Dolby contract, AV1 capability comment and HEVC-only test
comment. Historical HEVC release notes remain historical claims.

## Device acceptance still required

A read-only local Jellyfin request using the authorized test account found 247 video
items, five with AV1 streams and **zero AV1 Dolby Vision/Profile 10 candidates**. No viewing
state was changed. A suitable sample and an AV1-capable set are needed before grading panel
mode, picture/colours, audio-only copy, pause/resume, forward/backward seeks, ongoing playback
and undeclared HDR10 fallback. The original documented dev panel has no AV1 decoder.
No automatic device run is scheduled.

## Focused commits

- `bda40234` — profile/codec policy, AV1 declaration, copied-video handling and regressions.
- `33fa20a1` — Jellyfin metadata/range negotiation and the Dolby contract.
- `65a8c0e1` — existing range assertion and test-prose correction.
- `c876814f` — isolated diagnostics seeking-state fixture correction.
- The documentation-only successor records these checks and deferred device acceptance.
