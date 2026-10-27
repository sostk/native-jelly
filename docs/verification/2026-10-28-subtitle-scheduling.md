# Subtitle scheduling TV acceptance — 28 October 2026

Tested source: `db324a5927b4d19963403964f8acb944e8eba93a`, based on main `ff9576d5`.
The debug install was tested on webOS 6.5.3. The deployed binary's MD5 matched the local
candidate (`aedc918641f5f325a6973527a9cf997a`). Stable was not deployed to. No release/version
change or GitHub push accompanies this verification.

## Behavior

Actual local Jellyfin playback used a dedicated test account and an H.264 1080p/AAC source
with embedded ASS. Four seek requests (120, 60, 130 and 90 seconds) plus pause/resume used
**one native renderer and one player Load**, with 83 position samples and no ASS renderer
error. Selecting another ASS track committed successfully and continued playback through a
further seek; matching native resources remained reusable.

Controlled authored ASS media covers a static cue, a true blank gap, fade, movement, karaoke,
font attachments and Japanese/Chinese/Korean glyphs. Both subtitle tracks, timestamps,
attachment metadata and interleaving passed `tests/fixtures/make_ass_fixture.py`'s verifier.
Full-display captures confirm static/gap transitions, Off/On, external ASS after explicit
selection, and composition with the hardware video plane. The external script survives
forward/backward seeks.

A full-display sequence shows 648 pixels of moving-text travel, five identical moving-region
images during an accepted pause, and motion after resume. The karaoke caption's red pixels
decrease 3,344 → 1,719 → 276 while white pixels increase 9,128 → 10,753 → 12,196. This analysis
uses the caption's own region and relative RGB because the transport HUD dims authored text.
The in-app JPEG stream timed out on this TV; none of its attempts supplies acceptance evidence.

## Cache lifetime correction

TV job diagnostics exposed expensive reinitialization after seeks. The screen's temporary
loading path called `ass::clear()`, retiring the renderer before resource matching could reuse
it. A regression reproduced that loss. `db324a59` separates suspension from Off: old frames are
fenced/hidden during a busy seek while bounded native resources remain available. Off, a missing
source and unmount still release the renderer. Native tests include the busy cleanup interval
between ten matching seek identities, and a fixed-label native-open event verifies the lifetime
on device.

The final real-server case above proves one renderer across four uninterrupted seeks. A
controlled recovery run separately issued four seeks: three completed natively and one existing
pipeline fallback made a fresh Load. Two native opens match those two player lifetimes; a true
unmount intentionally releases the cache. Final Off/On creates one object initially and a fresh
one after On, confirming that suspension has not made the cache immortal.

## Performance scope

The initial scheduling build `2c5129e9` was compared with main in an A–B–B–A sequence on the
same long static caption. Samples begin only after the reported playhead reaches the static
interval; a fixed warm-up was unsuitable because of keyframe preroll. Each sample reads
process CPU counters over about 21 seconds at the observed `getconf CLK_TCK` rate of 100.
Capture, forced presentation, GPU timer and HWCNT triggers are absent from measured runs.

| Run | Build | Seconds | Process CPU, one core |
|---|---|---:|---:|
| A1 | Main | 20.97 | 10.44% |
| B1 | Scheduling | 21.02 | 10.37% |
| B2 | Scheduling | 20.92 | 10.42% |
| A2 | Main | 21.07 | 10.35% |

Both averages round to **10.40% of one core**: this workload establishes no whole-app CPU
gain. Both builds have a 60 fps median UI present rate over 45 heartbeat samples each; this
is not the 24 fps fixture's hardware-video rate. The candidate's identified subtitle worker
records zero additional CPU ticks in both static windows, at about 0.048% of one core per
tick/window resolution. The baseline's threads are mostly unnamed, so its worker identity is
not guessed.

A final static smoke sample of the corrected build records 10.25% process CPU and zero new
subtitle-worker ticks over 20.97 seconds. It is not another A/B. These results demonstrate
static scheduling and scoped functional acceptance, not a general CPU or video-FPS improvement.

## Final gates and handback

The corrected application tree was committed and clean before and after `make check`, the
shipping-feature check, `make check-ass`, debug ARM build, ELF/link evidence, the offline
firmware load matrix for all eight artifacts, and six simulator replays. All passed.

```text
test result: ok. 5071 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 48.79s
test result: ok. 5164 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 47.50s
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 5078 filtered out; finished in 0.17s
```

The last line includes all nine real-libass tests. The host's stock compatibility probe marks
ThreadSanitizer unavailable; ASan/UBSan and the remaining compatibility checks pass.

Each device run held and released the TV lease. Handback confirms the debug app is closed,
zero test triggers remain, and the lock is free. The initial panel state was restored and sound
was left unchanged. Other firmware, the full codec/HDR and subtitle-format matrices, and
millisecond seek-to-first-cue timing remain outside this check. The pending UI-performance
branches are separate from this subtitle branch.
