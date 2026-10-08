//! The television's own volume and mute, as last read by the port's volume reader.
//!
//! The app has no volume of its own: the player renders at full level and the set's volume and mute
//! are what the viewer hears, so they are what a playback report can honestly call the client's.
//! Nothing is known until the set has answered; an unanswered or refused read leaves `None`, and a
//! report then omits both fields instead of claiming a level.

use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    /// 0–100, the set's own scale.
    pub level: u8,
    pub muted: bool,
}

static LATEST: Mutex<Option<Volume>> = Mutex::new(None);

/// The last answer, or `None`. Performs no bus call.
pub fn latest() -> Option<Volume> {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner())
}

/// The reader's latest answer; `None` withdraws a value that can no longer be vouched for.
pub fn publish(volume: Option<Volume>) {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = volume;
}

/// Ask for a fresh reading without waiting for it: the next [`latest`] sees it once the set has
/// answered. Off-device this does nothing.
pub fn refresh() {
    crate::tv::refresh_volume();
}
