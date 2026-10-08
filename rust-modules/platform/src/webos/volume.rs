//! The set's volume and mute, read over `luna://com.webos.service.audio/getVolume` for
//! [`crate::devcaps::volume`].
//!
//! LG's public `com.webos.audio` only sets volume; the read is the system service, whose reply
//! (`{"returnValue":true,"muted":…,"volume":…}`) was seen on the dev set through `luna-send`
//! (`tools/tv-session.sh sound status`). Whether the hub lets the jailed app make the same call is
//! unmeasured, so a refusal is final and quiet: one log line, the worker stops, and reports go on
//! without the fields.
//!
//! One worker owns the registration for the life of the process (no LS2 handle crosses a thread
//! boundary); [`refresh`] only nudges it, coalescing requests while a call is in flight.

use crate::devcaps::volume::Volume;

#[cfg(all(not(feature = "hostsim"), not(any(test, feature = "test-support"))))]
const GET_VOLUME: &str = "luna://com.webos.service.audio/getVolume";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(any(feature = "hostsim", feature = "test-support"), allow(dead_code))]
enum Failure {
    Json,
    Refused,
    Level,
    Muted,
}

#[cfg_attr(any(feature = "hostsim", test, feature = "test-support"), allow(dead_code))]
impl Failure {
    const fn stage(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Refused => "refused",
            Self::Level => "volume",
            Self::Muted => "muted",
        }
    }
}

/// Grade the one reply shape seen on the set. A level outside 0–100 or a non-boolean mute is not
/// coerced into one.
#[cfg_attr(any(feature = "hostsim", feature = "test-support"), allow(dead_code))]
fn parse(reply: &str) -> Result<Volume, Failure> {
    let value: serde_json::Value = serde_json::from_str(reply).map_err(|_| Failure::Json)?;
    let root = value.as_object().ok_or(Failure::Json)?;
    if root.get("returnValue").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(Failure::Refused);
    }
    let level = root
        .get("volume")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u8::try_from(n).ok())
        .filter(|n| *n <= 100)
        .ok_or(Failure::Level)?;
    let muted = root.get("muted").and_then(serde_json::Value::as_bool).ok_or(Failure::Muted)?;
    Ok(Volume { level, muted })
}

#[cfg(any(feature = "hostsim", test, feature = "test-support"))]
pub fn refresh() {}

#[cfg(all(not(feature = "hostsim"), not(any(test, feature = "test-support"))))]
pub fn refresh() {
    use std::sync::mpsc;
    use std::sync::OnceLock;
    static REQUESTS: OnceLock<Option<mpsc::SyncSender<()>>> = OnceLock::new();
    let requests = REQUESTS.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(1);
        nj_base::task::spawn("webos volume", move || serve(rx)).map(|_| tx)
    });
    if let Some(tx) = requests {
        let _ = tx.try_send(());
    }
}

#[cfg(all(not(feature = "hostsim"), not(any(test, feature = "test-support"))))]
fn serve(requests: std::sync::mpsc::Receiver<()>) {
    use super::ls2::{self, Fail};
    const BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

    let registration = match ls2::register() {
        Ok(registration) => registration,
        Err(e) => {
            nj_base::eventlog::log(&format!("volume: {GET_VOLUME} not asked: {e}"));
            return;
        }
    };
    let mut logged: Option<&'static str> = None;
    while requests.recv().is_ok() {
        let (volume, outcome) = match registration.call(GET_VOLUME, "{}", BUDGET) {
            Ok(reply) => match parse(&reply) {
                Ok(volume) => (Some(volume), "answered"),
                Err(failure) => (None, failure.stage()),
            },
            Err(Fail::Timeout) => (None, "timeout"),
            Err(Fail::Setup { stage, .. }) => (None, stage),
        };
        crate::devcaps::volume::publish(volume);
        if logged != Some(outcome) {
            let detail = volume.map(|v| format!(" level={} muted={}", v.level, v.muted)).unwrap_or_default();
            nj_base::eventlog::log(&format!("volume: {GET_VOLUME} {outcome}{detail}"));
            logged = Some(outcome);
        }
        if outcome == Failure::Refused.stage() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, Failure, Volume};

    #[test]
    fn the_reply_seen_on_the_set_reads_as_level_and_mute() {
        assert_eq!(
            parse(r#"{"returnValue": true, "muted": false, "volume": 12}"#),
            Ok(Volume { level: 12, muted: false })
        );
        assert_eq!(
            parse(r#"{"returnValue":true,"muted":true,"volume":0}"#),
            Ok(Volume { level: 0, muted: true })
        );
    }

    #[test]
    fn anything_else_is_no_reading() {
        for (reply, failure) in [
            ("not json", Failure::Json),
            ("[]", Failure::Json),
            (r#"{"returnValue":false,"errorText":"Denied method call"}"#, Failure::Refused),
            (r#"{"muted":true,"volume":3}"#, Failure::Refused),
            (r#"{"returnValue":true,"muted":true}"#, Failure::Level),
            (r#"{"returnValue":true,"muted":true,"volume":101}"#, Failure::Level),
            (r#"{"returnValue":true,"muted":true,"volume":-1}"#, Failure::Level),
            (r#"{"returnValue":true,"muted":true,"volume":"12"}"#, Failure::Level),
            (r#"{"returnValue":true,"volume":12}"#, Failure::Muted),
            (r#"{"returnValue":true,"muted":"false","volume":12}"#, Failure::Muted),
            (
                r#"{"returnValue":true,"volumeStatus":{"muteStatus":false,"volume":12}}"#,
                Failure::Level,
            ),
        ] {
            assert_eq!(parse(reply), Err(failure), "{reply}");
        }
    }
}
