# Playback navigation verification — 28–29 October 2026

U3 is implemented locally: dedicated FF/REW taps, optional instant D-pad seeking and independent Previous/Next episode controls, adapted from [PlxNative PR #921](https://github.com/GLinnik21/plx-native/pull/921). Host, shipping, ARM, ELF and strict replay checks pass on final application commit `d437f01390c57832d35423486c2c40a7ede7e051`. Final TV follow-up passed after the user resumed testing. Scoped acceptance is complete on the available webOS 6.5.3 television; legacy webOS 4.x Segment playback remains unverified.

## Implemented behavior

- Dedicated FF/REW taps request the Skip interval on release, preserving Pause and keeping the HUD dismissed. Holding uses the existing continuous scrub and release behavior. Raw transport identity survives dispatch.
- Instant D-pad seeking defaults Off. When On, a hidden-HUD arrow Down jumps immediately; Repeat/Up add no seek. Visible-HUD preview and default hidden-HUD reveal remain available.
- Episode navigation defaults On and is independent of automatic Up Next. Previous/Next discs precede Subtitles/Audio/More for episodes with neighbours. Unavailable directions are dim and cannot take focus. Movies and neighbourless episodes keep the original controls. Previous restarts at or after five seconds; before five seconds it opens the preceding episode.
- Jellyfin adjacency is bounded and checked against the selected episode and series, with selected-only fallback for uncertain answers. Every transition refreshes the neighbourhood, retains stable item identities, and follows outgoing Stopped before incoming Started reporting.
- Both options use existing durable storage, shared settings/picker components and localization. No version, release, main merge or push was performed.

## Two regressions found during TV verification

**Rapid paused seeks.** The original candidate received two taps about 17 ms apart from roughly 5.5 s, requesting 15.5 s then 25.5 s, but restored Pause around 10.63 s. The watchdog adopted the newer request while an older keyframe was still queued. A host regression was observed failing before `b7609cec` and passing after it. The screen now retains only the newest additional destination until the preceding seek and paused-frame prime finish. Tests also cover Buffering, burst timeout, same-item reload including its Idle gap, background consumption into the saved resume offset, stop/error cleanup and a superseding scrub.

The strengthened TV retest on `aa73c4ce89e9d3387226096a7f4ed4abfef08b57` sent the second tap after the first demux seek had begun. NativeSeek accepted inputs **37 ms apart**, requested **15.505 s / 25.505 s**, restored the first paused frame before issuing the second seek, then restored the second paused frame at **20.838 s**. Reload performed two fresh Loads and restored the second paused frame at **20.963 s**. The synthetic file has sparse keyframes, so these are landed keyframes, not claims of exact target timestamps. HUD dismissal and Pause were retained. A separate same-frame burst correctly coalesced into one seek; the initial driver assertion requiring two seeks was corrected and its numerical evidence retained. The only subsequent application change is the storage adapter below; seek code is unchanged.

**Restoring preference defaults.** An earlier TV script selected defaults but did not verify them after relaunch. Strengthening it showed old non-default values returning. A host regression reproduced Instant D-pad On where Off was requested. The public-record merge copied omitted known default fields back from the previous snapshot. `d437f013` asks the canonical decoder whether an omitted name is an unknown extension and preserves only those extensions. Independent name probes retain opaque neighbours even when a recognized value is malformed. Nine policy tests pass, including both navigation defaults, other omitted defaults, future object/array/null values and recognized replacements.

## Final committed gates

All application gates ran on a clean committed tree; the tree remained clean afterward.

| Check | Result | Elapsed |
|---|---|---|
| host | exit 0 | 333.57 s |
| shipping | exit 0 | 12.88 s |
| debug-arm | exit 0 | 45.6 s |
| elf | exit 0 | 10.61 s |
| sim-build | exit 0 | 24.22 s |

Exact application-suite result lines:

```text
test result: ok. 5095 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 48.79s
test result: ok. 5188 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 48.16s
```

Shared-crate, Python and structural checks also passed. The 24 ignored application tests were not executed coverage. Shipping used `--no-default-features`; ARM used `FLAVOR=debug`.

Three synthetic anchors were naturally rerecorded after canonical queued-jump state changed. The guarded `rerecord` command preserved anchor flags and checked the closed alphabet; no fingerprint or expected ledger was hand-edited and no anchor was demoted. Full flows used shorter input cadence to stay within the existing decoded-memory bound. On final application commit `d437f01390c57832d35423486c2c40a7ede7e051`, **11 fixture-policy tests and all six complete Targets/Resolve replays pass**, with every difference counter zero. Their complete frame counts are 358 for Home, 839 for Filmography and 1,291 for Settings in each mode.

## TV and actual-server scope

The available TV runs **webOS 6.5.3**, with an exported video window and normally selected NativeSeek. Earlier scoped TV smoke on `832682c1` passed cross-season Next, Previous back to the original episode, restart after five seconds, disabled final direction, FF release, held REW, default D-pad reveal/debounce, real Load/video binding and outgoing stop-before-start reporting. Its deployed binary matched the local build. The later rapid-seek probe above verifies the seek fix; separated taps in the earlier smoke are not rapid-overlap evidence.

The earlier preferences run was interrupted when the TV switched to another app, and the user requested a pause. That timeout was not counted as either a playback failure or a pass. The other-app capture remains private and is excluded from retained shareable evidence.

After the user resumed testing, final navigation and preference smoke tests passed on clean documentation commit `b8dabc5fe701bc2e7bcc6cc82eec3845b6a834b6`, whose application sources are identical to gated application commit `d437f013`. The deployed binary matched the local binary (MD5 `dd8a1d750adbba38bc4802f5ca475f61`). These tests confirmed:

- Instant D-pad Down seeks with the HUD hidden; Repeat/Up add no seek. Episode navigation can be disabled independently, and FF remains functional with both options Off.
- Defaults restore durably after relaunch: Instant D-pad Off and Episode navigation On. Five episode controls return, and hidden D-pad input reveals the HUD without seeking.
- Cross-season Next, Previous back to the original episode, restart after five seconds, the disabled final direction, FF on release, held REW, default D-pad reveal and visible D-pad debounce pass on the final application build.
- Real Load/video binding and outgoing Stopped before incoming Started pass. The smoke's burst check exercises the native seek path; the earlier mode probe supplies the stronger 37 ms overlap evidence.

All seven final preference captures and four final navigation captures were individually inspected after the device leases were released. They contain synthetic media only. The navigation smoke uses a debug framedrop trigger, so its heartbeat FPS is not a production performance measurement.

Local Jellyfin test-account read-only API checks separately confirmed a two-episode neighbourhood and Next-then-Previous returning to the original episode. TV media was synthetic; this is not a claim that household media was played.

An earlier test incorrectly forced legacy Segment on this newer firmware and faulted with signal 11 in `libgstreamer-1.0.so.0.1602.0` after sendSegment. A fault event was collected, not a stack backtrace. The normal selector does not choose that path on this TV. The numerical summary is retained; it is not a compatibility pass or proof of a pre-existing bug. **Real webOS 4.x Segment testing remains pending.**

Inspected simulator settings captures retain a 601 ms stall warning and demonstrate layout only. Device captures include the hardware video plane. No performance gain, new GPU benchmark or broader codec/HDR coverage is claimed.

## Handoff and remaining hardware scope

The available TV's scoped navigation and settings checks are complete. Legacy Segment verification still requires compatible webOS 4.x hardware.

**Final debug preferences: Instant D-pad Off; Episode navigation On**, verified after relaunch. Read-only handback confirmation found **no debug process, zero test triggers and a free TV lock**. No sound commands were used; the user handled muting.

All implementation work remains in focused local commits on `feat/playback-navigation`, stacked on the local U2 branch. Main was not merged or pushed by this work.

The documentation contradiction audit searched 180 tracked Markdown files; 13 contained navigation/seek matches. Live guidance was corrected for immediate versus preview seeking, transport identity and the episode disc pair; nearby Rust comments no longer assert a fixed preference count or a universal disc primary of zero. Historical investigations remain dated records.

## Implementation commits

| Commit | Change |
|---|---|
| `872fe918` | feat(settings): add independent playback navigation preferences |
| `6c164c84` | feat(player): distinguish transport taps and accumulate immediate seeks |
| `7d93784e` | feat(player): navigate adjacent Jellyfin episodes from the HUD |
| `02fa2cb7` | test(settings): exercise both playback navigation pickers |
| `832682c1` | test(ui): rerecord synthetic anchors for playback navigation state |
| `b7609cec` | fix(player): serialize rapid immediate seeks until the paused frame lands |
| `d5127a3c` | docs(player): describe immediate seeks and episode navigation controls |
| `aa73c4ce` | test(ui): rerecord anchors for serialized immediate seeks |
| `d437f013` | fix(settings): clear recognized preferences when restoring defaults |
