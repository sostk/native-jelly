# U4 scrub preview verification — 29 October 2026

U4 is implemented on `feat/scrub-previews`, based on local main `50a24294`. It adapts
[PlxNative #767](https://github.com/GLinnik21/plx-native/pull/767) to Jellyfin trickplay sheets.
The available webOS 6.5.3 TV passed scoped image/absence/cancellation acceptance. Actual-server thumbnail selection, display and release cleanup also pass the follow-up below.
No performance gain or broader firmware/codec result is claimed.

## Initial implementation contract (35c4ebbf)

Visible D-pad previews, held scrubbing and pointer drags can show a thumbnail above the title and
control row, following the destination within the playbar margins. Immediate hidden-HUD jumps
keep their existing behavior. Images do not own seek, Pause, focus or progress reports. Missing
trickplay draws no box and leaves ordinary seeking available.

The actual negotiated item/MediaSourceId selects the manifest, so another version's timing is
never a fallback. Floor-based interval/tile mapping handles sheet boundaries and the final valid
thumbnail. Workers fetch under the catalog's existing credential policy using the MediaBrowser
header, with three-second optional reads, 512 KiB manifest and 4 MiB compressed-sheet bounds.
Layouts above 4096 pixels per axis or 6,291,456 total pixels are refused. One compressed sheet per
existing poster worker is retained; only a cropped tile is converted to RGBA and uploaded through
the normal budget. There are no preview disk entries or full-sheet GPU textures.

The memo keeps one logical pending request and coalesces movement to the latest destination.
Layout facts quantize subsequent requests to the true server interval. Old mismatched stills hide
after 400 ms; missing later tiles stay quiet; a deferred/stuck request retires after seven seconds.
Release/hide/unmount/reload and changed ownership retire pending work and shown textures. Source
slot generations reject late completions. Residency-only reads prevent old evicted stills from
starting an extra untracked request. The memo remains a render resource, with no canonical
screen-shape or fixture expectation change. See [the implementation contract](../jellyfin-trickplay.md).

## Initial clean committed gates

All checks below ran on clean committed tree `35c4ebbfb495a0557f83bd2488c4d7b8cd8c2094`; the tree remained clean.
A later verification-documentation commit changes no application source.

| Check | Result | Elapsed |
|---|---|---|
| host | exit 0 | 569.44 s |
| shipping | exit 0 | 24.01 s |
| debug-arm | exit 0 | 74.58 s |
| elf | exit 0 | 18.23 s |
| sim-build | exit 0 | 43.04 s |
| fixture-policy | exit 0 | 6.15 s |
| strict-replays | exit 0 | 59.67 s |

Exact default and hostsim application results:

```text
test result: ok. 5109 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 71.02s
test result: ok. 5202 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 64.47s
```

The 24 ignored tests were not counted as executed coverage. Shared-crate, structural and Python
checks also passed. The region-decoder regression passed separately, checking exact cropped
pixels and malformed/out-of-bounds inputs. Tracked tests include authenticated real HTTP,
compressed-sheet reuse, missing manifests, cancellation before sheet fetch, queued/loading
source retirement, version isolation, overflow/size rejection, stale/missing-image handling,
timeout wrap, one-request coalescing and safe card geometry.

All six strict Targets/Resolve replays passed unchanged, with zero differences in every counter:
358 Home, 839 content-return and 1,291 Settings frames per mode. No anchors were rerecorded or
weakened. Shipping used `--no-default-features`; ARM used `FLAVOR=debug` and passed ELF checks.
No dependencies, native ABI declarations, firmware offsets, version or release settings changed.

## TV acceptance

The debug binary MD5 was `ffa5d56e299a658751672871e6cdf764`, matching the TV copy. A synthetic authenticated
Jellyfin server supplied three 2×2 sheets at ten-second intervals, including labelled coloured
cells and synthetic ASS media. The test exercised real Load, video-plane binding, input dispatch,
HTTP/decode and budgeted texture upload.

Four complete 1920×1080 captures were individually inspected after the lease was released:

- At the end clamp, the live 1:57 seek target shows **PREVIEW 11 / 01:50**, the correct final tile
  from sheet 2. The card stays inside the right safe margin and above the title/control row.
- At the start clamp, live rewind shows **PREVIEW 00 / 00:00** at the left safe margin. Up Next and
  the existing title/control layout remain visible without overlapping the preview card.
- After release, the card is absent while the ordinary seek-loading UI is present. This capture
  verifies thumbnail cleanup; it is not a separate claim about seek completion latency.
- A movie with no trickplay keeps its ordinary seek UI and shows no placeholder or stale card.

Colour samples corroborate the selected red/purple cells. The held-key producer was asserted
alive during both live captures. Resting at one tile made no additional sheet request. The run
issued 3 manifest requests and 10 sheet requests across forward/reverse seeking,
missing trickplay and the cancellation case. The per-worker caches can each fetch the same sheet;
these counts are not a cross-worker network deduplication claim.

A separate slow-manifest leg returned to Home with Back before the optional response completed;
its cancelled result started no sheet fetch. Sound commands were never used. Handback confirms
no debug process, zero armed triggers and a free TV lock; the panel state was restored.

### Driver correction

The first capture run was invalid for visual acceptance: this firmware's `sleep` rejected
fractional seconds, so the input burst ended before the screenshots. Its original machine summary
is not counted as a visual pass. The driver was changed to host-paced stdin and explicit
live-gesture assertions, then repeated on the same unchanged application build. The invalid-run
verdict is retained separately alongside the accepted numerical evidence.

## Actual local Jellyfin and limits

A read-only check signed in with the supplied test account and sampled 24 Movie/Episode
items. **0 exposed trickplay manifests.** It did not play household media, generate
thumbnails or change library settings/watch state. At that checkpoint actual-library thumbnail display was not
claimed; authenticated image serving, version selection and pixels were verified using the
synthetic HTTP/TV fixtures. Libraries need generated trickplay data to show a thumbnail.
The later follow-up below supersedes that availability limitation.

No TV frame-pacing/GPU benchmark, worst-case whole-app memory measurement, legacy webOS 4.x
Segment test or broader codec/HDR matrix was performed for U4. The decoder/network/cache limits
are enforced bounds, not a measured whole-process memory ceiling. The prior U3 playback/seek
regressions remain covered by the full host suite; this work changes only optional preview art.

The documentation contradiction audit found the historical BIF gap entries in `parity-gaps.md`;
a dated Native Jelly update now distinguishes those findings from the implemented Jellyfin path.
The live seek guide describes when previews appear. U4 stays local in focused commits; it has
not been merged into main or pushed.

## Commits

- `1d2fb623` — Jellyfin addressing, negotiated version identity and bounded tile decoding.
- `d10c9e53` — poster-worker integration, cancellable preview card and regression coverage.
- `35c4ebbf` — behavior/resource contract and historical-claim corrections.
- The final documentation-only commit records these acceptance results.

## Actual-server follow-up after generation was enabled

The user enabled trickplay and started generation, then requested a retest. A paginated read-only
scan with the supplied dedicated test account found **3 of 247 Movie/Episode items** with
trickplay manifests. Generation-task progress was unavailable to this account. One available
video supplied 711 thumbnails at ten-second intervals, arranged in 320×180 cells on 10×10 sheets.
Authenticated sheet retrieval succeeded. This test did not alter library settings or start jobs.

The unchanged debug application (`a80f5894`, documentation-only successor of the gated source;
MD5 `ffa5d56e299a658751672871e6cdf764` matched on the TV) negotiated actual Jellyfin playback.
A stationary pointer drag displayed two different thumbnails, then removed the card on release.
Both full-screen captures were inspected privately after releasing the TV lease. Pixel comparison
against the actual generated JPEGs independently confirms the expected floor-based selection:

| Destination | Expected sheet / cell | Best matching cell | Mean squared RGB error |
|---|---|---|---|
| 10:33 | 0 / 63 | 63 | 0.340 |
| 27:42 | 1 / 66 | 66 | 0.169 |

A transparent local HTTP relay forwarded actual server responses unchanged for this run. It
recorded one successful manifest request (162 ms) and two successful sheet requests (58 / 66 ms,
795,598 / 730,625 bytes). These are individual request observations, not end-to-end latency or
performance benchmarks. No library titles, media identifiers, tokens or actual-library captures
are included in the public evidence.

The earlier fast-moving held-FF captures in this follow-up showed no card; they are **not** counted
as a successful actual-server rapid-scrub test. The stationary pointer test establishes actual
thumbnail selection and cleanup, while visibility during sustained rapid movement remains a
limitation of this acceptance scope. The captures also show the player's buffering indicator:
native Load/video-plane binding was observed, but these runs do not establish uninterrupted video
playback, transcoding throughput or seek completion latency.

The test used only the dedicated account for playback and its normal session reports. The debug
app was closed, triggers cleared, panel state restored and TV lock released afterward. No sound
commands were used. No application source, main merge, push, version or release changed.

## Rapid-scrub reliability follow-up

The follow-up implements two separate product changes and one test-only correction:

- `343bd4a5` restarts the finite stale-image grace window when a fresh worker result arrives.
  Previously the timer could expire before the first image and hide later completions throughout
  continuous movement. The new regression failed first (`None` instead of `Some(0)`), then passed;
  unchanged mismatched images still expire after 400 ms.
- `f35e5a38` replaces per-worker compressed caches with **one shared decoded RGB sheet**. Both
  workers crop the same pixels without re-entering the decoder or fetching the same cached sheet.
  The old allocation is evicted before replacement fetch/decode. The retained pixel bound is
  **18 MiB**, replacing the former 8 MiB retained-compressed bound; compressed responses remain
  temporary and bounded to 4 MiB. Image resolution is preserved and only tile-sized RGBA reaches
  the GPU. Decoder scratch, response bytes and tile buffers are additional transient allocations.
  Tests cover one decode across repeated crops, cross-worker HTTP reuse, account/sheet invalidation,
  failed/cancelled replacement, exact pixels, overflow and malformed-image bounds.
- `0ced2d2e` fixes test interference exposed by the full run. The queue-present test wrote a
  process-wide pending-byte atomic without the shared test lock, despite its cache being
  thread-local. Its 16-byte write raced a different test's expected 32-byte worker snapshot.
  The failed hostsim run (5,204 passed / 1 failed / 24 ignored) is retained. The competing test
  now takes the shared lock and republishes its cleanup through `mutate_cache`. Product behavior
  is unchanged by this commit.

### Final committed gates

All final gates ran on clean commit `0ced2d2e2798c37e4e123cf32946d6e71e68a0b2` and left it clean.
Later documentation changes no application source.

| Check | Result | Elapsed |
|---|---|---|
| host | exit 0 | 640.36 s |
| shipping | exit 0 | 27.52 s |
| debug-arm | exit 0 | 95.27 s |
| elf | exit 0 | 22.10 s |
| sim-build | exit 0 | 55.63 s |
| fixture-policy | exit 0 | 6.27 s |
| strict-replays | exit 0 | 88.82 s |

```text
test result: ok. 5112 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 77.11s
test result: ok. 5205 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 79.81s
```

Both application feature sets passed; ignored tests are not executed coverage. All six strict
replays remained identical (358 Home, 839 content-return and 1,291 Settings frames per mode, zero
in every difference counter). No expectation was rerecorded or weakened. Shipping, debug ARM,
ELF, simulator build and fixture policy also passed. No dependency, native ABI, firmware offset,
version or release setting changed.

### Actual-server TV grading

The webOS 6.5.3 debug binary MD5 `9fb092a60398b73c0fd418f9e3d1605d` matched the TV copy.
The dedicated account and actual generated sheets were used through a direct SSH TCP forward,
without an HTTP relay or automatic pause trigger. The driver asserted that the held-key producer
was alive during each capture. Captures were individually inspected after releasing the lease;
actual-library images and identifiers remain private.

The timer-only build restored visibility, but its three captured thumbnails lagged the accelerated
cursor by 108 / 127 / 89 seconds of film time. This motivated the shared decode rather than
counting visibility alone as complete acceptance. The final cache build produced:

| Capture | Cursor | Matched sheet / cell | Thumbnail time | Current ten-second interval? |
|---|---|---|---|---|
| First | 4:55 | 0 / 29 | 4:50 | yes |
| Cold boundary | 17:19 | 0 / 99 | 16:30 | no: previous sheet's last tile |
| Later | 29:04 | 1 / 74 | 29:00 | yes |

Pixel mean-squared RGB errors were 0.151 / 0.729 / 0.789 against the actual JPEG tiles. The middle
card sits at a fractional horizontal coordinate; comparison includes its half-pixel bilinear
sampling rather than treating that rasterization as a different image. Two captures match the
current interval exactly. At the cold boundary the preceding sheet's last image remains visible;
the later capture is current again. The 49-second middle difference is **film time during fast
scrubbing**, not a measured 49-second wall-clock/network delay. Cold-sheet transitions retain a
brief older still or gap; these captures do not measure that duration or prove every frame current.
The 400 ms stale rule is covered by the memo tests. This is scoped rapid-scrub acceptance with an
explicit cold-boundary limitation, not a latency distribution or a zero-delay claim.

Release removed the card and ordinary playback resumed. An unpaused control advanced **14 seconds
across 15 heartbeat samples**; after the final seek, another 9 samples advanced 8 seconds. Decoded
video was visible without the buffering overlay in the control and post-release captures. The
server reported a 3840×2160 H.264/AC3 conversion. Thus persistent buffering was **not reproduced**
in these short controls. Earlier indefinitely paused captures were not evidence of uninterrupted
playback failure; this run does not establish the sole cause of the earlier display or long-session
transcoding reliability. No server settings/jobs were changed and no owner account was used.

Selected debug-process observations (KiB):

| Point | VmRSS | VmHWM |
|---|---|---|
| Before held scrub, after reset seek | 119,100 | 121,024 |
| First held capture | 127,376 | 127,376 |
| Later held capture | 128,292 | 130,176 |
| After release | 124,516 | 130,176 |

The reported high-water RSS is about **127.13 MiB** for this process/run. These observations are not
a before/after memory benchmark, a worst-case whole-app ceiling or total TV/video-service memory.
The retained RGB cache's enforced bound remains distinct from process memory.

Handback confirms the debug process closed, zero armed triggers and a free TV lock. Panel state
was restored; no sound commands were used. The verification checkpoint was recorded on `feat/scrub-previews`; its subsequent main landing
is recorded below. The next planned port remains U5: shared SRT/VTT
subtitle rendering and legacy text-encoding detection. Broader firmware/GPU/codec work is separate.

## Main integration — 29 October 2026

At the user's request, U4 landed on main as six focused commits rather than one large squash.
The first two combine the backend and the UI/documentation work separately; subsequent fixes,
performance work and the test correction keep independent review points. Earlier no-merge/no-push
statements describe their original checkpoints.

| Main commit | Verified source checkpoint | Scope |
|---|---|---|
| `6ff06f19` | `1d2fb623` | Jellyfin addressing and bounded tile decoding |
| `eef4f3df` | `d11796ac` | Cancellable scrub UI and initial verification documentation |
| `0ee543e5` | `343bd4a5` | Fresh-completion grace window |
| `ce68e474` | `f35e5a38` | One shared decoded RGB sheet |
| `28681eb6` | `0ced2d2e` | Test-only pending-byte counter serialization |
| This documentation commit | `09227f47` plus this integration note | Final verification record |

Before the integration note, main's entire Git tree matched source checkpoint `09227f47`
exactly. After the note, only verification documentation differs: application, tests, build inputs
and replay expectations remain identical to the final gated checkpoint `0ced2d2e`. The existing
host/shipping/ARM/ELF/replay/device evidence above applies to that unchanged content; no new test
run is claimed for changing commit ancestry. The working tree was clean after each landing.

The push also includes the previously landed local U2 (`d8044d37`) and U3 (`50a24294`) commits.
No private server configuration, actual-library capture or credentials are included.
