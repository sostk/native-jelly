# U4 scrub preview verification — 29 October 2026

U4 is implemented on `feat/scrub-previews`, based on local main `50a24294`. It adapts
[PlxNative #767](https://github.com/GLinnik21/plx-native/pull/767) to Jellyfin trickplay sheets.
The available webOS 6.5.3 TV passed scoped image/absence/cancellation acceptance. Actual-server thumbnail selection, display and release cleanup also pass the follow-up below.
No performance gain or broader firmware/codec result is claimed.

## Delivered behavior

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

## Clean committed gates

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
