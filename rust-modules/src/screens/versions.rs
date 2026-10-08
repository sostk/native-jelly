//! **Version** — the versions of the item on screen (`Media[]`: a 4K and a 1080p file, two cuts),
//! as a list under the hero's *Version* pill.
//!
//! The pill is drawn only while the loaded leaf has more than one version (`Detail::versions`), and
//! it opens this panel: the *Also available* surface's object — a `Style::Compact` `TableView` on the
//! panel ground, anchored to the pill that opened it by the rect on its [`VersionsArg`].
//!
//! | mark | label | sub-line | read-out | badge |
//! |---|---|---|---|---|
//! | ✓ | `Movie - 2160p` | `HEVC · TrueHD` | `58.2 Mbps` | `4K` |
//! |   | `Movie - 1080p` | `H.264 · AC3`   | `9.8 Mbps`  | `1080p` |
//!
//! The label is the server's own name for the version (Jellyfin's `MediaSource.Name`, what its
//! own version selector lists); a server that names none gets the codecs there instead, and the
//! container under them.
//!
//! **OK SWAPS IN PLACE**, which is where this differs from *Also available* on purpose: every row
//! here is the same item on the same server — one resume point, one watch state — so choosing one
//! changes which file Play opens and what the page's technical read-outs describe, and nothing
//! else. Like every menu here the surface only REPORTS the choice ([`AppMsg::VersionChosen`] to the
//! Detail instance on its argument); the page writes it to the metadata store.
//!
//! Rows are in server order and the tick is on the version the page describes now.

use std::borrow::Cow;
use std::convert::Infallible;

use crate::catalog::ServerId;
use crate::metadata::Detail;
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
use crate::ui::table::{Badge, Row, TableView};
use crate::ui::{theme, Rect};

/// The fields [`VersionsScreen::write`] canonicalises, for the recorder's shape pin (§5.4). The
/// selected row is in it for *Also available*'s reason: UP/DOWN here changes nothing else.
pub(crate) const SHAPE: [&str; 2] = [
    "VersionsScreen{arg:VersionsArg{host:u32,sid:u32,rk:str,anchor:[u32;4]},rows:[{part:str,label:str,detail:opt<str>,value:opt<str>,badge:opt<str>,checked:bool}],sel:i32,table:TableViewMotion}",
    TableView::MOTION_SHAPE,
];

/// What the container is asked to present: the Detail instance the choice is reported to, the item
/// whose versions are listed, and the bit-preserving rest rect of the pill that opened it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct VersionsArg {
    pub(crate) host: InstanceId,
    pub(crate) sid: ServerId,
    pub(crate) rk: String,
    pub(crate) anchor: [u32; 4],
}

impl LogicalState for VersionsArg {
    fn write(&self, c: &mut Canon) {
        c.u32(self.host.0).u32(u32::from(self.sid.raw())).str(&self.rk);
        for value in self.anchor {
            c.u32(value);
        }
    }
    fn probe(&self, out: &mut String) {
        out.push_str("versions_arg");
    }
}

const PANEL_RAD: f32 = 20.0;
const RISE: f32 = crate::ui::popover::Popover::RISE;
const BTN_GAP: f32 = theme::space::MD;
const EDGE: f32 = theme::space::XL;
const EDGE_X: f32 = crate::ui::consts::MARGIN_X;

/// Under the pill when there is room, above it when not, hanging off its left edge inside the
/// screen's keep-out — *Also available*'s placement, for the same pill row.
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

/// One drawn row, resolved.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VersionRow {
    /// the version's part — its identity and what OK selects
    pub(crate) part: String,
    pub(crate) label: String,
    /// `label` is the server's name for the version (`true`) rather than codecs the app spelled
    pub(crate) named: bool,
    pub(crate) detail: Option<String>,
    /// the whole-file bitrate
    pub(crate) value: Option<String>,
    /// the resolution class
    pub(crate) badge: Option<String>,
    /// the version the page describes now
    pub(crate) checked: bool,
}

fn codecs(vcodec: &str, acodec: &str) -> Option<String> {
    let parts: Vec<String> = [
        (!vcodec.is_empty()).then(|| crate::appkit::info_panel::video_codec_name(vcodec)),
        (!acodec.is_empty()).then(|| crate::metadata::friendly_codec(acodec)),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The rows for `d`'s versions, in server order. PURE.
pub(crate) fn rows(d: &Detail) -> Vec<VersionRow> {
    d.versions
        .iter()
        .map(|v| {
            let f = &v.facts;
            let codecs = codecs(&f.vcodec, &f.acodec);
            let container = (!f.container.is_empty()).then(|| f.container.to_ascii_uppercase());
            let named = !v.title.trim().is_empty();
            let (label, detail) = if named {
                (v.title.trim().to_string(), codecs)
            } else {
                (codecs.unwrap_or_else(|| container.clone().unwrap_or_default()), container)
            };
            VersionRow {
                part: f.part.clone(),
                label,
                named,
                detail,
                value: (f.bitrate > 0).then(|| crate::ui::fmt::bitrate(f.bitrate)),
                badge: crate::ui::fmt::resolution(&f.video_resolution, f.width, f.height),
                checked: f.part == d.part,
            }
        })
        .collect()
}

fn row_for(r: &VersionRow) -> Row {
    let mut row = Row::new(r.label.clone()).checked(r.checked);
    if r.named {
        row = row.server_label();
    }
    if let Some(d) = &r.detail {
        row = row.detail(d.clone()).server_detail();
    }
    if let Some(v) = &r.value {
        row = row.value(v.clone());
    }
    if let Some(b) = &r.badge {
        row = row.badge(Badge::Text(b.clone()));
    }
    row
}

/// What OK on a row does: choose its version, unless it is already the one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    None,
    Choose { part: String },
}

pub(crate) fn action_for(row: &VersionRow) -> Action {
    if row.checked || row.part.is_empty() {
        Action::None
    } else {
        Action::Choose { part: row.part.clone() }
    }
}

/// The panel's one section: one `Choice` row per version, keyed by position and identified by its
/// part.
pub(crate) fn form_for(rows: &[VersionRow]) -> Form<String, Action, Infallible> {
    let mut sec = FormSection::new("");
    for (pos, r) in rows.iter().enumerate() {
        sec = sec.item_keyed(r.part.clone(), RowKey(pos as u32), RowKind::Choice, action_for(r), row_for(r));
    }
    Form::new().section(sec)
}

// ---- the surface -----------------------------------------------------------------------------

pub(crate) struct VersionsScreen {
    entry: EntryId,
    arg: VersionsArg,
    /// the rows the table was built from — the rebuild stamp
    rows: Vec<VersionRow>,
    pub(crate) form: FormTable<String, Action, Infallible>,
}

impl VersionsScreen {
    pub(crate) fn new(entry: EntryId, arg: VersionsArg, meta: crate::metadata::MetadataView<'_>) -> Self {
        let mut screen = Self { entry, arg, rows: Vec::new(), form: FormTable::new(crate::ui::table_screen::BAND_BASE) };
        screen.rows = screen.live_rows(meta);
        let here = screen.rows.iter().find(|r| r.checked).map(|r| r.part.clone());
        screen.rebuild(here.as_ref());
        screen
    }

    /// The rows of the item the PAGE is on — nothing while the store holds another item.
    fn live_rows(&self, meta: crate::metadata::MetadataView<'_>) -> Vec<VersionRow> {
        meta.current()
            .filter(|d| crate::catalog::same_item((d.sid, &d.rk), (self.arg.sid, &self.arg.rk)))
            .map(rows)
            .unwrap_or_default()
    }

    fn refresh(&mut self, meta: crate::metadata::MetadataView<'_>) -> bool {
        let next = self.live_rows(meta);
        if next == self.rows {
            return false;
        }
        self.rows = next;
        let held = self.form.selected_id().cloned();
        self.rebuild(held.as_ref());
        true
    }

    fn rebuild(&mut self, select: Option<&String>) {
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

    /// Commit row `elem` and close; the version already shown reports nothing.
    fn commit<H: AppLike>(&mut self, elem: u32, fx: &mut Effects<'_, H>) {
        let action = match self.form.index_of_key(RowKey(elem)).and_then(|i| self.form.activate(i)) {
            Some(crate::ui::form::Activation::Action(action)) => action,
            _ => Action::None,
        };
        self.dismiss(fx);
        if let Action::Choose { part } = action {
            fx.push(Fx::Deliver(
                MachineId::Instance(self.arg.host),
                Delivery::Screen(ScreenEvent::App(AppMsg::VersionChosen {
                    sid: self.arg.sid,
                    rk: self.arg.rk.clone(),
                    part,
                })),
            ));
        }
    }
}

impl<H: AppLike<Memory = PageMemory> + crate::screens::registry::MetadataLike> Machine<H> for VersionsScreen {
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

/// One column of `Bare` rows over the table's own cursor — `AltSourcesScreen`'s focus shape.
impl<H: AppLike<Memory = PageMemory>> Focusable<H> for VersionsScreen {
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

impl LogicalState for VersionsScreen {
    fn write(&self, c: &mut Canon) {
        self.arg.write(c);
        c.seq(self.rows.len());
        for r in &self.rows {
            c.str(&r.part)
                .str(&r.label)
                .option(r.detail.as_deref(), |c, v| {
                    c.str(v);
                })
                .option(r.value.as_deref(), |c, v| {
                    c.str(v);
                })
                .option(r.badge.as_deref(), |c, b| {
                    c.str(b);
                })
                .bool(r.checked);
        }
        c.u32(self.form.table.sel as u32);
        self.form.table.write_motion(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str("versions");
    }
}

impl<H: AppLike<Memory = PageMemory> + crate::screens::registry::MetadataLike> Screen<H> for VersionsScreen {
    fn name(&self) -> &'static str {
        "versions"
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
        crate::ui::profile::phase("dt.versions", || {
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
    use crate::metadata::{Version, VersionFacts};

    fn version(title: &str, part: &str, vcodec: &str, res: &str, kbps: i64) -> Version {
        Version {
            title: title.into(),
            facts: VersionFacts {
                part: part.into(),
                vcodec: vcodec.into(),
                acodec: "eac3".into(),
                video_resolution: res.into(),
                bitrate: kbps,
                container: "mkv".into(),
                ..Default::default()
            },
        }
    }

    /// Server order, the tick on the version the page shows, the server's name when it gave one and
    /// the codecs when it did not; the version already shown is not a destination.
    #[test]
    fn rows_list_every_version_and_tick_the_one_shown() {
        let d = Detail {
            part: "/p/1".into(),
            versions: vec![version("Film - 2160p", "/p/0", "hevc", "4k", 58_200), version("", "/p/1", "h264", "1080", 0)],
            ..Default::default()
        };
        let rows = rows(&d);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].label.as_str(), rows[0].named, rows[0].checked), ("Film - 2160p", true, false));
        assert!(rows[0].detail.as_deref().is_some_and(|s| s.starts_with("HEVC")), "{:?}", rows[0].detail);
        assert_eq!(rows[0].badge.as_deref(), Some("4K"));
        assert!(rows[0].value.is_some());
        assert!(!rows[1].named && rows[1].checked);
        assert!(rows[1].label.starts_with("H.264") || rows[1].label.starts_with("H264"), "{}", rows[1].label);
        assert_eq!(rows[1].detail.as_deref(), Some("MKV"));
        assert_eq!(rows[1].value, None, "no bitrate, no read-out");
        assert_eq!(action_for(&rows[0]), Action::Choose { part: "/p/0".into() });
        assert_eq!(action_for(&rows[1]), Action::None);
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

        fn two_version_panel() -> VersionsScreen {
            let sid = ServerId::from_raw(1);
            let mut p = VersionsScreen::new(
                EntryId(7),
                VersionsArg { host: InstanceId(3), sid, rk: "4".into(), anchor: [0.0f32, 0.0, 100.0, 40.0].map(f32::to_bits) },
                test_store().view(),
            );
            let d = Detail {
                part: "/p/1".into(),
                versions: vec![version("Film - 2160p", "/p/0", "hevc", "4k", 58_200), version("Film - 1080p", "/p/1", "h264", "1080", 9_800)],
                ..Default::default()
            };
            p.rows = rows(&d);
            p.rebuild(Some(&"/p/1".to_string()));
            p
        }

        /// OK on another version closes the panel and reports that version's part to the Detail
        /// instance on the argument, for the item on the argument; OK on the ticked one only closes.
        #[test]
        fn choosing_another_version_reports_it_to_the_page_and_dismisses() {
            let mut p = two_version_panel();
            let entry = p.entry;
            let cx = fixture_cx(Some(FocusKey { entry, elem: 0 }));
            let mut buf = Vec::new();
            let mut present = nj_machine::present::Present::default();
            {
                let mut fx = Effects::new(&mut buf, MachineId::Input, &mut present);
                assert_eq!(Machine::step(&mut p, &ScreenEvent::Activate(0), &cx, &mut fx), Handled::Yes);
            }
            assert!(buf.iter().any(|s| matches!(&s.fx, Fx::Nav(NavOp::Dismiss(e)) if *e == entry)));
            let chosen = buf.iter().find_map(|s| match &s.fx {
                Fx::Deliver(
                    MachineId::Instance(InstanceId(3)),
                    Delivery::Screen(ScreenEvent::App(AppMsg::VersionChosen { sid, rk, part })),
                ) => Some((sid.raw(), rk.clone(), part.clone())),
                _ => None,
            });
            assert_eq!(chosen, Some((1, "4".to_string(), "/p/0".to_string())));

            buf.clear();
            {
                let mut fx = Effects::new(&mut buf, MachineId::Input, &mut present);
                Machine::step(&mut p, &ScreenEvent::Activate(1), &cx, &mut fx);
            }
            assert!(buf.iter().any(|s| matches!(&s.fx, Fx::Nav(NavOp::Dismiss(_)))));
            assert!(buf.iter().all(|s| !matches!(&s.fx, Fx::Deliver(..))), "the version shown is not a choice");
        }
    }
}
