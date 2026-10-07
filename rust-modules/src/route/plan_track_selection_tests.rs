//! Audio and subtitle track selection tests: the Jellyfin user-preference ladder
//! (`PlayDefaultAudioTrack`, `AudioLanguagePreference`, `SubtitleMode`), direct-play-eligible
//! fallbacks, and subtitle ordinal resolution.

use super::*;
#[allow(unused_imports)]
use super::test_support::*;

fn lang(l: &'static str) -> AudioLangPrefs<'static> {
    AudioLangPrefs { language: Some(l), prefer_default: false }
}

fn subs_mode(mode: SubtitleMode, language: Option<&'static str>) -> SubtitleLangPrefs<'static> {
    SubtitleLangPrefs { language, mode }
}

fn forced(mut s: crate::metadata::Stream) -> crate::metadata::Stream {
    s.forced = true;
    s
}

fn flagged_default(mut s: crate::metadata::Stream) -> crate::metadata::Stream {
    s.default = true;
    s
}

#[test]
fn an_empty_track_list_falls_back_to_the_codec_default() {
    assert_eq!(
        pick_dp_audio(&[], "ac3").map(|(i, c, _)| (i, c)),
        Some((-1, "ac3".into()))
    );
    assert!(
        pick_dp_audio(&[], "truehd").is_none(),
        "a non-direct-playable default must convert"
    );
}

/// **With no language preference, the file's default track wins — English has no say.**
/// Issue #202: a French MKV whose default-flagged track is French, beside an English one, opened
/// in English because of a built-in English rung. Both orderings, so the pick is the flag and
/// not the list position.
#[test]
fn the_files_default_track_wins_without_a_language_preference() {
    let tracks = [trk(1, "ac3", "fre", true), trk(2, "ac3", "eng", false)];
    assert_eq!(pick_dp_audio(&tracks, "ac3"), Some((0, "ac3".into(), 1)));
    let tracks = [trk(1, "ac3", "eng", false), trk(2, "eac3", "fre", true)];
    assert_eq!(pick_dp_audio(&tracks, "eac3"), Some((1, "eac3".into(), 2)));
}

/// The conversion half of #202: a video transcode of the same French file must not swap the
/// French default for an English track either.
#[test]
fn a_conversion_without_a_preference_keeps_the_files_default_language() {
    let tracks = [trk(1, "ac3", "fre", true), trk(2, "dca", "eng", false)];
    let dp = pick_dp_audio(&tracks, "ac3").map(|(_, _, id)| id).unwrap_or(0);
    assert_eq!(dp, 1, "direct play takes the French default");
    assert_eq!(encode_audio_id(dp, 0, &tracks, AudioLangPrefs::default()), 1,
        "a cold conversion names the French default, not the English DTS");
}

/// `AudioLanguagePreference` without `PlayDefaultAudioTrack`: the preferred language outranks the
/// file's flag on every path.
#[test]
fn the_users_audio_language_selects_its_track_for_every_cold_play_path() {
    let tracks = [trk(1, "ac3", "eng", true), trk(2, "ac3", "fre", false)];
    let (_, _, dp) = pick_dp_audio_pref(&tracks, "ac3", lang("fra")).unwrap();
    assert_eq!(dp, 2, "direct play honours AudioLanguagePreference");
    assert_eq!(encode_audio_id(dp, 0, &tracks, lang("fra")), 2,
        "a conversion names the preferred-language track");
}

/// `PlayDefaultAudioTrack` (Jellyfin's default) puts the file's flagged track first; the
/// language preference only decides when the file flags nothing.
#[test]
fn play_default_audio_track_outranks_the_language_preference() {
    let prefs = AudioLangPrefs { language: Some("fra"), prefer_default: true };
    let tracks = [trk(1, "ac3", "eng", true), trk(2, "ac3", "fre", false)];
    assert_eq!(pick_dp_audio_pref(&tracks, "ac3", prefs), Some((0, "ac3".into(), 1)));
    assert_eq!(encode_audio_id(1, 0, &tracks, prefs), 1);
    let unflagged = [trk(1, "ac3", "eng", false), trk(2, "ac3", "fre", false)];
    assert_eq!(pick_dp_audio_pref(&unflagged, "ac3", prefs), Some((1, "ac3".into(), 2)),
        "no flagged track: the preferred language decides");
}

#[test]
fn an_unmatched_or_blank_language_falls_to_the_file_default() {
    let tracks = [trk(1, "ac3", "eng", true), trk(2, "ac3", "fra", false)];
    for language in ["deu", "", "  "] {
        assert_eq!(pick_dp_audio_pref(&tracks, "ac3", lang(language)), Some((0, "ac3".into(), 1)),
            "{language:?}");
        assert_eq!(encode_audio_id(1, 0, &tracks, lang(language)), 1, "{language:?}");
    }
}

#[test]
fn the_language_preference_matches_every_spelling() {
    for stream in ["fr", "fre", "fra"] {
        let tracks = [trk(1, "ac3", "eng", true), trk(2, "ac3", stream, false)];
        assert_eq!(pick_dp_audio_pref(&tracks, "ac3", lang("fra")), Some((1, "ac3".into(), 2)),
            "{stream}");
        assert_eq!(encode_audio_id(2, 0, &tracks, lang("fra")), 2, "{stream}");
    }
}

/// A preferred track that cannot direct-play does not force a conversion on direct play; the
/// conversion path, which converts anyway, carries the preferred track itself.
#[test]
fn an_undecodable_preferred_track_is_carried_only_by_a_conversion() {
    let tracks = [trk(1, "ac3", "eng", true), trk(2, "dca", "hun", false)];
    let (_, _, dp) = pick_dp_audio_pref(&tracks, "ac3", lang("hun")).unwrap();
    assert_eq!(dp, 1, "direct play keeps the decodable default");
    assert_eq!(encode_audio_id(dp, 0, &tracks, lang("hun")), 2, "a conversion carries the Hungarian DTS");
}

#[test]
fn the_flagged_default_wins_over_an_earlier_track() {
    let tracks = [trk(1, "ac3", "deu", false), trk(2, "ac3", "fra", true)];
    assert_eq!(pick_dp_audio(&tracks, "ac3"), Some((1, "ac3".into(), 2)));
}

#[test]
fn smart_dp_takes_a_playable_sibling_over_a_non_playable_default() {
    // A 4K HEVC item: TrueHD default + an AC3 sibling — direct play beats the server's
    // video-downscaling transcode.
    let tracks = [trk(1, "truehd", "eng", true), trk(2, "ac3", "eng", false)];
    assert_eq!(pick_dp_audio(&tracks, "truehd"), Some((1, "ac3".into(), 2)));
}

#[test]
fn no_direct_playable_track_means_conversion() {
    let tracks = [trk(1, "truehd", "eng", true), trk(2, "dts", "eng", false)];
    assert!(pick_dp_audio(&tracks, "truehd").is_none());
}

/// A conversion keeps a session/retry pick, and with nothing picked names the file's default
/// whatever its codec (the server converts it), not the smart-DP sibling.
#[test]
fn a_conversion_keeps_the_session_pick_then_the_default() {
    let tracks = [trk(1, "truehd", "eng", true), trk(2, "ac3", "fra", false)];
    let dp = pick_dp_audio(&tracks, "truehd").map(|(_, _, id)| id).unwrap_or(0);
    assert_eq!(dp, 2, "smart-DP sibling is the French AC3");
    assert_eq!(encode_audio_id(dp, 0, &tracks, AudioLangPrefs::default()), 1,
        "a cold conversion names the TrueHD default");
    assert_eq!(encode_audio_id(dp, 2, &tracks, AudioLangPrefs::default()), 2,
        "a retry keeps the track already playing");
}

/// The preferred dub already playing as the direct-play pick is kept by a conversion rather than
/// re-encoding a lossless sibling in the same language.
#[test]
fn a_conversion_keeps_a_direct_play_pick_already_in_the_preferred_language() {
    let tracks = [trk(1, "truehd", "eng", true), trk(2, "ac3", "eng", false)];
    assert_eq!(encode_audio_id(2, 0, &tracks, lang("eng")), 2);
}

#[test]
fn the_selected_subtitle_resolves_to_the_renderers_embedded_ordinal() {
    // Document order is NOT container order and a sidecar sits in the middle of the list:
    // the renderer counts only embedded streams, sorted on `Stream.index` — the same
    // identifier space the track menu commits (metadata::sub_render_ordinal).
    let subs = [
        sub(10, 7, "fra", true),  // sidecar — not in the container, not counted
        sub(11, 3, "rus", false), // embedded, container-first
        server_selected(sub(12, 4, "eng", false)),
    ];
    assert_eq!(pick_dp_subtitle(&subs), Some((12, 1)));
}

#[test]
fn an_external_selected_subtitle_is_left_to_the_sidecar_path() {
    let subs = [
        server_selected(sub(10, 3, "eng", true)),
        sub(11, 4, "rus", false),
    ];
    assert_eq!(pick_dp_subtitle(&subs), None);
    assert_eq!(pick_dp_subtitle_pref(&subs, subs_mode(SubtitleMode::Always, Some("rus")), "eng"), None,
        "an explicit sidecar selection keeps automatic picks out");
}

#[test]
fn a_selection_with_no_stream_id_is_left_off_rather_than_half_applied() {
    // id and ordinal travel together: the id is what the menu checkmark and the reports key
    // on, so an id-less stream would render subtitles while the menu said Off.
    let subs = [server_selected(sub(0, 3, "eng", false))];
    assert_eq!(pick_dp_subtitle(&subs), None);
}

#[test]
fn subtitle_mode_none_never_turns_a_subtitle_on() {
    let subs = [flagged_default(sub(10, 3, "eng", false)), forced(sub(11, 4, "eng", false))];
    assert_eq!(pick_dp_subtitle_pref(&subs, subs_mode(SubtitleMode::None, Some("eng")), "fra"), None);
}

/// `Default`: what the file flags (default or forced), preferred language ranked first; an
/// unflagged file stays off.
#[test]
fn subtitle_mode_default_follows_the_files_flags() {
    let subs = [sub(10, 3, "eng", false), sub(11, 4, "fra", false)];
    assert_eq!(pick_dp_subtitle_pref(&subs, SubtitleLangPrefs::default(), "eng"), None);
    let subs = [forced(sub(10, 3, "eng", false)), flagged_default(sub(11, 4, "fra", false))];
    assert_eq!(pick_dp_subtitle_pref(&subs, SubtitleLangPrefs::default(), "eng"), Some((10, 0)));
    assert_eq!(pick_dp_subtitle_pref(&subs, subs_mode(SubtitleMode::Default, Some("fra")), "eng"), Some((11, 1)),
        "a flagged track in the preferred language ranks first");
}

/// `Always`: the preferred language's FULL track, not the forced one before it; with none in it,
/// what the file flags.
#[test]
fn subtitle_mode_always_takes_the_full_preferred_track() {
    let subs = [sub(10, 3, "eng", false), forced(sub(11, 4, "hun", false)), sub(12, 5, "hun", false)];
    assert_eq!(pick_dp_subtitle_pref(&subs, subs_mode(SubtitleMode::Always, Some("hun")), "hun"), Some((12, 2)));
    let subs = [sub(10, 3, "eng", false), flagged_default(sub(11, 4, "deu", false))];
    assert_eq!(pick_dp_subtitle_pref(&subs, subs_mode(SubtitleMode::Always, Some("hun")), "eng"), Some((11, 1)));
}

/// `Smart`: the preferred language when the audio is not in it; with the audio already in it,
/// only a forced track.
#[test]
fn subtitle_mode_smart_judges_by_the_audio_language() {
    let subs = [sub(10, 3, "eng", false), forced(sub(11, 4, "hun", false)), sub(12, 5, "hun", false)];
    let smart = subs_mode(SubtitleMode::Smart, Some("hun"));
    assert_eq!(pick_dp_subtitle_pref(&subs, smart, "eng").map(|p| p.0), Some(11),
        "foreign audio: the first preferred-language track");
    assert_eq!(pick_dp_subtitle_pref(&subs, smart, "hun"), Some((11, 1)),
        "audio already in the preferred language: forced only");
    let unforced = [sub(10, 3, "eng", false), sub(12, 5, "hun", false)];
    assert_eq!(pick_dp_subtitle_pref(&unforced, smart, "hun"), None);
}

#[test]
fn subtitle_mode_only_forced_ignores_full_tracks() {
    let subs = [sub(10, 3, "eng", false), forced(sub(11, 4, "eng", false))];
    assert_eq!(pick_dp_subtitle_pref(&subs, subs_mode(SubtitleMode::OnlyForced, None), "eng"), Some((11, 1)));
    assert_eq!(pick_dp_subtitle_pref(&subs[..1], subs_mode(SubtitleMode::OnlyForced, None), "eng"), None);
}

#[test]
fn automatic_subtitles_skip_unrenderable_tracks_without_enabling_a_burn() {
    let mut subs = [sub(1, 0, "fra", false), sub(2, 1, "fra", true), sub(3, 2, "fra", false)];
    subs[0].codec = "unsupported".into();
    let always = subs_mode(SubtitleMode::Always, Some("fra"));
    assert_eq!(pick_dp_subtitle_pref(&subs, always, "eng"), Some((3, 1)));
    assert_eq!(pick_dp_subtitle_pref(&subs[..2], always, "eng"), None);
}

#[test]
fn subtitle_modes_parse_jellyfins_names() {
    for (name, mode) in [
        ("Default", SubtitleMode::Default),
        ("Always", SubtitleMode::Always),
        ("OnlyForced", SubtitleMode::OnlyForced),
        ("None", SubtitleMode::None),
        ("Smart", SubtitleMode::Smart),
        ("", SubtitleMode::Default),
    ] {
        assert_eq!(SubtitleMode::parse(name), mode, "{name:?}");
    }
}

#[test]
fn preferred_language_tags_match_both_three_letter_spellings() {
    assert!(lang_matches("hu-HU", "hun"));
    assert!(lang_matches("de-DE", "ger") && lang_matches("de-DE", "deu"));
    assert!(lang_matches("pt-BR", "por"));
    assert!(lang_matches("en-GB", "eng"));
    assert!(!lang_matches("hu-HU", "eng"));
    assert!(!lang_matches("", "hun"));
    assert!(!lang_matches("hu-HU", ""));
    assert!(lang_matches("fre", "fra") && lang_matches("ger", "deu") && lang_matches("no", "nob"));
    assert!(!lang_matches("fre", "eng") && !lang_matches("", "") && !lang_matches("xx", "yy"));
}

#[test]
fn known_audio_channels_cannot_fall_back_to_an_unknown_codec_default() {
    let caps = nj_platform::devcaps::Caps { audio_channels: [("aac".into(), 6)].into(), ..nj_platform::devcaps::Caps::assumed() };
    let mut track = trk(1, "aac", "eng", false);
    track.channels = 8;
    assert!(caps.audio_supports("aac", 0), "legacy unknown remains compatible");
    assert_eq!(pick_dp_audio_eligible(&[track.clone()], "aac", AudioLangPrefs::default(), |c, n| caps.audio_supports(c, n)), None);
    track.channels = 6;
    assert_eq!(pick_dp_audio_eligible(&[track], "aac", AudioLangPrefs::default(), |c, n| caps.audio_supports(c, n)), Some((0, "aac".into(), 1)));
}
