# C3 refusal records, attempt snapshot and audio recovery — host and ARM

30 October 2026. Application source: `4e52880386b155fad547018f442bd54765ccbbd5` on
`feat/opus-direct-play`, based on main `e673debb`. The tree was committed and clean before and
after every gate below; this record is a documentation-only successor.

## Scope

| Commit | Change |
|---|---|
| `414c6e3b` | Build-scoped refusal layer over the immutable boot capabilities (`devcaps::Refusal`, `Caps::minus`, `effective_caps`) |
| `71fa2f6f` | Bounded install preference `native_audio_refused`; named rows apply at once, counted rows after two proofs |
| `f43a47db` | One capability snapshot per play attempt, shared by local gates and the Jellyfin DeviceProfile |
| `7655b93b` | First pre-picture `type=18` number latched; Opus header refusal published separately |
| `8c8310be` | `route::audio_recovery`: one bounded audio-conversion rung per request, driven by the pump |
| `4e528803` | Regression pinning the failed open's intended position (resume or pending seek) |

Opus remains withheld from Auto/Force routing and Jellyfin profiles. No TV run, real-server
negotiation, merge, push, release, tag or version bump was performed for this checkpoint.

## Gates on the committed source

- Full `make check` (`umask 022`): passed in 650.10 seconds. Main application result lines:

```text
test result: ok. 5231 passed; 0 failed; 27 ignored; 0 measured; 0 filtered out; finished in 74.35s
test result: ok. 5327 passed; 0 failed; 27 ignored; 0 measured; 0 filtered out; finished in 77.95s
```

- Platform suite 238 passed in both configurations; the other workspace suites, dependency,
  module-layer, statics and Python/C gates passed.
- Shipping `--no-default-features` check passed.
- Debug ARM build (`make`): passed in 106.10 seconds.

Under this host's default `umask 0002`,
`catalog::session::migration_tests::recorded_identity_refusal_and_unavailability_preserve_secure_input_without_retry`
fails: its fixture file is group-writable and storage correctly refuses it as an unsafe mode
before the opener runs. It fails identically at `a963da40`, is unrelated to C3, and passes under
`umask 022`.

## New coverage

- `route::audio_recovery::tests` (9): named vs ambiguous evidence, proof only from a clean
  audio-only landing with exactly its own Load, one conversion per request, no learning from
  timeouts, failed conversions or headers, Force and pending-rollback behaviour, baseline codecs.
- `route::decision::audio_recovery_tests` (4, Jellyfin loopback): carried track, start position
  and copyable video in the PlaybackInfo ask; attempt narrowed while boot caps stay unchanged;
  Dolby Vision no-copy contract preserved and unproven; refused conversion spends the rung and a
  new plan starts fresh; a forced route never asks the server.
- `route::decision::attempt_caps_tests` (2), `player` refusal-latch and intended-position tests.

## Television checks — 30 October 2026

Set: webOS 6.5.3 build 6.5.3-47 (`kisscurl-koli`); synthetic 72 s H.264 720p30 + 48 kHz
stereo Opus served by a local mock Jellyfin through a reverse tunnel; one TV lease per case,
released after each; app closed, triggers cleared, panel restored. Sound output, volume and
mute were not touched (current output only; speakers/soundbar not separated).

1. **Pushed `main` (`0bb0bcda`, Opus withheld), H.264/AC-3 direct play:** profile
   `audio=aac,ac3,eac3,dts` with no Opus; one Load; native picture; position to 51 s; pause and
   resume; native seeks to 45 s and 5 s; no reopen, watchdog reload, recovery, pipeline error
   or crash.
2. **Local enable build** (an uncommitted `devcaps` patch admitting the measured stereo Opus
   candidate; reverted afterwards) — valid stereo Opus (pre-skip 312, gain 0): profile
   `audio=…,opus` with `opus:2`; one native `H264`/`OPUS` Load; position to 32 s; pause and
   resume; two native seeks; recovery not taken.
3. **Same build — the same file with a +1 dB OpusHead output gain** (legal, outside the fed
   subset): the demuxer refused the header; `audio recovery: HeaderUnsupported`; the second
   PlaybackInfo withheld direct play and kept `AudioStreamIndex` 1, `AllowVideoStreamCopy`
   true and `StartTimeTicks` 0; landing `video copied, audio converted`; reload as
   `H264`/`AAC`; position to 46 s with the converted picture on the video plane; pause and
   resume; no refusal remembered; no pipeline error or crash.

The conversion was a pre-made AAC file standing in for the server; real Jellyfin
negotiation, a pipeline refusal (`num=200`/`601`) on the set, listening and output
separation remain unverified.

## Limits

- Host tests do not drive the pump's failure-branch reload; on the TV only the
  unsupported-header trigger was exercised.
- No stall detector: surround stalls (debug fixtures only) are neither recovered nor learned.
- Remaining: test-account negotiation/recovery, restart persistence, track changes, then
  enabling stereo Opus together in routing and profiles and TV acceptance on both outputs.
