# Incremental Home and Library publications

Adapted from plx-native [#858](https://github.com/GLinnik21/plx-native/pull/858)
(`6635243fcaffe1b985c24a895759f19c9309fc67`) and
[#860](https://github.com/GLinnik21/plx-native/pull/860)
(`8629ca7b86d500b2689af3791b4b2cf615fd941f`). Native Jelly keeps its existing
Home catalog, twelve-shelf Library document and Jellyfin data adapters. Upstream's held-row ring,
Recommended page redesign and hold-band hysteresis do not apply to those stores.

Home retains a row's element vector and motion when the provider identity, ordered server/item
identities, linked-heading projection and position are unchanged. A publication still compares
the published row identities; it skips key registration and projection allocation for unchanged
rows. Metadata is drawn from the current retained view, so a title, resume position or watched
flag can update without rebuilding its keys. Empty item identities carry a publication generation
and rebuild. A moved row rebuilds its recovery positions. Home patches only changed rows in its
element-position map when row order stays the same. Reorder, duplicate row identities and restore
use the full projection path where needed to preserve first-draw and last-registration behavior.

Home key-table pruning retains its existing live, previous-publication and focused-key rules.
Stable tables below their cap avoid a whole item scan. When keys are removed, ordered compaction
moves surviving index positions in place; it does not clone or rehash the retained table. Hub and
item key strings use shared ownership while canonical serialization still writes the same bytes.
Recorded private navigation payloads serialize the underlying strings explicitly.

Library shelves similarly keep their element vectors and motion when the row's key basis is
unchanged at the same position: hub identity, landscape mode, heading presence, ordered server/item
identities and the query used for empty placeholder identities. Changed rows register their own
keys, and moved rows update group recovery positions. Duplicate hub identities take the complete
registration path. Restoring a key registry invalidates its old key projections and rebuilds them from the
current publication, retaining the row motion and saved viewport rules. Grid and rail paging keep their existing owners and rules.

The dispatcher offers an event-frame hash as a lazy callback. A tap that records the state calls
it; `NoTap` and other observers that do not read it avoid the walk. Repeated reads share one
computed hash. The debug assertion that prepare leaves logical state unchanged remains active.
The product recorder already requests its final tree hash separately; fixture recording explicitly
reads the dispatcher callback. Saved interaction replays check that canonical state and focus
behavior remain unchanged.

Regression coverage checks unchanged storage, one changed row, metadata updates, key recovery,
saved-memory isolation, duplicate identities, restore, compaction and hash readers. Host evidence
establishes those behaviors, not TV speed. Large-library remote latency, frame pacing and memory
should be compared on the same TV and build before claiming a performance improvement.
