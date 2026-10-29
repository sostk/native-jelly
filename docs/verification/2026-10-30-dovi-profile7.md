# C1 Dolby Vision Profile 7 conversion — 30 October 2026

The implementation is on `feat/dovi-profile7`, stacked on the completed local U5 tree
`f11c8cce`. Host, shipping, ARM, ELF and firmware checks passed on clean committed source
`b9b1c1d3`. The later verification commit changes documentation only. Neither U5 nor C1
has been merged into main or pushed in this work.

**TV acceptance is pending.** The user stopped the first device run before its checks
completed, then requested host-only completion and deferred TV work until the next morning.
No partial device result is counted as a pass. No automatic TV run is scheduled.

## Conversion and negotiation

The port adapts upstream PlxNative `17eb48b9` / #825. It uses quietvoid's
`dolby_vision` 3.4.0 Rust crate, the implementation behind libdovi and dovi_tool, with
default/optional C API, XML and serde features disabled. An eligible single-track HEVC
Profile 7 stream becomes single-layer Profile 8.1: enhancement-layer NALs are discarded
and RPUs are converted using `ConversionMode::To81`. The HDR10 base survives; FEL residual
enhancement detail does not. This does not reconstruct or feed the full enhancement layer.

The local adaptation accepts compatibility id 6 rather than every nonzero id. The demuxer
independently requires P7, EL, base and RPU configuration flags before conversion. A missing
or contradictory configuration for a route expecting P7 conversion aborts before feed;
other unsupported dual layers also stop. Malformed/unconvertible RPUs and unexpected RPU
profiles are dropped; already-Profile-8 RPUs remain unchanged. Existing P5/P8 feed paths are
not armed for P7 conversion. Conversion counters belong to the demux instance.

Jellyfin range offers include the two dual-layer DV range names only when HDR10 capability
is confirmed. Precise item policy still gates the request. A supported DV panel is declared
as profile 8, single-layer; an HDR10-capable non-DV panel uses the base without that node.
HLS retains its existing no-video-copy rule. The expected conversion flag is frozen on the
main thread before spawning demux, rather than reading mutable route state from the worker.
See [the Dolby Vision contract](../dolby-vision.md).

## Exact committed gates

| Check | Result | Elapsed |
|---|---|---|
| `make check` | exit 0 | 623.15 s |
| Shipping `--no-default-features` | exit 0 | 27.36 s |
| Debug ARM build | exit 0 | 93.06 s |
| ELF assertions | exit 0 | 25.50 s |
| Eight-ELF linker input/SHA-256 evidence | exit 0 | 2.14 s |

Exact default and hostsim application results:

```text
test result: ok. 5192 passed; 0 failed; 26 ignored; 0 measured; 0 filtered out; finished in 71.82s
test result: ok. 5285 passed; 0 failed; 26 ignored; 0 measured; 0 filtered out; finished in 77.75s
```

Shared-crate, C, structural and Python checks also passed. Ignored tests are not counted
as executed coverage. Scoped runs passed 67 framing tests, 29 Dolby Vision tests, 14 payload
tests and 7 capability tests; the full committed gate includes those regressions again.
The original eligible-P7 metadata test was observed failing with `Refuse("dual-layer")`
before implementation. The MEL rewrite matches dovi_tool's mode-2 fixture byte for byte;
FEL output parses as profile 8. Other tests cover EL removal, malformed RPUs, unchanged P8,
reserved compatibility ids, missing flags, non-HEVC refusal, payload and route decisions.

The first full gate passed default tests but failed one existing hostsim search assertion:
`equal_generation_browse_owners_publish_their_own_search_scope`. A cold `session::peek`
may queue a refresh between two complete-key reads. The separate test-only correction
creates and settles a private session while preserving the original key-equality assertion.
The entire gate then passed. No search production behavior or acceptance threshold changed.

## Compatibility, dependency and size review

The current application, storage helper, crash helper and five bundled ARM libraries passed
their link-evidence checks and firmware inventories from 4.4.2 through 11.2.0. Older rows
remain non-gating. The application retains 15 `DT_NEEDED` entries and 353 strong undefined
dynamic symbols after package resolution. ELF32/ARM/soft-float/ARMv7, CP15-barrier, host-path,
private-IP and unchanged-linkage assertions passed.

There are no new firmware calls, dynamic-loader candidates or C ABI declarations. Existing
FFmpeg configuration-record offsets are unchanged. The converter is linked as Rust code;
the extra demux argument is a Rust-only frozen boolean. These checks establish loader and
symbol compatibility, not picture correctness or playback on other firmware.

The minimal-feature dependency adds 13 registry packages: both ARM graphs measure 84, up
from 71 including U5. Their budgets are deliberately 87 with three packages of headroom;
duplicate-version counts remain within the existing limit. Notices cover every added crate
and the three unmodified MIT-licensed dovi_tool regression assets. The existing stripped
debug-binary budget passes: **13,476,784 bytes / 13,810,000 bytes**. This is a current size
observation, not an isolated before/after C1 binary-growth measurement.

## Build benchmark

Both runs used `make build-bench-quick` on the same Linux x86_64 host (12 cores),
Rust nightly 1.101.0, with one sample per scenario and non-incremental compilation.
The before checkpoint `705b1aff` has the same application/test/build inputs as U5 `f11c8cce`;
the after checkpoint is the committed C1 source `b9b1c1d3`.

| Scenario | Before C1 | After C1 | Rebuilt units |
|---|---|---|---|
| Warm no-op host build | 0.31 s | 0.32 s | 0/163 → 0/178 |
| Leaf edit/rebuild (`nj_base::cbuf`) | 91.82 s | 125.95 s | 11/163 → 11/178 |
| Existing target directories | 2,899.8 MB | 3,419.3 MB | Four directories each |

The after leaf sample warned of load 13.10 above 12 cores. The before sample had no
noise warning. These single samples do not establish a build-time regression or improvement;
repeated measurements under comparable load would be needed. Directory sizes include
accumulated artifacts and are not an isolated dependency footprint. Both benchmark runs
restored their edits and reported a clean tree. Compilation-unit totals are separate from
the deterministic 84-package ARM graph count above.


## Deferred device work

A read-only scan using the provided dedicated Jellyfin test account found 20 Profile 7
candidates among 247 Movie/Episode items. This establishes fixture availability only, not
successful playback. No owner account, library setting or generation job was used. Private
credentials, identifiers, media names and captures are not included in this record.

The stopped debug run closed the app, cleared injected triggers, stopped its tunnel and
released the TV lock. The screen was already active when restoration was requested. Sound
settings were untouched. No more TV commands were issued after the host-only instruction.

The prepared next device run must verify actual HEVC copy rather than server video encoding,
P7 RPU conversion, video-plane output, forward/backward seeks and continuing playback. It
must read the panel's dynamic-range state for Dolby Vision and repeat with a diagnostic
unsupported-DV capability override to check HDR10 fallback on the same HDR10-capable set.
The override cannot prove behavior on every non-DV hardware model. Throughput, long-session
stability, FEL detail, other firmware and a broader codec/HDR matrix remain outside host proof.

## Focused commits

- `49e45051` — converter, route/payload policy, fixtures and dependency notices.
- `f46a0631` — Jellyfin range offers and the conversion contract.
- `b9b1c1d3` — isolated search test-fixture correction found by the full gate.
- The documentation-only successor records these host results and deferred TV acceptance.
