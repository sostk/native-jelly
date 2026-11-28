# Playback settings

Settings has **Video & playback** and **Audio & subtitles** pages. Quality and Direct Play
are saved on this television; language preferences are saved to the signed-in Jellyfin user on
the server and are shared with that user's other Jellyfin clients.

## Quality and Direct Play

Default quality uses the same ladder and saved preference as the player's More menu. Settings
changes apply to the next playback; the player menu retains its existing current-playback behavior.

Direct Play defaults to **Auto**, which uses device capabilities and normal routing.
**Disabled** excludes original-file playback while allowing a remux or transcode.
**Force Direct Play (advanced)** attempts original playback through supported engine paths,
overrides the saved quality and disables automatic remux/transcode fallback. It does not add
codec implementations of its own: TrueHD and the other formats the app converts to AC-3 play under
Force exactly as under Auto. It cannot guarantee that the TV accepts a stream.

Entering Force requires an explicit acknowledgement, initially focused on Cancel. The warning
names missing sound, incorrect pictures, freezes, crashes and the possibility that the app
cannot recover gracefully. It instructs the user to enable Force only if they know how to
restart the app and return to Auto. An enabled-state warning remains visible in Settings;
the quality controls explain that the saved quality is overridden. Returning to Auto or
Disabled needs no confirmation.

Audio passthrough remains the television's Digital Sound Output setting.

## DTS passthrough

**Playback → DTS passthrough** (off by default, saved on this television) sends a DTS track's
core to an external receiver unconverted. Many sets have no DTS decoder: declared as `DTS`, a
webOS 6 C1 refuses the Load with `type=18 num=601` before any picture. With the setting on and
`com.webos.service.audio/getSoundOutput` reporting an HDMI ARC or optical output (`external_arc`,
its soundbar variants such as `external_arc_sound_alive` or `external_arc_sound_mode_sync`, or
`external_optical`), the app
packs each 48 kHz DTS core frame into an IEC 61937 burst (as FFmpeg's `spdif` muxer does) and
declares the stream `PCM` with `pcmInfo` S16LE stereo 48 (kHz). LG allocates a PCM decoder
instead, and a receiver set to passthrough sees DTS. Any other sound output keeps the setting
inert, because the bursts are noise on the TV's own speakers. The output is read again when each
DTS playback starts, and an output that does not answer within about a second counts as unknown,
so a switch from the receiver to the speakers since the last playback never plays the bursts.

Only the 5.1 core is sent: DTS-HD MA and DTS:X extensions are dropped, so the receiver shows
DTS, not DTS-HD MA. The setting admits DTS up to 8 channels for the same reason. A core that
does not fit the burst period (1536 kb/s at 512 samples per frame needs 2044 of 2040 bytes) is
refused before any audio is fed, so the open recovery converts it on the server. Full DTS-HD MA
or TrueHD would need an 8-channel 192 kHz PCM input; this player's PCM input is stereo 48 kHz.

## Jellyfin user preferences

**Audio & subtitles** edits the four playback fields of the signed-in user's Jellyfin
`UserConfiguration` (`jf/preferences.rs`, issue #41):

| Row | Field | Options |
|---|---|---|
| Preferred audio language | `AudioLanguagePreference` | No preference, or one of the server's languages |
| Subtitles | `SubtitleMode` | As the file marks them (`Default`), When audio isn't in the subtitle language (`Smart`), Always, Only forced subtitles (`OnlyForced`), Off (`None`) |
| Subtitle language | `SubtitleLanguagePreference` | No preference, or one of the server's languages |
| Default audio track | `PlayDefaultAudioTrack` | Play the file's default track, or Prefer my audio language |

The values live on the server, per user, so every Jellyfin client of that user shares them, and
nothing is stored on the television. Playback reads them from `/Users/Me` at each start
(`Jf::language_prefs`), so a save applies from the next playback.

A load reads `GET /Users/Me` (its `Id` must be the signed-in user, or the answer is refused as
stale) and `GET /Localization/Cultures`. The languages are the server's list, keyed by
`ThreeLetterISOLanguageName` (`eng`, `fre`): the codes the server stores and reports on a stream,
and the list jellyfin-web's settings page offers. A stored code the server no longer lists stays
visible and selected.

`POST /Users/Configuration?userId=` replaces the WHOLE configuration, so a save reads
`/Users/Me` again, changes only the field picked, and sends every other field back as the server
gave it. If the four fields no longer match what the page showed (another client changed them),
the save is refused as stale and the page reloads instead of overwriting a change nobody saw. A
pick of the value already checked sends nothing. Confirmed values stay authoritative if a save
fails; the page offers Retry. A request captured under one sign-in cannot be saved under another.

Jellyfin has no forced-subtitle preference beside the mode, so the Plex-era *Forced subtitles*
row is gone; `OnlyForced` is a Subtitles mode.

## Verification boundary

Host tests drive the load and save against a fake server (`jf/preferences_tests.rs`). A real
user's preferences must not be changed as part of verification: use a throwaway user on a test
server.

Host tests cannot establish DTS sound, synchronization, resource allocation or DTS-HD core
acceptance. Those require the synthetic DTS fixture and server-backed playback on the television.
A muted device run proves pipeline behavior, not audible output.

A DTS track is judged by the core it is fed as, not by its declared width. Every DTS stream is
fed as its core (`ff::dts_core_packet`), and a DTS core is at most 5.1, so a decoder whose table
row takes six channels is offered any DTS source, DTS-HD MA 7.1 included
(`devcaps::fed_audio_channels`, `devcaps::source_channel_ceiling`); a narrower DTS row or a learned
refusal still bounds it, and a track with no usable channel count still converts. The demuxer proves
the claim per packet: it reads the core's own AMODE and LFE bits (`ff::dts_core_channels`) and a
core wider than 5.1 refuses the native feed, before any audio is fed, as an unsupported header
that the open recovery converts on the server.

### Implementation verification — 2026-09-27

- `make check` passed: default and hostsim Rust suites, feature checks, native helpers and the
  Python harness. After the final initial-load focus correction, all ten composed Settings
  tests passed, including load/retry focus, save focus, and Force confirmation/cancellation.
- Shipping-feature check (`--no-default-features`, `CARGO_INCREMENTAL=0`), ARM cross-build,
  ELF assertions, and the supported firmware loader matrix passed. No new FFI was introduced.
- All three committed replay anchors were freshly recorded after the state-shape change and
  passed both Targets and Resolve: six runs, every difference counter zero. The alphabet adds
  only source-backed Direct Play enum values and fixed Settings copy.
- Simulator: full account language picker, initial focus, regional-code save to the mock,
  warning with Cancel selected, and persistent Force/quality-override presentation inspected.
- LG webOS 4.10.2 debug install: Force warning and persistent enabled-state copy captured and
  inspected. A server-backed H.264/AC3 case passed while Force was persisted, using the managed
  Guest test identity. Direct Play was then returned to Auto.
- Native H.264/DTS 5.1 synthetic playback passed all pipeline assertions, including declaration,
  video binding, clock progress, displayed frames and in-place seeking.
- DTS-HD MA 5.1 core playback passed the same assertions using a temporary interoperability
  fixture. The public source sample's truncated tail was removed before looping its valid first
  3.4 seconds; every one of the resulting 5,532 audio packets contained a complete core. The
  malformed packet guard remains intact. The sample media is not distributed in this repository.

The television remained muted with its panel off. These results do not claim audible output
verification, and no real Plex account language preferences were written. The throwaway-account
PMS propagation measurement was never made, and is moot since #41: the page no longer writes plex.tv.
