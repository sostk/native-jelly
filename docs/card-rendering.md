# Card settling and specialized rendering

P4 adapts PlxNative [#861](https://github.com/GLinnik21/plx-native/pull/861),
[#866](https://github.com/GLinnik21/plx-native/pull/866),
[#981](https://github.com/GLinnik21/plx-native/pull/981) and
[#983](https://github.com/GLinnik21/plx-native/pull/983) to Native Jelly's existing screens.

Released shelf and Detail episode cards land at exactly scale 1 with zero velocity after
their motion falls below a small relative tolerance. Focused springs keep their existing
integrator and timing. The final landing requests a present once; subsequent idle ticks do not.
Library keeps a sparse list of released springs keyed by card identity, so rapid navigation
does not discard an earlier card's release. Publications relocate these entries through the
retained key index; removed cards are dropped. Returning to a moving card resumes its spring.
Restores and reconciliation still adopt full focus scale. Collection's immediate scale path
has no release spring here and is unchanged.

Eligible resting card quads draw their perimeter and certainly plain interior under separate
scissor boxes. Each piece uses the original full quad and texture coordinates. The boxes
partition pixel space without overlap, intersect the caller's clip and restore its scissor.
Focused cards and reduced-resolution backdrop source passes retain the whole-card path.
The optional plain-texture shader handles square images and eligible card interiors; explicit
still/focus programs take precedence. If linking fails, the existing image shader handles them.
A separate optional two-stop gradient program uses the existing flat gradient expression and
precision; its fallback is the existing SDF program. No shadow-band optimization is included.

Developer builds expose `nativejelly-nocardsplit` for whole-card versus split comparisons.
The existing `nativejelly-overdraw` ledger reports logical card paths (`split`, `focus`, `clip`,
`nointerior`, `plain`, `off`). These are CPU draw/area counters, not GPU timing. Shipping builds
enable eligible splitting and compile out the diagnostic switch and counters.

Coverage includes exact settling and idle reporting, rapid presses, return during release,
identity movement/removal, pixel partitions and shader routing/fallbacks. The opt-in
`nj_gfx` `warm_tex_gl_tests` run under a real SDL/GLES context compares full-card and split
pixels, optional programs and fallbacks, caller clips, redirected page targets and reduced
source passes. Synthetic replay anchors are rerecorded through `tools/nativejelly-rec rerecord`
because Library's counted identity-keyed release sequence changes the logical state shape.

Host/ARM verification does not establish TV performance or LG pixel equivalence. Device
acceptance remains pending: compare uninstrumented navigation/frame pacing, separately measure
GPU work with splitting enabled/disabled, inspect card seams/shadows and modal glass captures,
and confirm settled pages stop presenting. Upstream measurements are not Native Jelly results.
