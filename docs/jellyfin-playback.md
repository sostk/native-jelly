# Jellyfin playback

How a play press becomes bytes on the panel and reports on the server, which Jellyfin behaviour
each step relies on, and where that behaviour was verified. The measured server facts are in
`docs/jf-spikes.md`; this page is the client's side of the contract.

Sources quoted below as "server" are the Jellyfin server repository at tags v10.10.7 and v12.0 and
`master` (read 2026-10-08): `MediaInfoHelper.cs`, `MediaInfoController.cs`, `StreamBuilder.cs`,
`SessionManager.cs`, `PlaystateController.cs`, `HlsSegmentController.cs`, `TranscodeManager.cs`.
"Web" is `jellyfin-web` `src/components/playback/playbackmanager.js`. The pinned API description is
`docs/jf-api/jellyfin-openapi-12.0.json`; it omits controller actions marked `IgnoreApi`.

## Layers

| Layer | Module | Owns |
|---|---|---|
| Platform detection | `nj_platform::devcaps`, `devcaps::dv` (published by the webOS port) | the device's decoder table, Dolby Vision probe |
| Client capabilities | `catalog::capabilities::Capabilities` | device ∩ pipeline: codecs, containers, subtitle formats, raster bound, what is unknown |
| DeviceProfile builder | `jf::playback::device_profile` | `Capabilities` + one playback's ask → Jellyfin `DeviceProfile` (reads nothing else) |
| Jellyfin protocol | `jf::playback` (`negotiate`, `transcode`, `transcode_stop`, `timeline`), `jf::url`, `jf::convert` | PlaybackInfo, source selection, method, URL, session table, reports |
| Facade | `catalog::transcoder` (`Client::negotiate` …) | backend-neutral entry points and types (`Negotiation`, `Refusal`, `PlayMethod`) |
| Playback orchestration | `route::plan::build_stream`, `route::decision` | track and quality policy, direct-play gates, encoder replacement, resume, stop |
| Native player | `player::*` behind `nj_platform::tv::sink::VideoSink` | demux, feed, render; knows URLs and codecs, never Jellyfin |

`ci/module-layers.ini` enforces the direction: `catalog`/`jf` may use the platform layer, the
player and route sit above, and only the `[port webos]` fence names webOS.

## Flow

```
play press ─► route::request_play ─► resolve worker: route::plan::build_stream
   │            pick audio (language prefs, SubtitleMode, smart direct play), subtitle,
   │            direct-play gates (codec/raster/DV), quality ceiling, link policy
   ▼
Client::negotiate(PlaybackAsk{ media_source_id = the part's MediaSourceId, AudioStreamIndex,
   SubtitleStreamIndex, ceiling, direct_play, video_copy, … })
   ▼
jf::playback::negotiate
   Capabilities::current() ─► device_profile() ─► POST /Items/{id}/PlaybackInfo
   ErrorCode?                       → Negotiation::Refused(Refusal)
   source = the one whose Id == MediaSourceId (else the first, logged)
   SupportsDirectPlay && asked      → DirectPlay, /Videos/{id}/stream.{ext}?static=true
                                      &MediaSourceId&PlaySessionId&ApiKey
   else TranscodingUrl              → Transcode (lanes copied or encoded from TranscodeReasons)
                                      + the subtitle's DeliveryMethod (see Subtitles below)
   else                             → Refused(NoDeliveryMethod)
   file PlaySessionId/MediaSourceId/indexes under the app's session key; log one line
   ▼
apply_plan (main thread) ─► player opens URL ─► VideoSink
   resume: direct play seeks the file at first open; a conversion is negotiated at the offset
   in the resolve (StartTimeTicks), so the landing keeps that encoder
   HLS: the master lists the film from zero and carries no StartTimeTicks (segments refuse it);
   the player opens the segment covering the offset and moves its display base onto its start
   ▼
timeline reporter ─► Client::timeline ─► /Sessions/Playing at the first picture, then /Progress
   every 10 s and at once on pause, resume, a landed seek or a track change
stop ─► ScrobbleWork: join reporter, /Sessions/Playing/Stopped with the real position
```

## PlaybackInfo request

Fields sent (all in `PlaybackInfoDto`): `UserId`, `MaxStreamingBitrate`, `MaxAudioChannels`,
`StartTimeTicks`, `MediaSourceId`, `AudioStreamIndex`, `SubtitleStreamIndex` (`-1` = off),
`DeviceProfile`, `EnableDirectPlay`, `EnableDirectStream`, `EnableTranscoding`,
`AllowVideoStreamCopy`, `AllowAudioStreamCopy`, `AutoOpenLiveStream: false`,
`AlwaysBurnInSubtitleWhenTranscoding`.

* **`MediaSourceId` is required for track picks to apply.** Server: `SetDeviceSpecificData`
  applies `AudioStreamIndex`/`SubtitleStreamIndex` only when the source's id equals the request's
  `MediaSourceId`. The first ask takes it from the part key (`jf::convert::media_source_id`); a
  replacement encoder inherits it from the session it continues. An ask with neither names the
  item's own source, whose id is the item's. Without one, "subtitles off" went unheard: the server
  embedded the item's default PGS track and its ffmpeg failed to encode it (`Unknown encoder
  'pgssub'`, live 12.0).
* Stream indexes are Jellyfin's `MediaStream.Index`, never list positions. The app's track ids are
  `Index + 1` (`0` = off); `jf::ids::{track_id, stream_index}` are the only conversion.
* Several versions: the server sorts the queried item's own source first, but the client selects
  by id and logs when the answer lacks the requested one. The part asked for is the version the
  Detail page describes: `Media[0]` until the viewer picks another from the hero's *Version* pill
  (`screens::versions`, shown while the item has more than one `MediaSource`). Rows are labelled
  with the source's `Name`, as the web client's version menu is. The choice is held by the
  metadata store for that item (`MetadataCmd::SelectVersion`) and survives a refresh of the
  page. The play path fetches that version (`metadata::fetch_playing_item`).
* `RequiredHttpHeaders`: the client opens only server URLs, so these headers are the server's
  to send to a remote source. Its static stream (`GetStaticRemoteStreamResult`) forwards only
  `User-Agent`, while its ffmpeg also sends `Referer`. A remote `Http` source that needs any other
  header is asked again with direct play withdrawn, so it plays through ffmpeg rather than a
  proxy that would drop the header. Web makes the same split: it opens `Path` directly only when
  `RequiredHttpHeaders` is empty. Header names, never values, go to the playback log.

## DeviceProfile

Built by `device_profile(&Capabilities, &ProfileAsk)`:

* `DirectPlayProfiles`: containers `mkv,mp4,m4v,mov`; video = pipeline codecs the SoC decodes
  (`h264`, `hevc` where measured); audio = pipeline codecs the device table lists (`aac`, `ac3`,
  `eac3`, `dts` where present).
* `TranscodingProfiles`: progressive `mkv` over `http`, `Context: Streaming`, video target
  `h264,hevc` (or `h264`), audio target the decodable surround codecs then AAC. The server encodes
  to the head of each list after moving to the end what it will not pick
  (`EncodingHelper.Shift{Video,Audio}CodecsIfNeeded`). For video, that is HEVC unless its
  administrator allowed HEVC encoding, which a client cannot read. H.264 is never moved, so it
  leads, as in the web client, and HEVC stays listed so an HEVC source can still be copied. For
  audio, the move depends on the source: DTS and TrueHD at six channels or more, AC-3 and E-AC-3
  below. The video and audio lanes (`lane_codec`) report the codec that results. The pipeline is
  loaded with them before the first byte arrives. An HLS contract
  (`TranscodeDelivery::FixedHls`, Auto's rungs) sends the same codecs as `ts` over `hls`, with
  `SegmentLength` = the contract's seconds and `MinSegments: 1`. Its `TranscodingUrl` is
  `/videos/{id}/master.m3u8?…`. The ask withdraws video copy (`AllowVideoStreamCopy`,
  `EnableDirectStream`): a copy is cut at the source's keyframes, the playlist follows them only
  when the server could extract them, and the player's timeline is built from the playlist.
* `CodecProfiles`: `Width`/`Height` ≤ the measured raster bound (or the quality ceiling),
  `VideoBitDepth` ≤ 10, per-codec `AudioChannels` ≤ the measured ceiling. `IsRequired: false`
  everywhere — an unreported property does not fail the condition.
* `VideoRangeType` (once the panel's HDR10 support is measured): HEVC `EqualsAny` `SDR`, plus
  `HDR10|HDR10Plus` on an HDR10 panel, `HLG` where HLG is supported (it follows HDR10, as in the
  web client), `DOVI` when Dolby Vision is confirmed, and the `DOVIWith…` fallbacks for the base
  layers the panel can show. H.264 is `SDR` only, as in the web client. This also decides whether
  the server may copy the video: an HDR stream for an SDR panel is tone-mapped rather than copied.
  HDR10 is read from `tv.model.supportHDR` in the same configd call that answers Dolby Vision.
* `SubtitleProfiles`: `Embed` for formats the renderer draws (direct play) or bitmap formats
  (conversion), `External` for text sidecars (see Subtitles below). An explicit burn sends none, which is how the server
  is asked to `Encode`.
* `MaxStaticBitrate`/`MaxStreamingBitrate`: unbounded (200 Mbit/s) unless the user picked a
  ceiling, which bounds both.
* Strict Original (Force Direct Play) sends the pipeline's formats without device limits and no
  transcoding profile.

Not stated, deliberately: `VideoRangeType` conditions while HDR10 support is unknown (an
unanswered probe sends none, rather than guessing), audio passthrough (the TV's output setting is
unreadable), and a device bitrate bound (the table's bitrate column is not read). The event log
prints all of this once per process:

```
jf: capabilities video=h264,hevc audio=aac,ac3,eac3 channels=- containers=mkv,mp4,m4v,mov max=3840x2176 bitrate=unbounded dv=… hdr10=yes hlg=yes hw_decode=yes passthrough=unknown table=measured
```

## Subtitles

Direct play draws the file's own track: the embedded renderer, or the sidecar file for an
external one. A conversion asks for the same pick in `SubtitleStreamIndex` — else the sidecar the
server has selected, else a retry's track — and never forces a burn
(`AlwaysBurnInSubtitleWhenTranscoding` stays `false`). The answer's `MediaStream.DeliveryMethod`
for that stream (`MediaInfoHelper.SetDeviceSpecificSubtitleInfo`, filled for every play method)
becomes `jf::playback::SubtitleDelivery`, and `route::adopt_subtitle_delivery` points the client
renderer at it wherever a conversion is installed (landing, rebuild for a track or quality change,
seek, Original remux):

| `DeliveryMethod` | Server (`StreamBuilder.GetSubtitleProfile`, `EncodingHelper`) | Client |
|---|---|---|
| `External` | text track extracted to `/Videos/{id}/{source}/Subtitles/{index}/0/Stream.{fmt}`; not in the `TranscodingUrl`, so the video stays copyable | the sidecar renderer fetches that path under the request header (the URL's `api_key` is dropped) |
| `Embed` | bitmap track muxed into the progressive Matroska as its only subtitle stream (`GetMapArgs`) | embedded renderer at ordinal **0** of the output, not the source ordinal |
| `Encode` | burned (bitmap on HLS, whose TS segments cannot carry it) | draws nothing (`route::client_renders_subtitle` is false) |
| `Drop` | not delivered | nothing |

Two answers are asked again with `AlwaysBurnInSubtitleWhenTranscoding: true`: an `External`
delivery that is not a path on the server or not a format the renderer draws (a remote source's
own subtitle URL), and `Hls`, which the profile never offers and the player does not read. A DVB
subtitle answered `Embed` is treated as burned, because the server burns it while saying so
(`NormalizeSubtitleEmbed`). A server that states no method is read from its URL: `SubtitleMethod`
names it, and a `SubtitleStreamIndex` alone is a burn.

The sidecar's cues are on the movie timeline on both paths: a conversion's `playpos_ns` is the
display base plus the fed timestamps, and the server's file starts at 0 because the transcoding
profile copies timestamps. A track switch during a conversion re-negotiates (a new encoder, the
old one retired as for any rebuild) and adopts the new answer.

## PlayMethod

Reported exactly as the server and web decide it:

| Delivery | URL | `PlayMethod` | Internal flags |
|---|---|---|---|
| Original file | static stream | `DirectPlay` | — |
| Remux (both lanes copied) | `TranscodingUrl` | `Transcode` | `contract.remux = true` |
| Audio-only conversion | `TranscodingUrl` | `Transcode` | `contract.remux = true` (video copied) |
| Video re-encode | `TranscodingUrl` | `Transcode` | `contract.remux = false` |

Server: when it hands out a `TranscodingUrl` it sets `PlayMethod = Transcode`; on Playing/Progress
reports whose method is not `Transcode` it calls `ClearTranscodingInfo`, which is what the
dashboard derives "Remux"/"Direct Stream" from. `DirectStream` (a static stream of a source the
server marks `SupportsDirectStream` only) never occurs on 10.10–12: `MediaInfoHelper` forces
`EnableDirectStream` off and sets `SupportsDirectStream = SupportsDirectPlay`.

## Session lifecycle

One *playback* may use several *encoders*: a seek, a track switch, a quality change and every
resumed conversion re-negotiate under a new `PlaySessionId` (web does the same in `changeStream`).

* `negotiate` files `{item, MediaSourceId, PlaySessionId, PlayMethod, indexes}` under the app's
  session key, `started = false`.
* The first timeline report sends `/Sessions/Playing` and stamps `PlaybackStartTimeTicks`; later
  ones send `/Progress` with the same stamp, `IsPaused`, positions in ticks, the indexes,
  `PlaylistItemId` when the queue has one.
* When the reporter reports (`player::threads::timeline_thread`): `/Sessions/Playing` as soon as
  the Engine has presented its first picture (`SHARED.seen_frame`) and knows the duration — not
  after a first 10-second wait. Then a heartbeat every 10 s counted from the last report, and a
  report at once when `player::report_now` is asked: the viewer's pause and resume
  (`lifecycle::set_transport_paused`), a seek (`player::request_seek`; the reporter waits out
  `seeking` and reports where it lands), and an in-place track change
  (`route::commit_in_place_route_projection`). A track change or seek that reloads the Engine gets
  its report from the new Engine's reporter at its first picture. Nudges that arrive together are
  one report. A report the route cannot take yet (resolving, applying, starting) is retried, not
  dropped (`route::TimelineTick::Deferred`).
* No `EventName` is sent: the 12.0 `PlaybackProgressInfo` has no such field
  (`docs/jf-api/jellyfin-openapi-12.0.json`); `IsPaused` carries the transport state.
* A replacement (`transcode(spec)` with `spec.continues` = the session the playback reports under)
  inherits `MediaSourceId`, `started` and the start stamp, so it reports `/Progress` — a second
  `/Sessions/Playing` would increment the server's play count (`OnPlaybackStart`).
* The replaced encoder is ended with `DELETE /Videos/ActiveEncodings?deviceId=&playSessionId=`
  (server: `HlsSegmentController.StopEncodingProcess`, hidden from OpenAPI; kills jobs by
  `PlaySessionId`; 204). Only on 404/405 does the client fall back to a `Stopped` report with
  `Failed: true`, which kills the job without touching user data. It never sends a `Stopped` with
  position 0: the server writes a stop's position into the user's resume point (`UpdatePlayState`),
  which is how every resumed conversion used to rewind "Continue Watching" to the start.
* The final stop is the timeline's `Stopped` with the real position, after the reporter thread is
  joined (`ScrobbleWork`). The server's `ReportPlaybackStopped` kills that playback's jobs first. A
  playback stopped before any report was taken sends `/Sessions/Playing` first, so the server
  never sees a stop for a session it did not see start. One that never presented a picture (a
  failed load, BACK while loading — `TimelineReport::presented`) reports neither: only its encoder
  is ended, because its stop's position would overwrite the resume point.
* `IsMuted`/`VolumeLevel` are the television's: the player renders at full level, so the set's
  volume is what the viewer hears. Each progress report nudges one long-lived worker to read
  `luna://com.webos.service.audio/getVolume`, and the report carries the last answer. While
  nothing has answered (off the set, or a refused call) both fields are omitted. A refusal is
  logged once and the worker stops asking.
* The table holds at most 64 entries and evicts the least recently written; the playing session
  is rewritten by every report, so it is never the one evicted.

## Resume

Item `UserData.PlaybackPositionTicks` → `Metadata.view_offset` (`jf::convert::item`) → the play
request's resume position → `arm_play_resume` tags it to the resolve generation → after the plan
lands, a direct play seeks the file at first open. A conversion is negotiated once, at the
offset: the resume is carried into the resolve, which asks PlaybackInfo with `StartTimeTicks`, and
the landing keeps that encoder. It replaces the encoder only when the landing resumes somewhere
else than the resolve asked for. Positions are reported in ticks
(`jf::ticks`). The server decides when a position counts as a resume point (its minimum and maximum
resume percentages, `docs/jf-spikes.md`).

## Refusals and errors

| Condition | `Negotiation` | Viewer reads (`widgets.verdict.*`) |
|---|---|---|
| `ErrorCode: NotAllowed` | `Refused(NotAllowed)` | `server_not_allowed` |
| `ErrorCode: NoCompatibleStream` | `Refused(NoCompatibleStream)` | `server_no_compatible_stream` |
| `ErrorCode: RateLimitExceeded` | `Refused(RateLimitExceeded)` | `server_rate_limited` |
| any other `ErrorCode` | `Refused(Unrecognized(code))` | `server_refused` |
| no media source | `Refused(NoMediaSource)` | `server_no_media_source` |
| neither direct play nor `TranscodingUrl` | `Refused(NoDeliveryMethod)` | `server_no_delivery` |
| transport failure, unknown item, malformed body | `Unreachable` | the existing network failure read-out |
| Force Direct Play could not play the original | — | `forced_*` (unchanged) |

The technical code goes to the event log (`jf: playbackinfo refused item=… code=…`); the failure
report sends neither the code nor any server text.

## Diagnostics

One event-log line per negotiation, ids and codecs only — never a URL or token:

```
jf: playback item=<guid> source=<id> of <n> container=mkv protocol=File headers=- method=Transcode delivery=audio-transcode video=hevc->hevc audio=#1:truehd->eac3 subtitle=off start=1200s reasons=AudioCodecNotSupported play_session=<id>
```

`headers` lists the source's `RequiredHttpHeaders` names (`-` when none). `delivery` is the
dashboard's vocabulary: `static`, `remux`, `audio-transcode`, `video-transcode`. `subtitle` is
`off`, or `#<index>` with the conversion's delivery appended (`:external`, `:embed`, `:burn`).
Also logged: the capability line above (once), a source the answer lacked, a refusal's code, an
`ActiveEncodings` failure or fallback. `route::plan` keeps its own `playbackinfo:` line with the
lanes and ceiling.

## Test matrix

Automated (host, `make check`; loopback Jellyfin in `route/decision_test_support.rs`):

| Scenario | Test |
|---|---|
| Direct play URL, PlaySessionId, ApiKey, indexes | `plan_tests::a_direct_play_answer_returns_the_static_stream_url` |
| Ask names the part's MediaSourceId | `plan_tests::the_playback_info_ask_names_the_parts_media_source` |
| Multiple versions: the asked source plays | `plan_tests::the_answer_source_is_the_one_asked_for_not_merely_the_first` |
| Version picker: the chosen version is described, played and kept | `metadata_watch_state_tests::a_chosen_version_is_…`, `a_refreshed_page_keeps_the_chosen_version`, `screens::detail::tests::the_version_pill_opens_the_chooser_…`, `screens::versions::tests::surface::*` |
| Resumed conversion negotiates once | `plan_tests::a_resumed_conversion_is_negotiated_once_at_the_resume`, `a_landing_resuming_elsewhere_…` |
| HDR10/HLG probe and `VideoRangeType` | `webos::caps::tests::hdr_and_dv_are_graded_…`, `capabilities::tests::video_ranges_follow_the_measured_panel`, `jf::playback::tests::the_profile_states_the_measured_panels_video_ranges` |
| Volume and mute reported once read | `webos::volume::tests::*`, `jf::playback::tests::the_report_states_the_sets_volume_only_once_read` |
| `RequiredHttpHeaders` | `jf_session_tests::a_remote_source_needing_a_header_…`, `a_remote_source_needing_only_a_user_agent_…` |
| Encoded lanes name the codec the server encodes | `jf::playback::tests::an_encoded_lane_is_the_codec_the_server_encodes`, `capabilities::tests::direct_play_video_is_the_pipeline_set_the_soc_decodes` |
| A conversion continuing nothing names the item's source | `jf_session_tests::a_conversion_that_continues_nothing_still_names_the_items_source` |
| HLS contract: profile, master URL kept, no copy | `jf::playback::tests::an_hls_ask_offers_an_hls_conversion`, `jf_session_tests::an_hls_contract_asks_for_hls_and_keeps_the_masters_own_url` |
| Jellyfin playlists: `ApiKey`, first variant, start by time | `hls::tests::the_jellyfin_credential_…`, `several_variants_play_the_first`, `a_playlist_without_a_start_tag_…`, `ff::redirect_tests::a_jellyfin_playlist_starts_…`, `a_jellyfin_candidate_opens_at_the_handoff_boundary_…` |
| Audio by language / PlayDefaultAudioTrack | `plan_tests::the_users_audio_language_preference_picks_the_track` |
| Smart direct play sibling track | `plan_tests::smart_direct_play_asks_for_the_ac3_sibling_of_a_truehd_default` |
| SubtitleMode Always | `plan_tests::subtitle_mode_always_turns_on_the_preferred_language` |
| Conversion carries the preferred text subtitle as the server's file, video copied | `plan_tests::a_conversion_carries_the_preferred_subtitle_as_the_servers_external_file` |
| Muxed subtitle drawn at the output's ordinal 0 | `plan_tests::an_embedded_subtitle_is_drawn_from_the_converted_streams_own_ordering` |
| `Encode` is the server's burn; nothing drawn | `plan_tests::an_encoded_subtitle_is_the_servers_burn_and_the_client_draws_nothing` |
| Track switch during a conversion is soft, not a burn | `jf_session_tests::a_subtitle_picked_during_a_conversion_is_delivered_softly_not_burned` |
| Undrawable external delivery asked again as a burn | `jf_session_tests::an_external_subtitle_outside_the_server_is_asked_again_as_a_burn` |
| Every `DeliveryMethod`, and a server that states none | `jf::playback::tests::a_subtitle_delivery_is_read_from_the_answer` |
| Re-encode / remux plans | `plan_tests::a_transcode_answer_…`, `a_direct_stream_answer_is_a_remux_…` |
| Remux reports `Transcode` | `jf_session_tests::a_remux_reports_transcode_on_the_wire` |
| Replaced encoder ended by DELETE, no Stopped | `jf_session_tests::retiring_a_replaced_encoder_…` |
| Replacement reports Progress with the same start | `jf_session_tests::a_replacement_encoder_continues_…` |
| Replacement names the MediaSourceId | `jf_session_tests::a_replacement_names_the_media_source_…` |
| Seek replaces and retires the encoder | `quality_recovery_tests::a_transcode_seek_swaps_…` |
| Preview conversion ended without a Stopped | `plan_tests::a_preview_never_plays_a_conversion` |
| Typed refusals, no raw code shown | `plan_tests::a_playback_info_error_code_…`, `every_playback_error_code_is_a_typed_refusal` |
| Fixed quality / remote Auto bounds | `plan_tests::a_fixed_quality_bounds_…`, `remote_auto_measures_…` |
| Unconfirmed DV Profile 5 withdraws copy | `plan_tests::an_unconfirmed_profile_5_withdraws_video_copy` |
| Profile shape, ceilings, forced, subtitles | `jf::playback::tests::*` |
| Session table eviction | `jf::playback::tests::a_long_run_of_encoders_never_evicts_the_playing_session` |
| Capabilities, offered ⊆ rendered subtitles | `catalog::capabilities::tests::*` |
| Report ordering, server attribution | `route::decision::timeline_tests::*` |
| First report at the first picture; heartbeat, nudge, seek waits | `player::threads::tests::the_first_report_goes_with_the_first_picture` |
| A nudge wakes the playing reporter at once | `player::threads::tests::a_nudge_wakes_the_current_reporter_at_once` |
| A short playback reports its start before its stop | `jf_session_tests::a_short_playback_reports_its_start_before_its_stop` |
| No picture, no report; encoder ended | `jf_session_tests::a_playback_that_never_showed_a_picture_reports_nothing_and_ends_its_encoder` |

Live (`jf::live_tests`, `--ignored`, needs a test server): negotiation, ranged direct-play GET,
Playing/Paused/Stopped reports, a 30 s-offset re-encode and its stop, and an HLS rung at a 60 s
offset: master, media playlist, the covering segment, and its stop.

On the television (not automated — see the `which-tier` skill): direct play of H.264/HEVC in
MKV/MP4; a TrueHD film (audio-only conversion, track switch mid-play); a remux; a 4K re-encode
under a fixed rung; seek during a conversion; resume of a converted film (Continue Watching must
keep its position); external SRT and embedded PGS; Dolby Vision Profile 5/8; pause/resume; stop and
the server's resume point; next episode; a user without transcoding permission (`NotAllowed`).

## Limitations

* HDR10 support and the volume read are verified from the platform's own tools and replies, not
  yet from the jailed app on a set. Whether the app may call `com.webos.service.audio/getVolume` is
  unmeasured; if it may not, the fields stay omitted.
* `ETag` on media sources is not modelled; neither is Live TV (`RequiresOpening`, `LiveStreamId`).
* HLS is asked for only by an HLS contract, which is Auto mid-play and its rung candidates. A first
  play under Auto is still progressive, bounded by the bitrate test. The HLS exchange is measured
  against a live 12.0 server (`docs/jf-spikes.md` S5). The player's segment loop on Jellyfin
  playlists is covered by loopback tests only, not yet by the simulator or a set.
* A re-encode is H.264 even on a server that allows HEVC encoding. That setting cannot be read, and
  a lane that guessed HEVC loaded the pipeline for a codec that never came.
* On HLS the first segment starts up to one segment before the asked time. The display base is
  moved there, so the position is right and up to 2 s are shown again.
