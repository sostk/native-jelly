//! The panel's HDR10 capability, read at boot in the same configd call as Dolby Vision
//! (`webos::caps`): `tv.model.supportHDR`, the key LG's own `webOSTV.js` reports as
//! `deviceInfo().hdr10`.
//!
//! Like [`super::dv`], only the port's worker publishes, and an absent or late answer stays
//! `Unknown` rather than reading as either yes or no.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HdrCapability {
    Unknown,
    Supported,
    Unsupported,
}

impl HdrCapability {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
        }
    }
}

static RESULT: OnceLock<HdrCapability> = OnceLock::new();

/// Cached result only. This performs no registration, filesystem access, wait or initialization.
pub fn capability() -> HdrCapability {
    RESULT.get().copied().unwrap_or(HdrCapability::Unknown)
}

/// The boot probe's answer. First write wins.
pub fn publish(capability: HdrCapability) {
    let _ = RESULT.set(capability);
}
