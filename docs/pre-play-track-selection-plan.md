# Pre-play audio & subtitle selection on the Detail page — implementation plan

Status: **proposal** (2026-10-09). Nothing here is built yet. File/line references are against
`main` at `adfcb01`.

## 1. Goal

On a movie or episode Detail page the viewer can:

1. **See** which audio track and which subtitle will play (e.g. `English · 5.1 EAC3` / `Subtitles: Off`).
2. **Choose** a different audio track and subtitle (or Off) from every track the selected version
   carries — before pressing Play.
3. Press **Play / Resume / Restart** and have the playback start on exactly that pair, with the
   in-player track menu showing the same checkmarks.

Non-goals for v1: show pages (see §7), alternate-source plays, trailers/extras, carrying the choice
to the next episode (§9).

## 2. What already exists (and is reused, not rebuilt)

The repo is ~90% of the way there; the missing piece is a *pre-play* entry point into machinery the
player and the retry path already own.

| Piece | Where | Reuse |
|---|---|---|
| Per-version audio/subtitle lists on the loaded item | `metadata::Detail::{audio, subs}` + `Version.facts.{audio,subs}` (`metadata.rs` ~1412, ~1570) | Source of the picker rows. `select_version` already swaps them. |
| Stream record incl. `id`, `lang`, `codec`, `channels`, `forced`, `sdh`, `external`, `selected`, `default` | `metadata::Stream` (`metadata.rs:738`) | Row model. `id` is Jellyfin `Index + 1` (`jf/ids.rs::track_id`), `0` = off. |
| Track naming | `metadata/track_label.rs`, `metadata/track_names.rs` | Row labels — identical words to the in-player menu. |
| Subtitle grouping (yours / other languages / forced / SDH ranking) | `metadata/sub_layout.rs` (pure data, host-tested) | Subtitle list structure. |
| In-player picker UI | `appkit/track_menu.rs` (TableView, keyed `FormTable`, page stack) | Visual idiom + row builders; `appkit/` is allowed from `screens/`. |
| Hero pill → anchored popover → report-back-by-message | `screens/versions.rs`, `HeroCtl::Version`, `ContentPanel::Versions`, `AppMsg::VersionChosen` → `MetadataCmd::SelectVersion` | **The template.** The track picker is the same shape. |
| Resolve env already carries an explicit selection | `route::ResolveEnv::{audio_sid, sub_sid, subtitle_override}` (`route/plan.rs:801`) | Filled today only by a retry (`RetryContext`, `route/decision.rs:6252`). We fill it from the Detail choice. |
| Default picks (user lang prefs, Smart/Always/OnlyForced, server `selected`) | `pick_dp_audio_*`, `pick_dp_subtitle_pref` (`route/plan.rs:1444–1700`) | Used to compute the **displayed default** so the page shows what Play would actually pick. |
| Sidecar (external .srt) start-up | `subtitle_override` marks `subs[].selected` (`plan.rs:1096`), `apply_plan` restores the sidecar | External picks work with no new player code. |
| "How this plays" preview | `route::playback_preview_of(.., audio_streams)` (`plan.rs:1807`) | Pass the chosen audio track so the hero badge stays honest. |
| Read-only Track information sheet | `screens/tracks_panel.rs`, `ContentPanel::Tracks` | Unchanged; optionally marks the chosen pair. |

## 3. Decisions needed from the owner before coding

1. **Hero row placement.** `hero.rs` has a guard test
   `the_hero_row_carries_no_track_information_disc` asserting the action row never carries track
   controls. This was deliberate. Options:
   - **A (recommended):** one new pill, **"Audio & Subtitles"**, shown only when the leaf has ≥2
     audio tracks or ≥1 subtitle, placed after *Version*. The guard test is replaced by one pinning
     the new pill's conditions. Summary text (`English 5.1 · Subtitles off`) sits in the hero facts
     line, not on the pill.
   - **B:** no hero change — make the About footer's Languages column the entry point (it is
     already focusable and opens Track information). Lower discoverability; the choice is far from
     Play.
   - **C:** two pills (Audio / Subtitles). Widest hero row; collides with Trailer + Version + Alt +
     watch face on long localized labels.
2. **Explicit audio that cannot direct-play.** Today `env.audio_sid` silently falls back to a
   different, direct-playable track (`plan.rs:1169–1172`) — fine for a retry, wrong for a user
   pick. Recommendation: mirror the in-player rule (`commit_audio_selection`, `decision.rs:8241`):
   **the viewer's pick wins and the server converts**; under *Direct Play: Forced* it is refused
   with `ForcedFailure::AudioNeedsConversion`. The picker marks such rows ("Converts").
3. **Lifetime of a choice.** Recommendation: per item + version, kept across same-item refreshes
   (return from player, watch-state write) exactly like the version pick (`metadata.rs:3640`),
   dropped on a version change and on leaving the item. Not persisted to disk in v1.

## 4. Design

### 4.1 Data — `metadata`

```rust
/// The viewer's pre-play track choice for the version the page describes. `None` = follow the
/// automatic pick (user prefs / server selection).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct TrackChoice {
    pub(crate) audio: Option<i64>,     // Stream.id, > 0
    pub(crate) subtitle: Option<i64>,  // Some(0) = explicit Off, Some(id) = track
}
```

- Add `pub(crate) tracks: TrackChoice` to `Detail` with
  `#[serde(default, skip_serializing_if = "TrackChoice::is_empty")]` — `Detail` is a recorded wire
  value and `record::validate` requires byte-identical round-trips of old recordings (same trick as
  `language_tag`, `can_normalize_loudness`).
- `Detail::select_tracks(choice) -> bool`: validates every id against `self.audio` / `self.subs`;
  rejects unknown ids.
- `Detail::select_version` clears `tracks` when the part actually changes (ids are per
  MediaSource).
- `install_landed_detail` (`metadata.rs:~3640`): after re-applying the held version, re-apply the
  held `tracks` if the ids still validate.
- New `MetadataCmd::SelectTracks { sid, rk, part, choice }` in `stores/metadata.rs` beside
  `SelectVersion` (doc list in the module header too). `part` guards against a stale report landing
  after a version swap.

### 4.2 The "effective pair" (what the page displays)

A pure function in `route/plan.rs` so the page and the resolve cannot disagree:

```rust
pub(crate) fn preplay_tracks(d: &Detail, prefs: Option<&LanguagePrefs>, mode: DirectPlayMode)
    -> EffectiveTracks { audio: Option<&Stream>, audio_converts: bool,
                         subtitle: Option<&Stream>, explicit_audio: bool, explicit_sub: bool }
```

- Explicit choice → that track.
- Otherwise → the same ladder `build_stream` runs (`pick_dp_audio_mode`, `pick_dp_subtitle_pref`
  with `audio_lang`), extracted into a helper both call. Language prefs come from
  `client.language_prefs()`; the page must not do I/O, so read the cached value only and fall back to
  the server's `selected`/`default` flags when it is not cached yet.
- Unit tests live in `route/plan_track_selection_tests.rs` beside the existing ones.

### 4.3 UI — `screens/track_choice.rs` (new surface)

Modelled 1:1 on `screens/versions.rs`:

- `TrackChoiceArg { host, sid, rk, part, anchor: [u32; 4] }` + `LogicalState` + `SHAPE` pin.
- `Style::Compact` popover anchored to the hero pill, with **two tabs** (Audio | Subtitles), LEFT /
  RIGHT switch, like `track_menu`'s root.
  - **Audio rows:** `track_label` + sub-line `codec · layout` + per-stream bitrate when two rows
    would otherwise read the same (the `tracks_panel` rule); badges `Atmos`, `AD`, `Default`; a
    dim `Converts` note when `!audio_direct_plays(mode, codec, channels)`; a leading
    *Automatic* row (= clear the explicit choice) showing what it resolves to.
  - **Subtitle rows:** *Automatic*, *Off*, then `sub_layout`'s grouping (yours / per-language
    sections / "Other languages" drill-in). Reuse `sub_layout` directly; do not fork it. Forced /
    SDH / External badges.
  - Checkmark on the effective pair; OK commits and dismisses (single pick, unlike Style pickers).
- Keyed `FormTable` with `RowKey` = family base + stream `index` (focus by identity, per
  `screens/CLAUDE.md`). `Infallible` `Dest`, `FormTable::activate`, `set_or_open`.
- Reports `AppMsg::TrackChoiceChosen { sid, rk, part, choice }` to the Detail instance; never
  writes the store itself.
- Register in `screens/registry.rs` (variant, screen id, mount arm, `SCREEN_SHAPES`),
  `screens/mod.rs`, `ContentPanel::TrackChoice { anchor }` + its `surface()` arm,
  `app/bootstrap/effects.rs` JSON tag, `dev/scenarios.rs`, `tests/manifest.json`. That is the
  spec's "new screen touches only…" list.
- `fit_report` test over the real row builder across `i18n::SHIPPED`.

### 4.4 Detail page changes — `screens/detail/`

- `hero.rs`: `HeroCtl::Tracks` (`ELEM_TRACKS`), `HeroSet.tracks: bool`, width in `HeroWidths`,
  `hero_ctls` ordering, `index_of`. Shown when `d.has_own_file()` and
  (`audio.len() > 1 || !subs.is_empty()`), and no alt source is selected (`self.selected()`).
- Hero facts line: append the effective pair (`English 5.1 · Off`) from `preplay_tracks`;
  truncation follows the existing facts-line rules.
- `mod.rs::activate_hero`: `HeroCtl::Tracks` builds the anchor exactly like `Version` and pushes
  `ContentReq::Panel(ContentPanel::TrackChoice { anchor })`.
- `mod.rs` event handler: `AppMsg::TrackChoiceChosen` → `MetadataCmd::SelectTracks` (same guard
  as `VersionChosen`).
- `route::playback_preview` call site: pass `[chosen audio]` when explicit, so the hero's
  Direct Play / Remux / Converts badge reflects the pick.
- `play_hero` (`mod.rs:3818`) and the Restart path: attach `d.tracks` to the `PlayIntent::Item`.
  Episode-strip plays on a show page and `PlayIntent::Movie` (alt source) attach nothing.
- `about.rs` / `tracks_panel.rs`: optional — mark the chosen tracks in Track information.

### 4.5 Play path — `screens/registry.rs`, `app/content.rs`, `route/`

1. `PlayIntent::Item` gains `tracks: Option<TrackChoice>` (update `extras.rs` and the other
   constructors to `None`).
2. `app/content.rs::request_play_intent` passes it to a new
   `route::request_play_with(…, tracks: Option<TrackChoice>)`; `request_play` becomes a thin
   wrapper passing `None` so the ~6 other callers do not change.
3. `PlaybackRequest` stores `tracks` (so a **terminal Retry / Choose quality** re-applies it; today
   `RetryContext` captures `cur_audio_sid/cur_sub_sid`, which after the first landing already
   equal the choice — verify with a test rather than assume).
4. `request_play_inner`: after `ResolveEnv::snapshot`, when `retry.is_none()` and
   `tracks.is_some()`:
   ```rust
   if let Some(a) = tracks.audio { env.audio_sid = a; env.audio_explicit = true; }
   if let Some(s) = tracks.subtitle { env.sub_sid = s; env.subtitle_override = Some(s); }
   ```
   `subtitle_override` already rewrites `subs[].selected` (incl. Off and external sidecars,
   `plan.rs:1096`).
5. `ResolveEnv.audio_explicit: bool` (new). In `build_stream`:
   - explicit + direct-playable → unchanged path;
   - explicit + not direct-playable → **do not** fall back to `pick_dp_audio_pref`; `audio_sel =
     None` so `direct_candidate` is false and `encode_audio_id` (which already honours
     `env_audio_sid`, `plan.rs:1576`) carries it into the conversion;
   - explicit + not direct-playable + `DirectPlayMode::Forced` → `PlayVerdict::Forced(
     ForcedFailure::AudioNeedsConversion)` (reuse the in-player verdict).
   - Log line: `route: viewer chose audio=<id> sub=<id|off> before play — <direct|converts>`.
6. Nothing changes in `player/`, `ff.rs`, Starfish/ACB: track switching at start is the same
   ordinal/codec hand-off as today. **No FFI change → no fw-compat review needed.**
7. Jellyfin side: `jf/playback.rs` already sends `AudioStreamIndex` / `SubtitleStreamIndex`
   (`playback.rs:669`) via `stream_index(track_id)`; the progress reports already carry
   `cur_audio_sid`. No `jf/` change expected.

### 4.6 Strings (`locales/*/browse.json` + `nj_platform::i18n::msg`)

`browse.detail.tracks` (pill: "Audio & Subtitles"), `…tracks.audio`, `…tracks.subtitles`,
`…tracks.automatic`, `…tracks.off`, `…tracks.converts`, `…tracks.summary` (facts-line pattern).
Every key needs `en` (with `description`), `be`, `es`; `ci/check-localization.py` gates it.
Reuse the player's existing audio/subtitle/off strings where the meaning is identical.

## 5. Phases & PR slicing

| # | PR | Contents | Gate |
|---|---|---|---|
| 1 | Route: explicit pre-play tracks | `TrackChoice`, `ResolveEnv.audio_explicit`, `request_play_with`, `PlaybackRequest.tracks`, `build_stream` rule, `preplay_tracks` helper | `make check`; new `plan_track_selection_tests` + `decision_*` tests |
| 2 | Metadata store | `Detail.tracks`, `select_tracks`, version-swap clear, refresh keep, `MetadataCmd::SelectTracks` | `make check`; record round-trip test on an old fixture |
| 3 | Picker surface | `screens/track_choice.rs` + registry/scenario/manifest wiring, strings | `make check`, `fit_report`, `ui-sim` capture (both tabs, 4K/Atmos/forced fixtures) |
| 4 | Detail wiring | hero pill + facts summary, activation, PlayIntent plumb, preview badge, guard-test replacement | `make check`, `ui-sim`, then TV (§6) |
| 5 | Docs | `docs/jellyfin-playback.md` "Subtitles", `player/CLAUDE.md` selection note, `README` features, `parity-gaps.md` row 623 | `doc-claim-auditor` |

Each lands on `main` as one squash commit (AGENTS.md).

## 6. Test plan

**Host (`make check` + `--no-default-features` check):**
- `build_stream`: explicit DP-able audio plays direct on that ordinal; explicit TrueHD/DTS-HD on a
  table without it → conversion carrying that id (not a fallback track); Forced mode → verdict.
- Explicit Off overrides a server-`selected` / Smart-mode subtitle; explicit external sidecar on
  direct play → `cur_sub_sidecar`; on conversion → `SubtitleDelivery` honoured.
- Retry after a start failure keeps the choice.
- `preplay_tracks` equals what `build_stream` picks for the same inputs (property-style table over
  the existing fixtures) — the "page and Play agree" invariant.
- `select_version` clears; same-item refresh keeps; stale `SelectTracks` (other part) ignored.
- Detail tests (`screens/detail/tests.rs`): pill presence rules, `Fx` emitted on OK, `PlayIntent`
  carries the choice, show page and alt source carry `None`.
- Old `metadata::record` fixtures still validate byte-for-byte.

**Simulator (`ui-sim`):** pill + popover, long Belarusian labels, focus return to the pill on
dismiss, pointer clicks on rows.

**TV (`tv-lock`, `--guest`, mocked or guest-safe content only — never press Play on a card whose
id you have not read):** multi-audio MKV (AC3 + EAC3 + a non-DP track), PGS + SRT + external
sidecar, forced-only file; confirm `route:` log line, the HUD track menu checkmarks, and audible /
visible result.

## 7. Show pages (phase 2, separate PR)

A show's `Detail.audio/subs` are **borrowed from its first episode** (`fetch_item_streams`,
`metadata.rs:2887`) and episodes in the strip carry no stream lists, so a picker there would
describe the wrong file. Options: (a) offer the pill on a show page only for the hero (on-deck)
episode after fetching *that* episode's streams (`fetch_playing_item`, worker + landing mailbox);
(b) choose by **language** rather than id ("English audio, Spanish subs") and resolve per episode
in `build_stream` with the existing `AudioLangPrefs`/`SubtitleLangPrefs` machinery. (b) also gives
§9 for free and is the recommended direction.

## 8. Risks

- **Page/Play disagreement** — mitigated by one shared `preplay_tracks` ladder and its equality test.
- **Recorded `Detail` wire format** — `skip_serializing_if` + an old-fixture test.
- **Hero width** on long locales — `hero_widths` + `fit_report`; fall back to icon-only pill if it
  overflows.
- **Ids are per MediaSource** — clear on version swap; `part` on the store command.
- **Auto/HLS "Original" candidate** (`AutoOriginalCandidate::retarget_audio`) — add a test that a
  cold Auto start with an explicit audio choice builds its Original candidate on the same track.

## 9. Follow-ups (not in scope)

- Carry the language (not id) into Up Next / continuous play.
- Persist per-item choice to session storage (`catalog::session`), as subtitle offset already is
  (`persist_subtitle_offset`).
- A "Remember for this series" toggle mapping onto Jellyfin's `RememberAudioSelections` /
  `RememberSubtitleSelections` user settings (verify server behaviour first).
