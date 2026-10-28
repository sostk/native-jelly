# U2 title and Continue Watching verification — 28 October 2026

U2 is implemented on `feat/player-captions`, based on main `93c0417b`:

| Commit | Scope |
|---|---|
| `b4436567` | Bound HUD title lines before the controls; reveal a paused display title once. |
| `7fb26768` | Put episode addresses before Continue Watching progress captions. |
| `b1e5f928` | Add one captured caption Width measurement to the Home replay anchor. |

The upstream references are [#914](https://github.com/GLinnik21/plx-native/pull/914),
[#859](https://github.com/GLinnik21/plx-native/pull/859) and
[#912](https://github.com/GLinnik21/plx-native/pull/912). This is a scoped adaptation:
Native Jelly already shows a focused card's caption, so the broad upstream poster-settings,
data propagation and caption-band redesign is not imported. Trailer title behavior is retained.

The HUD reserves measured control geometry, including settled focus scale, spring overshoot,
translated Skip labels and Up Next's still/caption. Both title lines fit that boundary.
An overflowing display title reveals once while paused, then holds its ending. Resume,
hidden transport, teardown/reload and changed title/context release or restart the reveal.
The one-shot clock belongs to the view and shares the existing tile-title hold and speed.

Continue Watching reads `S2 · E5 · 25 min left` or `S2 · E5 · New episode` for numbered
episodes. Missing season numbers use the existing episode-only formatter; unnumbered
episodes retain the show-name fallback. Movies retain remaining time alone. Caption height,
focus/reveal placement, resume-bar ownership and Jellyfin selection/reporting are unchanged.
There are no new dependencies, settings, protocol fields or native ABI changes.

## Host, shipping, ARM and replay evidence

Both reported regressions were observed failing first: long HUD text crossed the control
boundary, and the Jellyfin DTO-to-caption path emitted the show name instead of the episode
address. Host regressions cover all four shipped languages, every control-slot occupant,
focus motion, fitting and overflowing titles, paused reveal/reset/rest, missing indices,
movies and resume offsets at/past the end. Caption drawing is checked in edge and middle columns.

The full `make check` passed on clean committed application source `7fb26768` (332.80 seconds).
The main crate's exact default and hostsim results were:

```text
test result: ok. 5079 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 48.80s
test result: ok. 5172 passed; 0 failed; 24 ignored; 0 measured; 0 filtered out; finished in 47.66s
```

The shipping `--no-default-features` check, debug ARM build, ELF assertions and host simulator
build passed. ELF inspection reports zero CP15 barriers and 14,020 dmb instructions. The stock
host probe reports TSan unavailable; no TSan pass is claimed. An earlier broad title-test run
hit sandbox loopback-bind refusals; its unrestricted rerun passed (125 passed, one ignored).

The old Home recording initially refused the new progress string's missing measurement.
A fresh seed-1 controlled capture supplies one exact Width answer. Nine shared font answers
match bit for bit. The one new metric is placed at the old caption's frame 492; removing it
recovers all 10,364 original records byte for byte. Every original manifest, state hash, input,
effect, result, landing, present, focus and hit expectation is retained. The alphabet adds
only that exact synthetic output with its source. No state expectation was rebaselined.

After committing this fixture update (`b1e5f928`), all 11 replay-policy tests and all six
strict replays pass, with every difference counter zero. Targets and Resolve each grade
940 Home, 888 content-return and 1,291 Settings frames. The fixture commit changes no
application source; the subsequent verification documentation is also separate.

## Simulator and TV inspection

Three 1920×1080 simulator captures show resumed/new-episode labels and a return focus move.
The software-rendered simulator session also observed unlabeled >250ms watchdog stalls
(one ended at 401ms); the third capture retains that warning. Those stalls were not diagnosed,
and these captures establish layout rather than performance. The targeted TV logs contain
no `main-thread hang` observations.

On the webOS 6.5.3 TV, eight controlled full-display captures were individually inspected:
resumed/new Home captions, playing title ellipsis, paused start/motion/ending/rest and resumed
ellipsis. The title remains clear of the controls. The ending region `[96,800,1568,866]`
is byte-identical after another four seconds; the moving region differs. The resumed prefix
`[96,800,1520,866]` matches the playing prefix. The rightmost ten pixels differ during the
focused control's pop, so full-width playing/resumed image identity is not asserted.

After the first startup heartbeat, 62 player UI-heartbeat samples have a second-lowest value
of 60 fps, exceeding the scoped 50 fps motion floor. Home's final ten settled samples alternate
0/1 fps and meet the existing 3 fps ceiling. The bound video plane keeps player presents alive
even while paused, so a player idle-fps saving is not inferred; host tests check that the
title itself stops requesting frames. These are scoped acceptance observations, not a
before/after performance benchmark or a video-frame-rate measurement.

Actual local Jellyfin Home also loads with the supplied test account: seven resume items,
including a numbered episode. Its caption shows the episode address and remaining time.
No playback was started in that read-only Home check. Real-library screenshots and credentials
remain outside shareable evidence. An old cached token failed authorization before device
access; a fresh sign-in with the supplied credentials completed the check.

The tested debug binary's MD5 is `99a33a67461ad000ad08d0d42eae0904`, matching the TV copy;
SHA-256 is `a25212662f8cb014843b97dde80ac2af3a4eb4bf9410fdcb3993a0e05704fbd4`.
Final handback confirms the debug app is closed, zero armed triggers and a free TV lock.
The panel state was restored and sound was not changed. No stable install, release, version
change, main merge or push is part of U2 acceptance.

U3 (FF/REW taps, optional instant seeking and previous/next episode controls) is next.
The separate pending UI-performance branches remain outside this port.
