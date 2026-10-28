//! Optional seek thumbnails: one pending tile, one shown tile, no playback ownership.
//! Adapted from PlxNative #767; Jellyfin sheet addressing and cropping live below this view.
use crate::jf::trickplay::{self, LayoutId, Spec};
use crate::ui::{tex, theme, Painter, Rect};

const REQUEST_W: i32 = 320;
const REQUEST_H: i32 = 180;
/// Two bounded network reads can take three seconds each. Deferred jobs also get a finite life.
const PENDING_MAX_MS: u32 = 7_000;
const STALE_MAX_MS: u32 = 400;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    Ready,
    Waiting,
    Failed,
}
#[derive(Default)]
struct Memo {
    pending: Option<(i64, u32)>,
    shown: Option<i64>,
    absent: bool,
    stale_since: Option<u32>,
    missed: Option<i64>,
}
impl Memo {
    fn release(&mut self) -> Vec<i64> {
        let mut retired = Vec::new();
        if let Some((ms, _)) = self.pending.take() {
            retired.push(ms);
        }
        if let Some(ms) = self.shown.take() {
            if !retired.contains(&ms) {
                retired.push(ms);
            }
        }
        self.stale_since = None;
        self.missed = None;
        retired
    }
    fn step(
        &mut self,
        target: i64,
        now: u32,
        spec: Option<Spec>,
        mut look: impl FnMut(i64) -> Look,
        mut forget: impl FnMut(i64),
    ) -> Option<i64> {
        if self.absent {
            return None;
        }
        if let Some((ms, at)) = self.pending {
            match look(ms) {
                Look::Ready => {
                    if let Some(old) = self.shown.replace(ms) {
                        if old != ms {
                            forget(old);
                        }
                    }
                    self.pending = None;
                    // Movement can outpace the worker. Age the newly available still from
                    // this completion, not from an older image (or an empty first request).
                    self.stale_since = None;
                }
                Look::Failed => {
                    self.pending = None;
                    self.missed = Some(ms);
                    forget(ms);
                    if self.shown.is_none() {
                        self.absent = true;
                        return None;
                    }
                }
                Look::Waiting if now.wrapping_sub(at) >= PENDING_MAX_MS => {
                    self.pending = None;
                    forget(ms);
                }
                Look::Waiting => {}
            }
        }
        let same = |ms| spec.map_or(ms == target, |s| s.tile(ms) == s.tile(target));
        if self.pending.is_none() && !self.shown.is_some_and(same) && !self.missed.is_some_and(same)
        {
            match look(target) {
                Look::Ready => {
                    if let Some(old) = self.shown.replace(target) {
                        if old != target {
                            forget(old);
                        }
                    }
                }
                Look::Failed => {
                    self.missed = Some(target);
                    forget(target);
                    if self.shown.is_none() {
                        self.absent = true;
                        return None;
                    }
                }
                Look::Waiting => self.pending = Some((target, now)),
            }
        }
        if self.shown.is_some_and(same) {
            self.stale_since = None;
            return self.shown;
        }
        let at = *self.stale_since.get_or_insert(now);
        (now.wrapping_sub(at) < STALE_MAX_MS)
            .then_some(self.shown)
            .flatten()
    }
}

/// Resource memo only: it cannot seek, report progress, or alter the screen's canonical state.
#[derive(Default)]
pub(crate) struct ScrubPreview {
    owner: Option<LayoutId>,
    memo: Memo,
}
impl ScrubPreview {
    pub(crate) fn release(&mut self) {
        if let Some(id) = &self.owner {
            for ms in self.memo.release() {
                forget(id, ms);
            }
        }
    }
    pub(crate) fn draw(
        &mut self,
        ps: &crate::route::PlaybackSession,
        pos_ns: i64,
        dur_ns: i64,
        now: u32,
        bar: Rect,
        chrome_top: f32,
    ) {
        let Some((sid, owner)) = crate::route::scrub_preview_owner(ps) else {
            self.release();
            return;
        };
        let Some(client) = crate::catalog::client_for(sid) else {
            self.release();
            return;
        };
        let id = LayoutId {
            srv: sid.raw(),
            token: client.token_gen(),
            cache: nj_platform::imgcache::generation(),
            owner,
        };
        if self.owner.as_ref() != Some(&id) {
            self.release();
            self.memo = Memo::default();
            self.owner = Some(id.clone());
        }
        let known = trickplay::known(&id);
        if known == Some(None) {
            self.release();
            return;
        }
        let spec = known.flatten();
        let ms = pos_ns.max(0) / 1_000_000;
        let target = spec.map_or(ms, |s| {
            let n = (ms as u64 / u64::from(s.interval)).min(u64::from(s.thumbnail_count - 1));
            (n * u64::from(s.interval)) as i64
        });
        // An old still can lose residency while hidden. A read-only peek prevents its draw
        // from silently starting a second request beside the tracked pending one.
        if let Some(ms) = self.memo.shown {
            if tex::peek_wh_on(id.srv, &id.owner.key(ms), REQUEST_W, REQUEST_H, false).0 == 0 {
                self.memo.shown = None;
                forget(&id, ms);
            }
        }
        let shown = self.memo.step(
            target,
            now,
            spec,
            |ms| {
                let path = id.owner.key(ms);
                if tex::resolve_on(id.srv, &path, REQUEST_W, REQUEST_H, false) != 0 {
                    Look::Ready
                } else if tex::failed_on(id.srv, &path, REQUEST_W, REQUEST_H, false) {
                    Look::Failed
                } else {
                    Look::Waiting
                }
            },
            |ms| forget(&id, ms),
        );
        let Some(ms) = shown else {
            return;
        };
        let (texture, w, h) =
            tex::peek_wh_on(id.srv, &id.owner.key(ms), REQUEST_W, REQUEST_H, false);
        if texture == 0 {
            return;
        }
        let fraction = if dur_ns > 0 {
            (pos_ns as f64 / dur_ns as f64).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let rect = card_rect(bar.x + bar.w * fraction, bar, chrome_top, w, h);
        let p = Painter::root();
        p.shadow(rect, 12.0, 24.0, 8.0, theme::scrim_black(0.5));
        let fitted = fit(rect, w, h);
        if fitted != rect {
            p.rrect(rect, 12.0, 0.0, theme::scrim_black(1.0));
        }
        p.tex(
            texture,
            fitted,
            if fitted == rect { 12.0 } else { 0.0 },
            theme::TINT_WHITE,
        );
        p.rring(rect, 12.0, 1.5, theme::CARD_SHEEN);
    }
}
fn forget(id: &LayoutId, ms: i64) {
    tex::forget_on(id.srv, &id.owner.key(ms), REQUEST_W, REQUEST_H, false);
}

/// Keep the preview above both the title and control row, within the playbar's safe margins.
fn card_rect(knob: f32, bar: Rect, chrome_top: f32, w: f32, h: f32) -> Rect {
    let height = 180.0f32.min((chrome_top - theme::space::MD).max(0.0));
    let aspect = if w > 0.0 && h > 0.0 {
        w / h
    } else {
        16.0 / 9.0
    };
    let width = (height * aspect).clamp(height, height * 2.4).min(bar.w);
    Rect::new(
        (knob - width * 0.5).clamp(bar.x, (bar.x + bar.w - width).max(bar.x)),
        chrome_top - theme::space::MD - height,
        width,
        height,
    )
}
fn fit(r: Rect, w: f32, h: f32) -> Rect {
    if w <= 0.0 || h <= 0.0 {
        return r;
    }
    let k = (r.w / w).min(r.h / h);
    let (w, h) = (w * k, h * k);
    Rect::new(r.x + (r.w - w) * 0.5, r.y + (r.h - h) * 0.5, w, h)
}
impl Drop for ScrubPreview {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    fn spec() -> Spec {
        Spec {
            width: 320,
            height: 180,
            tile_width: 10,
            tile_height: 10,
            thumbnail_count: 100,
            interval: 10_000,
        }
    }
    #[test]
    fn scrub_preview_keeps_one_request_and_coalesces_to_the_latest_target() {
        let mut m = Memo::default();
        let probes = RefCell::new(Vec::new());
        let retired = RefCell::new(Vec::new());
        for ms in [0, 10_000, 20_000, 30_000] {
            assert_eq!(
                m.step(
                    ms,
                    10,
                    None,
                    |v| {
                        probes.borrow_mut().push(v);
                        Look::Waiting
                    },
                    |v| retired.borrow_mut().push(v)
                ),
                None
            );
            assert_eq!(m.pending, Some((0, 10)));
        }
        assert!(probes.borrow().iter().all(|v| *v == 0));
        assert_eq!(
            m.step(
                30_000,
                20,
                Some(spec()),
                |v| if v == 0 { Look::Ready } else { Look::Waiting },
                |v| retired.borrow_mut().push(v)
            ),
            Some(0)
        );
        assert_eq!(m.pending, Some((30_000, 20)));
        assert_eq!(
            m.step(
                30_000,
                30,
                Some(spec()),
                |_| Look::Ready,
                |v| retired.borrow_mut().push(v)
            ),
            Some(30_000)
        );
        assert_eq!(&*retired.borrow(), &[0]);
    }
    #[test]
    fn scrub_preview_fresh_completions_remain_visible_during_continuous_movement() {
        let mut m = Memo::default();
        m.step(0, 0, Some(spec()), |_| Look::Waiting, |_| {});
        // The first sheet arrives after the grace period, while the destination keeps moving.
        assert_eq!(
            m.step(
                20_000,
                500,
                Some(spec()),
                |ms| if ms == 0 { Look::Ready } else { Look::Waiting },
                |_| {}
            ),
            Some(0)
        );
        assert_eq!(m.pending, Some((20_000, 500)));
        // A fresh completion gets its own finite grace period, even if it trails the cursor.
        assert_eq!(
            m.step(
                40_000,
                1_000,
                Some(spec()),
                |ms| if ms == 20_000 {
                    Look::Ready
                } else {
                    Look::Waiting
                },
                |_| {}
            ),
            Some(20_000)
        );
        assert_eq!(m.pending, Some((40_000, 1_000)));
        assert_eq!(
            m.step(60_000, 1_401, Some(spec()), |_| Look::Waiting, |_| {}),
            None,
            "an unchanged stale still must still expire"
        );
    }
    #[test]
    fn scrub_preview_uses_manifest_intervals_and_hides_stale_scenes() {
        let mut m = Memo::default();
        assert_eq!(
            m.step(123, 0, Some(spec()), |_| Look::Ready, |_| {}),
            Some(123)
        );
        assert_eq!(
            m.step(
                9_999,
                100,
                Some(spec()),
                |_| panic!("same thumbnail needs no new request"),
                |_| {}
            ),
            Some(123)
        );
        assert_eq!(
            m.step(90_000, 200, Some(spec()), |_| Look::Waiting, |_| {}),
            Some(123)
        );
        assert_eq!(
            m.step(90_000, 601, Some(spec()), |_| Look::Waiting, |_| {}),
            None
        );
        assert_eq!(
            m.step(90_000, 610, Some(spec()), |_| Look::Ready, |_| {}),
            Some(90_000)
        );
    }
    #[test]
    fn scrub_preview_missing_images_stop_requests_and_release_clears_stills() {
        let mut m = Memo::default();
        assert_eq!(m.step(0, 0, None, |_| Look::Failed, |_| {}), None);
        assert_eq!(
            m.step(
                10_000,
                1,
                None,
                |_| panic!("absent playback must stay quiet"),
                |_| {}
            ),
            None
        );
        let mut m = Memo::default();
        m.step(0, 0, None, |_| Look::Ready, |_| {});
        m.step(10_000, 10, None, |_| Look::Waiting, |_| {});
        let mut retired = m.release();
        retired.sort();
        assert_eq!(retired, vec![0, 10_000]);
        assert_eq!(m.shown, None);
        assert_eq!(m.pending, None);
    }
    #[test]
    fn scrub_preview_timeout_retires_the_old_request_even_across_tick_wrap() {
        let mut m = Memo::default();
        let start = u32::MAX - 100;
        m.step(0, start, None, |_| Look::Waiting, |_| {});
        let mut retired = Vec::new();
        m.step(
            30_000,
            start.wrapping_add(PENDING_MAX_MS),
            None,
            |_| Look::Waiting,
            |v| retired.push(v),
        );
        assert_eq!(retired, vec![0]);
        assert_eq!(
            m.pending,
            Some((30_000, start.wrapping_add(PENDING_MAX_MS)))
        );
    }
    #[test]
    fn scrub_preview_card_clamps_at_both_ends_and_preserves_aspect() {
        let bar = Rect::new(96.0, 882.0, 1728.0, 8.0);
        for knob in [-10.0, 96.0, 960.0, 1824.0, 2000.0] {
            for (w, h) in [(320.0, 180.0), (80.0, 180.0), (600.0, 180.0)] {
                let r = card_rect(knob, bar, 750.0, w, h);
                let image = fit(r, w, h);
                assert!(r.x >= bar.x && r.x + r.w <= bar.x + bar.w);
                assert!(r.y + r.h <= 750.0);
                assert!((image.w / image.h - w / h).abs() < 0.001);
            }
        }
    }
    #[test]
    fn scrub_preview_a_missing_later_tile_does_not_retry_on_every_frame() {
        let mut m = Memo::default();
        m.step(0, 0, Some(spec()), |_| Look::Ready, |_| {});
        m.step(20_000, 1, Some(spec()), |_| Look::Failed, |_| {});
        assert_eq!(
            m.step(
                20_000,
                500,
                Some(spec()),
                |_| panic!("settled missing tile must stay quiet"),
                |_| {}
            ),
            None
        );
        assert_eq!(
            m.step(30_000, 600, Some(spec()), |_| Look::Ready, |_| {}),
            Some(30_000)
        );
    }
}
