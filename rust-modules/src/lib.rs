//! Native Jelly — an unofficial native Jellyfin client for LG webOS.
//! Copyright © 2026 Gleb Linnik and Native Jelly contributors. Licensed under GPL-3.0-or-later;
//! see LICENSE at the repository root, and THIRD-PARTY-NOTICES.md for the components this links
//! or redistributes. Fork of PlxNative. Not affiliated with, endorsed by, or sponsored by the
//! Jellyfin project or LG Electronics.
//!
//! nativejelly-modules — the Rust app core, built as a staticlib and linked into the C
//! boot shim. The crate's C surface is tiny: C calls `nj_run` (port.rs), writes the fallback
//! image marker through `nj_crash_write_image_marker`, re-enters the native-crash spool through
//! `nj_sentry_spool_external`, and forwards the two Starfish callbacks (`sf_on_event`/
//! `acb_on_event`, player/mod.rs). Everything else is Rust-internal (the per-module `repr(C)`
//! shapes are migration legacy, not ABI).
mod abr; // client-managed fixed-session HLS controller: estimate, propose, prime, then commit
mod app; // run_application — the Rust app core / event loop (the entry inverted from main.c; port.rs's `nj_run` hands over to it)
mod appkit; // widgets shared by several screens, composed from `ui` over application types (the player HUD, the track menus, the Sources row model)
mod aq;
mod auth; // leftover session-machine helpers; product sign-in is `screens/jf_login.rs`
mod browse; // Library browse: per-section paged catalog (sparse store + off-thread page fetches)
mod capture; // dev live UI capture stream: own-GLES-frame grab → MPEG1/TS or JPEG → TCP (UI plane only)
mod coldstart; // retires old last-page bookmarks; authenticated cold boots now stay on Home
mod curlio; // the HTTPS media plane: a remote file pulled by byte range over libcurl-multi (stream.rs is the plaintext-socket twin)
mod dev; // the /tmp/nativejelly-* trigger surface, behind one `devtriggers` feature — read it before adding a trigger
#[macro_use]
mod diag; // typed usage schema plus log/lab scrub, ring and zlib; native crashes have a separate allowlist
mod ff; // THE demuxer — the FFmpeg 9.0 this app BUNDLES and pins (majors 63/63/61), dlopen'd by absolute path beside the binary, never the television's
mod focusprobe; // dev: one diffable line naming everything app.rs's key ladder can move, logged when it changes
mod hls; // strict parser/auth/timeline for the measured one-variant PMS HLS shape
mod http; // the ONE door out of the control plane: dispatch a catalog REST request on its origin's scheme (stream.rs for http, net.rs/libcurl for https)
mod jf; // Jellyfin behind the catalog::Client facade: seat registry, DTOs, DTO→catalog conversion, ops
mod lab; // Cloud Lab bridge: pinned diagnostic uploads + optional outbound command long-poll
mod metadata; // item detail data layer (detail page): full metadata + seasons/episodes + cast + related
mod person; // person/actor page data layer: the header handed in by the cast row + /library/people/{id}/media
mod collection; // collection page model: tag resolution, header metadata and paged members
mod player; // buffer-feed video engine (was playback.c) — step 5
mod catalog; // typed catalog facade (was plex/) — Plex-shaped DTOs; live reads go through `jf/`
mod catalog_fetch;
// Pure RELEASE_LINE-parsing helpers, `include!`d verbatim by build.rs so `cargo test --lib`
// actually runs their unit tests (see the module for why). Nothing in the app itself calls
// them at runtime — the version rule they implement is applied once, at compile time, by
// build.rs — so they exist in THIS crate only for the test build; `#[cfg(test)]` here, not on
// the functions themselves, because build.rs's own separate compilation is never built with
// `--test` and needs them unconditionally.
#[cfg(test)]
mod release_line;
mod remote; // dev/testing remote-control channel: a FIFO the loop drains into synthetic SDL keys
mod screens; // the application's OWNED screens (restructure phase 5b): the Settings family on the dispatcher
mod route; // play_movie route selection (direct-play vs transcode) — step 3
mod search; // Search data layer: /hubs/search fanned out across every source, merged into typed shelves
#[cfg(feature = "hostsim")]
mod shot; // simulator screenshots: read the frame back and write a PNG (see the module doc)
mod stores; // stores as machines (restructure phase 4): one command vocabulary + one step per data store
mod system;
mod telemetry; // the opt-in crash + usage channels: consent, the spool, the worker, the two wire formats
mod viewstate; // watched / unwatched / remove-from-deck: the PMS view-state WRITES, off the SDL thread

mod textinput; // the TV's own on-screen keyboard, via plain SDL_StartTextInput (see the module doc)
mod ui;

/// The instance root, for the simulator binary.
///
/// `src/bin/sim.rs` is a separate crate and cannot see `pub(crate)` items, but it must create the
/// directory and truncate the event log inside it before the app starts. Exposing the resolver
/// keeps ONE definition of where that is — a second `env::var` read in the binary would be a
/// second answer waiting to drift from this one.
#[cfg(feature = "hostsim")]
pub fn sim_runtime_dir() -> std::path::PathBuf {
    nj_base::paths::runtime_dir().to_path_buf()
}

/// The event log's path, built by the ONE expression [`nj_base::eventlog::log`] uses.
///
/// `src/bin/sim.rs` truncates this file at startup. Spelling the name a second time over there
/// would mean a rename could leave the binary truncating a file the app never appends to — the
/// simulator's log would silently start non-empty, which is exactly the state `tests/run.py` dates
/// its first line from.
#[cfg(feature = "hostsim")]
pub fn sim_events_log() -> std::path::PathBuf {
    nj_base::eventlog::events_log()
}

/// The port holds `nj_run`, the C entry the simulator binary calls too. It is reached by path
/// (`nativejelly_modules::port::nj_run`) so the SAME entry the C shim calls is the one called, with
/// the compiler checking the signature. It previously re-declared `nj_run` in its own `extern "C"`
/// block, which meant the one binary whose whole premise is "cannot drift from the shipped boot
/// path" was the one place a signature change would become a silent ABI mismatch instead of a
/// compile error. There is deliberately no `pub use` here: a re-export would be a reference from
/// the crate root into the port.
#[cfg(feature = "hostsim")]
pub mod port;
#[cfg(not(feature = "hostsim"))]
mod port;
#[cfg(feature = "hostsim")]
pub use app::synthetic_home_initial;

