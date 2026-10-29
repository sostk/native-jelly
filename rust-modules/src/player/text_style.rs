//! Shared caption geometry for synthesized ASS and the HUD's subtitle notices.
//! Values preserve the existing size/position ladder on the 1920x1080 UI canvas.
use crate::catalog::session::{SubtitlePosition, SubtitleSize};

pub(crate) const SCR_W: f32 = nj_base::surface::LOGICAL_W;
pub(crate) const SCR_H: f32 = nj_base::surface::LOGICAL_H;
pub(crate) const CAPTION_FACE: f32 = 36.0;
pub(crate) const CAPTION_LINE_PITCH: f32 = 48.0;
const CAPTION_WRAP_W: f32 = 760.0;
const CAPTION_MAX_W: f32 = SCR_W * 0.75;
pub(crate) const SUB_BASE_Y: f32 = SCR_H - 100.0;
pub(crate) const SUB_CEIL_Y: f32 = SCR_H - 300.0;
pub(crate) const HUD_LIFT: f32 = SUB_BASE_Y - SUB_CEIL_Y;
pub(crate) const TOP_MARGIN_V: f32 = 100.0;
// libass measures Inter Bold through its OS/2 win metrics (2929 / 2048),
// while the old caption face used em pixels. Keep that conversion explicit.
const INTER_BOLD_WIN_EM: f64 = 2929.0 / 2048.0;

pub(crate) fn size_scale(size: SubtitleSize) -> f32 {
    match size {
        SubtitleSize::Small => 0.78,
        SubtitleSize::Medium => 1.0,
        SubtitleSize::Large => 1.25,
        SubtitleSize::ExtraLarge => 1.5,
    }
}
pub(crate) fn position_lift(position: SubtitlePosition) -> f32 {
    match position {
        SubtitlePosition::Low => 0.0,
        SubtitlePosition::Middle => 160.0,
        SubtitlePosition::High => 320.0,
    }
}
pub(crate) fn side_margin(scale: f32) -> u32 {
    ((SCR_W - (CAPTION_WRAP_W * scale).min(CAPTION_MAX_W)) / 2.0).round() as u32
}
pub(crate) fn bottom_margin_v(position: SubtitlePosition) -> u32 {
    (97.0 + position_lift(position)).round() as u32
}
pub(crate) fn bottom_anchor_y(position: SubtitlePosition) -> f32 {
    SCR_H - bottom_margin_v(position) as f32
}
pub(crate) fn font_size(scale: f32) -> f64 {
    CAPTION_FACE as f64 * scale as f64 * INTER_BOLD_WIN_EM
}
pub(crate) fn line_spacing(scale: f32) -> f64 {
    CAPTION_LINE_PITCH as f64 * scale as f64 - font_size(scale)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextStyle {
    pub size: SubtitleSize,
    pub position: SubtitlePosition,
}
impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: SubtitleSize::Medium,
            position: SubtitlePosition::Low,
        }
    }
}
impl TextStyle {
    pub(crate) fn current() -> Self {
        Self {
            size: crate::route::subtitle_size(),
            position: crate::route::subtitle_position(),
        }
    }
    pub(crate) fn scale(&self) -> f32 {
        size_scale(self.size)
    }
}
