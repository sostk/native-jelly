//! **Who's watching?** — every Jellyfin user kept on this television (`jf::store::roster`), one
//! avatar each, and *Add user* at the end of the row.
//!
//! The approved design (2026-10-09, "Multi-user profiles", board 1) is the old Plex Home picker's
//! layout without its PIN pad: a centred HERO title, a row of large circles whose focused one grows
//! and wears the focus rim, the names under them, and the server they belong to along the bottom.
//! Each person is their initial on a disc in their own tone (`widgets::avatar_disc`).
//!
//! The screen never handles a token. A pick is the user's POSITION in the roster it showed
//! (`LoopReq::PickJellyfinUser`); the loop reads the roster again, and either carries on as the
//! user already signed in or switches to the one picked (`app::jf_login::pick_user`). BACK carries
//! on as the signed-in user when there is one — this screen is a question, not a gate.

use std::borrow::Cow;

use crate::ui::consts::{SCR_H, SCR_W};
use crate::ui::frame::Budget;
use crate::ui::label::HAlign;
use crate::ui::route_screen::RouteGround;
use crate::ui::screen::{
    Activate, At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Enter, FocusSource, FocusTarget,
    Focusable, GroupKind, GroupSpec, HitSource, Hover, Placed, RenderStrategy, Screen, ScreenEvent,
    Seat, Step, Stop,
};
use crate::ui::text_view::TextView;
use crate::ui::widgets::CtlPop;
use crate::ui::{theme, Painter, Rect};
use nj_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind, Key,
    LogicalState, Machine, Measure,
};
use nj_machine::present::Provenance;
use nj_platform::i18n::msg;

use super::registry::{word, AppFx, AppLike, LoopReq};

pub(crate) const SHAPE: &str = "JfUsersScreen{entry:u32,users:u32,active:Option<u32>}";

/// The *Add user* tile's element; a person's element is their position in the roster.
const ADD: u32 = 1000;
const GROUP: GroupId = GroupId(0);
/// Every kept user and *Add user*.
const TILES: usize = crate::jf::store::MAX_USERS + 1;
/// How the circles stand: up to six in one row at the design's full size, more in two compact
/// rows of seven — every kept user and *Add user* always fit under the title.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Grid {
    avatar: f32,
    gap: f32,
    per_row: usize,
    /// The first row's top edge.
    top: f32,
}
const FULL: Grid = Grid { avatar: 220.0, gap: 72.0, per_row: 6, top: 384.0 };
const COMPACT: Grid = Grid { avatar: 160.0, gap: 56.0, per_row: 7, top: 288.0 };
/// Under a circle: the gap to its name, the name, and the air before a second row.
const NAME_GAP: f32 = 30.0;
const NAME_H: f32 = 44.0;
const ROW_AIR: f32 = 40.0;
const TITLE_Y: f32 = 132.0;
const TITLE_H: f32 = 96.0;
/// The server line along the bottom.
const FOOTER_H: f32 = 40.0;
const FOOTER_MARK: f32 = 28.0;
/// *Add user*'s plus mark inside its circle.
const PLUS: f32 = 76.0;

/// One person on the screen: what is drawn of them, never their token.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Person {
    id: String,
    name: String,
}

pub(crate) struct JfUsersScreen {
    entry: EntryId,
    people: Vec<Person>,
    /// The user signed in underneath (`Roster::active`): focus starts on them and BACK resumes them.
    active: Option<usize>,
    /// "Living Room · 192.168.1.20:8096" — the server these people belong to.
    server: String,
    pop: CtlPop<TILES>,
    ground: RouteGround,
}

impl JfUsersScreen {
    pub(crate) fn new(entry: EntryId) -> Self {
        let roster = crate::jf::store::roster();
        let people = roster
            .users
            .iter()
            .map(|u| Person {
                id: u.user_id.clone(),
                name: if u.user_name.trim().is_empty() { u.user_id.clone() } else { u.user_name.clone() },
            })
            .collect();
        let server = roster
            .current()
            .or(roster.users.first())
            .map(|u| {
                let address = u.origin().map(|o| format!("{}:{}", o.host(), o.port())).unwrap_or_default();
                match (u.server_name.trim().is_empty(), address.is_empty()) {
                    (true, _) => address,
                    (false, true) => u.server_name.clone(),
                    (false, false) => format!("{} \u{b7} {address}", u.server_name),
                }
            })
            .unwrap_or_default();
        Self { entry, people, active: roster.active, server, pop: CtlPop::new(), ground: RouteGround::new() }
    }

    fn key(&self, elem: u32) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem }
    }

    /// How many tiles: everyone, and *Add user*.
    fn tiles(&self) -> usize {
        self.people.len() + 1
    }

    fn elem_of(&self, tile: usize) -> u32 {
        if tile < self.people.len() { tile as u32 } else { ADD }
    }

    fn tile_of(&self, elem: u32) -> Option<usize> {
        match elem {
            ADD => Some(self.people.len()),
            i if (i as usize) < self.people.len() => Some(i as usize),
            _ => None,
        }
    }

    /// Where focus starts: the user signed in underneath, else the first person, else *Add user*.
    fn first(&self) -> u32 {
        self.elem_of(self.active.filter(|&i| i < self.people.len()).unwrap_or(0))
    }

    fn grid(&self) -> Grid {
        if self.tiles() <= FULL.per_row { FULL } else { COMPACT }
    }

    /// Tile `i`'s circle, at rest: rows of [`Grid::per_row`], each centred on the screen.
    fn tile_rect(&self, i: usize) -> Rect {
        let g = self.grid();
        let n = self.tiles();
        let row = i / g.per_row;
        let in_row = (n - row * g.per_row).min(g.per_row);
        let row_w = in_row as f32 * g.avatar + (in_row - 1) as f32 * g.gap;
        let x = (SCR_W - row_w) * 0.5 + (i % g.per_row) as f32 * (g.avatar + g.gap);
        let y = g.top + row as f32 * (g.avatar + NAME_GAP + NAME_H + ROW_AIR);
        Rect::new(x, y, g.avatar, g.avatar)
    }

    /// The tile `dir` leads to from tile `i`: LEFT/RIGHT along a row, UP/DOWN to the same column
    /// of the row above or below (the nearest tile of a shorter last row).
    fn step_from(&self, i: usize, dir: Dir) -> Option<usize> {
        let n = self.tiles();
        let per_row = self.grid().per_row;
        match dir {
            Dir::Left if i % per_row > 0 => Some(i - 1),
            Dir::Right if i % per_row < per_row - 1 && i + 1 < n => Some(i + 1),
            Dir::Up => i.checked_sub(per_row),
            Dir::Down if (i / per_row + 1) * per_row < n => Some((i + per_row).min(n - 1)),
            _ => None,
        }
    }

    fn reseat<H: AppLike>(&self, elem: u32, fx: &mut Effects<'_, H>) {
        let me = fx.from();
        fx.push(Fx::Deliver(
            me,
            Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::Elem(self.key(elem)) })),
        ));
    }

    fn activate<H: AppLike>(&mut self, elem: u32, fx: &mut Effects<'_, H>) {
        match self.tile_of(elem) {
            Some(i) if i == self.people.len() => fx.push(Fx::App(AppFx::Loop(LoopReq::AccountAddUser))),
            Some(i) => fx.push(Fx::App(AppFx::Loop(LoopReq::PickJellyfinUser(i as u8)))),
            None => {}
        }
    }

    /// BACK: carry on as the user signed in underneath, or hand the screen to the television when
    /// nobody is (the last one just signed out).
    fn back<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        let req = match self.active.filter(|&i| i < self.people.len()) {
            Some(i) => LoopReq::PickJellyfinUser(i as u8),
            None => LoopReq::BackAtRoot,
        };
        fx.push(Fx::App(AppFx::Loop(req)));
    }

    fn draw_tile(&self, p: Painter, i: usize, focused: bool, scale: f32) {
        let rest = self.tile_rect(i);
        let r = Rect::new(
            rest.cx() - rest.w * scale * 0.5,
            rest.cy() - rest.h * scale * 0.5,
            rest.w * scale,
            rest.h * scale,
        );
        let name = match self.people.get(i) {
            Some(person) => {
                let size = if self.grid() == FULL { theme::size::HERO } else { theme::size::DISPLAY };
                crate::ui::widgets::avatar_disc(p, r, &person.name, &person.id, focused, size);
                person.name.as_str()
            }
            None => {
                let rad = r.w * 0.5;
                p.rrect(r, rad, rad, theme::CONTROL_IDLE_FILL_UNKEYED);
                if focused {
                    p.rring(r, rad, 5.0, theme::CONTROL_RIM_FOCUS_UNKEYED);
                } else {
                    p.rring(r, rad, 2.0, theme::CONTROL_RIM_IDLE_UNKEYED);
                }
                let mark = PLUS * r.w / FULL.avatar;
                crate::ui::icons::draw(
                    p,
                    crate::ui::icons::Icon::Plus,
                    Rect::new(r.cx() - mark * 0.5, r.cy() - mark * 0.5, mark, mark),
                    if focused { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY },
                );
                msg::settings_account_add_user()
            }
        };
        let ink = if focused { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };
        let mut view = TextView::new(name, theme::size::BODY, ink).h(HAlign::Center).max_lines(1);
        if focused {
            view = view.bold();
        }
        let g = self.grid();
        let name_w = g.avatar + g.gap - 12.0;
        view.draw(p, Rect::new(rest.cx() - name_w * 0.5, r.y + r.h + NAME_GAP, name_w, NAME_H));
    }

    fn draw_footer(&self, p: Painter, measure: &dyn Measure) {
        if self.server.is_empty() {
            return;
        }
        let y = SCR_H - crate::ui::consts::SAFE.y - FOOTER_H;
        let gap = theme::space::SM;
        let room = crate::ui::consts::SAFE.w - FOOTER_MARK - gap;
        let tw = measure.width_str(&self.server, theme::size::CAPTION, false).min(room);
        let x = (SCR_W - FOOTER_MARK - gap - tw) * 0.5;
        crate::ui::icons::draw(
            p,
            crate::ui::icons::Icon::Server,
            Rect::new(x, y + (FOOTER_H - FOOTER_MARK) * 0.5, FOOTER_MARK, FOOTER_MARK),
            theme::TEXT_TERTIARY,
        );
        TextView::new(&self.server, theme::size::CAPTION, theme::TEXT_TERTIARY)
            .max_lines(1)
            .draw(p, Rect::new(x + FOOTER_MARK + gap, y + 4.0, tw + 1.0, FOOTER_H));
    }
}

impl<H: AppLike> Focusable<H> for JfUsersScreen {
    fn groups(&self, _cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        let extent = (0..self.tiles()).map(|i| self.tile_rect(i)).reduce(|a, b| a.union(b)).unwrap_or(Rect::FULL);
        out.push(GroupSpec {
            id: GROUP,
            kind: GroupKind::Free,
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Stop; 4],
            extent,
            len: self.tiles(),
            elem: ElemKind::Bare,
        });
    }
    fn group_of(&self, key: &u32, _cx: &Cx<'_, H>) -> Option<GroupId> {
        self.tile_of(*key).map(|_| GROUP)
    }
    /// LEFT/RIGHT along a row, UP/DOWN to the same column of the row above or below (the nearest
    /// tile of a shorter last row).
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, _cx: &Cx<'_, H>) -> Step<u32> {
        match self.tile_of(key.elem).and_then(|i| self.step_from(i, dir)) {
            Some(j) => Step::Move(self.key(self.elem_of(j))),
            None => Step::Edge,
        }
    }
    fn place(&self, key: &u32, _cx: &Cx<'_, H>, _at: At) -> Option<Placed> {
        let i = self.tile_of(*key)?;
        let rect = self.tile_rect(i);
        Some(Placed { rect, rest_rect: rect, clip: Rect::FULL, index: Some(i as u32) })
    }
    fn reconcile(&self, want: FocusKey<u32>, _cx: &Cx<'_, H>) -> FocusKey<u32> {
        if self.tile_of(want.elem).is_some() { want } else { self.key(self.first()) }
    }
    fn seat(&self, _g: GroupId, _from: Placed, _cx: &Cx<'_, H>) -> FocusKey<u32> {
        self.key(self.first())
    }
}

impl<H: AppLike> Machine<H> for JfUsersScreen {
    type Ev = ScreenEvent<H>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        match ev {
            ScreenEvent::Mount => self.reseat(self.first(), fx),
            ScreenEvent::Tick(t) => {
                let focused = cx.focus.current.filter(|k| k.entry == self.entry).and_then(|k| self.tile_of(k.elem));
                self.pop.step(focused, t.dt());
            }
            ScreenEvent::Activate(elem) => self.activate(*elem, fx),
            ScreenEvent::FocusMoved { .. } => fx.invalidate(Provenance::Input),
            ScreenEvent::Input(input) => match input.kind {
                InputKind::Key { key: Key::Back, edge: Edge::Down, .. } => self.back(fx),
                _ => return Handled::No,
            },
            _ => return Handled::No,
        }
        Handled::Yes
    }
}

impl<H: AppLike> Screen<H> for JfUsersScreen {
    fn name(&self) -> &'static str {
        word::PROFILES
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, H>) -> Option<Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        let p = f.painter;
        self.ground.draw_default(Painter::root());
        TextView::new(msg::settings_profiles_title(), theme::size::HERO, theme::TEXT_HEADING)
            .bold()
            .h(HAlign::Center)
            .max_lines(1)
            .draw(p, Rect::new(crate::ui::consts::SAFE.x, TITLE_Y, crate::ui::consts::SAFE.w, TITLE_H));
        let focus = f.focus.current.filter(|k| k.entry == self.entry).and_then(|k| self.tile_of(k.elem));
        for i in 0..self.tiles() {
            self.draw_tile(p, i, focus == Some(i), self.pop.scale_with(i, f.press.scale));
        }
        self.draw_footer(p, f.measure);
        if f.records_stops() {
            for i in 0..self.tiles() {
                let rect = self.tile_rect(i);
                f.stop(
                    p,
                    Stop {
                        key: self.key(self.elem_of(i)),
                        rect,
                        rest_rect: rect,
                        clip: Rect::FULL,
                        hover: Hover::Focus,
                        activate: Activate::Direct,
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
}

impl LogicalState for JfUsersScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.people.len() as u32);
        c.option(self.active, |c, i| {
            c.u32(i as u32);
        });
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("jf-users users={} active={:?}", self.people.len(), self.active));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest host the screen's effects can be stepped on.
    struct TestHost;
    impl nj_machine::machine::Host for TestHost {
        type Arg = super::super::family::SettingsPage;
        type Fx = AppFx;
        type Msg = crate::screens::registry::AppMsg;
        type Elem = u32;
        type Views<'a> = crate::auth::SessionRead<'a>;
        type Init = super::super::family::NoInit;
        type Memory = ();
    }

    fn screen(names: &[&str], active: Option<usize>) -> JfUsersScreen {
        let mut s = JfUsersScreen::new(EntryId(3));
        s.people = names.iter().map(|n| Person { id: format!("id-{n}"), name: n.to_string() }).collect();
        s.active = active;
        s
    }

    fn requests(s: &mut JfUsersScreen, f: impl FnOnce(&mut JfUsersScreen, &mut Effects<'_, TestHost>)) -> Vec<LoopReq> {
        let mut out = Vec::new();
        let mut present = nj_machine::present::Present::new();
        let mut fx = Effects::new(&mut out, nj_machine::machine::MachineId::Session, &mut present);
        f(s, &mut fx);
        drop(fx);
        out.into_iter()
            .filter_map(|e| match e.fx {
                Fx::App(AppFx::Loop(req)) => Some(req),
                _ => None,
            })
            .collect()
    }

    /// Focus starts on the user signed in underneath; with nobody signed in, on the first person;
    /// with nobody kept at all, on *Add user*.
    #[test]
    fn focus_starts_on_the_signed_in_user() {
        assert_eq!(screen(&["Alex", "Sam", "Kids"], Some(1)).first(), 1);
        assert_eq!(screen(&["Alex", "Sam"], None).first(), 0);
        assert_eq!(screen(&[], None).first(), ADD);
    }

    /// One row is centred on the screen, *Add user* last; a full roster stands in two compact rows,
    /// every circle inside the safe area and clear of the title and the server line; DOWN reaches
    /// the second row from any column, landing on the nearest tile of the shorter row.
    #[test]
    fn tiles_centre_in_rows_and_focus_walks_them() {
        let s = screen(&["Alex", "Sam", "Kids"], Some(0));
        assert_eq!(s.grid(), FULL);
        let (first, add) = (s.tile_rect(0), s.tile_rect(3));
        assert!(((first.x + add.x + add.w) * 0.5 - SCR_W * 0.5).abs() < 0.5, "the row is centred");
        assert_eq!(first.y, add.y);
        assert_eq!(s.elem_of(3), ADD);

        let s = screen(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"], None);
        assert_eq!(s.tiles(), TILES, "the most a roster keeps, and Add user");
        for r in (0..s.tiles()).map(|i| s.tile_rect(i)) {
            assert!(crate::ui::consts::inside_safe(r), "{r:?} leaves the safe area");
        }
        let last = s.tile_rect(TILES - 1);
        assert!(last.y + last.h + NAME_GAP + NAME_H < SCR_H - crate::ui::consts::SAFE.y - FOOTER_H,
            "the last row's names clear the server line");
        assert!(s.tile_rect(0).y > TITLE_Y + TITLE_H, "the first row clears the title");

        let s = screen(&["a", "b", "c", "d", "e", "f", "g", "h"], None);
        assert_eq!(s.grid(), COMPACT);
        assert!(s.tile_rect(7).y > s.tile_rect(0).y, "a second row");
        assert_eq!(s.step_from(6, Dir::Down), Some(8), "the nearest tile of the shorter row");
        assert_eq!(s.step_from(7, Dir::Up), Some(0));
        assert_eq!(s.step_from(6, Dir::Right), None, "a row ends at its last tile");
        assert_eq!(s.step_from(0, Dir::Left), None);
        assert_eq!(s.step_from(8, Dir::Down), None);
    }

    /// OK on a person asks the loop for that position; OK on *Add user* asks for the add flow;
    /// BACK resumes the user signed in underneath, or hands the screen back when nobody is.
    #[test]
    fn picks_and_back_are_requests_the_loop_performs() {
        let mut s = screen(&["Alex", "Sam"], Some(0));
        assert_eq!(requests(&mut s, |s, fx| s.activate(1, fx)), [LoopReq::PickJellyfinUser(1)]);
        assert_eq!(requests(&mut s, |s, fx| s.activate(ADD, fx)), [LoopReq::AccountAddUser]);
        assert_eq!(requests(&mut s, |s, fx| s.activate(7, fx)), [], "no such person");
        assert_eq!(requests(&mut s, |s, fx| s.back(fx)), [LoopReq::PickJellyfinUser(0)]);
        s.active = None;
        assert_eq!(requests(&mut s, |s, fx| s.back(fx)), [LoopReq::BackAtRoot]);
    }
}
