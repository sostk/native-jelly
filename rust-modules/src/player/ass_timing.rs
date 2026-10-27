//! When a rendered subtitle frame stops being the right one: libass's own clock reading and the
//! validity window of a frame (`ass.rs` publishes one beside every frame).
//!
//! Pure: no native call, no thread. The window is computed from event timings alone, with
//! libass's activity rule `start <= now < start + duration` for both edges.

/// Ordinary 32-bit timestamps follow `string2timecode` of libass 0.17.5 (`ass.c`):
/// `sscanf("%d:%d:%d.%d")` then
/// `((h * 60 + m) * 60 + s) * 1000 + frac * 10`. The fraction is read as an integer count of
/// CENTISECONDS whatever its digit count (`0:00:01.5` is 1050 ms, `0:00:01.123` is 2230 ms), and a
/// timestamp that does not yield all four numbers (no fraction, junk) is 0. `%d` skips leading
/// white space and takes a sign; text after the last number is ignored.
pub fn libass_clock_ms(s: &str) -> i64 {
    let mut rest = s.as_bytes();
    let mut take = |sep: Option<u8>| -> Option<i64> {
        if let Some(sep) = sep {
            let (&first, tail) = rest.split_first()?;
            if first != sep {
                return None;
            }
            rest = tail;
        }
        while let Some((&b, tail)) = rest.split_first() {
            if matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
                rest = tail;
            } else {
                break;
            }
        }
        let negative = match rest.first() {
            Some(b'-') => {
                rest = &rest[1..];
                true
            }
            Some(b'+') => {
                rest = &rest[1..];
                false
            }
            _ => false,
        };
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let mut value: i64 = 0;
        for &b in &rest[..digits] {
            value = (value * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
        }
        rest = &rest[digits..];
        let value = if negative { -value } else { value };
        Some(value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)))
    };
    let parsed = (|| {
        let h = take(None)?;
        let m = take(Some(b':'))?;
        let s = take(Some(b':'))?;
        let cs = take(Some(b'.'))?;
        Some(((h * 60 + m) * 60 + s) * 1000 + cs * 10)
    })();
    parsed.unwrap_or(0)
}

/// The interval of playheads for which a rendered frame stays correct, and whether it moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Validity {
    /// Latest event boundary at or before the rendered time (`i64::MIN` if none).
    pub from_ms: i64,
    /// Earliest event boundary after it (`i64::MAX` if none).
    pub until_ms: i64,
    /// An event active at the rendered time changes with the clock itself (a `\t`, `\move`,
    /// `\fad`, karaoke or an Effect): the frame is valid for that one time only.
    pub animated: bool,
}

impl Validity {
    /// Error frames: they change only with the key, never with the clock.
    pub const ALWAYS: Self = Self { from_ms: i64::MIN, until_ms: i64::MAX, animated: false };

    pub fn contains(&self, now_ms: i64) -> bool {
        (self.from_ms..self.until_ms).contains(&now_ms)
    }
}

/// How a span knows whether it animates; the payload is scanned only for spans active at `now`.
#[derive(Clone, Copy)]
pub enum Motion<'a> {
    Known(bool),
    /// A Matroska ASS packet: `ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`.
    Packet(&'a [u8]),
}

#[derive(Clone, Copy)]
pub struct Span<'a> {
    pub start_ms: i64,
    pub end_ms: i64,
    pub motion: Motion<'a>,
}

/// The window of `spans` at `now_ms`. A span that is never active (`end <= start`) is no boundary:
/// libass would not draw it at any time.
pub fn validity<'a>(spans: impl IntoIterator<Item = Span<'a>>, now_ms: i64) -> Validity {
    let mut out = Validity { from_ms: i64::MIN, until_ms: i64::MAX, animated: false };
    for span in spans {
        if span.end_ms <= span.start_ms {
            continue;
        }
        for edge in [span.start_ms, span.end_ms] {
            if edge <= now_ms {
                out.from_ms = out.from_ms.max(edge);
            } else {
                out.until_ms = out.until_ms.min(edge);
            }
        }
        if !out.animated
            && (span.start_ms..span.end_ms).contains(&now_ms)
            && match span.motion {
                Motion::Known(animated) => animated,
                Motion::Packet(payload) => packet_animated(payload),
            }
        {
            out.animated = true;
        }
    }
    out
}

/// Tags that make an event's picture depend on the clock: `\t(`, `\move(`, `\fad(`, `\fade(`
/// and the karaoke family `\k` `\K` `\kf` `\ko`. Case-insensitive and conservative: a false
/// positive costs a render per tick only while the event is active.
pub fn text_animated(text: &[u8]) -> bool {
    text.iter().enumerate().any(|(i, &b)| {
        if b != b'\\' {
            return false;
        }
        let tail = &text[i + 1..];
        let tail = &tail[tail.iter().take_while(|b| b.is_ascii_whitespace()).count()..];
        let starts = |tag: &[u8]| {
            tail.len() >= tag.len() && tail[..tag.len()].eq_ignore_ascii_case(tag)
        };
        starts(b"t") || starts(b"move") || starts(b"fad") || starts(b"k")
    })
}

/// A non-empty Effect field (`Scroll up`, `Banner`...) animates the event.
pub fn effect_animated(effect: &[u8]) -> bool {
    effect.iter().any(|b| !b.is_ascii_whitespace())
}

fn packet_animated(payload: &[u8]) -> bool {
    let mut fields = payload.splitn(9, |&b| b == b',');
    let effect = fields.nth(7);
    effect.is_some_and(effect_animated) || text_animated(payload)
}

/// Timing-only index of the complete sidecar already accepted by the native facade. Native
/// script/event/font limits stay unchanged; this does not split or rewrite the authored script.
pub struct ScriptTimings {
    lines: Vec<(i64, i64, bool)>,
}

impl ScriptTimings {
    pub fn parse(raw: &[u8]) -> Self {
        let unknown = || Self { lines: vec![(i64::MIN, i64::MAX, true)] };
        let Ok(raw) = std::str::from_utf8(raw) else { return unknown() };
        let mut events = false;
        // Without an explicit Format the native reader chooses an ASS/SSA fallback. Keep
        // rendering per tick rather than guess the field order or suppress an unfamiliar cue.
        let mut format: Option<Vec<String>> = None;
        let mut lines = Vec::new();
        for line in raw.split(['\r', '\n']) {
            let line = line.trim_start_matches('\u{feff}').trim_start();
            if line.starts_with('[') {
                if line.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("[Events]")) {
                    events = true;
                } else if ["[Script Info]", "[V4 Styles]", "[V4+ Styles]", "[Fonts]"].iter()
                    .any(|section| line.get(..section.len()).is_some_and(|p| p.eq_ignore_ascii_case(section))) {
                    events = false;
                }
                // Unknown section names do not change the pinned native parser's state.
                continue;
            }
            if !events { continue; }
            if let Some(body) = line.strip_prefix("Format:") {
                format = Some(body.split(',').map(|f| f.trim().to_ascii_lowercase()).collect());
            } else if let Some(body) = line.strip_prefix("Dialogue:") {
                let Some(names) = &format else { return unknown() };
                if names.last().map(String::as_str) != Some("text") { return unknown(); }
                // libass stops at the first Text, overwrites repeated Effect fields and treats
                // Duration as an alias of End. Ambiguous formats keep the per-tick path.
                if names.iter().filter(|name| name.as_str() == "text").count() != 1
                    || names.iter().filter(|name| name.as_str() == "effect").count() > 1
                    || names.iter().any(|name| name == "duration")
                { return unknown(); }
                let position = |name| {
                    let mut found = names.iter().enumerate().filter(|(_, n)| n.as_str() == name);
                    let (index, _) = found.next()?;
                    found.next().is_none().then_some(index)
                };
                let (Some(start), Some(end)) = (position("start"), position("end")) else { return unknown() };
                let fields: Vec<_> = body.splitn(names.len(), ',').collect();
                if fields.len() != names.len() { return unknown(); }
                let animated = text_animated(fields.last().unwrap().as_bytes())
                    || position("effect").is_some_and(|i| effect_animated(fields[i].as_bytes()));
                lines.push((libass_clock_ms(fields[start]), libass_clock_ms(fields[end]), animated));
            }
        }
        if lines.is_empty() { return unknown(); }
        Self { lines }
    }

    pub fn spans(&self) -> impl Iterator<Item = Span<'_>> {
        self.lines.iter().map(|&(start_ms, end_ms, animated)| Span {
            start_ms, end_ms, motion: Motion::Known(animated),
        })
    }
}

#[cfg(test)]
mod tests;
