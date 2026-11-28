//! The field-list pages. The choice list's own transaction tests live beside it
//! (`preferences/picker_tests.rs`); this file also owns the fixtures both share.
use super::*;
use crate::jf::preferences::PreferenceRequest;
use nj_machine::machine::{FocusRead, InputOwner, PressRead, Stamped, Tick};
use nj_machine::present::Present;
use crate::ui::fixture::FixtureMeasure;

pub(super) fn context(focus: u32) -> Cx<'static, InnerHost> {
    static MEASURE: FixtureMeasure = FixtureMeasure;
    Cx { views: crate::stores::browse::DirectoryView::empty_for_test(), tick: Tick::default(),
        measure: &MEASURE, press: PressRead::default(),
        focus: FocusRead { current: Some(FocusKey { entry: EntryId(0), elem: focus }), ..Default::default() },
        owner: InputOwner::Entry(EntryId(0)) }
}
/// Step `page` with `ev` and return what it emitted.
pub(super) fn drive<M: Machine<InnerHost, Ev = ScreenEvent<InnerHost>>>(page: &mut M, ev: ScreenEvent<InnerHost>, focus: u32) -> Vec<Stamped<InnerHost>> {
    let mut out = Vec::new(); let mut present = Present::new();
    page.step(&ev, &context(focus), &mut Effects::new(&mut out, MachineId::Instance(InstanceId(0)), &mut present));
    out
}
pub(super) fn tick() -> ScreenEvent<InnerHost> { ScreenEvent::Tick(Tick { ms: 16, dt_us: 16_000 }) }
pub(super) fn preference_commands(emitted: &[Stamped<InnerHost>]) -> usize {
    emitted.iter().filter(|e| matches!(&e.fx, Fx::App(AppFx::Preferences(_)))).count()
}
pub(super) fn popped(emitted: &[Stamped<InnerHost>]) -> bool {
    emitted.iter().any(|e| matches!(&e.fx, Fx::Nav(nj_machine::machine::NavOp::Pop)))
}
pub(super) fn pushed(emitted: &[Stamped<InnerHost>]) -> Vec<SettingsPage> {
    emitted.iter().filter_map(|e| match &e.fx { Fx::Nav(nj_machine::machine::NavOp::Push(p)) => Some(*p), _ => None }).collect()
}

/// A live Jellyfin sign-in with a synthetic (request, snapshot) pair, restoring the previous
/// sign-in on drop. The caller holds `testlock::serial()`. `generation` is part of the user id, so
/// [`Account::go_stale`] can move the sign-in on.
pub(super) struct Account {
    pub request: PreferenceRequest,
    pub snapshot: PreferenceSnapshot,
    user_id: String,
    /// The sign-ins this account made, newest last; each restores the one before it when dropped.
    sign_ins: std::cell::RefCell<Vec<crate::jf::preferences::LiveSignInForTest>>,
}
const ACCOUNT_SERVER: &str = "http://10.0.0.2:8096";
impl Account {
    pub fn new(user: &str, generation: u32, prefs: AudioPreferences) -> Self {
        let user_id = format!("{user}-{generation}");
        let signed_in = crate::jf::preferences::LiveSignInForTest::new(ACCOUNT_SERVER, &user_id);
        let (request, snapshot) = PreferenceRequest::fixture_for_test(ACCOUNT_SERVER, &user_id, prefs);
        Self { request, snapshot, user_id, sign_ins: std::cell::RefCell::new(vec![signed_in]) }
    }
    /// A fresh receipt body for the same identity, carrying `prefs`.
    pub fn reply(&self, prefs: AudioPreferences) -> AccountPreferenceReply {
        let (request, snapshot) = PreferenceRequest::fixture_for_test(ACCOUNT_SERVER, &self.user_id, prefs);
        AccountPreferenceReply { request: Some(request), outcome: Ok(snapshot) }
    }
    /// Another user signs in: every request captured before is now stale.
    pub fn go_stale(&self) {
        let next = crate::jf::preferences::LiveSignInForTest::new(ACCOUNT_SERVER, &format!("{}-next", self.user_id));
        self.sign_ins.borrow_mut().push(next);
    }
}
impl Drop for Account {
    /// Newest first, so the sign-in before this account is the one left live.
    fn drop(&mut self) {
        while self.sign_ins.borrow_mut().pop().is_some() {}
    }
}
/// An Audio & Subtitles field list that has loaded `account`'s snapshot (and published it).
pub(super) fn loaded_audio_page(account: &Account) -> PreferencesPage {
    let mut page = PreferencesPage::new(EntryId(0), Kind::AudioSubtitles);
    page.txn.request = Some(account.request.clone());
    page.txn.snapshot = Some(account.snapshot.clone());
    page.txn.publish();
    page.rebuild(None);
    page
}

#[test]
fn account_constructor_is_inert_and_first_enter_emits_one_load_effect() {
    let _serial = nj_base::testlock::serial();
    let mut page = PreferencesPage::new(EntryId(0), Kind::AudioSubtitles);
    assert!(page.txn.pending.is_none());
    assert!(page.txn.request.is_none());
    assert!(page.state.io.status.is_empty());
    let mut emitted = drive(&mut page, ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::ContainerGroup(GroupId(0)) }), 0);
    emitted.extend(drive(&mut page, tick(), 0));
    assert!(page.state.io.busy);
    assert!(page.txn.request.is_none(), "the screen never captures live credentials");
    assert_eq!(emitted.iter().filter(|event| matches!(&event.fx,
        Fx::App(AppFx::Preferences(PreferenceCmd::Load { .. })))).count(), 1);
}

#[test]
fn both_language_pickers_offer_the_servers_languages_and_an_empty_preference() {
    let prefs = AudioPreferences { cultures: std::sync::Arc::new(vec![
        crate::jf::preferences::Culture { code: "eng".into(), name: "English".into() },
        crate::jf::preferences::Culture { code: "fre".into(), name: "French".into() },
    ]), ..Default::default() };
    for field in [PickerKind::AudioLanguage, PickerKind::SubtitleLanguage] {
        let options = field_options(field, Quality::Original, DirectPlayMode::Auto, Some(&prefs));
        assert_eq!(options, vec![
            (nj_platform::i18n::msg::settings_audio_no_preference().to_string(), Value::Language(String::new())),
            ("English".to_string(), Value::Language("eng".into())),
            ("French".to_string(), Value::Language("fre".into())),
        ]);
    }
}

/// A stored code the server no longer lists still shows, so the checked row is never missing.
#[test]
fn a_stored_language_the_server_does_not_list_is_still_offered() {
    let prefs = AudioPreferences { audio_language: Some("tlh".into()), ..Default::default() };
    let options = field_options(PickerKind::AudioLanguage, Quality::Original, DirectPlayMode::Auto, Some(&prefs));
    assert_eq!(options.last().unwrap().1, Value::Language("tlh".into()));
}

/// Every field row is a `Nav` item onto its own picker page — the list pushes through the family's
/// one activation path and keeps no submenu of its own.
#[test]
fn every_field_row_pushes_its_picker_and_nothing_else() {
    let _serial = nj_base::testlock::serial();
    let _session = crate::catalog::session::TempSession::new("pref-field-rows-push");
    let account = Account::new("pref-field-rows", 11, AudioPreferences::default());
    let mut playback = PreferencesPage::new(EntryId(0), Kind::Playback);
    let mut audio = loaded_audio_page(&account);
    for (page, fields) in [
        (&mut playback, vec![PickerKind::Quality, PickerKind::DirectPlay, PickerKind::SubtitleSize, PickerKind::SubtitlePosition, PickerKind::NextEpisode, PickerKind::SkipInterval]),
        (&mut audio, vec![PickerKind::AudioLanguage, PickerKind::SubtitleMode, PickerKind::SubtitleLanguage, PickerKind::DefaultAudioTrack]),
    ] {
        for field in fields {
            let key = RowId::Field(field).key().0;
            let emitted = drive(page, ScreenEvent::Activate(key), key);
            assert_eq!(pushed(&emitted), vec![SettingsPage::Picker(field)], "{field:?}");
            assert_eq!(preference_commands(&emitted), 0);
        }
    }
}

/// After a picker's durable write the list's own rows show the new value: the picker publishes the
/// confirmed snapshot and the list adopts it when it is returned to.
#[test]
fn the_field_list_shows_the_value_a_picker_committed_after_the_pop() {
    let _serial = nj_base::testlock::serial();
    let _session = crate::catalog::session::TempSession::new("pref-parent-refresh");
    let account = Account::new("pref-parent-refresh", 12, AudioPreferences { subtitle_mode: crate::jf::preferences::SubtitleMode::None, ..Default::default() });
    let mut parent = loaded_audio_page(&account);
    let mode_row = parent.form.index_of(&RowId::Field(PickerKind::SubtitleMode)).unwrap();
    assert_eq!(parent.state.values[mode_row], nj_platform::i18n::msg::settings_audio_manual());
    let mut picker = PickerPage::new(EntryId(0), PickerKind::SubtitleMode);
    let emitted = drive(&mut picker, ScreenEvent::Activate(2), 2);
    let Some(Fx::App(AppFx::Preferences(PreferenceCmd::Save { reply, .. }))) = emitted.into_iter().map(|e| e.fx).find(|f| matches!(f, Fx::App(AppFx::Preferences(_)))) else {
        panic!("choosing Always must save");
    };
    reply.send(account.reply(AudioPreferences { subtitle_mode: crate::jf::preferences::SubtitleMode::Always, ..Default::default() })).unwrap();
    assert!(popped(&drive(&mut picker, tick(), 2)), "the durable receipt pops the picker");
    // The page beneath is re-entered by the pop.
    drive(&mut parent, ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::FirstInGroup(GroupId(0)) }), 0);
    assert_eq!(parent.state.values[mode_row], nj_platform::i18n::msg::settings_audio_always(),
        "the list's readout is the committed value, with no reload");
}

/// The local fields have no snapshot to publish: the list re-reads the session values on its tick.
#[test]
fn the_field_list_rereads_a_local_value_a_picker_committed() {
    let _serial = nj_base::testlock::serial();
    let _session = crate::catalog::session::TempSession::new("pref-parent-local");
    let previous = crate::route::quality();
    crate::route::restore_quality(Quality::Original);
    let mut parent = PreferencesPage::new(EntryId(0), Kind::Playback);
    let before = parent.state.values[0].clone();
    crate::route::restore_quality(Quality::P480);
    drive(&mut parent, tick(), 0);
    assert_ne!(parent.state.values[0], before);
    assert_eq!(parent.state.values[0], Quality::P480.label());
    crate::route::restore_quality(previous);
}

/// A Size/Position pick made anywhere (the player's Style pages publish the live value before
/// their write lands) shows in the list's cached read-outs on its next tick.
#[test]
fn the_field_list_rereads_a_subtitle_look_picked_elsewhere() {
    let _serial = nj_base::testlock::serial();
    let _session = crate::catalog::session::TempSession::new("pref-parent-look");
    let (size, position) = (crate::route::subtitle_size(), crate::route::subtitle_position());
    crate::route::restore_subtitle_size(crate::route::SubtitleSize::Medium);
    crate::route::restore_subtitle_position(crate::route::SubtitlePosition::Low);
    let mut parent = PreferencesPage::new(EntryId(0), Kind::Playback);
    let size_row = parent.form.index_of(&RowId::Field(PickerKind::SubtitleSize)).unwrap();
    let position_row = parent.form.index_of(&RowId::Field(PickerKind::SubtitlePosition)).unwrap();
    assert_eq!(parent.state.values[size_row], nj_platform::i18n::msg::settings_playback_subtitle_size_medium());

    crate::route::restore_subtitle_size(crate::route::SubtitleSize::Large);
    crate::route::restore_subtitle_position(crate::route::SubtitlePosition::High);
    drive(&mut parent, tick(), 0);
    assert_eq!(parent.state.values[size_row], nj_platform::i18n::msg::settings_playback_subtitle_size_large());
    assert_eq!(parent.state.values[position_row], nj_platform::i18n::msg::settings_playback_subtitle_position_high());
    crate::route::restore_subtitle_size(size);
    crate::route::restore_subtitle_position(position);
}

/// Every field belongs to exactly the page whose list shows it: the Audio & Subtitles page lists
/// its four account fields and nothing else, the Playback page the six local ones, so a picker's
/// `kind()` (its crumb, its copy, whether it loads the Jellyfin user preferences) can never disagree with the
/// list that opened it.
#[test]
fn every_field_reports_the_page_whose_list_shows_it() {
    let prefs = AudioPreferences::default();
    for kind in [Kind::Playback, Kind::AudioSubtitles] {
        let inputs = FieldListInputs { kind, quality: Quality::Original, direct_play: DirectPlayMode::Auto,
            prefs: Some(&prefs), busy: false, show_retry: false, hbr_row: true };
        for &field in fields_of(&inputs) {
            assert_eq!(field.kind(), kind, "{field:?} is listed by {kind:?}");
        }
    }
    let audio = FieldListInputs { kind: Kind::AudioSubtitles, quality: Quality::Original, direct_play: DirectPlayMode::Auto,
        prefs: Some(&prefs), busy: false, show_retry: false, hbr_row: true };
    assert_eq!(fields_of(&audio), &[PickerKind::AudioLanguage, PickerKind::SubtitleMode, PickerKind::SubtitleLanguage, PickerKind::DefaultAudioTrack]);
}

/// The HBR row is offered only while lossless audio is on, on the firmware its module is built for
/// (`hbr_row`); without it the Playback list is exactly what it was.
#[test]
fn the_hbr_row_shows_only_when_offered() {
    let playback = |hbr_row| FieldListInputs { kind: Kind::Playback, quality: Quality::Original, direct_play: DirectPlayMode::Auto,
        prefs: None, busy: false, show_retry: false, hbr_row };
    assert_eq!(fields_of(&playback(true)).last(), Some(&PickerKind::HbrSupport));
    assert!(!fields_of(&playback(false)).contains(&PickerKind::HbrSupport));
    assert_eq!(fields_of(&playback(true)).len(), fields_of(&playback(false)).len() + 1);
}

/// Not installed: the checked option is *Not installed*, and installing is a separate, explicit
/// choice. Installed: *Installed* is checked and *Remove* is the other choice.
#[test]
fn hbr_options_put_the_current_state_first() {
    use nj_platform::i18n::msg;
    let off = hbr_options(false);
    assert_eq!(off, vec![(msg::settings_playback_hbr_support_not_installed().to_string(), Value::HbrSupport(false)),
                         (msg::settings_playback_hbr_support_install().to_string(), Value::HbrSupport(true))]);
    let on = hbr_options(true);
    assert_eq!(on, vec![(msg::settings_playback_hbr_support_installed().to_string(), Value::HbrSupport(true)),
                        (msg::settings_playback_hbr_support_remove().to_string(), Value::HbrSupport(false))]);
}

#[test]
fn an_hbr_failure_says_why() {
    use crate::player::hbr_support::HbrError;
    use nj_platform::i18n::msg;
    assert_eq!(hbr_failure(&HbrError::NoRoot), msg::settings_playback_hbr_support_no_root());
    assert_eq!(hbr_failure(&HbrError::Download), msg::settings_playback_hbr_support_download_failed());
    assert!(hbr_failure(&HbrError::Refused("kernel 5.4".into())).contains("kernel 5.4"));
}
