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

use std::ffi::{CStr, CString};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use crate::jf::auth::{AuthError, QuickConnect, SignedIn};
use crate::plex::Origin;
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
use plx_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind,
    InputOwner, InstanceId, Key, LogicalState, Machine, MachineId, Measure, TextEdit, Tick,
};
use plx_machine::present::{PresentEvent, Provenance};
use plx_platform::i18n::msg;

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
const LABEL_H: f32 = 32.0;
const FIELD_H: f32 = 84.0;
const FIELD_R: f32 = 20.0;
const FIELD_PAD: f32 = 28.0;
const SERVER_ROW_H: f32 = 76.0;
const CODE_H: f32 = 132.0;
const CODE_CELL: f32 = 76.0;
const STATUS_H: f32 = 80.0;
const CTRL_H: f32 = 60.0;
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

struct Form {
    server_row: Option<Rect>,
    labels: Vec<(u32, Rect)>,
    fields: Vec<(u32, Rect)>,
    code: Option<Rect>,
    status: Rect,
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
    spin: plx_machine::motion::Phase,
    spin_ms: f32,
    pop: CtlPop<3>,
    ground: RouteGround,
}

impl JfLoginScreen {
    pub(crate) fn new(entry: EntryId, instance: InstanceId) -> Self {
        Self {
            entry,
            instance,
            stage: Stage::Server,
            server: TextBuffer::new(String::new(), 0),
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
            spin: plx_machine::motion::Phase::default(),
            spin_ms: 0.0,
            pop: CtlPop::new(),
            ground: RouteGround::new(),
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
            Stage::Server => &[CONNECT],
            Stage::Credentials => &[SIGN_IN, QUICK, CHANGE],
            Stage::QuickConnect => &[USE_PASSWORD, CHANGE],
        }
    }

    fn elems(&self) -> impl Iterator<Item = u32> + '_ {
        self.fields().iter().chain(self.buttons()).copied()
    }

    fn first(&self) -> u32 {
        self.elems().next().unwrap_or(CONNECT)
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

    fn form(&self, measure: &dyn Measure) -> Form {
        let layout = RouteLayout::screen();
        let x = layout.content.x;
        let w = layout.content.w;
        let mut y = layout.narrative_top(false);
        let mut form = Form {
            server_row: None,
            labels: Vec::new(),
            fields: Vec::new(),
            code: None,
            status: Rect::new(x, 0.0, w, STATUS_H),
            buttons: Vec::new(),
        };
        if self.stage != Stage::Server {
            form.server_row = Some(Rect::new(x, y, w, SERVER_ROW_H));
            y += SERVER_ROW_H + theme::space::MD;
        }
        if self.stage == Stage::QuickConnect {
            form.labels.push((0, Rect::new(x, y, w, LABEL_H)));
            y += LABEL_H + theme::space::XS;
            form.code = Some(Rect::new(x, y, w, CODE_H));
            y += CODE_H + theme::space::MD;
        }
        for &field in self.fields() {
            form.labels.push((field, Rect::new(x, y, w, LABEL_H)));
            y += LABEL_H + theme::space::XS;
            form.fields.push((field, Rect::new(x, y, w, FIELD_H)));
            y += FIELD_H + theme::space::MD;
        }
        form.status.y = y;
        y += STATUS_H + theme::space::SM;
        let mut cx = x;
        for &button in self.buttons() {
            let bw = Button::pill_w_measured(button_label(button), BUTTON_SZ, false, false, measure);
            if cx > x && cx + bw > x + w {
                cx = x;
                y += CTRL_H + CONTROL_GAP;
            }
            form.buttons.push((button, Rect::new(cx, y, bw, CTRL_H)));
            cx += bw + CONTROL_GAP;
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

    fn draw_field(&self, p: Painter, field: u32, rect: Rect, focused: bool, measure: &dyn Measure) {
        let editing = self.editing == Some(field);
        let fill = if focused { theme::with_a(theme::TEXT_PRIMARY, 0.14) } else { theme::CONTROL_IDLE_FILL };
        p.rrect(rect, FIELD_R, FIELD_R, fill);
        if focused {
            p.rring(rect, FIELD_R, 3.0, theme::ACCENT);
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

    fn draw_code(&self, p: Painter, rect: Rect) {
        p.rrect(rect, 24.0, 24.0, theme::SURFACE_PANEL);
        let Some(qc) = &self.qc else {
            return;
        };
        let n = qc.code.chars().count().max(1) as f32;
        let x0 = rect.x + (rect.w - n * CODE_CELL).max(0.0) * 0.5;
        for (i, ch) in qc.code.chars().enumerate() {
            let glyph = cstring(&ch.to_string());
            let cell = Rect::new(x0 + i as f32 * CODE_CELL, rect.y, CODE_CELL, rect.h);
            Label::new(glyph.as_ptr(), theme::size::HERO, theme::TEXT_PRIMARY)
                .bold()
                .h(HAlign::Center)
                .draw(p, cell);
        }
    }

    fn draw_status(&self, p: Painter, rect: Rect, measure: &dyn Measure) {
        if self.spinning() {
            let line = match self.busy {
                Some(Busy::Connecting) => msg::jellyfin_login_connecting_c(),
                Some(Busy::SigningIn | Busy::Adopting) => msg::jellyfin_login_signing_in_c(),
                Some(Busy::Starting) => msg::jellyfin_login_connecting_c(),
                None => msg::jellyfin_login_waiting_c(),
            };
            let row = Rect::new(rect.x, rect.y, rect.w, LABEL_H + theme::space::XS);
            Spinner::leading(rect.x, row.cy())
                .phase(self.spin_ms as u32)
                .tint(theme::TEXT_SECONDARY)
                .draw(&Env::inert(), p);
            let gutter = Spinner::inline_gutter();
            Label::new(line.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY)
                .draw(p, Rect::new(row.x + gutter, row.y, row.w - gutter, row.h));
        } else if let Some(error) = &self.error {
            TextView::new(error, theme::size::BODY, theme::DANGER)
                .with_measure(measure)
                .max_lines(2)
                .draw(p, rect);
        }
    }

    fn draw_server_row(&self, p: Painter, rect: Rect, measure: &dyn Measure) {
        let caption = Rect::new(rect.x, rect.y, rect.w, LABEL_H);
        Label::new(msg::jellyfin_login_server_row_c().as_ptr(), theme::size::CAPTION, theme::TEXT_TERTIARY)
            .draw(p, caption);
        let line = Rect::new(rect.x, rect.y + LABEL_H, rect.w, rect.h - LABEL_H);
        let name = cstring(&self.server_name);
        let name_w = Label::new(name.as_ptr(), theme::size::BODY, theme::TEXT_PRIMARY).bold().draw(p, line);
        if let Some(origin) = &self.origin {
            let address = cstring(&format!("{}:{}", origin.host(), origin.port()));
            let gap = theme::space::SM;
            if name_w + gap + width(&format!("{}:{}", origin.host(), origin.port()), measure) <= rect.w {
                Label::new(address.as_ptr(), theme::size::BODY, theme::TEXT_TERTIARY)
                    .draw(p, Rect::new(line.x + name_w + gap, line.y, line.w - name_w - gap, line.h));
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
    /// Fields stack: UP/DOWN walk them and DOWN off the last lands on the first button. The buttons
    /// are a row: LEFT/RIGHT walk it and UP returns to the last field.
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, _cx: &Cx<'_, H>) -> Step<u32> {
        let fields = self.fields();
        let buttons = self.buttons();
        let next = if let Some(i) = fields.iter().position(|&f| f == key.elem) {
            match dir {
                Dir::Up => i.checked_sub(1).map(|j| fields[j]),
                Dir::Down => fields.get(i + 1).or(buttons.first()).copied(),
                Dir::Left | Dir::Right => None,
            }
        } else if let Some(i) = buttons.iter().position(|&b| b == key.elem) {
            match dir {
                Dir::Left => i.checked_sub(1).map(|j| buttons[j]),
                Dir::Right => buttons.get(i + 1).copied(),
                Dir::Up => fields.last().copied(),
                Dir::Down => None,
            }
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
        let layout = RouteLayout::screen();
        let intro = match self.stage {
            Stage::Server => msg::jellyfin_login_server_intro(),
            Stage::Credentials => msg::jellyfin_login_credentials_intro(),
            Stage::QuickConnect => msg::jellyfin_login_quick_connect_intro(),
        };
        layout.draw_narrative(p, None, msg::jellyfin_login_title(), intro, theme::size::LABEL, f.measure);

        let form = self.form(f.measure);
        let focus = f.focus.current.filter(|k| k.entry == self.entry).map(|k| k.elem);
        if let Some(rect) = form.server_row {
            self.draw_server_row(p, rect, f.measure);
        }
        for &(field, rect) in &form.labels {
            Label::new(field_label(field).as_ptr(), theme::size::CAPTION, theme::TEXT_SECONDARY)
                .v(VAlign::Middle)
                .draw(p, rect);
        }
        for &(field, rect) in &form.fields {
            self.draw_field(p, field, rect, focus == Some(field), f.measure);
        }
        if let Some(rect) = form.code {
            self.draw_code(p, rect);
        }
        self.draw_status(p, form.status, f.measure);
        for (i, &(button, rect)) in form.buttons.iter().enumerate() {
            Button::new(button_label(button).as_ptr(), BUTTON_SZ, rect)
                .focused(focus == Some(button))
                .scale(self.pop.scale_with(i, f.press.scale))
                .draw(&Env::inert(), p);
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
        assert_eq!(s.elems().collect::<Vec<_>>(), [SERVER, CONNECT]);
        s.stage = Stage::Credentials;
        assert_eq!(s.elems().collect::<Vec<_>>(), [USER, PASS, SIGN_IN, QUICK, CHANGE]);
        s.stage = Stage::QuickConnect;
        assert_eq!(s.elems().collect::<Vec<_>>(), [USE_PASSWORD, CHANGE]);
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
