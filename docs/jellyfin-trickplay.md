# Jellyfin scrub preview thumbnails

While a visible seek gesture previews a destination, a thumbnail can appear above the title and
control row, centred over the playhead and clamped to the playbar margins. D-pad preview taps,
held scrubbing and pointer drags share this display. Dedicated immediate FF/REW taps and instant
hidden-HUD D-pad seeks retain their existing behavior. Thumbnail availability never gates seeking,
Pause or session reporting. Movies and episodes use the same path.

## Source and version

The design adapts [PlxNative #767](https://github.com/GLinnik21/plx-native/pull/767), replacing its
Plex per-offset still endpoint with Jellyfin's trickplay manifest and tiled JPEGs. The negotiated
playback receipt supplies the item and actual MediaSourceId; a different cut's manifest is never a
fallback. The optional worker reads `/Items/{id}?Fields=Trickplay&userId=…`, then
`/Videos/{id}/Trickplay/{width}/{index}.jpg?mediaSourceId=…`. Authentication uses the ordinary
MediaBrowser header and the catalog's credential/transport policy. The vendored Jellyfin 12.0
OpenAPI and upstream TrickplayController define these endpoints.

The manifest supplies thumbnail dimensions, columns/rows per sheet, interval in milliseconds and
thumbnail count. Time maps by floor division, including sheet boundaries and the final partial
sheet; destinations past the generated range use the last valid thumbnail. Selection prefers a
valid resolution at or below 320 pixels, otherwise the nearest larger valid one. Missing,
malformed or version-mismatched manifests produce no card. Oversized sheet layouts are refused.

## Resources and cancellation

The UI probes the installed poster source; it performs no network requests or image decoding.
Existing poster workers handle optional previews with three-second reads, a 512 KiB manifest
limit and 4 MiB compressed-sheet limit. Validated sheets are bounded to 4096 pixels per axis and
6,291,456 pixels in total. A worker keeps at most one compressed sheet; the existing two-worker
pool bounds their combined retained compressed bytes to 8 MiB. Decoding crops before RGBA
conversion. Only the chosen tile enters the ordinary texture cache and upload budget; the full
sheet is never a GPU texture. Previews have no disk-cache entries.

A bounded four-entry render-fact cache gives the UI the true manifest interval. One logical
request remains pending while movement coalesces to the newest destination. The previous still
can remain briefly while the next arrives; a mismatched still hides after 400 ms. Missing tiles
stay quiet rather than requesting again every frame. Deferred/stuck requests retire after seven
seconds. Finite network reads bound work already inside the transport.

Release, hide, unmount, reload and playback/version/account changes retire pending and shown
images. Source-slot generations reject late worker results; cancellation also removes queued
uploads and residency. Read-only residency probes prevent an evicted old still from starting an
untracked second fetch. The render memo is excluded from canonical screen state and cannot alter
transport, focus or replay expectations.

## Verification scope

Host checks cover interval/sheet mapping, last-thumbnail clamps, validation, source-version
isolation, authenticated HTTP, compressed-sheet reuse, pixel crops, absence, cancellation,
request coalescing, stale hiding, timeout wrap and card geometry. Device captures and the final
committed host/ARM/replay results are recorded in [U4 verification](verification/2026-10-29-scrub-previews.md). No frame
pacing or performance improvement is claimed by the implementation alone.
