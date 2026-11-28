//! HBR audio support on rooted TVs: runs one shell command as root through Homebrew Channel's
//! `exec` service, which a rooted set exposes to apps on the `public` LS2 group (seen on an LG C1,
//! webOS 6: `api-permissions.d/org.webosbrew.hbchannel.service.api.public.json`). Blocking, up to a
//! minute: call it from a worker thread. The command and what it installs are
//! `player::hbr_support`'s; this is only the bus call.

#[cfg(all(not(feature = "hostsim"), not(any(test, feature = "test-support"))))]
const EXEC: &str = "luna://org.webosbrew.hbchannel.service/exec";

/// The command's standard output, or why Homebrew Channel did not run it (not installed, not
/// rooted, refused). A command that ran and failed still answers its output when it printed any:
/// the installer's `hbr: REFUSED: …` line is the caller's to read.
#[cfg_attr(any(feature = "hostsim", feature = "test-support"), allow(dead_code))]
fn parse(reply: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(reply).map_err(|_| "unreadable reply".to_string())?;
    let out = value.get("stdoutString").and_then(serde_json::Value::as_str).unwrap_or("");
    if value.get("returnValue").and_then(serde_json::Value::as_bool) == Some(true) || !out.trim().is_empty() {
        return Ok(out.to_string());
    }
    let why = value.get("errorText").and_then(serde_json::Value::as_str).unwrap_or("refused");
    Err(format!("Homebrew Channel: {why}"))
}

#[cfg(any(feature = "hostsim", test, feature = "test-support"))]
pub fn exec(_command: &str) -> Result<String, String> {
    Err("not a television".into())
}

#[cfg(all(not(feature = "hostsim"), not(any(test, feature = "test-support"))))]
pub fn exec(command: &str) -> Result<String, String> {
    use super::ls2;
    const BUDGET: std::time::Duration = std::time::Duration::from_secs(60);
    let payload = serde_json::json!({ "command": command }).to_string();
    let registration = ls2::register().map_err(|e| format!("bus: {e}"))?;
    let reply = registration.call(EXEC, &payload, BUDGET).map_err(|_| "Homebrew Channel did not answer".to_string())?;
    parse(&reply)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn the_output_is_the_answer() {
        let reply = r#"{"returnValue":true,"stdoutString":"hbr: active\n","stdoutBytes":"aGJyOiBhY3RpdmUK","stderrString":"","stderrBytes":""}"#;
        assert_eq!(parse(reply), Ok("hbr: active\n".into()));
    }

    #[test]
    fn a_command_that_failed_still_answers_what_it_printed() {
        let reply = r#"{"returnValue":false,"errorText":"exit 1","stdoutString":"hbr: REFUSED: no module\n"}"#;
        assert_eq!(parse(reply), Ok("hbr: REFUSED: no module\n".into()));
    }

    #[test]
    fn a_refused_call_is_an_error() {
        assert!(parse(r#"{"returnValue":false,"errorText":"Denied method call"}"#).is_err());
        assert!(parse("not json").is_err());
    }
}
