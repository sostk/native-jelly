//! Player-owned ASS texture cache. Native parsing/rasterization is the worker's job;
//! the frame thread submits its clock and uploads only a changed completed image.
//! Authored placement is retained even while the transport HUD is visible.
use crate::player::{ass, ass_source, sidecar, text_ass, text_style};
use crate::ui::{
    consts::{SCR_H, SCR_W},
    Painter, Rect,
};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct AssSubtitles {
    frame: Option<Arc<ass::Frame>>,
    source: u64,
    clock: ass_source::Clock,
    textures: Vec<CachedTexture>,
    uploaded: u64,
    spans: Vec<(i32,i32)>,
    dys: Vec<f32>,
    viewport: crate::player::video_geometry::Viewport,
}

impl AssSubtitles {
    pub(crate) fn update(&mut self, ps: &crate::route::PlaybackSession, now: u32) {
        let renderable = crate::route::client_renders_subtitle(ps);
        if renderable && crate::player::loading(ps) {
            if self.source != 0 {
                ass::suspend();
            }
            self.source = 0;
            self.frame = None;
            return;
        }
        let source = if !renderable {
            None
        } else {
            sidecar::ass_source(false)
                .or_else(|| ass_source::selected(crate::player::desired_sub_idx()))
        };
        let Some(source) = source else {
            // A preceding busy interval may already have cleared self.source while keeping
            // native resources. A real Off/no-source transition must release those too.
            ass::clear();
            self.source = 0;
            self.frame = None;
            return;
        };
        if self.source != source.id {
            self.source = source.id;
            self.clock = ass_source::Clock::default();
            self.frame = None;
        }
        let clock = self.clock.sample(
            crate::player::playpos_ns(),
            now,
            crate::player::is_playing(ps),
        );
        let clock = clock.saturating_sub(crate::player::subtitle_offset_ms());
        let (coded_w, coded_h, viewport) = if matches!(source.origin, ass::Origin::Text(_)) {
            (SCR_W as i32, SCR_H as i32, crate::player::video_geometry::Viewport {
                x:0, y:0, width:SCR_W as i32, height:SCR_H as i32,
            })
        } else {
            let (w,h) = crate::player::video_raster();
            (w,h,crate::player::video_viewport(SCR_W as i32, SCR_H as i32))
        };
        if self.viewport != viewport {
            self.viewport = viewport;
            self.frame = None;
        }
        if let Some(frame) =
            ass::request(source, clock, viewport.width, viewport.height, coded_w, coded_h)
        {
            self.frame = Some(frame);
        }
    }

    pub(crate) fn fingerprint(&self) -> u64 {
        self.frame.as_ref().map_or(0, |f| f.serial)
    }

    pub(crate) fn draw(&mut self, lift: bool) {
        let Some(frame) = &self.frame else {
            if self.source == 0 {
                clear_textures(&mut self.textures);
                self.uploaded = 0;
            }
            return;
        };
        if self.uploaded != frame.serial {
            // Reserve all unchanged regions first. A changing region must not steal
            // a texture that a later static sign still needs in this same frame.
            let matches = retained_regions(&self.textures, &frame.rects);
            let mut old: Vec<_> = std::mem::take(&mut self.textures)
                .into_iter()
                .map(Some)
                .collect();
            let mut next: Vec<_> = matches
                .into_iter()
                .map(|index| index.and_then(|i| old[i].take()))
                .collect();
            for (slot, rect) in next.iter_mut().zip(&frame.rects) {
                if slot.is_none() {
                    let previous = old.iter_mut().find_map(Option::take);
                    let id = nj_base::diag::spans::span("assup", || nj_gfx::gfx::upload_rgba(
                        previous.as_ref().map_or(0, |t| t.id),
                        rect.width,
                        rect.height,
                        rect.rgba.as_ptr(),
                    ));
                    if id != 0 {
                        crate::player::ass_perf::note_upload(rect.rgba.len());
                    }
                    *slot = Some(CachedTexture {
                        id,
                        width: rect.width,
                        height: rect.height,
                        pixels: rect.rgba.clone(),
                    });
                }
            }
            for unused in old.into_iter().flatten() {
                nj_gfx::gfx::delete_tex(unused.id);
            }
            self.textures = next.into_iter().flatten().collect();
            self.uploaded = frame.serial;
        }
        self.spans.clear();
        self.spans.extend(frame.rects.iter().map(|r| (r.y,r.height)));
        lift_dys(frame.origin, lift, &self.spans, frame.aligns, &mut self.dys);
        let ink = crate::appkit::player_hud::subtitle_ink();
        for ((texture, rect), dy) in self.textures.iter().zip(&frame.rects).zip(&self.dys) {
            Painter::root().tex(
                texture.id,
                Rect::new(
                    (self.viewport.x + rect.x) as f32,
                    (self.viewport.y + rect.y) as f32 + dy,
                    rect.width as f32,
                    rect.height as f32,
                ),
                0.0,
                ink,
            );
        }
    }

    pub(crate) fn error(&self) -> Option<&'static str> {
        self.frame.as_ref().and_then(|f| f.error).map(crate::player::ass::Fault::message)
    }

    pub(crate) fn release(&mut self) {
        clear_textures(&mut self.textures);
        self.uploaded = 0;
        self.frame = None;
        self.source = 0;
        ass::clear();
    }

    pub(crate) fn render_report(&self) -> crate::ui::frame::RenderReport {
        crate::ui::frame::RenderReport {
            textures: self.textures.iter().filter(|t| t.id != 0).count() as u32,
            bytes: self.textures.iter().map(|t| t.pixels.len()).sum(),
        }
    }
}

fn lift_dys(
    origin: ass::Origin,
    lift: bool,
    regions: &[(i32, i32)],
    aligns: text_ass::AlignSet,
    dys: &mut Vec<f32>,
) {
    dys.clear();
    dys.resize(regions.len(), 0.0);
    let ass::Origin::Text(style) = origin else { return };
    if !lift || !aligns.bottom {
        return;
    }
    let pitch = text_style::CAPTION_LINE_PITCH * style.scale();
    // `dys` doubles as the "moves" mark (-1) until the lift is known.
    if aligns.middle || aligns.top {
        let band = text_style::bottom_anchor_y(style.position) - pitch / 2.0;
        let mut merged = false;
        for (dy, &(y, h)) in dys.iter_mut().zip(regions) {
            if (y + h) as f32 >= band {
                *dy = -1.0;
                merged |= (y as f32) < text_style::SCR_H / 2.0;
            }
        }
        if merged {
            dys.fill(0.0);
            return;
        }
        // A block whose lines are separate regions rises as one: a region sitting right above
        // the moving group (a gap under half a pitch) joins it, up to a fixed point.
        while let Some(top) = moving_top(regions, dys) {
            let mut grew = false;
            for (dy, &(y, h)) in dys.iter_mut().zip(regions) {
                if *dy == 0.0 && y < top && ((y + h) as f32) >= top as f32 - pitch / 2.0 {
                    *dy = -1.0;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
    } else {
        dys.fill(-1.0);
    }
    let Some(top) = moving_top(regions, dys) else { return };
    let lift = text_style::HUD_LIFT.min(top.max(0) as f32);
    for dy in dys.iter_mut().filter(|d| **d != 0.0) {
        *dy = -lift;
    }
}

/// The top edge of the regions marked as moving (`dys` entry non-zero).
fn moving_top(regions: &[(i32, i32)], dys: &[f32]) -> Option<i32> {
    regions.iter().zip(dys).filter(|(_, &d)| d != 0.0).map(|(r, _)| r.0).min()
}


struct CachedTexture {
    id: u32,
    width: i32,
    height: i32,
    pixels: Arc<[u8]>,
}

fn clear_textures(textures: &mut Vec<CachedTexture>) {
    for texture in textures.drain(..) {
        nj_gfx::gfx::delete_tex(texture.id);
    }
}

/// The worker interns unchanged RGBA. Pixel identity can therefore retain an
/// upload even if a sign moved or libass reordered its disjoint output regions.
fn retained_regions(textures: &[CachedTexture], rects: &[ass::Rect]) -> Vec<Option<usize>> {
    let mut used = vec![false; textures.len()];
    rects
        .iter()
        .map(|rect| {
            let found = textures.iter().enumerate().position(|(i, texture)| {
                !used[i]
                    && texture.width == rect.width
                    && texture.height == rect.height
                    && Arc::ptr_eq(&texture.pixels, &rect.rgba)
            });
            if let Some(i) = found {
                used[i] = true;
            }
            found
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_lifts_bottom_text_without_moving_authored_or_top_cues() {
        let style=text_style::TextStyle::default();
        let mixed=text_ass::AlignSet {top:true,bottom:true,middle:false};
        let mut dys=Vec::new();
        let regions=[(100,40),(940,40)];
        lift_dys(ass::Origin::Text(style),true,&regions,mixed,&mut dys);
        assert_eq!(dys,[0.0,-200.0]);
        lift_dys(ass::Origin::Authored,true,&regions,mixed,&mut dys);
        assert_eq!(dys,[0.0,0.0]);
        lift_dys(ass::Origin::Text(style),true,&[(100,880)],mixed,&mut dys);
        assert_eq!(dys,[0.0],"a merged top/bottom region conservatively stays still");
        lift_dys(ass::Origin::Text(style),false,&regions,mixed,&mut dys);
        assert_eq!(dys,[0.0,0.0]);
    }

    #[test]
    fn moving_and_reordered_regions_retain_uploads_before_recycling_changed_ones() {
        let a: Arc<[u8]> = Arc::from([255, 0, 0, 255]);
        let b: Arc<[u8]> = Arc::from([0, 0, 255, 255]);
        let old = vec![
            CachedTexture {
                id: 11,
                width: 1,
                height: 1,
                pixels: a.clone(),
            },
            CachedTexture {
                id: 12,
                width: 1,
                height: 1,
                pixels: b.clone(),
            },
        ];
        let rect = |x, rgba| ass::Rect {
            x,
            y: 0,
            width: 1,
            height: 1,
            rgba,
        };
        let changed: Arc<[u8]> = Arc::from([0, 255, 0, 255]);
        let next = vec![rect(40, b), rect(90, changed), rect(120, a.clone())];
        assert_eq!(retained_regions(&old, &next), [Some(1), None, Some(0)]);
        assert_eq!(
            retained_regions(&old, &[rect(0, a.clone()), rect(20, a)]),
            [Some(0), None]
        );
        assert!(retained_regions(&old, &[]).is_empty());
    }
}
