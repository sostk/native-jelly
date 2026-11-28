//! Where the television sends its sound, as last read by the port's sound-output reader.
//!
//! DTS passthrough (`devcaps::set_dts_passthrough`) feeds a DTS core as IEC 61937 bursts in stereo
//! PCM. A receiver's DTS decoder plays those; the set's own speakers, headphones or a Bluetooth
//! sink would play them as full-scale noise. Nothing counts as a receiver until the set has said so.

use std::sync::Mutex;

/// The set's `soundOutput` value, e.g. `external_arc`, `tv_speaker`, `bt_soundbar`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundOutput(Option<String>);

impl SoundOutput {
    pub fn named(output: &str) -> Self {
        Self(Some(output.to_owned()))
    }

    /// HDMI ARC/eARC or optical: the links that carry an IEC 61937 bitstream to an external
    /// decoder (the same two outputs dts_restore_plus passes a DTS core on). webOS names variants
    /// of the ARC output for LG soundbar features (`external_arc_sound_alive` with an LG soundbar
    /// on eARC, `external_arc_sound_mode_sync`, `external_arc_bno`), so the family counts by its
    /// prefix; the mixed `tv_speaker_optical_arc` also plays on the set's speakers and does not.
    pub fn is_receiver(&self) -> bool {
        self.0.as_deref().is_some_and(|s| s.starts_with("external_arc") || s.starts_with("external_optical"))
    }

    /// For the log: the set's own word, or `unknown` before it has answered.
    pub fn label(&self) -> &str {
        self.0.as_deref().unwrap_or("unknown")
    }
}

static LATEST: Mutex<SoundOutput> = Mutex::new(SoundOutput(None));
/// How many answers [`publish`] has taken, so a caller can tell a reading made after its ask from
/// one left over from before it.
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// The last answer. Performs no bus call.
pub fn latest() -> SoundOutput {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The reader's latest answer; an unknown output withdraws a value that can no longer be vouched for.
pub fn publish(output: SoundOutput) {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = output;
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Release);
}

/// The count of answers so far; pass it to [`fresh_since`] after asking with [`refresh`].
pub fn seq() -> u64 {
    SEQ.load(std::sync::atomic::Ordering::Acquire)
}

/// The first answer published after `asked` (a [`seq`] taken before the [`refresh`]), waiting at
/// most `wait`. Unknown when none arrives in time: the owner may have moved the sound from a
/// receiver to the set's speakers since the last reading, and there a DTS burst is full-scale noise.
pub fn fresh_since(asked: u64, wait: std::time::Duration) -> SoundOutput {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if seq() > asked {
            return latest();
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return SoundOutput::default();
        }
        std::thread::sleep((deadline - now).min(std::time::Duration::from_millis(10)));
    }
}

/// Ask for a fresh reading without waiting for it: the next [`latest`] sees it once the set has
/// answered. Off-device this does nothing.
pub fn refresh() {
    crate::tv::refresh_sound_output();
}

#[cfg(test)]
mod tests {
    use super::SoundOutput;

    #[test]
    fn every_arc_and_optical_variant_the_c1_reports_counts_as_a_receiver() {
        // The values webOS 6.5.3's audio daemon names on an LG C1 (strings of its libraries,
        // 2026-11-28); `external_arc_sound_alive` is what `getSoundOutput` answered with an LG
        // soundbar on eARC. `tv_speaker_optical_arc` also plays on the set's own speakers.
        for receiver in ["external_arc", "external_arc_sound_alive", "external_arc_sound_mode_sync", "external_arc_bno",
            "external_optical"] {
            assert!(SoundOutput::named(receiver).is_receiver(), "{receiver}");
        }
        for speakers in ["tv_speaker_optical_arc", "tv_speakerbar", "builtin_soundbar", "wisa_speaker", "lineout"] {
            assert!(!SoundOutput::named(speakers).is_receiver(), "{speakers}");
        }
    }

    #[test]
    fn a_reading_counts_only_once_it_is_newer_than_the_ask() {
        let _g = super::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::publish(SoundOutput::named("external_arc"));
        let asked = super::seq();
        let started = std::time::Instant::now();
        assert_eq!(super::fresh_since(asked, std::time::Duration::from_millis(60)), SoundOutput::default(),
            "a reading from before the ask is not vouched for: unknown keeps passthrough off");
        assert!(started.elapsed() >= std::time::Duration::from_millis(60));
        let t = std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            super::publish(SoundOutput::named("tv_speaker"));
        });
        assert_eq!(super::fresh_since(asked, std::time::Duration::from_secs(5)), SoundOutput::named("tv_speaker"));
        t.join().unwrap();
    }

    #[test]
    fn only_arc_and_optical_count_as_a_receiver() {
        assert!(SoundOutput::named("external_arc").is_receiver());
        assert!(SoundOutput::named("external_optical").is_receiver());
        for other in ["tv_speaker", "headphone", "bt_soundbar", "tv_external_speaker", ""] {
            assert!(!SoundOutput::named(other).is_receiver(), "{other}");
        }
        assert!(!SoundOutput::default().is_receiver());
        assert_eq!(SoundOutput::default().label(), "unknown");
    }
}
