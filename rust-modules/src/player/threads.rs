//! player::threads — the worker threads beside the demuxer (load / timeline) + the
//! SendPtr spawn seam. The demux thread body is `ff::demux` (libavformat over a custom
//! AVIO on stream.rs); its HttpStream box lives in the Engine (main owns it, closes it
//! to interrupt, and outlives the threads).
use super::SHARED;
use std::os::raw::c_char;
use std::sync::atomic::Ordering;

/// raw ptr we assert is Send for the spawn (the boxes/queue outlive the thread).
pub(crate) struct SendPtr<T>(pub *mut T);
unsafe impl<T> Send for SendPtr<T> {}

/// media/load thread: construct + Load (uid=NULL). The library owns its own GMainContext + loop,
/// and callbacks arrive on its thread — but Load itself is SYNCHRONOUS here and on some chassis
/// blocks for a long time inside video-output init (issue #74, k5lp/k3lp), which is why the C seam
/// refuses every other verb until it returns (`g_load_returned`) and the pump bounds the wait
/// with `NATIVE_LOAD_BUDGET`.
pub(crate) fn load_thread(
    payload: SendPtr<c_char>,
    native_epoch: u32,
    route_start: Option<crate::route::RouteStartAttempt>,
) {
    super::log("SMP: calling Load (uid=NULL)");
    let ok = unsafe { super::sink().load(payload.0, native_epoch) };
    super::log(&format!("SMP: Load returned ok={ok}"));
    // `/tmp/nativejelly-holdload[=ms]`: on demand, hold the flip below so issue #74 D.1's budget
    // (the pump's "deferring" line, then, past `NATIVE_LOAD_BUDGET`, the failure read-out) is
    // observable on a real television without a set that hangs here for real. Read AFTER
    // `sf_load` returns and BEFORE `mark_native_load_returned`, matching where this needs to bite.
    if let Some(ms) = nj_base::devtrig::holdload_delay_ms() {
        super::log(&format!(
            "holdload: armed — holding the Load-returned flag for {ms}ms"
        ));
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    // issue #74 D.1: flip the epoch-scoped gate BEFORE the route ticket is published and BEFORE
    // `load_failed` is set below — `pump.rs`'s loadCompleted arm (and every other Starfish/ACB
    // verb) must never observe "Load has returned" any later than this. Route tickets are a
    // SEPARATE mechanism (a ticket can be rejected as stale independent of this gate) and are
    // published after, not before, on purpose.
    if let Some(elapsed) = SHARED.mark_native_load_returned(native_epoch) {
        // Captured HERE, at the moment the flag actually flips, so the "native: Load returned
        // after Nms" log measures the real in-flight duration rather than however long it took
        // the main-thread pump to next run and notice. Logged unconditionally, right at the gate
        // transition, rather than from `pump.rs`'s `loadCompleted` arm — that arm only runs once
        // `loadCompleted` also arrives, so on a session where Load returns but `loadCompleted`
        // never does, the old placement left this line unlogged forever, with no way to tell
        // "Load is still on the stack" from "Load returned and the pipeline went quiet". See
        // finding `load-returned-log-is-conditional-on-loadcompleted-and-unbounded-after`.
        let elapsed_ms = elapsed.as_millis() as u64;
        SHARED
            .native_load_elapsed_ms
            .store(elapsed_ms, Ordering::Relaxed);
        super::log(&format!("native: Load returned after {elapsed_ms}ms"));
        // issue #74 item 3: the Load-returned gate opening, bucketed — never the millisecond
        // count. This is the ordinary path; `pump.rs`'s two D.1.4 budget arms record the other
        // half, for a Load that never returns at all.
        super::report::note_load_gate_for(
            crate::route::playback_trace_generation(),
            super::report::LoadElapsedClass::from_ms(elapsed_ms as i64),
        );
    }
    if let Some(ticket) = route_start {
        crate::route::publish_route_start_result(
            ticket,
            if ok != 0 {
                crate::route::RouteStartResult::Started
            } else {
                crate::route::RouteStartResult::StartFailed
            },
        );
    }
    if ok == 0 {
        // Publish it. This used to be logged and discarded, so a refused payload was
        // indistinguishable from a slow one and the pump waited on a `loadCompleted` that could
        // never come.
        SHARED.publish_native_load_failure(native_epoch);
    }
}

/// How long the progress reporter waits between /:/timeline posts.
const REPORT_INTERVAL_S: u64 = 10;

/// One reporter's stop signal — **owned by that reporter and its Engine**, not shared.
///
/// It used to be `SHARED.report_stop` + `SHARED.report_wake`, which `reset_session` clears at the
/// END of teardown. So a reporter still alive at that moment — parked in its POST — came back to a
/// cleared flag and looped forever, which is precisely why teardown had to JOIN it before letting
/// the session reset. That join is on the MAIN thread, and `stream`'s one-shot wrappers box their
/// socket privately, so nothing could interrupt the POST: against a server that accepts and then
/// goes quiet the frame loop parked for the rest of `SO_RCVTIMEO`. **Measured at 6974 ms** with
/// `tools/netcond.py` in `stall@/:/timeline` mode.
///
/// That proxy's SCOPE was broken until 2026-08-23 (`relay` discarded it), so the run actually
/// stalled every connection rather than only the reporter's. The number survives: it was recorded
/// as `THREADJOIN timeline 6974ms`, one NAMED line per join, and `timeline` is the only join that
/// could have parked whatever else was stalled — `engine::teardown` wakes the demux socket and
/// both AU lanes before joining them, while this POST had no such wake, which is the whole reason
/// this type exists.
///
/// Per-session ownership makes the stop unambiguous: a detached reporter always sees ITS OWN flag
/// set and exits, no matter what the next session does to `SHARED`. That is what lets the join
/// move off the main thread.
pub(crate) struct ReportStop {
    state: std::sync::Mutex<Signal>,
    cv: std::sync::Condvar,
}

#[derive(Default)]
struct Signal {
    stopped: bool,
    nudged: bool,
}

/// Why [`ReportStop::wait`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wake {
    Stop,
    Nudge,
    Timeout,
}

impl ReportStop {
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(ReportStop { state: Default::default(), cv: std::sync::Condvar::new() })
    }
    /// Tell this reporter to exit, and wake it so it notices now rather than up to 10 s from now.
    pub(crate) fn stop(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
        self.cv.notify_all();
    }
    /// Ask for a report now. Several before the reporter wakes are one report.
    fn nudge(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).nudged = true;
        self.cv.notify_all();
    }

    /// Wait up to `d` for a stop or a nudge, a stop first. A nudge is consumed by the wait that
    /// reports it.
    ///
    /// The loop re-checks the predicate because `wait_timeout` may wake spuriously, and a spurious
    /// wake must not shorten the interval into an early extra POST.
    fn wait(&self, d: std::time::Duration) -> Wake {
        let deadline = std::time::Instant::now() + d;
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if g.stopped {
                return Wake::Stop;
            }
            if std::mem::take(&mut g.nudged) {
                return Wake::Nudge;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Wake::Timeout;
            }
            g = self.cv.wait_timeout(g, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

/// The reporter of the playback on screen, for [`report_now`]. Each reporter registers itself as
/// it starts, so the newest Engine's wins; nudging one that is already stopping does nothing.
static CURRENT: std::sync::Mutex<std::sync::Weak<ReportStop>> = std::sync::Mutex::new(std::sync::Weak::new());

fn register(stop: &std::sync::Arc<ReportStop>) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = std::sync::Arc::downgrade(stop);
}

/// Ask the playing reporter for a report now — after the viewer pauses, resumes, seeks or changes
/// a track, so the server hears it then rather than at the next heartbeat. Free when nothing is
/// playing.
pub(crate) fn report_now() {
    if let Some(stop) = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).upgrade() {
        stop.nudge();
    }
}

/// What the reporter knows when it wakes.
#[derive(Clone, Copy, Debug, Default)]
struct Due {
    /// Nothing has been reported for this Engine yet.
    first: bool,
    /// A picture has been presented this session (`SHARED.seen_frame`).
    presented: bool,
    /// The duration is known.
    duration_known: bool,
    /// A seek is in flight; the position is the old one.
    seeking: bool,
    /// The viewer did something the server should hear about now.
    nudged: bool,
    /// The heartbeat interval has run out.
    heartbeat: bool,
}

/// Whether to report now. Nothing until a picture is on the panel and the duration is known —
/// there is no playback to report before that — and nothing while a seek is in flight, whose
/// position is the one being left. Then: at once the first time (`/Sessions/Playing`), and after
/// that on the heartbeat or a nudge.
fn report_due(d: Due) -> bool {
    d.presented && d.duration_known && !d.seeking && (d.first || d.nudged || d.heartbeat)
}

/// What the reporter reads off the shared clock when it wakes: the [`Due`] facts and the duration
/// it would report, ns. `first`, `nudged` and `heartbeat` are the reporter's own.
fn sample_due(first: bool, nudged: bool, heartbeat: bool) -> (Due, i64) {
    let dur = SHARED.duration_ns.load(Ordering::Relaxed);
    let due = Due {
        first,
        presented: SHARED.seen_frame.load(Ordering::Relaxed),
        duration_known: dur > 0,
        seeking: SHARED.seeking.load(Ordering::Relaxed),
        nudged,
        heartbeat,
    };
    (due, dur)
}

/// How often a reporter with something pending (its first report, a nudge waiting out a seek)
/// looks again.
const REPORT_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// The playback reporter: `/Sessions/Playing` the moment the first picture is on the panel, then
/// `/Sessions/Playing/Progress` every [`REPORT_INTERVAL_S`] seconds and whenever [`report_now`]
/// is asked — a pause, a resume, a landed seek, a track change. The heartbeat counts from the
/// last report. The final `Stopped` is the stop's own (`route::ScrobbleWork`).
///
/// The lease names one exact Engine. Route identity, server, PlayQueue and track projection are
/// sampled together under `PlayerControl`; no field of the main-thread `Session` is touched here.
pub(crate) fn timeline_thread(
    lease: crate::route::TimelineLease,
    stop: std::sync::Arc<ReportStop>,
) {
    use crate::catalog::TimelineState;
    use crate::route::TimelineTick;
    register(&stop);
    let interval = std::time::Duration::from_secs(REPORT_INTERVAL_S);
    let mut last: Option<std::time::Instant> = None;
    let mut nudged = false;
    loop {
        // Something pending (the first report, a nudge waiting out a seek or a route transition)
        // is looked at again shortly; otherwise sleep until the heartbeat.
        let wait = match last {
            Some(at) if !nudged => interval.saturating_sub(at.elapsed()),
            _ => REPORT_POLL,
        };
        match stop.wait(wait) {
            Wake::Stop => return,
            Wake::Nudge => nudged = true,
            Wake::Timeout => {}
        }
        let (due, dur) = sample_due(
            last.is_none(),
            nudged,
            last.is_some_and(|at| at.elapsed() >= interval),
        );
        if !report_due(due) {
            continue;
        }
        let t = SHARED.playpos_ns.load(Ordering::Relaxed) / 1_000_000;
        let d = dur / 1_000_000;
        let state = if super::TX.paused.load(Ordering::Relaxed) {
            TimelineState::Paused
        } else {
            TimelineState::Playing
        };
        match crate::route::report_timeline_tick(&lease, state, t, d) {
            TimelineTick::Retired => return,
            // Pending, not lost: poll until the route settles.
            TimelineTick::Deferred => {
                nudged = true;
                continue;
            }
            TimelineTick::Sent(_) => {}
        }
        last = Some(std::time::Instant::now());
        nudged = false;
        super::log(&format!(
            "timeline {} t={}s/{}s",
            state.as_str(),
            t / 1000,
            d / 1000
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first report goes the moment a picture is on the panel — not ten seconds later, which
    /// left a short playback with a `Stopped` and no start. After it, the heartbeat or the viewer.
    #[test]
    fn the_first_report_goes_with_the_first_picture() {
        let at_start = Due { first: true, presented: true, duration_known: true, ..Default::default() };
        assert!(report_due(at_start), "no ten-second wait for the start");
        assert!(!report_due(Due { presented: false, ..at_start }), "nothing played yet");
        assert!(!report_due(Due { duration_known: false, ..at_start }));
        let playing = Due { first: false, ..at_start };
        assert!(!report_due(playing), "between heartbeats, nothing");
        assert!(report_due(Due { heartbeat: true, ..playing }));
        assert!(report_due(Due { nudged: true, ..playing }), "a pause, resume or track change goes now");
        assert!(!report_due(Due { nudged: true, seeking: true, ..playing }), "a seek reports where it lands");
        assert!(!report_due(Due { heartbeat: true, seeking: true, ..playing }));
    }

    /// **Regression: a transcode sent no `/Sessions/Playing` and no `/Progress` at all.** A live
    /// progressive conversion has no container duration, so `duration_known` was false for the
    /// whole playback and the reporter never found a report due. The open now publishes the
    /// item's runtime from the negotiation, and the reporter reports it.
    #[test]
    fn a_transcode_with_no_container_duration_still_reports() {
        const FILM_NS: i64 = 8_177_216_000_000;
        let _g = nj_base::testlock::serial();
        SHARED.reset_session();
        // What the progressive demuxer's open does for a pipe-written Matroska: no Duration.
        let _ = crate::ff::publish_open_duration(0, FILM_NS);
        SHARED.seen_frame.store(true, Ordering::Relaxed);
        let (first, dur) = sample_due(true, false, false);
        let (heartbeat, _) = sample_due(false, false, true);
        SHARED.reset_session();

        assert!(report_due(first), "/Sessions/Playing goes with the first picture: {first:?}");
        assert!(report_due(heartbeat), "/Progress goes on the heartbeat: {heartbeat:?}");
        assert_eq!(dur / 1_000_000, 8_177_216, "the reported duration is the film's, in ms");
    }

    /// A nudge wakes the reporter of the playback on screen at once; a stop still wins.
    #[test]
    fn a_nudge_wakes_the_current_reporter_at_once() {
        let _g = nj_base::testlock::serial();
        let stop = ReportStop::new();
        register(&stop);
        let waiting = stop.clone();
        let started = std::time::Instant::now();
        let h = std::thread::spawn(move || waiting.wait(std::time::Duration::from_secs(30)));
        std::thread::sleep(std::time::Duration::from_millis(50));
        report_now();
        assert_eq!(h.join().unwrap(), Wake::Nudge);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        stop.stop();
        report_now();
        assert_eq!(stop.wait(std::time::Duration::from_secs(1)), Wake::Stop, "a stop outranks a nudge");
    }
}
