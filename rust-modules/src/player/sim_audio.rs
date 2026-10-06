//! **The simulator's sound** (`nativejelly-simaudio`, with the clock sink armed): the audio access
//! units the clock sink already accepts are ALSO piped to a system `ffmpeg` child that decodes them
//! and plays them on this machine's sound server — the audio twin of [`super::sim_video`].
//!
//! **Paced by the sink's clock, not by the feeder.** The engine feeds audio up to
//! `MAX_FEED_AHEAD_NS + AUDIO_SLACK_NS` ahead of presentation; written straight through, the child
//! would play that much early. Each AU is held until the sink's clock is within [`lead_ns`] of its
//! PTS, so a paused player goes quiet (nothing more is written) and a seek, which is a fresh Load
//! or a flush, kills the child and starts clean.
//!
//! **What reaches it.** `ff.rs` reframes raw AAC as ADTS for the television, and AC-3, E-AC-3 and
//! DTS frames are self-delimiting, so every lane the Load payload can name is a stream a raw
//! demuxer can read with no container. `hostsim`-only, like the picture, and like it a facility
//! for a person looking at the simulator: nothing heard here says anything about LG's decoder.
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};

fn armed() -> bool {
    static ONCE: OnceLock<bool> = OnceLock::new();
    *ONCE.get_or_init(|| {
        let on = nj_base::devtrig::flag("simaudio");
        if on {
            nj_base::eventlog::log(
                "simaudio: ARMED — audio AUs are also decoded by a system ffmpeg and played on this \
                 machine. Nothing here measures the television.",
            );
        }
        on
    })
}

/// How far ahead of the clock an AU is written: the child's decode plus the sound server's buffer,
/// so what is heard lines up with the frame on screen. `NJ_SIM_AUDIO_LEAD_MS` overrides it.
fn lead_ns() -> i64 {
    static ONCE: OnceLock<i64> = OnceLock::new();
    *ONCE.get_or_init(|| {
        std::env::var("NJ_SIM_AUDIO_LEAD_MS")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(120)
            .clamp(0, 2000)
            * 1_000_000
    })
}

/// The raw demuxer for the Load payload's audio lane, `None` when the payload has no audio.
fn demuxer_for(payload: &str) -> Option<&'static str> {
    let at = payload.find("\"audio\":\"")? + "\"audio\":\"".len();
    let name = &payload[at..at + payload[at..].find('"')?];
    match name {
        "AC3" => Some("ac3"),
        "AC3 PLUS" => Some("eac3"),
        "AAC" => Some("aac"),
        "DTS" => Some("dts"),
        _ => None,
    }
}

struct Session {
    child: Child,
    feed: Sender<(i64, Vec<u8>)>,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static DEMUXER: Mutex<Option<&'static str>> = Mutex::new(None);
static CLOCK: OnceLock<fn() -> i64> = OnceLock::new();

/// A new stream: remember its audio codec and the clock, and drop the last session.
pub(crate) fn load(payload: &str, clock: fn() -> i64) {
    if !armed() {
        return;
    }
    let _ = CLOCK.set(clock);
    *DEMUXER.lock().unwrap_or_else(|e| e.into_inner()) = demuxer_for(payload);
    reset();
}

/// A flush or an unload: whatever is queued or playing belongs to a timeline that just ended.
pub(crate) fn stop() {
    if armed() {
        reset();
    }
}

fn reset() {
    GENERATION.fetch_add(1, Relaxed);
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// One audio access unit at `pts` in the fed timeline. Never blocks the feeder.
pub(crate) fn feed(au: &[u8], pts: i64) {
    if !armed() {
        return;
    }
    let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let Some(demuxer) = *DEMUXER.lock().unwrap_or_else(|e| e.into_inner()) else { return };
        *guard = spawn(demuxer);
    }
    let Some(session) = guard.as_ref() else { return };
    if session.feed.send((pts, au.to_vec())).is_err() {
        *guard = None;
    }
}

fn ffmpeg_path() -> std::ffi::OsString {
    std::env::var_os("NJ_SIM_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

/// `-f pulse` by default (PulseAudio and PipeWire both answer it); `NJ_SIM_AUDIO_OUT=alsa`
/// for a machine with neither.
fn output_args() -> Vec<String> {
    match std::env::var("NJ_SIM_AUDIO_OUT").ok().as_deref() {
        Some("alsa") => vec!["-f".into(), "alsa".into(), "default".into()],
        _ => vec!["-f".into(), "pulse".into(), "-buffer_duration".into(), "60".into(), "Native Jelly simulator".into()],
    }
}

fn spawn(demuxer: &'static str) -> Option<Session> {
    let generation = GENERATION.load(Relaxed);
    let spawned = Command::new(ffmpeg_path())
        .args(["-hide_banner", "-loglevel", "error", "-fflags", "nobuffer", "-probesize", "32768"])
        .args(["-analyzeduration", "0", "-f", demuxer, "-i", "pipe:0", "-vn", "-ac", "2"])
        .args(output_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            nj_base::eventlog::log(&format!("simaudio: could not start ffmpeg ({e}); no sound this session"));
            return None;
        }
    };
    let mut stdin = child.stdin.take()?;
    let (tx, rx) = channel::<(i64, Vec<u8>)>();
    let lead = lead_ns();
    let writer = std::thread::Builder::new().name("simaudio-in".into()).spawn(move || {
        let mut written = 0u64;
        for (pts, au) in rx {
            while CLOCK.get().is_some_and(|clock| clock() < pts - lead) {
                if GENERATION.load(Relaxed) != generation {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(4));
            }
            if GENERATION.load(Relaxed) != generation || stdin.write_all(&au).is_err() {
                return;
            }
            written += 1;
            if written == 1 {
                nj_base::eventlog::log(&format!("simaudio: first {demuxer} frame playing"));
            }
        }
    });
    if writer.is_err() {
        nj_base::eventlog::log("simaudio: could not spawn the pipe thread; no sound this session");
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    Some(Session { child, feed: tx })
}

#[cfg(test)]
mod tests {
    use super::demuxer_for;

    #[test]
    fn every_audio_lane_the_payload_can_name_has_a_raw_demuxer() {
        let p = |a: &str| format!(r#"{{"codec":{{"video":"H264","audio":"{a}"}}}}"#);
        assert_eq!(demuxer_for(&p("AC3")), Some("ac3"));
        assert_eq!(demuxer_for(&p("AC3 PLUS")), Some("eac3"));
        assert_eq!(demuxer_for(&p("AAC")), Some("aac"));
        assert_eq!(demuxer_for(&p("DTS")), Some("dts"));
        assert_eq!(demuxer_for(r#"{"codec":{"video":"H264"}}"#), None, "a video-only Load plays nothing");
    }
}
