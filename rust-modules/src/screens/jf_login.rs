//! Jellyfin sign-in: the server's address first, then a username and password or a Quick Connect
//! code approved from a device that is already signed in.
//!
//! Every network step runs on the app's workers (`AppFx::JfAuth`); its answer comes back on a
//! channel this instance owns and drains on `Tick`, so a screen that is torn down mid-request simply
//! drops the receiver and the late answer goes nowhere. A successful sign-in is handed back as
//! `JfAuthCmd::Adopt`, and the app installs the server and leaves this route.
//!
//! The password never reaches the logical state, the probe or the log: the canon carries field
//! lengths only.
//!
//! **Layout** (approved redesign, 2026-10-09): two columns, each vertically centred on the screen.
//! The left one is the narrative — a step eyebrow, a title that names the step, its explanation
//! (on Quick Connect, three numbered instructions) and, on the first two steps, a two-row
//! *Server → Sign in* tracker. The right one is ONE panel on the shared panel ground
//! (`widgets::panel_ground`) holding the step's form; every control in it is full width except
//! the server card's *Change server* pill and Quick Connect's centred pair. Focus walks the panel
//! top to bottom ([`JfLoginScreen::order`]); Quick Connect's pair is a row.

use std::ffi::{CStr, CString};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use crate::jf::auth::{AuthError, QuickConnect, SignedIn};
use crate::catalog::Origin;
use crate::ui::frame::Budget;
use crate::ui::label::{HAlign, Label, VAlign};
use crate::ui::route_screen::{RouteGround, RouteLayout};
use crate::ui::screen::{
    Activate, At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Enter, FocusSource, FocusTarget,
    Focusable, GroupKind, GroupSpec, HitSource, Hover, Placed, RenderStrategy, Screen, ScreenEvent,
    Seat, Step, Stop,
};
use crate::ui::text_buffer::TextBuffer;
use crate::ui::text_view::TextView;
use crate::ui::widgets::{Button, CtlPop, Spinner, CONTROL_GAP};
use crate::ui::{theme, Env, Painter, Rect, View};
use nj_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind,
    InputOwner, InstanceId, Key, LogicalState, Machine, MachineId, Measure, TextEdit, Tick,
};
use nj_machine::present::{PresentEvent, Provenance};
use nj_platform::i18n::msg;

use super::registry::{word, AppFx, AppLike, JfAuthCmd, JfAuthReply, LoopReq};

pub(crate) const SHAPE: &str = "JfLoginScreen{entry:u32,instance:u32,step:server|credentials|quick_connect,server_len:u32,user_len:u32,pass_len:u32,editing:Option<u32>,busy:Option<connecting|signing_in|starting|adopting>,waiting:bool,error:bool,origin:bool}";

const SERVER: u32 = 1;
const CONNECT: u32 = 2;
const USER: u32 = 3;
const PASS: u32 = 4;
const SIGN_IN: u32 = 5;
const QUICK: u32 = 6;
const CHANGE: u32 = 7;
const USE_PASSWORD: u32 = 8;
/// `GroupId(0)` is the container's default fresh-mount target, so the first frame seats with no
/// correction of its own.
const GROUP: GroupId = GroupId(0);

/// Quick Connect is polled, not pushed; the web client asks every five seconds, a television that
/// is being looked at can afford twice that.
const POLL_MS: u32 = 2_500;
/// The *Recent* row: the server this television is still signed in to, one press from reconnecting.
const RECENT: u32 = 9;
const LABEL_H: f32 = 32.0;
const FIELD_H: f32 = 84.0;
const FIELD_R: f32 = 20.0;
const FIELD_PAD: f32 = 28.0;
/// The panel the form stands in: its corner and its inner padding.
const PANEL_R: f32 = 28.0;
const PANEL_PAD: f32 = 48.0;
/// A full-width primary control (Connect, Sign in, Quick Connect).
const WIDE_H: f32 = 76.0;
/// The server card on the sign-in step: a disc, the name and the address, and *Change server*.
const CARD_H: f32 = 72.0;
const CARD_DISC: f32 = 56.0;
const CHANGE_H: f32 = 56.0;
/// The *Recent* row and its disc.
const RECENT_H: f32 = 96.0;
const RECENT_DISC: f32 = 52.0;
/// The status band (spinner line, or the error box): reserved whether or not it is drawn, so a
/// landing answer never moves a control the viewer is standing on.
const STATUS_H: f32 = 128.0;
/// Quick Connect's code: one cell per character.
const CODE_CELL_W: f32 = 96.0;
const CODE_CELL_H: f32 = 128.0;
const CODE_GAP: f32 = 14.0;
const CODE_R: f32 = 18.0;
/// The QC step's pair of buttons.
const PAIR_H: f32 = 68.0;
/// The narrative's step tracker.
const STEP_DISC: f32 = 44.0;
const STEP_ROW_H: f32 = 76.0;
const EYEBROW_H: f32 = 30.0;
const FIELD_SZ: i32 = theme::size::HEADLINE;
const BUTTON_SZ: i32 = theme::size::BODY;
const CARET_W: f32 = 3.0;
const POP_MS: u32 = 450;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Server,
    Credentials,
    QuickConnect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Busy {
    Connecting,
    SigningIn,
    Starting,
    Adopting,
}

/// The server this television is still signed in to, offered as the *Recent* row.
#[derive(Clone, Debug, PartialEq)]
struct Recent {
    name: String,
    /// What the address field would hold: `host:port`.
    address: String,
}

/// Every rect the panel draws, in screen space.
struct Form {
    panel: Rect,
    card: Option<Rect>,
    labels: Vec<(u32, Rect)>,
    fields: Vec<(u32, Rect)>,
    hint: Option<Rect>,
    code: Option<Rect>,
    code_caption: Option<Rect>,
    status: Rect,
    or_rule: Option<Rect>,
    recent_caption: Option<Rect>,
    buttons: Vec<(u32, Rect)>,
}

pub(crate) struct JfLoginScreen {
    entry: EntryId,
    instance: InstanceId,
    stage: Stage,
    server: TextBuffer,
    user: TextBuffer,
    pass: TextBuffer,
    editing: Option<u32>,
    busy: Option<Busy>,
    rx: Option<Receiver<JfAuthReply>>,
    error: Option<String>,
    origin: Option<Origin>,
    server_name: String,
    qc: Option<QuickConnect>,
    next_poll_ms: Option<u32>,
    now_ms: u32,
    pop_until_ms: u32,
    spin: nj_machine::motion::Phase,
    spin_ms: f32,
    pop: CtlPop<3>,
    ground: RouteGround,
    recent: Option<Recent>,
}

impl JfLoginScreen {
    pub(crate) fn new(entry: EntryId, instance: InstanceId) -> Self {
        // Still signed in means the saved server failed to come up: offer it again, as the
        // *Recent* row, one press from reconnecting; the field stays free for another address.
        let recent = crate::jf::store::current().and_then(|stored| {
            let origin = stored.origin()?;
            let host = origin.host();
            let address =
                if host.contains(':') { format!("[{host}]:{}", origin.port()) } else { format!("{host}:{}", origin.port()) };
            let name = if stored.server_name.trim().is_empty() { address.clone() } else { stored.server_name.clone() };
            Some(Recent { name, address })
        });
        let server = String::new();
        let caret = 0;
        Self {
            entry,
            instance,
            stage: Stage::Server,
            server: TextBuffer::new(server, caret),
            user: TextBuffer::new(String::new(), 0),
            pass: TextBuffer::new(String::new(), 0),
            editing: None,
            busy: None,
            rx: None,
            error: None,
            origin: None,
            server_name: String::new(),
            qc: None,
            next_poll_ms: None,
            now_ms: 0,
            pop_until_ms: 0,
            spin: nj_machine::motion::Phase::default(),
            spin_ms: 0.0,
            pop: CtlPop::new(),
            ground: RouteGround::new(),
            recent,
        }
    }

    fn key(&self, elem: u32) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem }
    }

    fn fields(&self) -> &'static [u32] {
        match self.stage {
            Stage::Server => &[SERVER],
            Stage::Credentials => &[USER, PASS],
            Stage::QuickConnect => &[],
        }
    }

    fn buttons(&self) -> &'static [u32] {
        match self.stage {
            Stage::Server => &[CONNECT, RECENT],
            Stage::Credentials => &[SIGN_IN, QUICK, CHANGE],
            Stage::QuickConnect => &[USE_PASSWORD, CHANGE],
        }
    }

    /// Every focusable element, in the order focus walks it: down the panel, or along Quick
    /// Connect's row.
    fn order(&self) -> Vec<u32> {
        match self.stage {
            Stage::Server => {
                let mut v = vec![SERVER, CONNECT];
                if self.recent.is_some() {
                    v.push(RECENT);
                }
                v
            }
            Stage::Credentials => vec![CHANGE, USER, PASS, SIGN_IN, QUICK],
            Stage::QuickConnect => vec![USE_PASSWORD, CHANGE],
        }
    }

    /// Quick Connect's two buttons are a row; every other step is a column.
    fn is_row(&self) -> bool {
        self.stage == Stage::QuickConnect
    }

    fn elems(&self) -> impl Iterator<Item = u32> {
        self.order().into_iter()
    }

    /// Where a step seats focus: the Recent server when there is one (one press to reconnect), the
    /// username, or the way back to the password.
    fn first(&self) -> u32 {
        match self.stage {
            Stage::Server if self.recent.is_some() => RECENT,
            Stage::Server => SERVER,
            Stage::Credentials => USER,
            Stage::QuickConnect => USE_PASSWORD,
        }
    }

    fn buffer(&self, elem: u32) -> &TextBuffer {
        match elem {
            USER => &self.user,
            PASS => &self.pass,
            _ => &self.server,
        }
    }

    fn buffer_mut(&mut self, elem: u32) -> &mut TextBuffer {
        match elem {
            USER => &mut self.user,
            PASS => &mut self.pass,
            _ => &mut self.server,
        }
    }

    fn waiting(&self) -> bool {
        self.qc.is_some() && self.error.is_none()
    }

    fn spinning(&self) -> bool {
        self.busy.is_some() || self.waiting()
    }

    fn reseat<H: AppLike>(&self, elem: u32, fx: &mut Effects<'_, H>) {
        let me = fx.from();
        fx.push(Fx::Deliver(
            me,
            Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::Elem(self.key(elem)) })),
        ));
    }

    fn keyboard<H: AppLike>(&self, up: bool, fx: &mut Effects<'_, H>) {
        fx.push(Fx::Deliver(MachineId::Instance(self.instance), Delivery::Keyboard { up }));
        fx.invalidate(Provenance::Input);
    }

    fn open<H: AppLike>(&mut self, field: u32, fx: &mut Effects<'_, H>) {
        if self.busy.is_some() {
            return;
        }
        let buf = self.buffer_mut(field);
        *buf = TextBuffer::new(buf.text().to_owned(), usize::MAX);
        let was_up = self.editing.is_some();
        self.editing = Some(field);
        if !was_up {
            self.keyboard(true, fx);
        }
    }

    fn close<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        if self.editing.take().is_some() {
            self.keyboard(false, fx);
        }
    }

    fn edit<H: AppLike>(&mut self, field: u32, edit: &TextEdit, fx: &mut Effects<'_, H>) {
        self.buffer_mut(field).edit(edit);
        if self.busy.is_none() {
            self.error = None;
        }
        fx.invalidate(Provenance::Input);
    }

    fn go<H: AppLike>(&mut self, stage: Stage, fx: &mut Effects<'_, H>) {
        self.close(fx);
        self.stage = stage;
        self.rx = None;
        self.busy = None;
        self.qc = None;
        self.next_poll_ms = None;
        self.error = None;
        if stage == Stage::Server {
            self.origin = None;
        }
        self.reseat(self.first(), fx);
        fx.invalidate(Provenance::Input);
    }

    fn request<H: AppLike>(
        &mut self,
        busy: Busy,
        fx: &mut Effects<'_, H>,
        command: impl FnOnce(Sender<JfAuthReply>) -> JfAuthCmd,
    ) {
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.busy = Some(busy);
        self.error = None;
        fx.push(Fx::App(AppFx::JfAuth(command(tx))));
        fx.invalidate(Provenance::Input);
    }

    fn connect<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        let candidates = crate::jf::address::candidates(self.server.text());
        if candidates.is_empty() {
            self.error = Some(msg::jellyfin_login_error_address().to_owned());
            fx.invalidate(Provenance::Input);
            return;
        }
        self.request(Busy::Connecting, fx, |reply| JfAuthCmd::Probe { candidates, reply });
    }

    fn sign_in<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        let Some(origin) = self.origin.clone() else { return };
        let username = self.user.text().trim().to_owned();
        if username.is_empty() {
            self.reseat(USER, fx);
            self.open(USER, fx);
            return;
        }
        let password = self.pass.text().to_owned();
        self.request(Busy::SigningIn, fx, |reply| JfAuthCmd::Password { origin, username, password, reply });
    }

    fn start_quick_connect<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        let Some(origin) = self.origin.clone() else { return };
        self.go(Stage::QuickConnect, fx);
        self.request(Busy::Starting, fx, |reply| JfAuthCmd::QuickConnectStart { origin, reply });
    }

    fn poll<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        let (Some(origin), Some(qc)) = (self.origin.clone(), self.qc.clone()) else { return };
        self.next_poll_ms = None;
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        fx.push(Fx::App(AppFx::JfAuth(JfAuthCmd::QuickConnectPoll { origin, qc, reply: tx })));
    }

    fn adopt<H: AppLike>(&mut self, signed_in: SignedIn, fx: &mut Effects<'_, H>) {
        let Some(origin) = self.origin.clone() else { return };
        self.pass = TextBuffer::new(String::new(), 0);
        self.qc = None;
        self.next_poll_ms = None;
        self.busy = Some(Busy::Adopting);
        fx.push(Fx::App(AppFx::JfAuth(JfAuthCmd::Adopt { origin, signed_in })));
    }

    fn fail(&mut self, error: AuthError) {
        self.error = Some(message(&error));
        self.busy = None;
        self.qc = None;
        self.next_poll_ms = None;
    }

    fn drain<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        let Some(rx) = &self.rx else { return };
        let reply = match rx.try_recv() {
            Ok(reply) => reply,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                self.fail(AuthError::Unreachable);
                fx.invalidate(Provenance::Input);
                return;
            }
        };
        self.rx = None;
        match reply {
            JfAuthReply::Probed(Ok((origin, info))) => {
                self.server_name = if info.server_name.trim().is_empty() {
                    origin.host().to_owned()
                } else {
                    info.server_name
                };
                self.go(Stage::Credentials, fx);
                self.origin = Some(origin);
            }
            JfAuthReply::SignedIn(Ok(signed_in)) | JfAuthReply::Polled(Ok(Some(signed_in))) => {
                self.adopt(signed_in, fx);
            }
            JfAuthReply::QuickConnect(Ok(qc)) => {
                self.busy = None;
                self.qc = Some(qc);
                self.next_poll_ms = Some(self.now_ms.wrapping_add(POLL_MS));
            }
            JfAuthReply::Polled(Ok(None)) => {
                self.next_poll_ms = Some(self.now_ms.wrapping_add(POLL_MS));
            }
            JfAuthReply::Probed(Err(e))
            | JfAuthReply::SignedIn(Err(e))
            | JfAuthReply::QuickConnect(Err(e))
            | JfAuthReply::Polled(Err(e)) => self.fail(e),
        }
        fx.invalidate(Provenance::Input);
    }

    fn tick<H: AppLike>(&mut self, t: Tick, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) {
        self.now_ms = t.ms;
        self.drain(fx);
        if self.rx.is_none() && self.next_poll_ms.is_some_and(|due| t.ms.wrapping_sub(due) < u32::MAX / 2) {
            self.poll(fx);
        }
        if self.spinning() {
            self.spin_ms = self.spin.advance(t, &mut fx.present());
        }
        if t.ms.wrapping_sub(self.pop_until_ms) > u32::MAX / 2 {
            fx.present().note(PresentEvent::Motion);
        }
        let focused = cx
            .focus
            .current
            .filter(|k| k.entry == self.entry)
            .and_then(|k| self.buttons().iter().position(|&b| b == k.elem));
        self.pop.step(focused, t.dt());
    }

    fn activate<H: AppLike>(&mut self, elem: u32, fx: &mut Effects<'_, H>) {
        if !self.elems().any(|e| e == elem) {
            return;
        }
        match elem {
            CHANGE => return self.go(Stage::Server, fx),
            USE_PASSWORD => {
                let origin = self.origin.clone();
                self.go(Stage::Credentials, fx);
                self.origin = origin;
                return;
            }
            _ => {}
        }
        if self.busy.is_some() {
            return;
        }
        match elem {
            SERVER | USER | PASS => self.open(elem, fx),
            CONNECT => self.connect(fx),
            RECENT => {
                if let Some(recent) = &self.recent {
                    let address = recent.address.clone();
                    let caret = address.len();
                    self.server = TextBuffer::new(address, caret);
                    self.connect(fx);
                }
            }
            SIGN_IN => self.sign_in(fx),
            QUICK => self.start_quick_connect(fx),
            _ => {}
        }
    }

    /// OK on the keyboard moves the form on: the address connects, the username hands over to the
    /// password, and the password signs in.
    fn submit<H: AppLike>(&mut self, field: u32, fx: &mut Effects<'_, H>) {
        match field {
            USER => {
                self.editing = Some(PASS);
                self.pass = TextBuffer::new(self.pass.text().to_owned(), usize::MAX);
                self.reseat(PASS, fx);
                fx.invalidate(Provenance::Input);
            }
            SERVER => {
                self.close(fx);
                self.connect(fx);
            }
            _ => {
                self.close(fx);
                self.sign_in(fx);
            }
        }
    }

    fn back<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        match self.stage {
            Stage::Server if self.busy.is_some() => {
                self.rx = None;
                self.busy = None;
                fx.invalidate(Provenance::Input);
            }
            Stage::Server => fx.push(Fx::App(AppFx::Loop(LoopReq::BackAtRoot))),
            Stage::Credentials => self.go(Stage::Server, fx),
            Stage::QuickConnect => {
                let origin = self.origin.clone();
                self.go(Stage::Credentials, fx);
                self.origin = origin;
            }
        }
    }

    /// The panel's rects for this step. Built top-down from 0, then the whole panel is centred
    /// vertically on the screen and every rect moved with it.
    fn form(&self, measure: &dyn Measure) -> Form {
        let layout = RouteLayout::screen();
        let px = layout.content.x;
        let pw = layout.content.w;
        let x = px + PANEL_PAD;
        let w = pw - 2.0 * PANEL_PAD;
        let mut y = PANEL_PAD;
        let mut form = Form {
            panel: Rect::new(px, 0.0, pw, 0.0),
            card: None,
            labels: Vec::new(),
            fields: Vec::new(),
            hint: None,
            code: None,
            code_caption: None,
            status: Rect::new(x, 0.0, w, STATUS_H),
            or_rule: None,
            recent_caption: None,
            buttons: Vec::new(),
        };
        let field = |form: &mut Form, y: &mut f32, field: u32| {
            form.labels.push((field, Rect::new(x, *y, w, LABEL_H)));
            *y += LABEL_H + theme::space::XS;
            form.fields.push((field, Rect::new(x, *y, w, FIELD_H)));
            *y += FIELD_H;
        };
        match self.stage {
            Stage::Server => {
                field(&mut form, &mut y, SERVER);
                y += theme::space::XS;
                form.hint = Some(Rect::new(x, y, w, LABEL_H));
                y += LABEL_H + theme::space::XS;
                form.status.y = y;
                y += STATUS_H;
                form.buttons.push((CONNECT, Rect::new(x, y, w, WIDE_H)));
                y += WIDE_H;
                if self.recent.is_some() {
                    y += theme::space::LG;
                    form.or_rule = Some(Rect::new(x, y, w, 1.0));
                    y += 1.0 + theme::space::MD;
                    form.recent_caption = Some(Rect::new(x, y, w, LABEL_H));
                    y += LABEL_H + theme::space::SM;
                    form.buttons.push((RECENT, Rect::new(x, y, w, RECENT_H)));
                    y += RECENT_H;
                }
            }
            Stage::Credentials => {
                let card = Rect::new(x, y, w, CARD_H);
                form.card = Some(card);
                let cw = Button::pill_w_measured(button_label(CHANGE), BUTTON_SZ, false, false, measure);
                form.buttons.push((CHANGE, Rect::new(x + w - cw, card.cy() - CHANGE_H * 0.5, cw, CHANGE_H)));
                y += CARD_H + theme::space::MD;
                form.or_rule = Some(Rect::new(x, y, w, 1.0));
                y += 1.0 + theme::space::MD;
                field(&mut form, &mut y, USER);
                y += theme::space::MD;
                field(&mut form, &mut y, PASS);
                y += theme::space::XS;
                form.status.y = y;
                y += STATUS_H;
                form.buttons.push((SIGN_IN, Rect::new(x, y, w, WIDE_H)));
                y += WIDE_H + theme::space::MD;
                form.recent_caption = Some(Rect::new(x, y, w, LABEL_H));
                y += LABEL_H + theme::space::MD;
                form.buttons.push((QUICK, Rect::new(x, y, w, WIDE_H)));
                y += WIDE_H;
            }
            Stage::QuickConnect => {
                form.card = Some(Rect::new(x, y, w, LABEL_H));
                y += LABEL_H + theme::space::LG;
                form.code_caption = Some(Rect::new(x, y, w, LABEL_H));
                y += LABEL_H + theme::space::SM;
                form.code = Some(Rect::new(x, y, w, CODE_CELL_H));
                y += CODE_CELL_H + theme::space::SM;
                form.status.y = y;
                y += STATUS_H;
                let ws: Vec<f32> = [USE_PASSWORD, CHANGE]
                    .iter()
                    .map(|&b| Button::pill_w_measured(button_label(b), BUTTON_SZ, false, false, measure))
                    .collect();
                let total = ws.iter().sum::<f32>() + CONTROL_GAP;
                let mut bx = x + ((w - total) * 0.5).max(0.0);
                for (&b, bw) in [USE_PASSWORD, CHANGE].iter().zip(ws) {
                    form.buttons.push((b, Rect::new(bx, y, bw, PAIR_H)));
                    bx += bw + CONTROL_GAP;
                }
                y += PAIR_H;
            }
        }
        let h = y + PANEL_PAD;
        let safe = crate::ui::consts::SAFE;
        let top = (safe.y + (safe.h - h) * 0.5).max(layout.content.y.min(safe.y));
        form.panel = Rect::new(px, top, pw, h);
        let mv = |r: &mut Rect| r.y += top;
        form.card.as_mut().map(mv);
        form.hint.as_mut().map(mv);
        form.code.as_mut().map(mv);
        form.code_caption.as_mut().map(mv);
        form.or_rule.as_mut().map(mv);
        form.recent_caption.as_mut().map(mv);
        mv(&mut form.status);
        for (_, r) in form.labels.iter_mut().chain(form.fields.iter_mut()).chain(form.buttons.iter_mut()) {
            mv(r);
        }
        form
    }

    fn elem_rect(&self, elem: u32, measure: &dyn Measure) -> Option<Rect> {
        let form = self.form(measure);
        form.fields
            .iter()
            .chain(form.buttons.iter())
            .find(|(e, _)| *e == elem)
            .map(|(_, r)| *r)
    }

    /// Which field an error is about, for its red rim: the address, or the password.
    fn faulted(&self, field: u32) -> bool {
        self.error.is_some()
            && self.busy.is_none()
            && match self.stage {
                Stage::Server => field == SERVER,
                Stage::Credentials => field == PASS,
                Stage::QuickConnect => false,
            }
    }

    fn draw_field(&self, p: Painter, field: u32, rect: Rect, focused: bool, measure: &dyn Measure) {
        let editing = self.editing == Some(field);
        let fill = if focused { theme::with_a(theme::TEXT_PRIMARY, 0.14) } else { theme::CONTROL_IDLE_FILL };
        p.rrect(rect, FIELD_R, FIELD_R, fill);
        if focused {
            p.rring(rect, FIELD_R, 3.0, theme::ACCENT);
        } else if self.faulted(field) {
            p.rring(rect, FIELD_R, 2.0, theme::DANGER);
        } else {
            p.rring(rect, FIELD_R, 1.5, theme::HAIRLINE);
        }
        let inner = Rect::new(rect.x + FIELD_PAD, rect.y, rect.w - 2.0 * FIELD_PAD, rect.h);
        let buf = self.buffer(field);
        let (shown, head) = if field == PASS {
            let mask = |s: &str| "\u{2022}".repeat(s.chars().count());
            (mask(buf.text()), mask(&buf.text()[..buf.caret()]))
        } else {
            (buf.text().to_owned(), buf.text()[..buf.caret()].to_owned())
        };
        let ink = if editing { theme::FIELD_EDITING_INK } else { theme::TEXT_PRIMARY };
        let mut caret_x = inner.x;
        if shown.is_empty() {
            if field == SERVER && !editing {
                Label::new(msg::jellyfin_login_server_hint_c().as_ptr(), FIELD_SZ, theme::TEXT_TERTIARY)
                    .draw(p, inner);
            }
        } else {
            let full_w = width(&shown, measure);
            let room = inner.w - if editing { CARET_W + 4.0 } else { 0.0 };
            let visible = tail_fit(&shown, room, measure);
            let dropped = full_w - width(visible, measure);
            let text = cstring(visible);
            Label::new(text.as_ptr(), FIELD_SZ, ink).draw(p, inner);
            caret_x = (inner.x + width(&head, measure) - dropped).clamp(inner.x, inner.x + inner.w - CARET_W);
        }
        if editing {
            let cap = measure.cap_h(FIELD_SZ);
            let h = cap * 1.7;
            p.rrect(Rect::new(caret_x + 2.0, rect.y + (rect.h - h) * 0.5, CARET_W, h), 1.5, 1.5, theme::ACCENT);
        }
    }

    /// One inset cell per character of the Quick Connect code, centred in `rect`.
    fn draw_code(&self, p: Painter, rect: Rect) {
        let Some(qc) = &self.qc else {
            return;
        };
        let n = qc.code.chars().count();
        if n == 0 {
            return;
        }
        let total = n as f32 * CODE_CELL_W + (n - 1) as f32 * CODE_GAP;
        let cell_w = if total > rect.w { (rect.w - (n - 1) as f32 * CODE_GAP) / n as f32 } else { CODE_CELL_W };
        let total = n as f32 * cell_w + (n - 1) as f32 * CODE_GAP;
        let x0 = rect.x + (rect.w - total) * 0.5;
        for (i, ch) in qc.code.chars().enumerate() {
            let cell = Rect::new(x0 + i as f32 * (cell_w + CODE_GAP), rect.y, cell_w, rect.h);
            p.rrect(cell, CODE_R, CODE_R, theme::scrim(0.40));
            p.rring(cell, CODE_R, 1.0, theme::HAIRLINE);
            let glyph = cstring(&ch.to_string());
            Label::new(glyph.as_ptr(), theme::size::HERO, theme::TEXT_PRIMARY)
                .bold()
                .h(HAlign::Center)
                .v(VAlign::Middle)
                .draw(p, cell);
        }
    }

    fn draw_status(&self, p: Painter, rect: Rect, centred: bool, measure: &dyn Measure) {
        if self.spinning() {
            let line = match self.busy {
                Some(Busy::Connecting) => msg::jellyfin_login_connecting_c(),
                Some(Busy::SigningIn | Busy::Adopting) => msg::jellyfin_login_signing_in_c(),
                Some(Busy::Starting) => msg::jellyfin_login_connecting_c(),
                None => msg::jellyfin_login_waiting_c(),
            };
            let gutter = Spinner::inline_gutter();
            let text_w = measure.width(line, theme::size::BODY, false);
            let x = if centred { rect.x + ((rect.w - gutter - text_w) * 0.5).max(0.0) } else { rect.x };
            let row = Rect::new(x, rect.y, rect.w - (x - rect.x), rect.h);
            Spinner::leading(row.x, row.cy())
                .phase(self.spin_ms as u32)
                .tint(theme::TEXT_SECONDARY)
                .draw(&Env::inert(), p);
            Label::new(line.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY)
                .v(VAlign::Middle)
                .draw(p, Rect::new(row.x + gutter, row.y, row.w - gutter, row.h));
        } else if let Some(error) = &self.error {
            // Sized to its sentence and hung from the top of the band, so the band's remainder is
            // always air between the box and the control below it — a focused control's pop
            // included.
            let mark = 28.0;
            let pad = theme::space::SM;
            let mx = rect.x + pad;
            let tx = mx + mark + pad;
            let tw = rect.x + rect.w - pad - tx;
            let view = TextView::new(error, theme::size::LABEL, theme::TEXT_PRIMARY)
                .with_measure(measure)
                .max_lines(2);
            let th = view.measure_h(tw);
            let boxed = Rect::new(rect.x, rect.y + theme::space::XS, rect.w, th + 2.0 * pad);
            p.rrect(boxed, 16.0, 16.0, theme::with_a(theme::DANGER, theme::DANGER_IDLE_TINT));
            crate::ui::icons::draw(
                p,
                crate::ui::icons::Icon::Alert,
                Rect::new(mx, boxed.cy() - mark * 0.5, mark, mark),
                theme::DANGER,
            );
            view.draw(p, Rect::new(tx, boxed.y + pad, tw, th));
        }
    }

    /// A disc holding a mark: the server card's, the Recent row's and the step tracker's.
    fn disc(p: Painter, center_x: f32, center_y: f32, d: f32, fill: [f32; 4]) -> Rect {
        let r = Rect::new(center_x - d * 0.5, center_y - d * 0.5, d, d);
        p.rrect(r, d * 0.5, d * 0.5, fill);
        r
    }

    /// The server card: the server's mark, its name and its address, *Change server* beside them.
    fn draw_card(&self, p: Painter, rect: Rect, change_x: f32) {
        let disc = Self::disc(p, rect.x + CARD_DISC * 0.5, rect.cy(), CARD_DISC, theme::CONTROL_IDLE_FILL_UNKEYED);
        let mark = CARD_DISC * 0.5;
        crate::ui::icons::draw(
            p,
            crate::ui::icons::Icon::Server,
            Rect::new(disc.cx() - mark * 0.5, disc.cy() - mark * 0.5, mark, mark),
            theme::TEXT_PRIMARY,
        );
        let tx = disc.x + disc.w + theme::space::MD;
        let tw = (change_x - theme::space::MD - tx).max(0.0);
        TextView::new(&self.server_name, theme::size::BODY, theme::TEXT_PRIMARY)
            .bold()
            .max_lines(1)
            .draw(p, Rect::new(tx, rect.y + 2.0, tw, rect.h * 0.5));
        if let Some(origin) = &self.origin {
            let address = format!("{}:{}", origin.host(), origin.port());
            TextView::new(&address, theme::size::CAPTION, theme::TEXT_TERTIARY)
                .max_lines(1)
                .draw(p, Rect::new(tx, rect.y + rect.h * 0.5 + 4.0, tw, rect.h * 0.5));
        }
    }

    /// Quick Connect's one-line server reminder, centred over the code.
    fn draw_server_line(&self, p: Painter, rect: Rect, measure: &dyn Measure) {
        let line = match &self.origin {
            Some(origin) => format!("{} \u{b7} {}:{}", self.server_name, origin.host(), origin.port()),
            None => self.server_name.clone(),
        };
        let mark = 26.0;
        let gap = theme::space::SM;
        let tw = measure.width_str(&line, theme::size::CAPTION, false).min(rect.w - mark - gap);
        let x = rect.x + ((rect.w - mark - gap - tw) * 0.5).max(0.0);
        crate::ui::icons::draw(
            p,
            crate::ui::icons::Icon::Server,
            Rect::new(x, rect.cy() - mark * 0.5, mark, mark),
            theme::TEXT_SECONDARY,
        );
        TextView::new(&line, theme::size::CAPTION, theme::TEXT_SECONDARY)
            .max_lines(1)
            .draw(p, Rect::new(x + mark + gap, rect.y + 2.0, tw + 1.0, rect.h));
    }

    /// The *Recent* row: a full-width row that wears the focus fill like a table row.
    fn draw_recent(&self, p: Painter, rect: Rect, focused: bool, scale: f32) {
        let Some(recent) = &self.recent else { return };
        let r = Rect::new(
            rect.cx() - rect.w * scale * 0.5,
            rect.cy() - rect.h * scale * 0.5,
            rect.w * scale,
            rect.h * scale,
        );
        let (fill, ink, sub) = if focused {
            (theme::ACCENT, theme::INK_ON_ACCENT, theme::with_a(theme::INK_ON_ACCENT, 0.62))
        } else {
            (theme::CONTROL_IDLE_FILL, theme::TEXT_PRIMARY, theme::TEXT_TERTIARY)
        };
        p.rrect(r, FIELD_R, FIELD_R, fill);
        let disc_fill = if focused { theme::with_a(theme::INK_ON_ACCENT, 0.10) } else { theme::CONTROL_IDLE_FILL_UNKEYED };
        let disc = Self::disc(p, r.x + theme::space::MD + RECENT_DISC * 0.5, r.cy(), RECENT_DISC, disc_fill);
        let mark = RECENT_DISC * 0.5;
        crate::ui::icons::draw(
            p,
            crate::ui::icons::Icon::Server,
            Rect::new(disc.cx() - mark * 0.5, disc.cy() - mark * 0.5, mark, mark),
            ink,
        );
        let tx = disc.x + disc.w + theme::space::MD;
        let tw = (r.x + r.w - theme::space::MD - tx).max(0.0);
        TextView::new(&recent.name, theme::size::BODY, ink)
            .bold()
            .max_lines(1)
            .draw(p, Rect::new(tx, r.y + 16.0, tw, r.h * 0.5));
        TextView::new(&recent.address, theme::size::CAPTION, sub)
            .max_lines(1)
            .draw(p, Rect::new(tx, r.y + r.h * 0.5 + 6.0, tw, r.h * 0.5));
    }

    /// A small upper-case heading in the panel ("RECENT", "YOUR CODE").
    fn draw_caption(p: Painter, text: &str, rect: Rect, centred: bool) {
        let up = cstring(&text.to_uppercase());
        let label = Label::new(up.as_ptr(), theme::size::MICRO, theme::TEXT_TERTIARY).bold().v(VAlign::Middle);
        if centred { label.h(HAlign::Center).draw(p, rect) } else { label.draw(p, rect) };
    }

    /// The thin rule between two parts of the panel; with a word, the word sits in a gap at its
    /// centre ("or").
    fn draw_rule(p: Painter, rect: Rect, word: Option<&CStr>, measure: &dyn Measure) {
        match word {
            None => p.rrect(Rect::new(rect.x, rect.y, rect.w, 1.0), 0.0, 0.0, theme::HAIRLINE),
            Some(word) => {
                let ww = measure.width(word, theme::size::CAPTION, false);
                let gap = theme::space::MD;
                let side = ((rect.w - ww) * 0.5 - gap).max(0.0);
                let y = rect.cy();
                p.rrect(Rect::new(rect.x, y, side, 1.0), 0.0, 0.0, theme::HAIRLINE);
                p.rrect(Rect::new(rect.x + rect.w - side, y, side, 1.0), 0.0, 0.0, theme::HAIRLINE);
                Label::new(word.as_ptr(), theme::size::CAPTION, theme::TEXT_TERTIARY)
                    .h(HAlign::Center)
                    .v(VAlign::Middle)
                    .draw(p, rect);
            }
        }
    }

    /// The narrative column, vertically centred: the step eyebrow, the title, then the copy (on
    /// Quick Connect, three numbered instructions) and, on the first two steps, the tracker.
    fn draw_narrative(&self, p: Painter, measure: &dyn Measure) {
        let layout = RouteLayout::screen();
        let x = layout.narrative.x;
        let w = layout.narrative.w;
        let eyebrow = match self.stage {
            Stage::Server => msg::jellyfin_login_step_server(),
            Stage::Credentials => msg::jellyfin_login_step_sign_in(),
            Stage::QuickConnect => msg::jellyfin_login_step_quick_connect(),
        };
        let title = match self.stage {
            Stage::Server => msg::jellyfin_login_title_server().to_owned(),
            Stage::Credentials if !self.server_name.trim().is_empty() => {
                msg::jellyfin_login_title_sign_in(&self.server_name)
            }
            Stage::Credentials => msg::jellyfin_login_title().to_owned(),
            Stage::QuickConnect => msg::jellyfin_login_title_quick_connect().to_owned(),
        };
        let title_view = RouteLayout::narrative_title(&title).with_measure(measure);
        let title_h = title_view.measure_h(w);
        let copy_size = theme::size::LABEL;
        let copy_lead = copy_size as f32 + theme::space::XS;
        let steps: [&str; 3] = [
            msg::jellyfin_login_qc_step_open(),
            msg::jellyfin_login_qc_step_profile(),
            msg::jellyfin_login_qc_step_code(),
        ];
        let step_text_x = STEP_DISC + theme::space::MD;
        let copy_h = match self.stage {
            Stage::QuickConnect => steps
                .iter()
                .map(|s| {
                    TextView::new(s, copy_size, theme::TEXT_READING)
                        .with_measure(measure)
                        .leading(copy_lead)
                        .measure_h(w - step_text_x)
                        .max(STEP_DISC)
                })
                .sum::<f32>()
                + 2.0 * theme::space::MD,
            _ => {
                let copy = match self.stage {
                    Stage::Server => msg::jellyfin_login_server_intro(),
                    _ => msg::jellyfin_login_credentials_intro(),
                };
                TextView::new(copy, copy_size, theme::TEXT_READING).with_measure(measure).leading(copy_lead).measure_h(w)
            }
        };
        let tracker_h = if self.stage == Stage::QuickConnect { 0.0 } else { theme::space::LG + 2.0 * STEP_ROW_H };
        let total = EYEBROW_H + theme::space::SM + title_h + theme::space::MD + copy_h + tracker_h;
        let safe = crate::ui::consts::SAFE;
        let mut y = (safe.y + (safe.h - total) * 0.5).max(layout.narrative.y);

        Self::draw_caption(p, eyebrow, Rect::new(x, y, w, EYEBROW_H), false);
        y += EYEBROW_H + theme::space::SM;
        title_view.draw(p, Rect::new(x, y, w, title_h));
        y += title_h + theme::space::MD;
        match self.stage {
            Stage::QuickConnect => {
                for (i, s) in steps.iter().enumerate() {
                    let view = TextView::new(s, copy_size, theme::TEXT_READING).with_measure(measure).leading(copy_lead);
                    let h = view.measure_h(w - step_text_x).max(STEP_DISC);
                    let disc = Self::disc(p, x + STEP_DISC * 0.5, y + STEP_DISC * 0.5, STEP_DISC, theme::CONTROL_IDLE_FILL_UNKEYED);
                    let n = cstring(&(i + 1).to_string());
                    Label::new(n.as_ptr(), theme::size::CAPTION, theme::TEXT_PRIMARY)
                        .bold()
                        .h(HAlign::Center)
                        .v(VAlign::Middle)
                        .draw(p, disc);
                    view.draw(p, Rect::new(x + step_text_x, y + 4.0, w - step_text_x, h));
                    y += h + theme::space::MD;
                }
            }
            _ => {
                let copy = match self.stage {
                    Stage::Server => msg::jellyfin_login_server_intro(),
                    _ => msg::jellyfin_login_credentials_intro(),
                };
                let view = TextView::new(copy, copy_size, theme::TEXT_READING).with_measure(measure).leading(copy_lead);
                view.draw(p, Rect::new(x, y, w, copy_h));
                y += copy_h + theme::space::LG;
                self.draw_tracker(p, x, y, w);
            }
        }
    }

    /// *Server → Sign in*: the current step a filled disc with its number, a finished one a tick
    /// with what it settled (the server's name and address), a step still to come an outline.
    fn draw_tracker(&self, p: Painter, x: f32, y: f32, w: f32) {
        let rows: [(&CStr, bool, bool); 2] = [
            (msg::jellyfin_login_server_row_c(), self.stage == Stage::Server, self.stage != Stage::Server),
            (msg::jellyfin_login_sign_in_c(), self.stage == Stage::Credentials, false),
        ];
        for (i, &(label, current, done)) in rows.iter().enumerate() {
            let row = Rect::new(x, y + i as f32 * STEP_ROW_H, w, STEP_ROW_H);
            let cx = x + STEP_DISC * 0.5;
            let tx = x + STEP_DISC + theme::space::MD;
            let tw = w - (tx - x);
            if current {
                let disc = Self::disc(p, cx, row.cy(), STEP_DISC, theme::ACCENT);
                let n = cstring(&(i + 1).to_string());
                Label::new(n.as_ptr(), theme::size::CAPTION, theme::INK_ON_ACCENT)
                    .bold()
                    .h(HAlign::Center)
                    .v(VAlign::Middle)
                    .draw(p, disc);
                Label::new(label.as_ptr(), theme::size::BODY, theme::TEXT_PRIMARY)
                    .bold()
                    .v(VAlign::Middle)
                    .draw(p, Rect::new(tx, row.y, tw, row.h));
            } else if done {
                let disc = Self::disc(p, cx, row.cy(), STEP_DISC, theme::CONTROL_IDLE_FILL_UNKEYED);
                let mark = STEP_DISC * 0.5;
                crate::ui::icons::draw(
                    p,
                    crate::ui::icons::Icon::Check,
                    Rect::new(disc.cx() - mark * 0.5, disc.cy() - mark * 0.5, mark, mark),
                    theme::TEXT_PRIMARY,
                );
                Label::new(label.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY)
                    .draw(p, Rect::new(tx, row.y + 2.0, tw, row.h * 0.5));
                let settled = match &self.origin {
                    Some(origin) => format!("{} \u{b7} {}:{}", self.server_name, origin.host(), origin.port()),
                    None => self.server_name.clone(),
                };
                TextView::new(&settled, theme::size::CAPTION, theme::TEXT_TERTIARY)
                    .max_lines(1)
                    .draw(p, Rect::new(tx, row.y + row.h * 0.5 + 2.0, tw, row.h * 0.5));
            } else {
                let disc = Rect::new(cx - STEP_DISC * 0.5, row.cy() - STEP_DISC * 0.5, STEP_DISC, STEP_DISC);
                p.rring(disc, STEP_DISC * 0.5, 2.0, theme::CONTROL_RIM_IDLE_UNKEYED);
                let n = cstring(&(i + 1).to_string());
                Label::new(n.as_ptr(), theme::size::CAPTION, theme::TEXT_TERTIARY)
                    .bold()
                    .h(HAlign::Center)
                    .v(VAlign::Middle)
                    .draw(p, disc);
                Label::new(label.as_ptr(), theme::size::BODY, theme::TEXT_TERTIARY)
                    .v(VAlign::Middle)
                    .draw(p, Rect::new(tx, row.y, tw, row.h));
            }
        }
    }

    fn stop<H: AppLike>(&self, f: &mut DrawFrame<'_, '_, H>, p: Painter, elem: u32, rect: Rect) {
        f.stop(
            p,
            Stop {
                key: self.key(elem),
                rect,
                rest_rect: rect,
                clip: Rect::FULL,
                hover: Hover::Focus,
                activate: Activate::Direct,
            },
        );
    }
}

fn button_label(button: u32) -> &'static CStr {
    match button {
        CONNECT => msg::jellyfin_login_connect_c(),
        SIGN_IN => msg::jellyfin_login_sign_in_c(),
        QUICK => msg::jellyfin_login_use_quick_connect_c(),
        USE_PASSWORD => msg::jellyfin_login_use_password_c(),
        _ => msg::jellyfin_login_change_server_c(),
    }
}

fn field_label(field: u32) -> &'static CStr {
    match field {
        SERVER => msg::jellyfin_login_server_label_c(),
        USER => msg::jellyfin_login_username_c(),
        PASS => msg::jellyfin_login_password_c(),
        _ => msg::jellyfin_login_code_label_c(),
    }
}

fn message(error: &AuthError) -> String {
    match error {
        AuthError::Unreachable => msg::jellyfin_login_error_unreachable().to_owned(),
        AuthError::NotJellyfin => msg::jellyfin_login_error_not_jellyfin().to_owned(),
        AuthError::NotSetUp => msg::jellyfin_login_error_not_set_up().to_owned(),
        AuthError::BadCredentials => msg::jellyfin_login_error_credentials().to_owned(),
        AuthError::QuickConnectDisabled => msg::jellyfin_login_error_quick_connect_off().to_owned(),
        AuthError::Refused(status) => msg::jellyfin_login_error_refused(*status as i64),
        AuthError::Malformed => msg::jellyfin_login_error_malformed().to_owned(),
    }
}

fn cstring(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

fn width(s: &str, measure: &dyn Measure) -> f32 {
    measure.width(&cstring(s), FIELD_SZ, false)
}

/// The longest tail of `s` that fits `room`: a field too narrow for its text shows the end being
/// typed, not the beginning.
fn tail_fit<'a>(s: &'a str, room: f32, measure: &dyn Measure) -> &'a str {
    if width(s, measure) <= room {
        return s;
    }
    s.char_indices()
        .map(|(i, _)| &s[i..])
        .find(|tail| width(tail, measure) <= room)
        .unwrap_or("")
}

impl<H: AppLike> Focusable<H> for JfLoginScreen {
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        let form = self.form(cx.measure);
        let extent = form
            .fields
            .iter()
            .chain(form.buttons.iter())
            .map(|(_, r)| *r)
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect::FULL);
        out.push(GroupSpec {
            id: GROUP,
            kind: GroupKind::Free,
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Stop; 4],
            extent,
            len: self.elems().count(),
            elem: ElemKind::Bare,
        });
    }
    fn group_of(&self, key: &u32, _cx: &Cx<'_, H>) -> Option<GroupId> {
        self.elems().any(|e| e == *key).then_some(GROUP)
    }
    /// The panel is a column: UP/DOWN walk [`JfLoginScreen::order`]. Quick Connect's two buttons
    /// are a row: LEFT/RIGHT walk them. Everything else is an edge.
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, _cx: &Cx<'_, H>) -> Step<u32> {
        let order = self.order();
        let Some(i) = order.iter().position(|&e| e == key.elem) else { return Step::Edge };
        let (back, ahead) = if self.is_row() { (Dir::Left, Dir::Right) } else { (Dir::Up, Dir::Down) };
        let next = if dir == back {
            i.checked_sub(1).map(|j| order[j])
        } else if dir == ahead {
            order.get(i + 1).copied()
        } else {
            None
        };
        next.map_or(Step::Edge, |elem| Step::Move(self.key(elem)))
    }
    fn place(&self, key: &u32, cx: &Cx<'_, H>, _at: At) -> Option<Placed> {
        let index = self.elems().position(|e| e == *key)?;
        let rect = self.elem_rect(*key, cx.measure)?;
        Some(Placed { rect, rest_rect: rect, clip: Rect::FULL, index: Some(index as u32) })
    }
    fn reconcile(&self, want: FocusKey<u32>, cx: &Cx<'_, H>) -> FocusKey<u32> {
        if self.group_of(&want.elem, cx).is_some() {
            want
        } else {
            self.key(self.first())
        }
    }
    fn seat(&self, _g: GroupId, _from: Placed, _cx: &Cx<'_, H>) -> FocusKey<u32> {
        self.key(self.first())
    }
}

impl<H: AppLike> Machine<H> for JfLoginScreen {
    type Ev = ScreenEvent<H>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        match ev {
            ScreenEvent::Mount => self.reseat(self.first(), fx),
            ScreenEvent::Unmount => {
                self.close(fx);
                self.rx = None;
            }
            ScreenEvent::WillLeave(_) | ScreenEvent::Cover | ScreenEvent::Suspend => self.close(fx),
            ScreenEvent::Tick(t) => self.tick(*t, cx, fx),
            ScreenEvent::Activate(elem) => self.activate(*elem, fx),
            ScreenEvent::FocusMoved { to, .. } => {
                if self.editing.is_some_and(|field| field != to.elem) {
                    self.close(fx);
                }
                self.pop_until_ms = self.now_ms.wrapping_add(POP_MS);
                fx.invalidate(Provenance::Input);
            }
            ScreenEvent::Input(input) => match &input.kind {
                InputKind::SystemKeyboard(up) => {
                    if !*up {
                        self.editing = None;
                    } else if self.editing.is_none() {
                        let focused = cx.focus.current.map(|k| k.elem).filter(|e| self.fields().contains(e));
                        self.editing = focused;
                    }
                    fx.invalidate(Provenance::Input);
                }
                InputKind::Text(edit) if self.editing.is_some() || matches!(cx.owner, InputOwner::System(_)) => {
                    let field = self
                        .editing
                        .or_else(|| cx.focus.current.map(|k| k.elem).filter(|e| self.fields().contains(e)));
                    if let Some(field) = field {
                        self.edit(field, edit, fx);
                    }
                }
                InputKind::Key { key, sym, edge, .. } if *edge != Edge::Up => {
                    if let Some(field) = self.editing {
                        let edit = match (*key, *sym) {
                            (Key::Left, _) => Some(TextEdit::Left),
                            (Key::Right, _) => Some(TextEdit::Right),
                            (_, crate::ui::consts::SDLK_BACKSPACE) => Some(TextEdit::Backspace),
                            (_, crate::ui::consts::SDLK_CLEAR) => Some(TextEdit::Clear),
                            _ => None,
                        };
                        if let Some(edit) = edit {
                            self.edit(field, &edit, fx);
                            return Handled::Yes;
                        }
                        match key {
                            Key::Back => {
                                self.close(fx);
                                return Handled::Yes;
                            }
                            Key::Ok => {
                                self.submit(field, fx);
                                return Handled::Yes;
                            }
                            Key::Up | Key::Down => {
                                self.close(fx);
                                return Handled::No;
                            }
                            _ => {}
                        }
                    }
                    if *key == Key::Back {
                        self.back(fx);
                        return Handled::Yes;
                    }
                    return Handled::No;
                }
                _ => return Handled::No,
            },
            _ => return Handled::No,
        }
        Handled::Yes
    }
}

impl<H: AppLike> Screen<H> for JfLoginScreen {
    fn name(&self) -> &'static str {
        word::LOGIN
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, H>) -> Option<std::borrow::Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        let p = f.painter;
        self.ground.draw_default(Painter::root());
        self.draw_narrative(p, f.measure);

        let form = self.form(f.measure);
        let focus = f.focus.current.filter(|k| k.entry == self.entry).map(|k| k.elem);
        crate::ui::widgets::panel_ground(p, form.panel, PANEL_R, f.underlay);
        if let Some(rect) = form.card {
            match self.stage {
                Stage::QuickConnect => self.draw_server_line(p, rect, f.measure),
                _ => {
                    let change_x = form.buttons.iter().find(|(b, _)| *b == CHANGE).map_or(rect.x + rect.w, |(_, r)| r.x);
                    self.draw_card(p, rect, change_x);
                }
            }
        }
        for &(field, rect) in &form.labels {
            Label::new(field_label(field).as_ptr(), theme::size::CAPTION, theme::TEXT_SECONDARY)
                .v(VAlign::Middle)
                .draw(p, rect);
        }
        for &(field, rect) in &form.fields {
            self.draw_field(p, field, rect, focus == Some(field), f.measure);
        }
        if let Some(rect) = form.hint {
            TextView::new(msg::jellyfin_login_port_hint(), theme::size::CAPTION, theme::TEXT_TERTIARY)
                .max_lines(1)
                .draw(p, Rect::new(rect.x, rect.y + 2.0, rect.w, rect.h));
        }
        if let Some(rect) = form.code_caption {
            Self::draw_caption(p, msg::jellyfin_login_code_label(), rect, true);
        }
        if let Some(rect) = form.code {
            self.draw_code(p, rect);
        }
        self.draw_status(p, form.status, self.stage == Stage::QuickConnect, f.measure);
        if let Some(rect) = form.or_rule {
            Self::draw_rule(p, rect, None, f.measure);
        }
        if let Some(rect) = form.recent_caption {
            match self.stage {
                Stage::Credentials => Self::draw_rule(p, rect, Some(msg::jellyfin_login_or_c()), f.measure),
                _ => Self::draw_caption(p, msg::jellyfin_login_recent(), rect, false),
            }
        }
        let buttons = self.buttons();
        for &(button, rect) in &form.buttons {
            let scale = buttons
                .iter()
                .position(|&b| b == button)
                .map_or(1.0, |i| self.pop.scale_with(i, f.press.scale));
            if button == RECENT {
                self.draw_recent(p, rect, focus == Some(button), scale);
                continue;
            }
            let mut b = Button::new(button_label(button).as_ptr(), BUTTON_SZ, rect)
                .focused(focus == Some(button))
                .scale(scale);
            if button == QUICK {
                b = b.icon(crate::ui::icons::Icon::Phone);
            }
            b.draw(&Env::inert(), p);
        }
        if f.records_stops() {
            for &(elem, rect) in form.fields.iter().chain(form.buttons.iter()) {
                self.stop(f, p, elem, rect);
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

impl LogicalState for JfLoginScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.instance.0).u32(match self.stage {
            Stage::Server => 0,
            Stage::Credentials => 1,
            Stage::QuickConnect => 2,
        });
        c.u32(self.server.text().len() as u32)
            .u32(self.user.text().len() as u32)
            .u32(self.pass.text().len() as u32);
        c.option(self.editing, |c, field| {
            c.u32(field);
        });
        c.option(self.busy, |c, busy| {
            c.u32(busy as u32);
        });
        c.bool(self.waiting()).bool(self.error.is_some()).bool(self.origin.is_some());
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!(
            "jf-login stage={:?} editing={:?} busy={:?} waiting={} error={}",
            self.stage,
            self.editing,
            self.busy,
            self.waiting(),
            self.error.is_some()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_stage_walks_its_own_elements_and_nothing_else() {
        let mut s = JfLoginScreen::new(EntryId(0), InstanceId(0));
        s.recent = None;
        assert_eq!(s.elems().collect::<Vec<_>>(), [SERVER, CONNECT]);
        assert_eq!(s.first(), SERVER);
        s.recent = Some(Recent { name: "Living Room".into(), address: "192.168.1.20:8096".into() });
        assert_eq!(s.elems().collect::<Vec<_>>(), [SERVER, CONNECT, RECENT]);
        assert_eq!(s.first(), RECENT, "a saved server is one press from reconnecting");
        s.stage = Stage::Credentials;
        assert_eq!(s.elems().collect::<Vec<_>>(), [CHANGE, USER, PASS, SIGN_IN, QUICK]);
        assert_eq!(s.first(), USER);
        s.stage = Stage::QuickConnect;
        assert_eq!(s.elems().collect::<Vec<_>>(), [USE_PASSWORD, CHANGE]);
        assert_eq!(s.first(), USE_PASSWORD);
    }

    #[test]
    fn the_logical_state_carries_no_credential_text() {
        let mut s = JfLoginScreen::new(EntryId(0), InstanceId(0));
        s.pass = TextBuffer::new("hunter2".into(), 7);
        s.user = TextBuffer::new("someone".into(), 7);
        let mut probe = String::new();
        s.probe(&mut probe);
        assert!(!probe.contains("hunter2") && !probe.contains("someone"));
        let a = s.hash();
        s.pass = TextBuffer::new("hunter3".into(), 7);
        assert_eq!(a, s.hash(), "same length, same state: the password itself is never hashed");
    }

    /// The status band is tall enough for the error box at its largest (two lines of `LABEL`,
    /// padded) with air left before the control under it.
    #[test]
    fn the_status_band_holds_a_two_line_error_with_air_to_spare() {
        let two_lines = 2.0 * (theme::size::LABEL as f32 + theme::space::XS);
        let boxed = theme::space::XS + two_lines + 2.0 * theme::space::SM;
        assert!(boxed + theme::space::SM <= STATUS_H, "{boxed} in {STATUS_H}");
    }

    /// The two marks the panel adds rasterize at the sizes it draws them, full ink inside and none
    /// on the border.
    #[test]
    fn the_server_and_phone_marks_rasterize_clean() {
        use crate::ui::icons::Icon;
        for (id, svg) in [
            (Icon::Server, include_str!("../../../assets/icons/server.svg")),
            (Icon::Phone, include_str!("../../../assets/icons/phone.svg")),
        ] {
            for px in [26, 28, 32] {
                let rgba = nj_gfx::svg::rasterize(svg, px, px).unwrap_or_else(|| panic!("{id:?} at {px}px"));
                let alpha = |x: i32, y: i32| rgba[((y * px + x) * 4 + 3) as usize];
                let max = (0..px).flat_map(|y| (0..px).map(move |x| alpha(x, y))).max().unwrap();
                assert_eq!(max, 255, "{id:?} never reaches full ink at {px}px");
                for i in 0..px {
                    for (x, y) in [(i, 0), (i, px - 1), (0, i), (px - 1, i)] {
                        assert_eq!(alpha(x, y), 0, "{id:?} inks the border at {px}px ({x},{y})");
                    }
                }
            }
        }
    }

    /// The stages walk as drawn: a column, except Quick Connect's pair, which is a row.
    #[test]
    fn focus_walks_the_panel_as_it_is_drawn() {
        let mut s = JfLoginScreen::new(EntryId(1), InstanceId(0));
        s.recent = None;
        s.stage = Stage::Credentials;
        let order = s.order();
        assert_eq!(order, [CHANGE, USER, PASS, SIGN_IN, QUICK]);
        assert!(!s.is_row());
        s.stage = Stage::QuickConnect;
        assert!(s.is_row());
    }

    #[test]
    fn every_error_has_a_sentence() {
        for e in [
            AuthError::Unreachable,
            AuthError::NotJellyfin,
            AuthError::NotSetUp,
            AuthError::BadCredentials,
            AuthError::QuickConnectDisabled,
            AuthError::Refused(503),
            AuthError::Malformed,
        ] {
            assert!(!message(&e).is_empty());
        }
        assert!(message(&AuthError::Refused(503)).contains("503"));
    }
}
