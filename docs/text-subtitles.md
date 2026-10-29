# Shared text subtitles

Embedded SRT/SubRip, WebVTT and mov_text, and external SRT/WebVTT, now use the same
`player::text_ass` converter and pinned libass worker as authored ASS/SSA. Basic italics,
bold, underline, SRT colours, line breaks and alignment survive conversion; WebVTT
entities are decoded, metadata blocks are skipped, and `line:0` places a sign at the top.
Other WebVTT positioning and CSS are not implemented. Authored ASS/SSA remains intact.

The existing track, forced-caption and delivery decisions are unchanged. An external
Jellyfin subtitle is downloaded once; server-burned selections suppress every client
subtitle path. External scripts survive backward seeks and Off/On. Embedded tracks
retain a bounded read-ahead/history window and raw packet identities across seeks and
same-media demux reopens. Late producers cannot publish into a different media generation.

For each vertical alignment class, the newest active cue wins. A longer older cue resumes
when its shorter replacement expires; top signs and bottom dialogue can coexist. External
timestamps are quantized down to libass centiseconds, at most 9.999 ms early.

Size and position update synthesized headers; tone and timing offset retain their existing
controls. Embedded restyles share event/font allocations. Large external scripts are
rejoined on a worker; the old look remains until the new revision publishes. Text captions
use the UI canvas while authored ASS uses the video viewport. With the transport open,
bottom text moves up; top/middle cues stay. A compositor region merging top and bottom
content conservatively stays still. Native parsing, font loading and rasterization stay off
the UI thread, and unchanged native regions retain their texture uploads.

Charset order is BOM (including UTF-16), valid UTF-8, then pure-Rust `chardetng` and
`encoding_rs`. BOM decisions persist across embedded packets. Other embedded decisions
settle after 160 non-ASCII bytes; language tags supply hints, while unqualified Chinese
leaves GBK versus Big5 to the bytes. Whole sidecars are detected in one pass. Detection is
probabilistic, especially for short untagged cues; it is not a promise to identify every file.

Bounds remain explicit: external downloads are 4 MiB, synthesized scripts fit the native
8 MiB / 20,000-event limits, and one embedded packet is at most 256 KiB. Embedded raw text
and its expanded ASS view share the per-track budget derived from a 16 MiB global event
allowance; each holds at most half its share, with at most 512 raw cues per text track.
Under expansion pressure, the view favours the subtitle clock before distant cues. Fonts,
image subtitles and native pixel caches retain their existing separate bounds. Oversized
external conversions show the existing subtitle failure notice.

Adapted from upstream [charset detection #708](https://github.com/GLinnik21/plx-native/pull/708)
and [shared sidecars #734](https://github.com/GLinnik21/plx-native/pull/734), with the text
converter from the related libass series. This port retains the existing longer-cue resume
semantics and native size limits; it does not import upstream's long-script windowing.
