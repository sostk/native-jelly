//! **Audio & Subtitles** — the pre-play track chooser, a list under the Detail hero's
//! *Audio & Subtitles* pill.
//!
//! The pill is drawn only while the loaded leaf has a file of its own and something to choose: more
//! than one audio track, or any subtitle (`Detail::has_track_choice`). It opens this panel — the
//! *Version* surface's object (`screens::versions`): a `Style::Compact` `TableView` on the panel
//! ground, anchored to the pill that opened it by the rect on its [`TrackChoiceArg`].
//!
//! | section   | rows |
//! |-----------|------|
//! | Audio     | Automatic · one row per track (`Original: English`, `Dolby Digital Plus 5.1`) |
//! | Subtitles | Automatic · Off · one row per track, with the Forced / SDH / External badges |
//!
//! **Automatic** leaves that half to the automatic pick — the user's Jellyfin preferences and the
//! file's own flags, exactly as a Play from anywhere else would (`route::build_stream`); it is
//! ticked until the viewer chooses. A row whose audio the television cannot decode says the server
//! will convert it: the choice stands and the server converts (`ResolveEnv::audio_explicit`), as
//! a pick in the player's own track menu does.
//!
//! **OK commits and the panel stays open**, so the viewer can choose audio and then a subtitle in
//! one visit; BACK closes it. Like every menu here the surface only REPORTS the choice
//! ([`AppMsg::TracksChosen`] to the Detail instance on its argument); the page writes it to the
//! metadata store (`Detail::select_tracks`), and Play starts on it.

use std::borrow::Cow;
use std::convert::Infallible;

use crate::catalog::ServerId;
use crate::metadata::{Detail, Stream, TrackChoice};
use crate::screens::registry::{AppLike, AppMsg, PageMemory};
use crate::ui::frame::Budget;
use nj_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind,
    InstanceId, Key, LogicalState, Machine, MachineId, NavOp,
};
use crate::ui::screen::{
    Activate, At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, FocusSource, Focusable, GroupKind,
    GroupSpec, Hover, HitSource, Placed, RenderStrategy, Screen, ScreenEvent, Scrim, Seat, Step,
    Stop,
};
use crate::ui::form::{Form, FormSection, FormTable, RowKey, RowKind};
use crate::ui::table::{Badge, Row};
use crate::ui::{theme, Rect};

/// The fields [`TrackChoiceScreen::write`] canonicalises, for the recorder's shape pin (§5.4).
pub(crate) const SHAPE: [&str; 2] = [
    "TrackChoiceScreen{arg:TrackChoiceArg{host:u32,sid:u32,rk:str,anchor:[u32;4]},part:str,rows:[{pick:Pick,label:str,detail:opt<str>,value:opt<str>,badges:[str],checked:bool}],sel:i32,table:TableViewMotion}",
    crate::ui::table::TableView::MOTION_SHAPE,
];

/// What the container is asked to present: the Detail instance the choice is reported to, the item
/// whose tracks are listed, and the bit-preserving rest rect of the pill that opened it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct TrackChoiceArg {
    pub(crate) host: InstanceId,
    pub(crate) sid: ServerId,
    pub(crate) rk: String,
    pub(crate) anchor: [u32; 4],
}

impl LogicalState for TrackChoiceArg {
    fn write(&self, c: &mut Canon) {
        c.u32(self.host.0).u32(u32::from(self.sid.raw())).str(&self.rk);
        for value in self.anchor {
            c.u32(value);
        }
    }
    fn probe(&self, out: &mut String) {
        out.push_str("track_choice_arg");
    }
}

const PANEL_RAD: f32 = 20.0;
const RISE: f32 = crate::ui::popover::Popover::RISE;
const BTN_GAP: f32 = theme::space::MD;
const EDGE: f32 = theme::space::XL;
const EDGE_X: f32 = crate::ui::consts::MARGIN_X;

/// Under the pill when there is room, above it when not — the *Version* surface's placement.
fn panel_at(a: Rect, content_w: f32, content_h: f32) -> Rect {
    use crate::ui::consts::{SCR_H, SCR_W};
    let width = content_w.clamp(crate::ui::table::MENU_MIN_W, crate::ui::table::MENU_MAX_W);
    let h = content_h.clamp(120.0, SCR_H - 2.0 * EDGE);
    let below = a.y + a.h + BTN_GAP;
    let y = if below + h <= SCR_H - EDGE { below } else { (a.y - BTN_GAP - h).max(EDGE) };
    let x = a.x.clamp(EDGE_X, (SCR_W - EDGE_X - width).max(EDGE_X));
    Rect::new(x, y, width, h)
}

// ---- the model (pure) ------------------------------------------------------------------------

/// What a row chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    /// Audio back to the automatic pick.
    AudioAuto,
    /// This audio track (`Stream::id`).
    Audio(i64),
    /// Subtitles back to the automatic pick.
    SubtitleAuto,
    /// Subtitles off.
    SubtitleOff,
    /// This subtitle track (`Stream::id`).
    Subtitle(i64),
}

impl Pick {
    /// `choice` with this row's half replaced. PURE.
    pub(crate) fn apply(self, choice: TrackChoice) -> TrackChoice {
        match self {
            Pick::AudioAuto => TrackChoice { audio: None, ..choice },
            Pick::Audio(id) => TrackChoice { audio: Some(id), ..choice },
            Pick::SubtitleAuto => TrackChoice { subtitle: None, ..choice },
            Pick::SubtitleOff => TrackChoice { subtitle: Some(0), ..choice },
            Pick::Subtitle(id) => TrackChoice { subtitle: Some(id), ..choice },
        }
    }

    fn is_audio(self) -> bool {
        matches!(self, Pick::AudioAuto | Pick::Audio(_))
    }

    fn write(self, c: &mut Canon) {
        match self {
            Pick::AudioAuto => c.u32(0),
            Pick::Audio(id) => c.u32(1).u32(id as u32),
            Pick::SubtitleAuto => c.u32(2),
            Pick::SubtitleOff => c.u32(3),
            Pick::Subtitle(id) => c.u32(4).u32(id as u32),
        };
    }
}

/// One drawn row, resolved.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChoiceRow {
    pub(crate) pick: Pick,
    /// the row's position in its section — its focus key
    pub(crate) key: u32,
    pub(crate) label: String,
    /// `label` (or `detail`) came from the file rather than the app's catalog
    pub(crate) server_text: bool,
    pub(crate) detail: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) badges: Vec<String>,
    pub(crate) checked: bool,
}

/// The key band of the Subtitles section; the Audio section counts up from 0.
const SUB_KEY_BASE: u32 = 0x1000;

/// "Dolby Digital Plus 5.1", "AAC Stereo" — the codec and channel layout, as the player's own
/// track menu spells them.
fn audio_descriptor(s: &Stream) -> String {
    let codec = crate::metadata::friendly_codec(&s.codec);
    let base = s.layout.split('(').next().unwrap_or("").trim();
    let ch = match (base, s.channels) {
        ("mono", _) | ("", 1) => nj_platform::i18n::msg::widgets_tracks_mono().to_string(),
        ("stereo", _) | ("", 2) => nj_platform::i18n::msg::widgets_tracks_stereo().to_string(),
        ("", n) if n > 2 => format!("{}.{}", n - 1, if n >= 6 { 1 } else { 0 }),
        ("", _) => String::new(),
        (other, _) => other.to_string(),
    };
    [codec, ch].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

fn language(s: &Stream) -> String {
    if s.lang.is_empty() {
        nj_platform::i18n::msg::widgets_tracks_unknown().to_string()
    } else {
        s.lang.clone()
    }
}

/// The rows for `d`'s tracks, Audio then Subtitles, with the tick on `d.tracks`. PURE.
pub(crate) fn rows(d: &Detail) -> Vec<ChoiceRow> {
    let msg = nj_platform::i18n::msg::browse_detail_tracks_automatic;
    let choice = d.tracks;
    let mut out = Vec::with_capacity(d.audio.len() + d.subs.len() + 3);
    out.push(ChoiceRow {
        pick: Pick::AudioAuto,
        key: 0,
        label: msg().to_string(),
        server_text: false,
        detail: None,
        value: None,
        badges: Vec::new(),
        checked: choice.audio.is_none(),
    });
    for (i, s) in d.audio.iter().enumerate() {
        let lang = language(s);
        let label = if s.default { nj_platform::i18n::msg::widgets_tracks_original(&lang) } else { lang.clone() };
        let name = crate::metadata::track_label::track_name(&s.title, "", &lang);
        let descriptor = audio_descriptor(s);
        // A title that already says the codec ("Dolby TrueHD 7.1") is not said twice.
        let descriptor = if name.eq_ignore_ascii_case(&descriptor) { String::new() } else { descriptor };
        let detail = [name, descriptor]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" \u{b7} ");
        let mut badges = Vec::new();
        if s.has_atmos() {
            badges.push("Atmos".to_string());
        }
        if s.ad {
            badges.push(Badge::Ad.text().to_string());
        }
        out.push(ChoiceRow {
            pick: Pick::Audio(s.id),
            key: 1 + i as u32,
            label,
            server_text: true,
            detail: (!detail.is_empty()).then_some(detail),
            value: (!crate::catalog::is_dp_audio_track(&s.codec, s.channels))
                .then(|| nj_platform::i18n::msg::browse_detail_tracks_converts().to_string()),
            badges,
            checked: choice.audio == Some(s.id),
        });
    }
    out.push(ChoiceRow {
        pick: Pick::SubtitleAuto,
        key: SUB_KEY_BASE,
        label: msg().to_string(),
        server_text: false,
        detail: None,
        value: None,
        badges: Vec::new(),
        checked: choice.subtitle.is_none(),
    });
    out.push(ChoiceRow {
        pick: Pick::SubtitleOff,
        key: SUB_KEY_BASE + 1,
        label: nj_platform::i18n::msg::widgets_tracks_off().to_string(),
        server_text: false,
        detail: None,
        value: None,
        badges: Vec::new(),
        checked: choice.subtitle == Some(0),
    });
    for (i, s) in d.subs.iter().enumerate() {
        let lang = language(s);
        let name = crate::metadata::track_label::track_name(&s.title, "", &lang);
        let codec = crate::metadata::friendly_codec(&s.codec);
        let detail = [name, codec]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" \u{b7} ");
        let mut badges = Vec::new();
        if s.forced {
            badges.push(Badge::Forced.text().to_string());
        }
        if s.sdh {
            badges.push(Badge::Sdh.text().to_string());
        }
        if s.external {
            badges.push(nj_platform::i18n::msg::widgets_tracks_external_badge().to_string());
        }
        out.push(ChoiceRow {
            pick: Pick::Subtitle(s.id),
            key: SUB_KEY_BASE + 2 + i as u32,
            label: lang,
            server_text: true,
            detail: (!detail.is_empty()).then_some(detail),
            value: None,
            badges,
            checked: choice.subtitle == Some(s.id),
        });
    }
    out
}

fn row_for(r: &ChoiceRow) -> Row {
    let mut row = Row::new(r.label.clone()).checked(r.checked);
    if r.server_text {
        row = row.server_label();
    }
    if let Some(d) = &r.detail {
        row = row.detail(d.clone()).server_detail();
    }
    if let Some(v) = &r.value {
        row = row.value(v.clone()).value_dim(true);
    }
    for b in &r.badges {
        row = row.badge(Badge::Text(b.clone()));
    }
    row
}

/// The panel's two sections, Audio and Subtitles, each row keyed by its place in its section.
pub(crate) fn form_for(rows: &[ChoiceRow]) -> Form<Pick, Pick, Infallible> {
    let mut audio = FormSection::new(nj_platform::i18n::msg::widgets_tracks_audio());
    let mut subs = FormSection::new(nj_platform::i18n::msg::widgets_tracks_subtitles());
    for r in rows {
        if r.pick.is_audio() {
            audio = audio.item_keyed(r.pick, RowKey(r.key), RowKind::Choice, r.pick, row_for(r));
        } else {
            subs = subs.item_keyed(r.pick, RowKey(r.key), RowKind::Choice, r.pick, row_for(r));
        }
    }
    Form::new().section(audio).section(subs)
}

// ---- the surface -----------------------------------------------------------------------------

pub(crate) struct TrackChoiceScreen {
    entry: EntryId,
    arg: TrackChoiceArg,
    /// the version the rows describe — reported with a choice so a stale one is refused
    part: String,
    /// the choice the rows tick
    choice: TrackChoice,
    /// the rows the table was built from — the rebuild stamp
    rows: Vec<ChoiceRow>,
    pub(crate) form: FormTable<Pick, Pick, Infallible>,
}

impl TrackChoiceScreen {
    pub(crate) fn new(entry: EntryId, arg: TrackChoiceArg, meta: crate::metadata::MetadataView<'_>) -> Self {
        let mut screen = Self {
            entry,
            arg,
            part: String::new(),
            choice: TrackChoice::default(),
            rows: Vec::new(),
            form: FormTable::new(crate::ui::table_screen::BAND_BASE),
        };
        screen.take(meta);
        // open on the ticked audio row
        let here = screen.rows.iter().find(|r| r.checked).map(|r| r.pick);
        screen.rebuild(here.as_ref());
        screen
    }

    /// The item the PAGE is on — nothing while the store holds another item.
    fn live<'a>(&self, meta: crate::metadata::MetadataView<'a>) -> Option<&'a Detail> {
        meta.current().filter(|d| crate::catalog::same_item((d.sid, &d.rk), (self.arg.sid, &self.arg.rk)))
    }

    /// Adopt the live item's rows; `true` when they changed.
    fn take(&mut self, meta: crate::metadata::MetadataView<'_>) -> bool {
        let (part, choice, next) = match self.live(meta) {
            Some(d) => (d.part.clone(), d.tracks, rows(d)),
            None => (String::new(), TrackChoice::default(), Vec::new()),
        };
        self.part = part;
        self.choice = choice;
        if next == self.rows {
            return false;
        }
        self.rows = next;
        true
    }

    fn refresh(&mut self, meta: crate::metadata::MetadataView<'_>) -> bool {
        if !self.take(meta) {
            return false;
        }
        let held = self.form.selected_id().cloned();
        self.rebuild(held.as_ref());
        true
    }

    fn rebuild(&mut self, select: Option<&Pick>) {
        let form = form_for(&self.rows);
        self.form.table.compact = false;
        self.form.set(form, select);
    }

    pub(crate) fn frame(&self, measure: &dyn nj_machine::machine::Measure) -> Rect {
        let [x, y, w, h] = self.arg.anchor.map(f32::from_bits);
        panel_at(Rect::new(x, y, w, h), self.form.table.measured_width(measure), self.form.table.measured_height())
    }

    fn dismiss<H: AppLike>(&self, fx: &mut Effects<'_, H>) {
        fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
    }

    /// Commit row `elem`: report the choice it makes to the page, and stay open.
    fn commit<H: AppLike>(&mut self, elem: u32, fx: &mut Effects<'_, H>) {
        let Some(crate::ui::form::Activation::Action(pick)) =
            self.form.index_of_key(RowKey(elem)).and_then(|i| self.form.activate(i))
        else {
            return;
        };
        let next = pick.apply(self.choice);
        if next == self.choice || self.part.is_empty() {
            return;
        }
        fx.push(Fx::Deliver(
            MachineId::Instance(self.arg.host),
            Delivery::Screen(ScreenEvent::App(AppMsg::TracksChosen {
                sid: self.arg.sid,
                rk: self.arg.rk.clone(),
                part: self.part.clone(),
                choice: next,
            })),
        ));
        fx.invalidate(nj_machine::present::Provenance::Input);
    }
}

impl<H: AppLike<Memory = PageMemory> + crate::screens::registry::MetadataLike> Machine<H> for TrackChoiceScreen {
    type Ev = ScreenEvent<H>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        match ev {
            ScreenEvent::Mount | ScreenEvent::Enter(_) => {
                self.refresh(H::metadata(cx));
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) => {
                if *ord == crate::stores::StoreId::Metadata.ord() && self.refresh(H::metadata(cx)) {
                    fx.invalidate(nj_machine::present::Provenance::Landing(fx.from()));
                }
                Handled::Yes
            }
            ScreenEvent::Tick(tick) => {
                self.form.table.sel = cx
                    .focus
                    .current
                    .filter(|key| key.entry == self.entry)
                    .and_then(|key| self.form.index_of_key(RowKey(key.elem)))
                    .map_or(self.form.table.sel, |i| i as i32);
                self.form.table.update(tick.dt(), self.frame(cx.measure).h);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                if let Some(i) = self.form.index_of_key(RowKey(to.elem)) {
                    self.form.table.sel = i as i32;
                }
                fx.invalidate(nj_machine::present::Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                self.commit(*elem, fx);
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(key) = cx.focus.current {
                    self.commit(key.elem, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(input) => match input.kind {
                InputKind::Key { key: Key::Back, edge: Edge::Down, .. } => {
                    self.dismiss(fx);
                    Handled::Yes
                }
                _ => Handled::No,
            },
            _ => Handled::No,
        }
    }
}

/// One column of `Bare` rows over the table's own cursor — the *Version* surface's focus shape.
impl<H: AppLike<Memory = PageMemory>> Focusable<H> for TrackChoiceScreen {
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        out.push(GroupSpec {
            id: GroupId(0),
            kind: GroupKind::Column,
            seat: Seat::Remembered,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Stop; 4],
            extent: self.frame(cx.measure),
            len: self.form.focusable_len(),
            elem: ElemKind::Bare,
        });
    }
    fn group_of(&self, elem: &u32, _cx: &Cx<'_, H>) -> Option<GroupId> {
        self.form.index_of_key(RowKey(*elem)).map(|_| GroupId(0))
    }
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, _cx: &Cx<'_, H>) -> Step<u32> {
        let delta = match dir {
            Dir::Up => -1,
            Dir::Down => 1,
            _ => return Step::Edge,
        };
        match self.form.step_key(RowKey(key.elem), delta) {
            Some(next) => Step::Move(FocusKey { entry: self.entry, elem: next.0 }),
            None => Step::Edge,
        }
    }
    fn place(&self, elem: &u32, cx: &Cx<'_, H>, _at: At) -> Option<Placed> {
        let index = self.form.index_of_key(RowKey(*elem))?;
        let rect = self.form.table.row_frame(self.frame(cx.measure), index as i32)?;
        Some(Placed { rect, rest_rect: rect, clip: self.frame(cx.measure), index: Some(index as u32) })
    }
    fn reconcile(&self, want: FocusKey<u32>, cx: &Cx<'_, H>) -> FocusKey<u32> {
        if self.group_of(&want.elem, cx).is_some() {
            want
        } else {
            FocusKey { entry: self.entry, elem: self.form.opening_key().map_or(0, |k| k.0) }
        }
    }
    fn seat(&self, _g: GroupId, _from: Placed, _cx: &Cx<'_, H>) -> FocusKey<u32> {
        FocusKey {
            entry: self.entry,
            elem: self.form.selected_key().or_else(|| self.form.opening_key()).map_or(0, |k| k.0),
        }
    }
}

impl LogicalState for TrackChoiceScreen {
    fn write(&self, c: &mut Canon) {
        self.arg.write(c);
        c.str(&self.part);
        c.seq(self.rows.len());
        for r in &self.rows {
            r.pick.write(c);
            c.str(&r.label)
                .option(r.detail.as_deref(), |c, v| {
                    c.str(v);
                })
                .option(r.value.as_deref(), |c, v| {
                    c.str(v);
                });
            c.seq(r.badges.len());
            for b in &r.badges {
                c.str(b);
            }
            c.bool(r.checked);
        }
        c.u32(self.form.table.sel as u32);
        self.form.table.write_motion(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str("track_choice");
    }
}

impl<H: AppLike<Memory = PageMemory> + crate::screens::registry::MetadataLike> Screen<H> for TrackChoiceScreen {
    fn name(&self) -> &'static str {
        "track_choice"
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, H>) -> Option<Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    fn scrim(&self) -> Scrim {
        Scrim::dim(theme::underlay::DIM_PANEL)
    }
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        let appear = f.page_alpha;
        let r = self.frame(f.measure);
        let p = f.painter.alpha(appear).translate(0.0, RISE * (1.0 - appear));
        let measure = f.measure;
        let field = f.underlay;
        crate::ui::profile::phase("dt.track_choice", || {
            crate::ui::widgets::panel_ground(p, r, PANEL_RAD, field);
            self.form.table.draw(p, r, measure);
        });
        let hit_p = f.painter.alpha(appear);
        for index in 0..self.form.table.n_rows() as usize {
            let Some(elem) = self.form.key_at(index).map(|k| k.0) else {
                continue;
            };
            if let Some(placed) = <Self as Focusable<H>>::place(self, &elem, f.cx, At::Drawn) {
                f.stop(
                    hit_p,
                    Stop {
                        key: FocusKey { entry: self.entry, elem },
                        rect: placed.rect,
                        rest_rect: placed.rest_rect,
                        clip: placed.clip,
                        hover: Hover::Focus,
                        activate: Activate::Immediate,
                    },
                );
            }
        }
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn focus_source(&self) -> FocusSource {
        FocusSource::Engine
    }
    fn hit_source(&self) -> HitSource {
        HitSource::Engine
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(id: i64, lang: &str, codec: &str, channels: i64, default: bool) -> Stream {
        Stream { id, index: id - 1, lang: lang.into(), lang_code: lang.to_ascii_lowercase(), codec: codec.into(), channels, default, ..Default::default() }
    }

    fn sub(id: i64, lang: &str, forced: bool, external: bool) -> Stream {
        Stream { id, index: id - 1, lang: lang.into(), codec: "srt".into(), forced, external, ..Default::default() }
    }

    fn film() -> Detail {
        Detail {
            rk: "film".into(),
            part: "/p/0".into(),
            audio: vec![audio(2, "English", "truehd", 8, true), audio(3, "English", "ac3", 6, false), audio(4, "French", "aac", 2, false)],
            subs: vec![sub(5, "English", false, false), sub(6, "English", true, false), sub(7, "Spanish", false, true)],
            ..Default::default()
        }
    }

    /// Audio then Subtitles; Automatic ticked on both while nothing is chosen; the file's default
    /// audio is named as the original; a track the panel cannot decode says it converts; the
    /// subtitle badges say forced and external.
    #[test]
    fn rows_list_every_track_under_automatic_and_off() {
        let rows = rows(&film());
        let picks: Vec<Pick> = rows.iter().map(|r| r.pick).collect();
        assert_eq!(
            picks,
            vec![
                Pick::AudioAuto,
                Pick::Audio(2),
                Pick::Audio(3),
                Pick::Audio(4),
                Pick::SubtitleAuto,
                Pick::SubtitleOff,
                Pick::Subtitle(5),
                Pick::Subtitle(6),
                Pick::Subtitle(7),
            ]
        );
        let ticked: Vec<Pick> = rows.iter().filter(|r| r.checked).map(|r| r.pick).collect();
        assert_eq!(ticked, vec![Pick::AudioAuto, Pick::SubtitleAuto]);
        assert!(rows[1].label.contains("English") && rows[1].label != "English", "{}", rows[1].label);
        assert!(rows[1].value.is_some(), "TrueHD 7.1 does not direct-play");
        assert!(rows[2].value.is_none(), "AC-3 5.1 does");
        assert!(rows[7].badges.iter().any(|b| b == Badge::Forced.text()));
        assert_eq!(rows[8].badges.len(), 1, "external");
        let keys: std::collections::HashSet<u32> = rows.iter().map(|r| r.key).collect();
        assert_eq!(keys.len(), rows.len(), "every row has its own key");
    }

    /// A track whose title is its codec description reads it once, not twice.
    #[test]
    fn a_title_that_names_the_codec_is_not_repeated() {
        let mut d = film();
        d.audio[0].title = "Dolby TrueHD 7.1".into();
        d.audio[0].layout = "7.1".into();
        let detail = rows(&d)[1].detail.clone().unwrap_or_default();
        assert_eq!(detail.matches("TrueHD").count(), 1, "{detail}");
    }

    /// A row replaces only its own half of the choice; the tick follows the choice.
    #[test]
    fn a_pick_replaces_only_its_own_half() {
        let mut d = film();
        let c = Pick::Audio(3).apply(TrackChoice::default());
        assert_eq!(c, TrackChoice { audio: Some(3), subtitle: None });
        let c = Pick::SubtitleOff.apply(c);
        assert_eq!(c, TrackChoice { audio: Some(3), subtitle: Some(0) });
        assert_eq!(Pick::AudioAuto.apply(c), TrackChoice { audio: None, subtitle: Some(0) });
        d.tracks = c;
        let ticked: Vec<Pick> = rows(&d).iter().filter(|r| r.checked).map(|r| r.pick).collect();
        assert_eq!(ticked, vec![Pick::Audio(3), Pick::SubtitleOff]);
    }

    /// The pill is offered for a leaf with a choice to make, never for a show container or a file
    /// with one audio track and no subtitles.
    #[test]
    fn the_pill_is_offered_only_when_there_is_a_choice() {
        assert!(film().has_track_choice());
        let mut one = film();
        one.audio.truncate(1);
        one.subs.clear();
        assert!(!one.has_track_choice());
        let mut show = film();
        show.part.clear();
        assert!(!show.has_track_choice());
    }

    /// Every app string in the panel fits in every shipped language.
    #[test]
    fn fit_report() {
        use nj_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
        let mut out = Vec::new();
        for language in SHIPPED {
            let _guard = language_on_this_thread_for_test(language);
            let mut form = FormTable::<Pick, Pick, Infallible>::new(crate::ui::table_screen::BAND_BASE);
            form.table.compact = false;
            form.set(form_for(&rows(&film())), None);
            out.extend(form.table.app_fit_failures(crate::ui::table::MENU_MAX_W, language.tag()));
            out.extend(form.table.app_fit_failures_hugged(language.tag()));
        }
        crate::ui::table::assert_no_fit_failures(&out);
    }

    mod surface {
        use super::*;
        use crate::screens::registry::{AppFx, PageMemory};
        use nj_machine::machine::{Chrome, FocusRead, Host, InputOwner, PressRead, ScreenId, Tick};
        use crate::ui::screen::ScreenArg;

        thread_local! {
            static TEST_METADATA: std::cell::UnsafeCell<crate::stores::metadata::MetadataStore> =
                std::cell::UnsafeCell::new(crate::stores::metadata::MetadataStore::default());
        }
        fn test_store() -> &'static mut crate::stores::metadata::MetadataStore {
            TEST_METADATA.with(|cell| unsafe { &mut *cell.get() })
        }

        #[derive(Clone)]
        struct FixtureArg;
        impl LogicalState for FixtureArg {
            fn write(&self, _: &mut Canon) {}
            fn probe(&self, _: &mut String) {}
        }
        impl ScreenArg for FixtureArg {
            fn chrome(&self) -> Chrome {
                Chrome::None
            }
            fn id(&self) -> ScreenId {
                ScreenId(1)
            }
            fn title(&self) -> Option<&str> {
                None
            }
            fn same_instance(&self, _: &Self) -> bool {
                true
            }
        }
        struct HostFixture;
        impl Host for HostFixture {
            type Arg = FixtureArg;
            type Fx = AppFx;
            type Msg = AppMsg;
            type Elem = u32;
            type Views<'a> = ();
            type Init = FixtureArg;
            type Memory = PageMemory;
        }
        impl crate::screens::registry::MetadataLike for HostFixture {
            fn metadata<'a>(_cx: &Cx<'a, Self>) -> crate::metadata::MetadataView<'a> {
                test_store().view()
            }
        }
        fn fixture_cx(focus: Option<FocusKey<u32>>) -> Cx<'static, HostFixture> {
            Cx {
                views: (),
                tick: Tick::default(),
                measure: &crate::ui::fixture::FixtureMeasure,
                focus: FocusRead { current: focus, ..Default::default() },
                press: PressRead::default(),
                owner: InputOwner::Entry(EntryId(5)),
            }
        }

        fn panel() -> TrackChoiceScreen {
            let sid = ServerId::from_raw(1);
            let mut p = TrackChoiceScreen::new(
                EntryId(7),
                TrackChoiceArg { host: InstanceId(3), sid, rk: "film".into(), anchor: [0.0f32, 0.0, 100.0, 40.0].map(f32::to_bits) },
                test_store().view(),
            );
            let d = film();
            p.part = d.part.clone();
            p.rows = rows(&d);
            p.rebuild(None);
            p
        }

        fn step(p: &mut TrackChoiceScreen, ev: ScreenEvent<HostFixture>) -> Vec<nj_machine::machine::Stamped<HostFixture>> {
            let cx = fixture_cx(None);
            let mut buf = Vec::new();
            let mut present = nj_machine::present::Present::default();
            {
                let mut fx = Effects::new(&mut buf, MachineId::Input, &mut present);
                Machine::step(p, &ev, &cx, &mut fx);
            }
            buf
        }

        fn chosen(buf: &[nj_machine::machine::Stamped<HostFixture>]) -> Option<(String, String, TrackChoice)> {
            buf.iter().find_map(|s| match &s.fx {
                Fx::Deliver(
                    MachineId::Instance(InstanceId(3)),
                    Delivery::Screen(ScreenEvent::App(AppMsg::TracksChosen { rk, part, choice, .. })),
                ) => Some((rk.clone(), part.clone(), *choice)),
                _ => None,
            })
        }

        /// OK on a track reports the choice to the Detail instance on the argument, for the
        /// version the panel lists, and leaves the panel open; OK on what is already chosen says
        /// nothing; BACK closes.
        #[test]
        fn choosing_a_track_reports_it_and_keeps_the_panel_open() {
            let mut p = panel();
            let buf = step(&mut p, ScreenEvent::Activate(2));
            assert_eq!(chosen(&buf), Some(("film".into(), "/p/0".into(), TrackChoice { audio: Some(3), subtitle: None })));
            assert!(buf.iter().all(|s| !matches!(&s.fx, Fx::Nav(NavOp::Dismiss(_)))), "stays open");

            let buf = step(&mut p, ScreenEvent::Activate(SUB_KEY_BASE + 1));
            assert_eq!(chosen(&buf).map(|c| c.2), Some(TrackChoice { audio: None, subtitle: Some(0) }));

            let buf = step(&mut p, ScreenEvent::Activate(0));
            assert_eq!(chosen(&buf), None, "Automatic is already the audio choice");
        }
    }
}
