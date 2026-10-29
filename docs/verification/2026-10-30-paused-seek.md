# Pause/seek overlap recovery — 30 October 2026

The fix is local on `fix/paused-seek-recovery`, based on main `28b7392e`. Clean committed
source **`8412101e51f6459bb841330abc6b4e3968db4b9a`** passed the gates and TV checks below. The later evidence commit
changes documentation only. Nothing from this follow-up has been merged or pushed.

## Cause and behavior

The original C2 AV1 pause/seek overlap recovered through two reopen attempts and a reload.
Two interacting issues caused that recovery:

1. The 1.2-second seek watchdog kept counting while accepted viewer Pause deliberately
   prevented the first post-seek keyframe from being fed. The first regression observed
   two retries where zero were expected. The pump now refreshes the watchdog deadline
   while feeding is intentionally held, giving Resume a fresh recovery window.
2. A seek armed after Pause transferred the native clock's user hold to seek priming even
   when no paused-frame preview was requested. Resume then had no accepted user hold.
   The watchdog-only candidate passed host checks but exposed this failure on the TV.
   A second regression reproduced refused Resume. The hold transfer is now conditional
   on an explicit paused-frame preroll request; an ordinary held seek remains resumable.

Viewer pause intent stays authoritative. A paused scrub's bounded one-frame preroll still
runs its watchdog, so a real stall can recover. The refused-native-Seek fallback is unchanged.
The fix adds no dependency, firmware call, native ABI or global state.

## Committed verification

| Check | Result | Elapsed |
|---|---|---|
| host | exit 0 | 644.64 s |
| shipping | exit 0 | 31.27 s |
| arm | exit 0 | 88.16 s |
| elf | exit 0 | 22.4 s |

Exact application test results:

```text
test result: ok. 5198 passed; 0 failed; 26 ignored; 0 measured; 0 filtered out; finished in 76.46s
test result: ok. 5294 passed; 0 failed; 26 ignored; 0 measured; 0 filtered out; finished in 80.21s
```

The ignored counts are not executed coverage. Focused suites passed 8 native-seek pump tests
and 5 paused/preroll tests; these scopes overlap and are not added into a unique-test count.
They cover both input orderings, held feeding, Resume, native Seek acceptance, preview
watchdog recovery and refused-Seek fallback. The original failing assertions were observed
before their corresponding fixes. The first incomplete candidate's host/TV evidence is kept
privately, separately from this final run.

Eight debug ELFs passed linker-input/SHA-256 evidence and firmware inventory checks from
4.4.2 through 11.2.0; older rows remain non-gating. ELF linkage/architecture assertions passed.
The stripped private debug copy is **13,542,344 / 13,810,000 bytes**.
The deployed unstripped application SHA-256 is `dd88435042a7c0a4bb8d6fa48b98a36bec55aa0bf118747b1bd7b75cdb33b90d`.
No build-input or module-layout change was made, so no new build benchmark was required.

## TV regression

The connected AV1-capable webOS **6.5.3** set ran the same private synthetic Jellyfin
Profile 10 fixture used in the C2 check. Its public AV1 video is copied unchanged and looped
with silent AC3 audio. This is not real-Jellyfin Profile 10 or audible-audio validation.

| Configured Pause hold | Pause / Resume | Accepted native seeks | Native loads | Reopen retries / watchdog reloads | Later advancement |
|---|---|---|---|---|---|
| 3000 ms | accepted | 2 | 1 | 0 / 0 | 11 s |
| 6500 ms | accepted | 2 | 1 | 0 / 0 | 11 s |

A preliminary final-build run was rejected because Pause occurred after seek anchoring.
The final runs schedule Pause slightly earlier; no non-overlap result is counted as a pass.
Both runs requested 45 s then 5 s. Assertions verified that the first seek was armed before
Resume and that native Seek happened only after accepted Resume: these are actual overlap
cases, not separate pause/seek actions. Native load and video-plane binding succeeded, the
Profile 10 node remained present, and the panel reported `dolbyHdr`. Both DISPLAY captures
were opened after release and showed decoded pictures. No crash was logged.

Each bounded run closed the app, cleared automation, terminated its tunnel, requested panel
restoration and released the TV lease. Final status confirmed the lease was free. Sound
settings were untouched. Captures and runtime logs remain private outside the repository.

These results establish the observed overlap fix on this firmware and the tested host state
machine. They do not establish all codec/firmware combinations or long-session performance.
C2's real-server Profile 10, audio-only copy and Profile 10.1 HDR10 fallback checks remain
pending independently. The next feature port remains C3 Opus direct play with proven fallback.

## Focused commits

- `5164cdf6` — do not spend watchdog retries while the viewer holds feeding.
- `8412101e` — retain Resume ownership unless paused-frame preroll explicitly takes it.
- A documentation-only successor records the final results.
