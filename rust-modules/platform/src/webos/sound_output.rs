//! The set's sound output, read over `luna://com.webos.service.audio/getSoundOutput` for
//! [`crate::devcaps::sound_output`].
//!
//! The reply (`{"returnValue":true,"soundOutput":"external_arc","lastSoundOutput":"tv_speaker"}`)
//! was seen on an LG C1 (webOS 6) through `luna-send`; the jailed app reaches the same service's
//! `getVolume` there. A refusal is final and quiet, like the volume reader's: one log line, the
//! worker stops, and the output stays unknown, which keeps DTS passthrough off.

use crate::devcaps::sound_output::SoundOutput;

#[cfg(all(not(feature = "hostsim"), not(any(test, feature = "test-support"))))]
const GET_SOUND_OUTPUT: &str = "luna://com.webos.service.audio/getSoundOutput";

/// The reply's `soundOutput`, or `None` for a refusal or any other shape.
#[cfg_attr(any(feature = "hostsim", feature = "test-support"), allow(dead_code))]
fn parse(reply: &str) -> Option<SoundOutput> {
    let value: serde_json::Value = serde_json::from_str(reply).ok()?;
    if value.get("returnValue").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    value.get("soundOutput").and_then(serde_json::Value::as_str).filter(|s| !s.is_empty()).map(SoundOutput::named)
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
        nj_base::task::spawn("webos sound output", move || serve(rx)).map(|_| tx)
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
            nj_base::eventlog::log(&format!("sound output: {GET_SOUND_OUTPUT} not asked: {e}"));
            return;
        }
    };
    let mut logged: Option<String> = None;
    while requests.recv().is_ok() {
        let (output, outcome) = match registration.call(GET_SOUND_OUTPUT, "{}", BUDGET) {
            Ok(reply) => match parse(&reply) {
                Some(output) => (output, "answered"),
                None => (SoundOutput::default(), "refused"),
            },
            Err(Fail::Timeout) => (SoundOutput::default(), "timeout"),
            Err(Fail::Setup { stage, .. }) => (SoundOutput::default(), stage),
        };
        let line = format!("{outcome} {}", output.label());
        crate::devcaps::sound_output::publish(output);
        if logged.as_deref() != Some(line.as_str()) {
            nj_base::eventlog::log(&format!("sound output: {GET_SOUND_OUTPUT} {line}"));
            logged = Some(line);
        }
        if outcome == "refused" {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, SoundOutput};

    #[test]
    fn the_reply_seen_on_the_set_reads_as_its_output() {
        assert_eq!(
            parse(r#"{"returnValue":true,"soundOutput":"external_arc","lastSoundOutput":"tv_speaker"}"#),
            Some(SoundOutput::named("external_arc"))
        );
    }

    #[test]
    fn anything_else_is_no_reading() {
        for reply in [
            "not json",
            "[]",
            r#"{"returnValue":false,"errorText":"Denied method call"}"#,
            r#"{"soundOutput":"external_arc"}"#,
            r#"{"returnValue":true}"#,
            r#"{"returnValue":true,"soundOutput":""}"#,
            r#"{"returnValue":true,"soundOutput":3}"#,
        ] {
            assert_eq!(parse(reply), None, "{reply}");
        }
    }
}
