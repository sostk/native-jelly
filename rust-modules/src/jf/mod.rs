//! Jellyfin, behind the Plex-shaped facade.
//!
//! The app above `plex::Client` speaks one vocabulary — PMS containers, rating keys, part keys,
//! `/:/timeline` reports. A server registered as a Jellyfin [`seat`] answers the same `Client`
//! methods through [`Jf`]: each op asks the Jellyfin API and [`convert`]s the answer into the PMS
//! types, so stores, screens and the player do not know which server they are reading.
//!
//! * [`ids`] — Jellyfin GUIDs interned to the integer keys the app's types carry.
//! * [`ticks`] — 100 ns ticks ⇄ milliseconds.
//! * [`url`] — the `Authorization: MediaBrowser …` header, the per-user DeviceId and the
//!   `ApiKey=` query credential media URLs carry.
//! * [`models`] — the Jellyfin DTOs.
//! * [`convert`] — DTO → PMS model, artwork paths, markers.
//! * `api` / `playback` — the operations.
#![allow(dead_code)]

mod api;
pub mod auth;
pub mod blurhash;
pub mod convert;
pub mod ids;
pub mod models;
mod playback;
pub mod seat;
pub mod ticks;
pub(crate) mod url;

pub use api::Jf;

#[cfg(test)]
mod live_tests;
