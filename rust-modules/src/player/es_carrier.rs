//! Lossless audio to a receiver: DTS-HD bitstreams over LG's own Dolby Digital Plus
//! pass-through route (`docs/playback-settings.md`).
//!
//! The C1 has no DTS path, but it does pass DD+ through to an eARC receiver: libpf, LG's audio
//! library (liblxa) and libkadaptor all run inside this process, and liblxa packs each E-AC-3
//! frame into an IEC 61937 burst (24576 bytes at the 192 kHz stereo carrier, 16-bit big-endian)
//! that it writes into LG's ES renderer with `ioctl(fd, 0x40285207, arg)` (word 0 = buffer,
//! word 2 = size, word 8 = 1 for the ES renderer). `src/es_swap.c` exports `ioctl` from the
//! executable, so libkadaptor's call lands there first and hands such a write to [`nj_es_swap`].
//!
//! The engine declares the title `AC3 PLUS`; the demuxer feeds one silent E-AC-3 frame per 32 ms on
//! contiguous timestamps and queues the real track for the same 32 ms: three DTS-HD frames
//! (3 x 512 samples) as IEC 61937 type IV bursts ([`Packer`]). [`nj_es_swap`] points each LG write
//! at the next queued chunk, so the receiver gets the bitstream while LG keeps timing, A/V sync,
//! pause and seek exactly as for a DD+ title. Tested on an LG C1 (webOS 6) with a Sony STR-DN1080.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

/// LG's DD+ burst on the 192 kHz stereo carrier: 1536 samples x 4 (rate) x 4 bytes.
pub(crate) const CHUNK: usize = 24576;
/// One DTS-HD frame's burst at the same carrier: 512 samples x 4 x 4 bytes (IEC 61937 type IV,
/// repetition period 2048, subtype 2).
const BURST: usize = 8192;
/// A silent 48 kHz stereo E-AC-3 frame (1536 samples, 96 kb/s), made with
/// `ffmpeg -f lavfi -i anullsrc=r=48000:cl=stereo -c:a eac3 -b:a 96k -f eac3`; every frame of
/// that encode is identical, so one frame repeats for the whole title.
pub(crate) static SILENT_EAC3: &[u8] = include_bytes!("silent-eac3.bin");

static ON: AtomicBool = AtomicBool::new(false);
/// What `src/es_swap.c` reads before it touches an ioctl argument at all: non-zero only while
/// [`ON`], so with the carrier off the exported `ioctl` is a plain forward of every call.
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static nj_es_armed: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// One queued chunk: `(offset, bytes)` segments, every other byte of the chunk is zero. Stored
/// without the zero padding (a DTS-HD MA chunk carries about 7.7 KB), so two minutes of read-ahead
/// stay in the tens of MB.
type Chunk = Vec<(u32, Vec<u8>)>;
static QUEUE: Mutex<VecDeque<Chunk>> = Mutex::new(VecDeque::new());
/// The chunk LG's current write points at: kept alive until the next write replaces it.
static CURRENT: Mutex<Option<Box<[u8]>>> = Mutex::new(None);
static DROPPED: AtomicU32 = AtomicU32::new(0);
static SWAPPED: AtomicU32 = AtomicU32::new(0);
static MISSED: AtomicU32 = AtomicU32::new(0);
static OTHER: AtomicU32 = AtomicU32::new(0);
/// The renderer refused the last ES write (`-1`, buffer full); liblxa retries the same write, which
/// must carry the same chunk again rather than the next one.
static LAST_FAILED: AtomicBool = AtomicBool::new(false);
static RETRIED: AtomicU32 = AtomicU32::new(0);
/// liblxa's own words 0 (buffer) and 2 (size) of the write [`nj_es_swap`] repointed, put back by
/// [`nj_es_swap_done`] after the syscall: the swap lasts for the kernel's copy only, and liblxa
/// reads back exactly the block it passed. Writes come from one liblxa thread.
static ORIGINAL: Mutex<Option<(u32, u32)>> = Mutex::new(None);
static CORE_ONLY: AtomicU32 = AtomicU32::new(0);

/// The DD+ route never reached LG's ES renderer in this app run (see [`silent_route`]): the
/// carrier stays off until the app restarts, and DTS/TrueHD use the PCM core and the AC-3 converter.
static UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// The webOS major the ES-renderer write (`ioctl` request `0x40285207`, its 40-byte argument and
/// the 24576-byte DD+ burst) was measured on: an LG C1, webOS 6. Elsewhere the carrier stays off
/// and `src/es_swap.c` never reads an argument.
const MEASURED_MAJOR: u32 = 6;

/// May this playback use the carrier at all? Only on the measured firmware, and not after
/// [`silent_route`] found LG's DD+ route not in pass-through this app run.
pub(crate) fn available() -> bool {
    firmware_measured(nj_platform::tv::device::info().major) && !UNAVAILABLE.load(Ordering::Acquire)
}

fn firmware_measured(major: u32) -> bool {
    major == MEASURED_MAJOR
}

/// Is the carrier on for the playback being started?
pub(crate) fn on() -> bool {
    ON.load(Ordering::Acquire)
}

/// Pictures to wait for before deciding LG's DD+ route is not in pass-through (about 3 s at 24 fps;
/// the first ES write comes with the first audio, well before that).
const SILENT_ROUTE_FRAMES: i32 = 75;

/// Called every pump tick with the pictures shown so far: `true` once when the carrier is on, the
/// title has shown [`SILENT_ROUTE_FRAMES`] pictures, and LG has made no ES write at all. LG then
/// plays our silent E-AC-3 itself (TV speakers, or a receiver without DD+ on its eARC/ARC), so the
/// caller reloads at the same position and the carrier stays off for this app run.
pub(crate) fn silent_route(frames: i32) -> bool {
    if !ON.load(Ordering::Acquire) || frames < SILENT_ROUTE_FRAMES {
        return false;
    }
    let writes = SWAPPED.load(Ordering::Relaxed) + MISSED.load(Ordering::Relaxed) + RETRIED.load(Ordering::Relaxed);
    if writes > 0 || UNAVAILABLE.swap(true, Ordering::AcqRel) {
        return false;
    }
    set_on(false);
    true
}

/// Start or stop swapping LG's ES writes; both clear the queue and the counters.
pub(crate) fn set_on(on: bool) {
    clear();
    LAST_FAILED.store(false, Ordering::Relaxed);
    for c in [&SWAPPED, &MISSED, &OTHER, &CORE_ONLY, &DROPPED, &RETRIED] {
        c.store(0, Ordering::Relaxed);
    }
    ON.store(on, Ordering::Release);
    nj_es_armed.store(i32::from(on), Ordering::Release);
}

/// Drop everything queued (a seek: the bursts belong to a position that no longer exists).
pub(crate) fn clear() {
    if let Ok(mut q) = QUEUE.lock() {
        q.clear();
    }
}

fn push(chunk: Chunk) {
    if let Ok(mut q) = QUEUE.lock() {
        // ponytail: a fixed cap instead of PTS matching. The demuxer reads 10-20 s ahead of the
        // playhead, so the cap must cover that (4096 chunks = 131 s); a drop skips audio LG has not
        // played and the receiver loses lock. Upgrade path: key chunks by the write's timestamp
        // (words 4-5, ns) and bound the queue by the demuxer's own audio lane.
        if q.len() >= 4096 {
            q.pop_front();
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        q.push_back(chunk);
    }
}

/// Called from `src/es_swap.c` for every write to LG's ES renderer (`w` is the ioctl argument).
#[no_mangle]
pub extern "C" fn nj_es_swap(w: *mut u32) {
    if w.is_null() || !ON.load(Ordering::Acquire) {
        return;
    }
    if LAST_FAILED.swap(false, Ordering::AcqRel) {
        // A retry of a refused write: send the chunk it carried, not the next one.
        if let Ok(current) = CURRENT.lock() {
            if let Some(buf) = current.as_deref() {
                // SAFETY: as below; CURRENT still owns the buffer.
                unsafe {
                    keep_original(w);
                    *w = buf.as_ptr() as usize as u32;
                    *w.add(2) = buf.len() as u32;
                }
                RETRIED.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }
    // SAFETY: the C side passes libkadaptor's 40-byte renderer-write argument, words 0..9.
    let size = unsafe { *w.add(2) } as usize;
    if size != CHUNK {
        OTHER.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let next = QUEUE.lock().ok().and_then(|mut q| q.pop_front());
    let Some(chunk) = next else {
        MISSED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let Ok(mut current) = CURRENT.lock() else { return };
    let buf = current.get_or_insert_with(|| vec![0u8; CHUNK].into_boxed_slice());
    buf.fill(0);
    for (at, part) in &chunk {
        let at = *at as usize;
        if at + part.len() <= buf.len() {
            buf[at..at + part.len()].copy_from_slice(part);
        }
    }
    // SAFETY: 32-bit ARM, so the buffer address fits word 0; CURRENT keeps the buffer until the
    // next write, i.e. past the real ioctl that copies it (writes come from one liblxa thread).
    unsafe {
        keep_original(w);
        *w = buf.as_ptr() as usize as u32;
        *w.add(2) = CHUNK as u32;
    }
    let n = SWAPPED.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n % 250 == 0 {
        crate::player::log(&format!(
            "es_carrier: {n} DD+ bursts replaced (queued {}, dropped {}, missed {}, retried {}, other sizes {}, core-only frames {})",
            QUEUE.lock().map_or(0, |q| q.len()),
            DROPPED.load(Ordering::Relaxed),
            MISSED.load(Ordering::Relaxed),
            RETRIED.load(Ordering::Relaxed),
            OTHER.load(Ordering::Relaxed),
            CORE_ONLY.load(Ordering::Relaxed)
        ));
    }
}

/// Remember liblxa's words 0 and 2 before [`nj_es_swap`] overwrites them.
///
/// # Safety
/// `w` is the 40-byte renderer-write argument the C side passed.
unsafe fn keep_original(w: *mut u32) {
    let words = (*w, *w.add(2));
    if let Ok(mut o) = ORIGINAL.lock() {
        *o = Some(words);
    }
}

/// Called after the real ioctl of every ES-renderer write. A refused write (`-1`: the renderer's
/// buffer is full) is retried by liblxa with the same argument block; [`nj_es_swap`] then resends
/// the same chunk.
#[no_mangle]
pub extern "C" fn nj_es_swap_done(w: *mut u32, ret: i32) {
    if w.is_null() {
        return;
    }
    let original = ORIGINAL.lock().ok().and_then(|mut o| o.take());
    if ON.load(Ordering::Acquire) {
        done_counts(w, ret);
    }
    if let Some((buffer, size)) = original {
        // SAFETY: the same argument block; liblxa gets back the words it wrote.
        unsafe {
            *w = buffer;
            *w.add(2) = size;
        }
    }
}

fn done_counts(w: *const u32, ret: i32) {
    static SEEN: AtomicU32 = AtomicU32::new(0);
    if ret < 0 {
        LAST_FAILED.store(true, Ordering::Release);
    }
    // SAFETY: the same 40-byte argument the C side passed to `nj_es_swap`.
    let size = unsafe { *w.add(2) } as usize;
    if size != CHUNK && SEEN.fetch_add(1, Ordering::Relaxed) < 10 {
        crate::player::log(&format!("es_carrier: unexpected ES write size {size} ret {ret}"));
    }
}

/// One DTS-HD frame as an IEC 61937 type IV burst for the 192 kHz stereo carrier, big-endian, as
/// FFmpeg's spdif muxer writes it with `-dtshd_rate 192000 -spdif_flags be`. A frame too large for
/// the burst sends its core alone (`core_len`), for this frame only. `None` when not even the core
/// fits.
fn dtshd_burst(frame: &[u8], core_len: usize) -> Option<Vec<u8>> {
    const START: [u8; 10] = [0x01, 0, 0, 0, 0, 0, 0, 0, 0xfe, 0xfe];
    let period = BURST;
    let fits = |n: usize| START.len() + 2 + n <= period - 8;
    let payload = if fits(frame.len()) {
        frame
    } else if core_len > 0 && core_len <= frame.len() && fits(core_len) {
        CORE_ONLY.fetch_add(1, Ordering::Relaxed);
        &frame[..core_len]
    } else {
        return None;
    };
    let out_bytes = START.len() + 2 + payload.len();
    let length_code = (out_bytes + 8 + 15) / 16 * 16 - 8;
    let mut out = vec![0u8; period];
    out[..4].copy_from_slice(&[0xf8, 0x72, 0x4e, 0x1f]);
    out[4..6].copy_from_slice(&0x0211u16.to_be_bytes());
    out[6..8].copy_from_slice(&(length_code as u16).to_be_bytes());
    out[8..18].copy_from_slice(&START);
    out[18..20].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    out[20..20 + payload.len()].copy_from_slice(payload);
    Some(out)
}

/// Groups DTS-HD frames three at a time into one queued chunk; the demuxer feeds a silent E-AC-3
/// frame with the first frame's timestamp whenever [`Packer::frame`] answers `Some`.
/// One carrier frame: 1536 samples at 48 kHz.
const FRAME_NS: i64 = 32_000_000;
/// A source timestamp further than this from the contiguous one restarts the carrier timeline.
const REANCHOR_NS: i64 = 100_000_000;

#[derive(Default)]
pub(crate) struct Packer {
    acc: Vec<Vec<u8>>,
    first_pts: i64,
    /// The timestamp the next carrier frame gets when the source agrees within [`REANCHOR_NS`].
    /// Contiguous carrier timestamps matter: LG schedules each 32 ms E-AC-3 frame at its PTS, and a
    /// gap between frames (container rounding, or a file whose timestamps are stretched) becomes a
    /// gap in the IEC 61937 stream, where the receiver drops the DTS-HD lock.
    next_pts: Option<i64>,
}

impl Packer {
    /// Add one 512-sample DTS-HD frame. `Err` when it cannot be packed at all.
    pub(crate) fn frame(&mut self, frame: &[u8], core_len: usize, pts: i64) -> Result<Option<i64>, ()> {
        let mut burst = dtshd_burst(frame, core_len).ok_or(())?;
        let used = 20 + usize::from(u16::from_be_bytes([burst[18], burst[19]]));
        burst.truncate(used);
        if self.acc.is_empty() {
            self.first_pts = pts;
        }
        self.acc.push(burst);
        if self.acc.len() < 3 {
            return Ok(None);
        }
        let period = BURST;
        push(std::mem::take(&mut self.acc).into_iter().enumerate().map(|(i, b)| ((i * period) as u32, b)).collect());
        let pts = match self.next_pts {
            Some(next) if (self.first_pts - next).abs() <= REANCHOR_NS => next,
            _ => self.first_pts,
        };
        self.next_pts = Some(pts + FRAME_NS);
        Ok(Some(pts))
    }

    pub(crate) fn reset(&mut self) {
        self.acc.clear();
        self.next_pts = None;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_carrier_runs_only_on_the_firmware_its_write_was_measured_on() {
        assert!(super::firmware_measured(6), "LG C1, webOS 6: the measured ES-renderer write");
        for major in [0, 3, 4, 5, 7, 10, 25] {
            assert!(!super::firmware_measured(major), "webOS {major}: never arm the ioctl hook");
        }
    }

    /// The carrier is process-global state (one LG ES renderer per process); tests that touch it
    /// take this lock so cargo's parallel runner cannot interleave them.
    static GLOBALS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn globals() -> std::sync::MutexGuard<'static, ()> {
        GLOBALS.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn burst_matches_ffmpeg_spdif_dtshd_192k_be() {
        // First burst of `ffmpeg -i dtshd.mkv -c copy -f spdif -spdif_flags be -dtshd_rate 192000`
        // on a 2568-byte DTS-HD MA frame: header, start code, size, then the frame verbatim.
        let mut frame = vec![0x5a; 2568];
        frame[..4].copy_from_slice(&[0x7f, 0xfe, 0x80, 0x01]);
        let out = super::dtshd_burst(&frame, 1000).unwrap();
        assert_eq!(out.len(), 8192);
        assert_eq!(
            out[..20],
            [0xf8, 0x72, 0x4e, 0x1f, 0x02, 0x11, 0x0a, 0x18, 1, 0, 0, 0, 0, 0, 0, 0, 0xfe, 0xfe, 0x0a, 0x08]
        );
        assert_eq!(out[20..20 + 2568], frame[..]);
        assert!(out[20 + 2568..].iter().all(|b| *b == 0));
        // 2560 bytes: the same length code (rounded so the low nibble is 8), size 0x0a00.
        let out = super::dtshd_burst(&frame[..2560], 1000).unwrap();
        assert_eq!(out[6..8], [0x0a, 0x18]);
        assert_eq!(out[18..20], [0x0a, 0x00]);
    }

    #[test]
    fn oversized_frame_sends_core_and_unfit_core_fails() {
        let _g = globals();
        let frame = vec![0u8; 9000];
        let out = super::dtshd_burst(&frame, 2000).unwrap();
        assert_eq!(out[18..20], 2000u16.to_be_bytes());
        assert!(super::dtshd_burst(&frame, 8180).is_none());
    }

    #[test]
    fn swap_rebuilds_the_padded_chunk_and_points_the_write_at_it() {
        let _g = globals();
        super::set_on(true);
        let mut p = super::Packer::default();
        let frame = vec![7u8; 300];
        for t in 0..3 {
            p.frame(&frame, 0, t).unwrap();
        }
        let mut w = [0u32; 10];
        w[2] = super::CHUNK as u32;
        w[8] = 1;
        super::nj_es_swap(w.as_mut_ptr());
        // SAFETY: the test reads the buffer CURRENT keeps alive (32-bit address in word 0 on the
        // TV; on a 64-bit host the low half is not a pointer, so compare through CURRENT instead).
        let cur = super::CURRENT.lock().unwrap();
        let buf = cur.as_deref().unwrap();
        for i in 0..3 {
            let b = &buf[i * super::BURST..(i + 1) * super::BURST];
            assert_eq!(b[..4], [0xf8, 0x72, 0x4e, 0x1f]);
            assert_eq!(b[20..320], frame[..]);
            assert!(b[320..].iter().all(|x| *x == 0));
        }
        assert_eq!(super::SWAPPED.load(std::sync::atomic::Ordering::Relaxed), 1);
        drop(cur);
        super::set_on(false);
    }

    #[test]
    fn liblxa_reads_back_the_argument_it_passed_after_the_write() {
        // Words 0 (buffer) and 2 (size) point at our chunk only for the syscall; liblxa may read
        // the block afterwards (to advance its ring or count bytes), so it must find its own words.
        let _g = globals();
        super::set_on(true);
        let mut p = super::Packer::default();
        for t in 0..3 {
            p.frame(&[9u8; 80], 0, t).unwrap();
        }
        let mut w = [0u32; 10];
        w[0] = 0x1234_5678;
        w[2] = super::CHUNK as u32;
        w[8] = 1;
        super::nj_es_swap(w.as_mut_ptr());
        assert_eq!(super::SWAPPED.load(std::sync::atomic::Ordering::Relaxed), 1, "the write was swapped");
        super::nj_es_swap_done(w.as_mut_ptr(), 0);
        assert_eq!((w[0], w[2]), (0x1234_5678, super::CHUNK as u32));
        // A write left alone (nothing queued) is left alone after the call too.
        super::nj_es_swap(w.as_mut_ptr());
        super::nj_es_swap_done(w.as_mut_ptr(), 0);
        assert_eq!((w[0], w[2]), (0x1234_5678, super::CHUNK as u32));
        super::set_on(false);
    }

    #[test]
    fn a_refused_write_is_retried_with_the_same_chunk() {
        let _g = globals();
        super::set_on(true);
        let mut p = super::Packer::default();
        for t in 0..6 {
            p.frame(&[t as u8; 50], 0, t).unwrap();
        }
        let mut w = [0u32; 10];
        w[2] = super::CHUNK as u32;
        w[8] = 1;
        super::nj_es_swap(w.as_mut_ptr());
        let first = super::CURRENT.lock().unwrap().as_deref().unwrap()[20];
        super::nj_es_swap_done(w.as_mut_ptr(), -1);
        // liblxa retries with the size it was given; the queue must not advance.
        super::nj_es_swap(w.as_mut_ptr());
        assert_eq!(super::CURRENT.lock().unwrap().as_deref().unwrap()[20], first);
        super::nj_es_swap_done(w.as_mut_ptr(), 0);
        w[2] = super::CHUNK as u32;
        super::nj_es_swap(w.as_mut_ptr());
        assert_ne!(super::CURRENT.lock().unwrap().as_deref().unwrap()[20], first);
        super::set_on(false);
    }

    #[test]
    fn a_route_lg_never_writes_to_turns_the_carrier_off_once() {
        let _g = globals();
        super::UNAVAILABLE.store(false, std::sync::atomic::Ordering::Relaxed);
        super::set_on(true);
        assert!(!super::silent_route(10), "too early to judge");
        assert!(super::silent_route(super::SILENT_ROUTE_FRAMES));
        assert!(!super::on() && !super::available());
        assert!(!super::silent_route(500), "fires once");
        // A route that does write is never judged silent.
        super::UNAVAILABLE.store(false, std::sync::atomic::Ordering::Relaxed);
        super::set_on(true);
        super::SWAPPED.store(1, std::sync::atomic::Ordering::Relaxed);
        assert!(!super::silent_route(500));
        super::set_on(false);
    }

    #[test]
    fn packer_emits_one_chunk_per_three_frames() {
        let _g = globals();
        super::clear();
        let mut p = super::Packer::default();
        let frame = vec![1u8; 100];
        assert_eq!(p.frame(&frame, 0, 10), Ok(None));
        assert_eq!(p.frame(&frame, 0, 20), Ok(None));
        assert_eq!(p.frame(&frame, 0, 30), Ok(Some(10)));
        // Stretched or rounded source timestamps: the carrier stays contiguous...
        for t in [44_000_000, 55_000_000, 66_000_000] {
            p.frame(&frame, 0, t).ok();
        }
        assert_eq!(p.frame(&frame, 0, 0).ok(), Some(None));
        assert_eq!(p.frame(&frame, 0, 0).ok(), Some(None));
        assert_eq!(p.frame(&frame, 0, 0).ok(), Some(Some(32_000_010 + 32_000_000)));
        // ...until the source jumps (a seek or a discontinuity).
        for t in [900_000_000, 910_000_000] {
            p.frame(&frame, 0, t).ok();
        }
        assert_eq!(p.frame(&frame, 0, 920_000_000), Ok(Some(900_000_000)));
        let q = super::QUEUE.lock().unwrap();
        assert_eq!(q.back().map(|c| c.iter().map(|(_, v)| v.len()).sum::<usize>()), Some(3 * (20 + 100)));
    }
}
