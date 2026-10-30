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

## Limits

- Host tests do not drive the pump's failure-branch reload; that path needs the TV.
- No stall detector: surround stalls (debug fixtures only) are neither recovered nor learned.
- Remaining: test-account negotiation/recovery, restart persistence, track changes, then
  enabling stereo Opus together in routing and profiles and TV acceptance on both outputs.
