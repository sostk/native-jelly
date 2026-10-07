//! The Jellyfin backend.
//!
//! A server registered as a Jellyfin [`seat`] is reached through [`Jf`]. The list stores (Home,
//! Library, Search, Person, Collection, Related) ask it for Jellyfin results directly — shelves,
//! pages and `BaseItemDto` rows — and build their screen rows from those. The detail page, the
//! player and the route planner still read the full item record [`convert`] assembles.
//!
//! * [`ids`] — Jellyfin GUIDs interned to the integer keys the app's types carry.
//! * [`ticks`] — 100 ns ticks ⇄ milliseconds.
//! * [`url`] — the `Authorization: MediaBrowser …` header, the per-user DeviceId and the
//!   `ApiKey=` query credential media URLs carry.
//! * [`models`] — the Jellyfin DTOs.
//! * [`images`] — artwork paths and the image request each resolves to.
//! * [`convert`] — DTO → the full item record the detail page and player read, markers.
//! * `api` / `playback` — the operations.
//! * [`address`] / [`store`] — what a typed server address can mean, and the kept sign-in.
#![allow(dead_code)]

pub mod address;
mod api;
pub mod auth;
pub mod blurhash;
pub mod convert;
pub mod ids;
pub mod images;
pub mod models;
pub(crate) mod playback;
pub mod seat;
pub mod store;
pub mod ticks;
pub(crate) mod url;

pub use api::{Jf, JfPage, JfSearch, JfShelf, SEARCH_GROUPS};

#[cfg(test)]
mod live_tests;
