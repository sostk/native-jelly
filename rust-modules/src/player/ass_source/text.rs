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
    if raw.len() > MAX_PACKET_BYTES {
        return;
    }
    let dialect = {
        let st = store();
        if generation != st.generation {
            return;
        }
        let Some(text) = st.texts.get(&track) else {
            return;
        };
        text.dialect
    };
    let Some(converted) = text_ass::convert(decoded.trim_matches('\0'), dialect) else {
        return;
    };
    let mut st = store();
    if generation != st.generation {
        return;
    }
    let budget = (MAX_EVENT_BYTES / 2).min(8 * 1024 * 1024 / st.tracks.len().max(1));
    let Some(text) = st.texts.get_mut(&track) else {
        return;
    };
    let start = start_ns / 1_000_000;
    let end = end_ns / 1_000_000;
    if end <= start {
        return;
    }
    let hash = raw.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    });
    let key = (start, end, hash);
    if text.raw.contains_key(&key) {
        return;
    }
    if converted.body.len() + 128 > MAX_PACKET_BYTES || converted.body.len() > budget {
        return;
    }
    let floor = crate::player::subtitle_floor_ns() / 1_000_000;
    text.raw.retain(|(_, end, _), _| *end >= floor);
    if track == crate::player::desired_sub_idx() {
        crate::player::log(&format!(
            "sub cue [{}..{}ms] len={}",
            start,
            end,
            text_ass::visible_chars(&converted.body)
        ));
    }
    text.raw
        .insert(key, (text.next, converted.align, converted.body.into()));
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
    let raw: Vec<_> = text.raw.values().collect();
    let mut spans = text_ass::newest_wins_spans(&cues);
    // An interrupted long cue can repeat its body in several ASS events. Bound
    // the expanded view too, favouring the subtitle clock before distant cues.
    let now = crate::player::subtitle_clock_ns(crate::player::playpos_ns()) / 1_000_000;
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
    let events: Arc<[Event]> = spans
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
        .into();
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

#[cfg(test)]
mod tests {
    use super::*;
    const S: i64 = 1_000_000_000;
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
    fn an_older_cue_resumes_and_rereads_seeks_and_reopens_keep_one_copy() {
        let _g = nj_base::testlock::serial();
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
