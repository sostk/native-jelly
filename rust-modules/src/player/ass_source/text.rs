//! Bounded embedded text tracks, converted once on the demux thread and published
//! through the existing immutable ASS store. Styling replaces only a small header.
use super::*;
use crate::player::{
    text_ass::{self, Align, ClipCue, TextDialect},
    text_style::TextStyle,
};

type RawKey = (i64, i64, u64);
pub(super) struct TextTrack {
    dialect: TextDialect,
    raw: BTreeMap<RawKey, (u32, Align, Arc<str>)>,
    next: u32,
}

pub(crate) fn begin_with_text(
    media_key: &str,
    mut headers: Vec<(i32, Vec<u8>)>,
    tracks: Vec<(i32, TextDialect)>,
    fonts: Vec<Font>,
) -> u64 {
    let style = TextStyle::current();
    let header = text_ass::header(&style);
    headers.extend(
        tracks
            .iter()
            .map(|(track, _)| (*track, header.as_bytes().to_vec())),
    );
    let mut st = store();
    let same = st.media_key.as_deref() == Some(media_key)
        && headers.len() == st.tracks.len()
        && headers.iter().all(|(track, bytes)| {
            st.tracks.get(track).is_some_and(|source|
            matches!(&source.content, Content::Embedded {header, ..} if header.as_ref() == bytes))
        });
    let mut old = if same {
        std::mem::take(&mut st.texts)
    } else {
        BTreeMap::new()
    };
    let generation = st.begin(media_key, headers, fonts);
    for (track, dialect) in tracks {
        if let Some(source) = st.tracks.get_mut(&track) {
            Arc::make_mut(source).origin = ass::Origin::Text(style);
            let text = old
                .remove(&track)
                .filter(|t| t.dialect == dialect)
                .unwrap_or(TextTrack {
                    dialect,
                    raw: BTreeMap::new(),
                    next: 0,
                });
            st.texts.insert(track, text);
        }
    }
    generation
}

impl Store {
    pub(super) fn restyle_text(&mut self, track: i32) {
        if !self.texts.contains_key(&track) {
            return;
        }
        let style = TextStyle::current();
        let Some(old) = self.tracks.get(&track) else {
            return;
        };
        if old.origin == ass::Origin::Text(style) {
            return;
        }
        let Content::Embedded { events, fonts, .. } = &old.content else {
            return;
        };
        self.tracks.insert(
            track,
            Arc::new(Source {
                id: old.id,
                revision: old.revision.wrapping_add(1),
                origin: ass::Origin::Text(style),
                content: Content::Embedded {
                    header: text_ass::header(&style).into_bytes().into(),
                    events: events.clone(),
                    fonts: fonts.clone(),
                },
            }),
        );
    }
}

pub(crate) fn push_text(
    generation: u64,
    track: i32,
    start_ns: i64,
    end_ns: i64,
    raw: &[u8],
    decoded: &str,
) {
    push_text_with(
        generation, track, start_ns, end_ns, raw, decoded, build_view,
    );
}

struct ViewInput {
    cues: Vec<ClipCue>,
    raw: Vec<(u32, Align, Arc<str>)>,
    budget: usize,
    now: i64,
}

fn push_text_with(
    generation: u64,
    track: i32,
    start_ns: i64,
    end_ns: i64,
    raw: &[u8],
    decoded: &str,
    build: impl FnOnce(ViewInput) -> Arc<[Event]>,
) {
    if raw.len() > MAX_PACKET_BYTES {
        return;
    }
    let start = start_ns / 1_000_000;
    let end = end_ns / 1_000_000;
    if end <= start {
        return;
    }
    let hash = raw.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    });
    let key = (start, end, hash);
    let (dialect, budget) = {
        let st = store();
        if generation != st.generation {
            return;
        }
        let Some(text) = st.texts.get(&track) else {
            return;
        };
        if text.raw.contains_key(&key) {
            return;
        }
        (
            text.dialect,
            (MAX_EVENT_BYTES / 2).min(8 * 1024 * 1024 / st.tracks.len().max(1)),
        )
    };
    let Some(converted) = text_ass::convert(decoded.trim_matches('\0'), dialect) else {
        return;
    };
    if converted.body.len() + 128 > MAX_PACKET_BYTES || converted.body.len() > budget {
        return;
    }
    let shown_chars = (track == crate::player::desired_sub_idx())
        .then(|| text_ass::visible_chars(&converted.body));
    let align = converted.align;
    let body: Arc<str> = converted.body.into();
    let mut st = store();
    if generation != st.generation {
        return;
    }
    let Some(text) = st.texts.get_mut(&track) else {
        return;
    };
    if text.raw.contains_key(&key) {
        return;
    }
    let floor = crate::player::subtitle_floor_ns() / 1_000_000;
    text.raw.retain(|(_, end, _), _| *end >= floor);
    text.raw.insert(key, (text.next, align, body));
    text.next = text.next.wrapping_add(1);
    let mut bytes: usize = text.raw.values().map(|(_, _, body)| body.len()).sum();
    while text.raw.len() > 512 || bytes > budget {
        let key = *text.raw.keys().min_by_key(|(_, end, _)| *end).unwrap();
        bytes -= text.raw.remove(&key).unwrap().2.len();
    }
    let cues: Vec<_> = text
        .raw
        .iter()
        .map(|(&(start, end, _), &(seq, align, _))| ClipCue {
            start,
            end,
            seq,
            class: text_ass::AlignClass::of_align(align),
        })
        .collect();
    let raw: Vec<_> = text.raw.values().cloned().collect();
    let now = crate::player::subtitle_clock_ns(crate::player::playpos_ns()) / 1_000_000;
    // The main thread reads this store. Keep only descriptor/Arc work under its
    // lock; conversion, hashing and expanded payload copies belong to the producer.
    drop(st);
    if let Some(chars) = shown_chars {
        crate::player::log(&format!("sub cue [{}..{}ms] len={chars}", start, end));
    }
    let events = build(ViewInput {
        cues,
        raw,
        budget,
        now,
    });
    let mut st = store();
    if generation != st.generation {
        return;
    }
    let Some(old) = st.tracks.get(&track).cloned() else {
        return;
    };
    let Content::Embedded { header, fonts, .. } = &old.content else {
        return;
    };
    st.tracks.insert(
        track,
        Arc::new(Source {
            id: old.id,
            revision: old.revision.wrapping_add(1),
            origin: old.origin,
            content: Content::Embedded {
                header: header.clone(),
                fonts: fonts.clone(),
                events,
            },
        }),
    );
}

fn build_view(input: ViewInput) -> Arc<[Event]> {
    let ViewInput {
        cues,
        raw,
        budget,
        now,
    } = input;
    let mut spans = text_ass::newest_wins_spans(&cues);
    // An interrupted long cue can repeat its body in several ASS events. Bound
    // the expanded view too, favouring the subtitle clock before distant cues.
    spans.sort_by_key(|&(_, start, end)| {
        (
            !(start <= now && now < end),
            start.saturating_sub(now).unsigned_abs(),
            start,
        )
    });
    let mut view_bytes = 0;
    spans.retain(|&(i, _, _)| {
        let bytes = raw[i].2.len() + 128;
        if bytes > budget - view_bytes {
            false
        } else {
            view_bytes += bytes;
            true
        }
    });
    spans.sort_by_key(|&(i, start, _)| (start, raw[i].0, i));
    spans
        .into_iter()
        .enumerate()
        .map(|(readorder, (i, start, end))| Event {
            start_ms: start,
            duration_ms: end - start,
            payload: text_ass::event_payload(readorder as u32, raw[i].1, &raw[i].2)
                .into_bytes()
                .into(),
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    const S: i64 = 1_000_000_000;
    struct Restore {
        size: crate::catalog::session::SubtitleSize,
        position: crate::catalog::session::SubtitlePosition,
        clock: i64,
        offset: i64,
    }
    impl Restore {
        fn new() -> Self {
            Self {
                size: crate::route::subtitle_size(),
                position: crate::route::subtitle_position(),
                clock: crate::player::playpos_ns(),
                offset: crate::player::subtitle_offset_ms(),
            }
        }
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::route::restore_subtitle_size(self.size);
            crate::route::restore_subtitle_position(self.position);
            crate::player::SHARED
                .playpos_ns
                .store(self.clock, std::sync::atomic::Ordering::Relaxed);
            crate::player::restore_subtitle_offset(self.offset);
            super::super::reset();
        }
    }
    fn source() -> Arc<Source> {
        selected(0).unwrap()
    }
    fn events(source: &Source) -> Arc<[Event]> {
        let Content::Embedded { events, .. } = &source.content else {
            panic!()
        };
        events.clone()
    }
    #[test]
    fn a_refresh_does_not_hold_the_store_while_building_its_view() {
        let _g = nj_base::testlock::serial();
        let _restore = Restore::new();
        let generation = begin_with_text("view-lock", vec![], vec![(0, TextDialect::Srt)], vec![]);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            push_text_with(generation, 0, S, 2 * S, b"cue", "cue", |input| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                build_view(input)
            })
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let (read_tx, read_rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let _ = source();
            read_tx.send(()).unwrap();
        });
        let responsive = read_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .is_ok();
        release_tx.send(()).unwrap();
        producer.join().unwrap();
        reader.join().unwrap();
        super::super::reset();
        assert!(
            responsive,
            "main-thread source lookup must finish while view work is held"
        );
    }

    #[test]
    fn a_late_view_keeps_current_style_and_cannot_replace_a_new_item() {
        let _g = nj_base::testlock::serial();
        let _restore = Restore::new();
        for new_item in [false, true] {
            crate::route::restore_subtitle_size(crate::catalog::session::SubtitleSize::Medium);
            let generation =
                begin_with_text("view-before", vec![], vec![(0, TextDialect::Srt)], vec![]);
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let producer = std::thread::spawn(move || {
                push_text_with(generation, 0, S, 2 * S, b"cue", "cue", |input| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    build_view(input)
                })
            });
            started_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            // Preference writes belong to the test thread holding the serial guard.
            if !new_item {
                crate::route::restore_subtitle_size(crate::catalog::session::SubtitleSize::Large);
            }
            let (changed_tx, changed_rx) = std::sync::mpsc::channel();
            let changer = std::thread::spawn(move || {
                if new_item {
                    begin_with_text("view-after", vec![], vec![(0, TextDialect::Srt)], vec![]);
                }
                let _ = source();
                changed_tx.send(()).unwrap();
            });
            let responsive = changed_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_ok();
            release_tx.send(()).unwrap();
            producer.join().unwrap();
            changer.join().unwrap();
            assert!(
                responsive,
                "selection/restyle must not wait for a view build"
            );
            let current = source();
            if new_item {
                assert!(events(&current).is_empty(), "late old-media view rejected");
            } else {
                assert!(!events(&current).is_empty());
                let style = TextStyle::current();
                assert_eq!(current.origin, ass::Origin::Text(style));
                let Content::Embedded { header, .. } = &current.content else {
                    panic!()
                };
                assert_eq!(header.as_ref(), text_ass::header(&style).as_bytes());
            }
        }
    }

    #[test]
    fn an_older_cue_resumes_and_rereads_seeks_and_reopens_keep_one_copy() {
        let _g = nj_base::testlock::serial();
        let _restore = Restore::new();
        crate::player::SHARED
            .playpos_ns
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let begin = || begin_with_text("text-fixture", vec![], vec![(0, TextDialect::Srt)], vec![]);
        let generation = begin();
        push_text(generation, 0, 0, 10 * S, b"long", "long");
        push_text(generation, 0, 2 * S, 3 * S, b"short", "short");
        let first = source();
        let view = events(&first);
        assert_eq!(
            view.iter()
                .map(|e| (e.start_ms, e.duration_ms))
                .collect::<Vec<_>>(),
            [(0, 2000), (2000, 1000), (3000, 7000)]
        );
        assert!(std::str::from_utf8(&view[2].payload)
            .unwrap()
            .ends_with(",,long"));
        push_text(
            generation,
            0,
            2 * S,
            3 * S,
            b"short",
            "a later provisional charset guess",
        );
        assert_eq!(
            source().revision,
            first.revision,
            "raw packet identity deduplicates a reread"
        );
        super::super::seek(generation);
        assert_ne!(source().id, first.id);
        assert_eq!(events(&source()), view);
        let reopened = begin();
        assert_eq!(events(&source()), view);
        push_text(reopened, 0, 2 * S, 3 * S, b"short", "short");
        assert_eq!(
            events(&source()),
            view,
            "demux reopen preserves raw dedup and the rendered view"
        );
        super::super::reset();
    }
    #[test]
    fn overlap_expansion_is_bounded_and_keeps_the_cue_at_the_subtitle_clock() {
        let _g = nj_base::testlock::serial();
        let _restore = Restore::new();
        let generation = begin_with_text(
            "bounded-text",
            vec![],
            (0..64).map(|track| (track, TextDialect::Srt)).collect(),
            vec![],
        );
        crate::player::SHARED
            .playpos_ns
            .store(25 * S, std::sync::atomic::Ordering::Relaxed);
        crate::player::restore_subtitle_offset(0);
        let long = "é".repeat(16_384);
        push_text(generation, 0, 0, 60 * S, long.as_bytes(), &long);
        for n in 1..20 {
            push_text(generation, 0, n * S, n * S + S / 2, b"short", "short");
        }
        let view = events(&source());
        assert!(view.iter().map(|e| e.payload.len()).sum::<usize>() <= 8 * 1024 * 1024 / 64);
        assert!(view.iter().any(|e| e.start_ms <= 25_000
            && 25_000 < e.start_ms + e.duration_ms
            && e.payload.ends_with(long.as_bytes())));
        crate::player::SHARED
            .playpos_ns
            .store(0, std::sync::atomic::Ordering::Relaxed);
        super::super::reset();
    }

    #[test]
    fn new_media_rejects_old_text_producers_and_style_changes_share_the_events() {
        let _g = nj_base::testlock::serial();
        let _restore = Restore::new();
        let old_size = crate::route::subtitle_size();
        crate::route::restore_subtitle_size(crate::catalog::session::SubtitleSize::Medium);
        let begin = |key| begin_with_text(key, vec![], vec![(0, TextDialect::WebVtt)], vec![]);
        let old = begin("first");
        let current = begin("second");
        push_text(old, 0, S, 2 * S, b"stale", "stale");
        assert!(events(&source()).is_empty());
        push_text(current, 0, S, 2 * S, b"a &amp; b", "a &amp; b");
        let first = source();
        assert!(std::str::from_utf8(&events(&first)[0].payload)
            .unwrap()
            .ends_with(",,a & b"));
        crate::route::restore_subtitle_size(crate::catalog::session::SubtitleSize::Large);
        let restyled = source();
        assert_eq!(first.id, restyled.id);
        assert!(restyled.revision > first.revision);
        assert!(Arc::ptr_eq(&events(&first), &events(&restyled)));
        crate::route::restore_subtitle_size(old_size);
        super::super::reset();
    }
}
