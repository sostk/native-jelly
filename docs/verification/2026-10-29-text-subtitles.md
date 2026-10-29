# U5 shared text subtitles — 29 October 2026

Implemented on `feat/shared-text-subtitles`, based on main `2c6f951b`. Application,
tests and build inputs were gated on clean committed tree
`705b1aff65ddc712d42229320e893e64e1e3ba1c`. The verification-documentation successor
changes no application, test or build input. No U5 main integration or push is claimed.

## Scope

Embedded SRT/SubRip, WebVTT and mov_text, and external SRT/WebVTT, use the shared
converter and bundled libass worker. Legacy encoding detection is per embedded track
and whole sidecar, with BOM precedence and language hints. Authored ASS keeps its own
styles and video viewport. Synthesized text uses viewer size/position and the UI canvas;
bottom dialogue clears the transport while top signs stay fixed.

The port adapts upstream charset and shared-sidecar work while preserving the older
long-cue resume behavior and local native limits. Embedded raw packets and expanded
views are bounded together. Conversion, hashing and expanded payload construction run
outside the source-store lock; publication rechecks media ownership and current style.
See [the behavior and resource contract](../text-subtitles.md).

## Committed host and ARM checks

| Check | Result | Elapsed |
|---|---|---|
| `make check` | exit 0 | 717.80 s |
| Shipping `--no-default-features` check | exit 0 | 36.36 s |
| `make -j4 FLAVOR=debug` | exit 0 | 110.88 s |
| `ci/check-elf.sh` | exit 0 | 25.79 s |
| Link evidence, app/storage/crash helper | exit 0 | 2.25 s |
| Link evidence, five bundled ARM libraries | exit 0 | 0.17 s |

Exact default and hostsim application results from `make check`:

```text
test result: ok. 5180 passed; 0 failed; 26 ignored; 0 measured; 0 filtered out; finished in 77.71s
test result: ok. 5273 passed; 0 failed; 26 ignored; 0 measured; 0 filtered out; finished in 89.36s
```

Shared-crate, structural, C and Python checks also passed. Ignored cases are not counted
as executed coverage. Separate scoped runs executed:

- Player unit coverage: 434 passed, 0 failed, 10 ignored.
- Pinned host libass integration: 18 passed, 0 failed, including external/embedded exact
  pixel parity at overlapping-cue boundaries and backward seeks, plus authored ASS cases.
- Bundled host FFmpeg: 2 passed, 0 failed, including the small committed GBK SubRip fixture.

The charset regression artifact failed the previous lossy decoder for UTF-16 and
Windows-1252. The source-lock regression was observed failing before the lock change
and passing afterward; related tests cover style changes and media replacement during
view construction. These are functional/concurrency checks, not a measured FPS gain.

The initial full-host failure was a verification-driver environment error: inherited
umask `0002` made private storage fixtures unsafe. The accepted run sets umask `0022`.
The initial C fixture also needed the new private line-spacing stub; that correction is
included in the gated renderer commit.

## Native compatibility review

**PASS; graded the current ARM ELF and all five bundled ARM libraries.** The application
retains 15 `DT_NEEDED` entries and has 353 strong undefined dynamic symbols after bundled
resolution. Link input/SHA-256 evidence passed for app, storage helper, crash helper,
libass, avutil, avcodec, avformat and swscale. Every gated firmware inventory from 4.4.2
through 11.2.0 passed; older rows remain non-gating.

The new `plx_ass_set_line_spacing` declaration matches C `double` and Rust `f64` under
the ARM soft-float ABI. The private facade requires ABI 3; it loads all symbols before
publication from the application's absolute asset directory. No libass upstream struct
crosses that facade, no firmware import was added, and C frame layouts are unchanged.
The FFmpeg language lookup copies through the existing pinned dictionary seam and adds
no FFmpeg symbol. The two encoding crates are pure Rust. ELF identity, CP15 regression,
host-path/private-IP and unchanged-linkage assertions passed.

These matrices establish loader/symbol compatibility, not playback on every firmware
or correctness of firmware structs and payloads. Device acceptance below is on one set.

## TV acceptance

The available webOS 6.5.3 TV ran the debug build; the session driver verified that its
deployed binary matched the local gated build. Seven synthetic authenticated Jellyfin
cases passed native Load, hardware video-plane binding, renderer-open and no subtitle
error/crash checks:

| Case | Additional evidence |
|---|---|
| UTF-8 external SRT | Ready sidecar; italics/accent/top sign; transport lift |
| External WebVTT | Ready sidecar; entities/italics/`line:0` top sign |
| Windows-1252 external SRT | Ready sidecar; French accents visibly correct |
| UTF-16 BOM external SRT | Ready sidecar; decoded styling and accents |
| Embedded GBK SubRip | Chinese characters visibly correct |
| Embedded UTF-8 SubRip | Pause/resume accepted; seeks to 60 s then 15 s; captions return |
| Authored ASS control | Renderer remained active; separate paused visible-cue capture |

Full 1920×1080 DISPLAY captures were inspected after handback. Top signs stay in place
while bottom dialogue rises above the HUD. The authored control retains its green
positioned sign and bottom dialogue at its authored position. Its first capture fell in
an intentional blank interval and was not counted as visible-caption acceptance; a
separate paused run captured the static cue.

The first synthetic attempt inherited subtitle Off and did not exercise the renderer.
The accepted run explicitly selected each track. The unchanged app was retested after
correcting the driver; the earlier attempt is not counted as a subtitle pass.

Scoped debug-process observations across the seven cases: RSS 96,896–103,544 KiB,
highest observed VmHWM 123,292 KiB (about 120.40 MiB). These are individual process
observations, not a before/after memory benchmark, worst-case ceiling, or total TV memory.

## Real local Jellyfin

The supplied dedicated test account was identity-checked. A read-only sample found
95 of 100 Movie/Episode items with text subtitles and no external text sidecars. One
H.264 item played through the real server with an embedded SubRip track, native Load,
video-plane binding, a seek and accepted pause. The final held capture visibly confirms
the actual embedded caption above the transport; 12 native render calls were recorded.
No owner account, library settings or generation jobs were used. Credentials, identifiers,
titles, dialogue and actual-media captures remain private.

The first real capture after seeking fell before the next demuxed cue and showed no
caption. Embedded retention preserves known packets within its bounds; this acceptance
does not establish recovery of every previously unread cue spanning an arbitrary seek.
The final visual check held a later demuxed cue. Driver scheduling/assertion mistakes
were corrected without application changes.

Each device run closed the debug app, removed injected automation, stopped its tunnel,
restored the panel state and released the TV lease. No volume or mute commands were used.
No long-session, exhaustive charset/codec, whole-app GPU/frame-pacing or multi-firmware
playback benchmark is claimed.

## Build benchmark

Both runs used `make build-bench-quick` on the same Linux x86_64 host (12 cores),
Rust nightly 1.101.0, with one sample per scenario and non-incremental builds.

| Scenario | Main `2c6f951b` | U5 `705b1aff` | Rebuilt units |
|---|---|---|---|
| Warm no-op host test build | 0.50 s | 0.31 s | 0/161 → 0/163 |
| Leaf edit/rebuild (`nj_base::cbuf`) | 128.17 s | 91.82 s | 11/161 → 11/163 |
| Existing target directories | 2,419.8 MB | 2,899.8 MB | Four directories each |

The baseline leaf sample warned of load 13.90 above 12 cores; the after run had no
noise warning. These single samples do not establish a build-speed improvement. Target
directory sizes include different accumulated build artifacts, not just the new crates.
The two extra compilation units are the charset dependencies. Both accepted benchmark
runs restored their source edits and reported a clean tree. An interrupted attempt left
only its temporary comment; it was explicitly restored before the accepted rerun.

## Reviewable commits

- `116481c3` — per-track and sidecar legacy decoding.
- `f0b726ef` — shared libass text rendering and regression coverage.
- `705b1aff` — build embedded views outside the UI source lock, including race tests.
- The documentation-only successor records this verification and corrects historical claims.
