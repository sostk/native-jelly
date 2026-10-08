# Jellyfin spikes (S1–S10)

What a real Jellyfin server does, measured before the port depends on it. The server is the
owner's own Jellyfin **12.0.0** on the development machine, reached over plaintext LAN HTTP, with
a dedicated test user. `tools/jf-spike.py` is the probe; it writes raw answers to
`tests/fixtures/jf/live/` (gitignored — those bodies hold a private library and a live token) and a
summary to `_report.json` there. This page quotes only statuses, counts and shapes.

The pinned API description is `docs/jf-api/jellyfin-openapi-12.0.json` (344 paths, fetched from
the same server's `/api-docs/openapi.json`). The DTOs in `rust-modules/src/jf/models.rs` are a
subset of it.

The facade is graded against the same server by `rust-modules/src/jf/live_tests.rs`
(`JF_URL=… JF_USER=… JF_PASS=… cargo test --lib jf::live_tests -- --ignored --test-threads=1`),
which exercises every op below through `catalog::Client` exactly as the app calls it.

## S1 — Auth

| Request | Status |
|---|---|
| `Authorization: MediaBrowser Client=…, Device=…, DeviceId=…, Version=…, Token=…` | 200 |
| `?ApiKey=<token>` | 200 |
| `?api_key=<token>` (pre-10.10 spelling) | **401** |
| `X-Emby-Token: <token>` | **401** |
| `/emby/...` route prefix | **404** |

* 12.0 accepts exactly two credential forms: the `MediaBrowser` header (control plane) and the
  `ApiKey` query parameter (media URLs the player opens without headers). `jf::url` builds both;
  `http::carries_credential` and `eventlog::redact_tokens` recognise both.
* One token per `DeviceId`: signing in again from the same DeviceId revokes the previous token,
  so the DeviceId is derived per user — `sha256(install id + "\n" + lowercase username)[..16]`.
  A token sent with a different DeviceId is still accepted (the server re-associates it).
* **Images are anonymous**: `GET /Items/{id}/Images/Primary?maxWidth=…` answered 200
  `image/jpeg` with no credential. Poster URLs therefore carry no token, which also keeps them
  out of the poster cache key.
* `TranscodingUrl` carries its own `ApiKey=` (one parameter, never `api_key`).

## S2 — Direct play (the gate)

`GET /Videos/{id}/stream.mkv?static=true&MediaSourceId=…&ApiKey=…` with `Range: bytes=1000-1999`
answered **206**, `Content-Range: bytes 1000-1999/<size>`, `Accept-Ranges: bytes`,
`video/x-matroska`, exactly 1000 bytes. Byte-range seeking through the existing plaintext media
transport (`stream.rs`) and libcurl (`curlio.rs`) needs no change. The live test repeats this
through `Client::direct_play_url` (which adds `PlaySessionId` and `ApiKey`).

**Verdict: pass.** Direct play needs no Plex code at all; the part key is the static stream path.

## S3 — PlaybackInfo

`POST /Items/{id}/PlaybackInfo` with a `DeviceProfile` built from the client's capabilities
(`catalog::capabilities`, over `devcaps` — `jf::playback::device_profile`) answers per media source
`SupportsDirectPlay`, `SupportsDirectStream`, `SupportsTranscoding` and, when it would transcode,
a `TranscodingUrl` whose `TranscodeReasons` say why.

* A TrueHD-audio HEVC film: `SupportsDirectPlay=false`, reasons `AudioCodecNotSupported` only — so
  the synthesized PMS verdict is `Part.decision=transcode` with video `copy`, audio `transcode`,
  the same answer PMS's MDE gives for that file.
* With no profile at all the server answers transcode-only (`SupportsDirectPlay=false`) — the
  profile is required, not advisory.
* `ErrorCode` (`PlaybackErrorCode`: `NotAllowed`, `NoCompatibleStream`, `RateLimitExceeded`) is a
  typed `jf::playback::Refusal`; the read-out words each category from the catalog and the code
  itself goes only to the event log (`docs/jellyfin-playback.md`).

## S4 — Progressive transcode

`TranscodingUrl` is progressive Matroska: `/videos/{id}/stream.mkv?…&VideoCodec=…&AudioCodec=…
&TranscodeReasons=…&ApiKey=…`. Measured through the facade: a 720p / 3 Mbit/s re-encode with
`StartTimeTicks` for a 30 s offset answered **200** with Matroska bytes within the first read.

* Start offset: the client appends `StartTimeTicks` (ticks = µs × 10) when the URL lacks it.
* Stopping (corrected 2026-10-08): `DELETE /Videos/ActiveEncodings?deviceId=&playSessionId=`
  exists on 10.10.7, 12.0 and 12.2 but is **absent from the OpenAPI document** — its controller
  action is marked `[ApiExplorerSettings(IgnoreApi = true)]` (`HlsSegmentController`). This page
  first concluded from the pinned spec that the route did not exist; the server source says
  otherwise. It kills the jobs whose `PlaySessionId` matches and answers 204; it is what the web
  client's `stopActiveEncodings` calls. The app ends a superseded encoder with it — a `Stopped`
  report would also end the job, but the server writes its position into the user's resume point.
  Measured 2026-10-08 against 12.0: the call answered **204**.
* Track change / seek: a new PlaybackInfo with the new indexes and offset, i.e. a new encoder,
  which is what the PMS `EncodeContract` rebuild already does. The new ask names the playing
  `MediaSourceId` — without it the server ignores `AudioStreamIndex`/`SubtitleStreamIndex`.

## S5 — HLS shape

12.0's OpenAPI and the server expose `/Videos/{id}/master.m3u8` (fMP4 or TS segments chosen by the
profile's `SegmentContainer`). A progressive `mkv` TranscodingProfile never gets an HLS URL, so the
app sends an HLS/TS profile when the route's contract is HLS (`jf::playback::device_profile`).

Read from the v12.0 source (`StreamInfo.cs`, `DynamicHlsHelper.cs`, `DynamicHlsController.cs`,
`DynamicHlsPlaylistGenerator.cs`) and measured 2026-10-08 against 12.0 on a test account
(`jf::live_tests::live_hls_rung_from_an_offset`): the URL, the playlist tags, the one-variant
master, the 2.002 s segments and the on-request start at a mid-film segment (the server's ffmpeg
ran `-ss` at that segment's start with `-start_number` its index). The multi-variant cases are
from the source only.

* `TranscodingUrl` is `/videos/{id}/master.m3u8?…&SegmentContainer=&SegmentLength=&MinSegments=
  &PlaySessionId=&ApiKey=…`. It carries **no `StartTimeTicks`**: `StreamInfo` writes the start only
  on the progressive branch.
* The master copies its whole query string into `main.m3u8?…`, and the media playlist copies that
  into every segment URI (`hls1/main/{n}.ts?…&runtimeTicks=&actualSegmentLengthTicks=`). So the
  credential is `ApiKey` in every child, and a segment request with `StartTimeTicks > 0` throws
  ("StartTimeTicks is not allowed"). A resume or seek has to start at the segment that covers
  the time.
* The media playlist is complete from the start: `#EXT-X-PLAYLIST-TYPE:VOD`, `#EXT-X-VERSION:3`
  (7 for fMP4), `#EXT-X-MEDIA-SEQUENCE:0`, `#EXTINF:<s>, nodesc` for every segment, then
  `#EXT-X-ENDLIST`. There is no `EXT-X-START`. Segments are equal length for a video encode,
  stretched for a fractional frame rate (2 s at 23.976 fps is 2.002 s). For a video copy the
  playlist follows the file's keyframes only when the server can extract them
  (`AllowOnDemandMetadataBasedKeyframeExtractionForExtensions`, `mkv` by default). Otherwise it
  lists equal lengths, while ffmpeg still cuts the copy at keyframes.
* The master can list more than one `EXT-X-STREAM-INF`. A Dolby Vision copy adds a `dvh1`
  variant first; an HDR copy adds SDR re-encode alternates. Two lower-bitrate variants are added
  only for a remote client that sends `EnableAdaptiveBitrateStreaming=true` (the controller's
  default is `false`, and `StreamInfo` does not write the parameter). In every case the first
  variant's URI is the main playlist the request asked for. Attributes
  include `AVERAGE-BANDWIDTH`, `VIDEO-RANGE`, `CODECS`, `SUPPLEMENTAL-CODECS`, `RESOLUTION`,
  `FRAME-RATE` and, with subtitles in the manifest, `SUBTITLES` plus `EXT-X-MEDIA` lines.

**Verdict (first spike):** v1 ships without Auto (recorded in `docs/adaptive-playback.md`). Fixed
quality picks are honoured through the re-encode's `MaxStreamingBitrate` and resolution conditions.

**Update 2026-10-08:** an HLS contract (Auto's rungs) now asks for HLS. `hls.rs` takes `ApiKey` as
the credential and the first variant of a master. Without `EXT-X-START` the player starts on the
segment covering the content time and moves its display base onto that segment's start. A rung
candidate must open exactly on the handoff boundary or it is refused. The video is always
re-encoded on HLS, because a copy follows keyframes the playlist matches only when the server
could extract them. The full player path has not been run on the simulator or a set.

## S6 — Media segments

`GET /MediaSegments/{id}` answers 200 with `{Items: []}` on a server with no segment provider —
the type list is empty, never an error. Types on 12.0: `Unknown | Commercial | Preview | Recap |
Outro | Intro`. `Intro` → Plex `intro`, `Outro` → `credits` (final when it reaches ≥ 98 % of the
runtime); the rest are dropped. With a provider plugin installed the detail page's Skip Intro /
Skip Credits work unchanged.

## S7 — Browse

* Paging: `StartIndex` + `Limit` + `EnableTotalRecordCount=true` → `TotalRecordCount`.
* Sorting: `SortBy` takes a comma list (`SortName`, `DateCreated`, `PremiereDate`,
  `ProductionYear`, `CommunityRating`, `CriticRating`, `DatePlayed`, `PlayCount`, `Runtime`,
  `Random`, `SeriesSortName,ParentIndexNumber,IndexNumber` for episodes). The Plex sort menu is
  synthesized from that set (`jf::api::sort_menu`); every advertised key maps.
* Genres: `/Items/Filters2?ParentId=&IncludeItemTypes=` → `Genres[{Name, Id}]`; filter with
  `GenreIds`.
* A–Z: `NameStartsWith` filters, but there is no per-letter count endpoint. The jump bar is built
  from ONE `Fields=SortName` read grouped client-side (the live test asserts the letter counts sum
  to the library total).
* Unwatched: `Filters=IsUnplayed`. Collections: `IncludeItemTypes=BoxSet` (server-wide, not per
  library).
* Cross-server match: `AnyProviderIdEquals=Imdb.tt…` plus a client-side provider-id check.
* `/Items/Latest` groups episodes into their series with `GroupItems=true`.
* Ticks: 10,000,000 per second (a 2 h 16 min film reports `RunTimeTicks` 81,772,160,000).

## S8 — Subtitles

External subtitles are fetched from
`/Videos/{id}/{mediaSourceId}/Subtitles/{index}/0/Stream.{fmt}`; the server converts text formats
to `srt` on request and serves ASS/SSA as-is. Embedded text and image subtitles direct-play under
the `Embed` subtitle profile and are rendered by the app's own demuxer path. A burned subtitle is
requested by sending the transcode profile with no Embed/External method for it.

## S9 — Sign-in edge cases

* `GET /QuickConnect/Enabled` → `true` on this server; `/QuickConnect/Initiate` is **POST** (GET is
  405). A disabled QuickConnect reports `false` (not measured here — this server has it on), and
  the login screen then offers password sign-in only.
* `GET /Users/Public` returned **0** users (hidden users), so the login flow must always offer a
  typed username — a user picker cannot be the only path.
* Empty passwords are legal (`Pw: ""`).

## S10 — Network

* Plain HTTP LAN servers are the common case (port 8096). They go through the existing plaintext
  credential authority (`plex::grant`): developer builds allow them, store builds require the
  consent screen — the `Authorization` header counts as a credential exactly like `X-Plex-Token`.
* HTTPS (8920 or a reverse proxy) uses the libcurl arm unchanged. Reverse proxies with a path
  prefix are **not** supported in v1 (`Origin` carries no path).
* UDP discovery (`who is JellyfinServer?` on 7359) is deferred: v1 takes a typed address.

## Corrections to the analysis

* `api_key` / `X-Emby-Token` / `/emby` are gone on 12.0, not merely deprecated.
* ~~There is no `ActiveEncodings` route; transcodes stop via the Stopped report.~~ Wrong, inferred
  from the OpenAPI document, which hides the route; see S4.
* No `HideFromResume`: "Remove from Continue Watching" resets the resume position to 0.
* `/Sessions/Playing/Progress` reports below the server's minimum resume percentage do not
  set a resume point (`resume_ticks_after_report` was 0 for a 1 s report).
