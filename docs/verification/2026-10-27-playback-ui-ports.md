# Playback and UI ports: verification on 27 October 2026

This record covers the playback reliability and P1–P4 ports integrated after v0.9.0.
The focused commits are retained separately for review. The existing v0.9.0 version,
package metadata and release audit are preserved; this integration does not cut a release.

## Changes and review references

| Area | Focused commits |
|---|---|
| Offscreen texture warm-up, upload budgeting and pacing diagnostics | `9c1957dc`, `08e5f856`, `83b10d28` |
| Visible poster priority, bounded lookahead and placeholder diagnostics | `dd44f5a6`, `1e5e3022`, `2105d0b5` |
| Incremental Home/Library projections and lazy diagnostic state hashes | `ffa86d27`, `c016310b`, `492fe2f8` |
| Exact card settling, clipped card pieces and specialized shaders with fallbacks | `9ca54ed9`, `a0c6b8cc`, `8aab52bd` |
| Chunked EOF, bounded transfer recovery, skewed-track starvation and failed-pipeline cleanup/reporting | `951a7c1c`, `c8860c95`, `fb93b57f`, `3eaa8048` |
| TV harness exported-window/seek evidence and realistic eviction pressure | `ae40a2a1`, `bafbdac9`, `830b339b` |

The TV-tested application logic is unchanged by reapplying these commits onto the newer
main release metadata. Integration checks are rerun on the committed candidate before push.

## Functional device results

Tests used the debug install on webOS 6.5.3 with a Mali-G51 MP3. Stable installation was
untouched. Synthetic Jellyfin catalogs supplied deterministic artwork for UI checks;
actual local Jellyfin playback/reporting used an explicitly authorized dedicated test account.

- Eleven initial focused playback cases exercised H.264/HEVC, seek, EOS/replay, interrupted
  sized-body recovery, chunked EOF, late-audio/audio-ahead starvation, terminal pipeline
  release and actual Jellyfin playback/stop. Complete device logs were retained privately.
- A media-only connection failure released the failed Engine while retaining a nonzero
  position. Jellyfin received exactly one stop for that failed session. Retry succeeded,
  normal stop persisted resume, and a fresh app launch resumed near that position.
- Opening the TV Home app caused actual background events. Returning the existing debug
  app caused foreground events, native pipeline reload and advancing playback near the
  saved position; simulated lifecycle events were not used.
- After app-switch recovery, playback advanced **633 seconds over 633.75 seconds** with
  3,270 native displayed-frame reports, no reported native dropped frames and no terminal
  playback error. Start/end 1920×1080 captures visibly contained video-plane content.
  The final Jellyfin stop and saved resume agreed. Audio was not muted for testing;
  feed acceptance and captures do not establish loudspeaker audibility.
- The original RSS sampler mishandled `fuser` whitespace. A supplemental read-only sampler
  obtained 27 samples from roughly 90 seconds onward: 112,324–123,160 KiB, first 114,012 KiB,
  last 121,044 KiB. This short soak does not establish long-term memory/thermal behavior.
- Poster scroll/settle passed at 60.1 moving fps, zero moving admissions and a fully drawn
  settled window. Corrected eviction/reversal and hero/grid pressure scenes passed at
  57.4 and 56.5 moving fps, with actual eviction/refusal/rearming and complete settled art.
- An 800-item grid seated row 120/column 2. Ten rapid DOWN and ten UP inputs returned to
  the same focused item and fully drawn window, with 99 responsive Library heartbeats.
  This is functional navigation evidence, not per-key input latency.

The eviction test's old artificial ceiling could not fit its own middle viewport. The
developer-only harness correction permits two warm head windows, capped at 12 MiB; actual
eviction/refusal/rearming assertions remain. The mock fixture also needed backdrop metadata
and realistic dimensions to create the intended pressure. Those initial failures are not
counted as passes. The regression was observed failing before the ceiling fix and passing
afterward. No performance thresholds were relaxed.

## Full-resolution visual checks

Thirteen native 1920×1080 DISPLAY captures covered grid clipping, sort/account surfaces,
circular cast/person artwork and split/whole/baseline controls. Representative captures
were visually inspected. Only the changing developer FPS badge rectangle
`(1760,40)–(1840,130)` was excluded from comparisons.

Grid and cast images match both controls exactly. Account differences are at most two
channel codes in 17 pixels. The sort menu's own 296×495 rectangle differs by at most one
channel code. The full sort screenshots have differences in the page latched underneath
while opening; full-frame sort equivalence is not claimed. No new seams, damaged circles,
scissor leaks or glass defects were seen. Captures are not photographs of panel behavior.

## Performance findings: acceptance remains open

The unchanged upload gate **fails**: **80/187 upload frames (43%)** took at least 22 ms,
against a 35% ceiling. Throughput passes at 1.45 uploads/frame against 1.25; the robust
loop floor reaches 50. The imported budgets/thresholds remain provisional for this GPU.

The whole-batch comparison used pre-port baseline `95944164` and the combined current
application, the same ARM SDK, 800-item synthetic catalog, row 10/column 2 and scroll
oscillator. Each launch warmed ten seconds and measured approximately 22 seconds.
Normal pacing and GPU-timer sequences were separate, both baseline/current/current/baseline.

| Build | Normal FPS medians | GPU `frame.ui` medians | GPU p95 |
|---|---:|---:|---:|
| Baseline | 60, 60 | 16.58748, 16.59312 ms | 18.57980, 18.28479 ms |
| Current | 54, 53 | 17.21674, 17.08536 ms | 25.97930, 26.05928 ms |

Average normal run medians fell from **60 to 53.5 fps (10.8%)**. Average GPU run medians
rose from 16.59030 to 17.15105 ms (3.4%). There were 5,275 valid GPU timer samples and zero
disjoint events. Artwork work differs: across warm-up/measurement, baseline launches made
8/50 image requests versus 712/766 for current. Equal moving-scene artwork coverage was
not established, so the result is an end-to-end pacing regression without attribution to
one port. Baseline upload telemetry is unavailable, not evidence of zero uploads.

A separate warmed static-grid control fixed the animation clock and forced presentation,
without scrolling. All four measured windows had zero artwork requests. GPU medians in
baseline/current/current/baseline order were **16.55899 / 17.46839 / 17.71230 / 16.55782 ms**:
average medians 16.55841 versus 17.59035 ms, a 6.2% increase. All 5,328 timer samples were
valid with zero disjoint events. A cost remains after network artwork loading settles;
the control does not identify a particular shader or policy.

Both builds correctly released settled idle rendering to 0–1 presents/s with a 60 Hz loop.
Instrumented FPS is not quoted as normal pacing; global GPU IRQ counts are not utilization.

An earlier split/whole toggle on the current implementation collected 91 normal heartbeats
and 5,001 valid GPU samples. Average run medians were 54 versus 55.5 fps and 17.08360 versus
16.99743 ms. Small differences overlapped repeat variation: no measurable split-card benefit
was demonstrated. Legacy Midgard hardware-counter probing is unsupported on this Bifrost
driver; no T82x counter names/utilization were inferred. GLES timer queries were supported.

The next performance work is to isolate these costs, tune poster upload latency and repeat
the same controls, retaining these failures and thresholds. No Native Jelly speedup is claimed.

## Host/ARM evidence and limits

The committed pre-integration application/harness tree passed `make check`, the shipping
feature check, debug ARM build and ELF assertions. Exact main host results were:

```text
test result: ok. 5025 passed; 0 failed; 20 ignored; 0 measured; 0 filtered out; finished in 48.70s
test result: ok. 5118 passed; 0 failed; 20 ignored; 0 measured; 0 filtered out; finished in 47.42s
```

Earlier application verification also passed real SDL/GLES regressions, six interaction
replays with zero divergence, release ARM linkage and the supported firmware loader matrix.
These establish their stated properties; loader success does not establish runtime playback.

Every bounded TV run closed the debug app, cleared automation, stopped temporary tunnels,
removed its GPU helpers, restored the panel and released the lease. Final read-only checks
confirmed the lock free, no triggers/helpers, app stopped and current debug payload restored.
Private addresses, credentials, actual media metadata and real-content captures are excluded
from this public record. Detailed private logs and synthetic/numerical evidence were retained
in the testing workspace.

This completes the requested focused follow-up checks, not the entire codec/HDR matrix,
all-device performance, per-port cost attribution, per-key input latency or long-term soak
coverage. Functional passes do not override the unmet performance gate.
