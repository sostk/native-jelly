//! C3: one capability snapshot per play attempt, shared by the local route gates and the
//! Jellyfin DeviceProfile, and never the boot answer moved in place.
use super::*;
use super::test_support::*;

const HEVC_EAC3: &str = r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"eac3","Index":1,"Channels":6}"#;

fn eac3_profile_limit(body: &serde_json::Value) -> Option<String> {
    body["DeviceProfile"]["CodecProfiles"].as_array()?.iter()
        .find(|row| row["Type"] == "VideoAudio" && row["Codec"] == "eac3")
        .map(|row| row["Conditions"][0]["Value"].as_str().unwrap_or_default().to_string())
}

/// Resolve the loopback 4K HEVC / 5.1 E-AC3 item under `caps` and return the plan and the
/// PlaybackInfo body the server received.
#[cfg(feature = "devtriggers")]
fn resolve_under(caps: std::sync::Arc<nj_platform::devcaps::Caps>) -> (Plan, serde_json::Value) {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    assert!(nj_net::net::global_init());
    let lb = JfLoopback::start(playback_info(true, "mkv", HEVC_EAC3, None), user_config(None, true, None, "Default"));
    let rk = jf_rk();
    let mut env = ResolveEnv::snapshot(&ps, crate::stores::metadata::MetadataStore::default().view(), lb.sid, &rk);
    let mut track = eac3_track();
    track.id = 2;
    let mut item = fourk_item(lb.sid, vec![track]);
    item.rk = rk.clone();
    env.cached_item = Some(item);
    env.caps = caps;
    let part = format!("/Videos/{JF_GUID}/stream.mkv?static=true&MediaSourceId={JF_GUID}");
    let plan = build_stream(&rk, &part, "hevc", "eac3", &env);
    let requests = lb.finish();
    (plan, playback_info_body(&requests))
}

/// A refusal stored for this build narrows the attempt's snapshot, and BOTH consumers of the
/// attempt see it: the local direct-play gate stops offering the surround E-AC3 track and the
/// DeviceProfile carries the same stereo ceiling. The boot snapshot is untouched.
#[test]
#[cfg(feature = "devtriggers")]
fn a_stored_refusal_narrows_the_route_and_the_profile_from_one_snapshot() {
    let boot = nj_platform::devcaps::Caps::assumed();
    let _boot = nj_platform::devcaps::caps_on_this_thread_for_test(boot.clone());
    let _fw = nj_platform::devcaps::firmware_on_this_thread_for_test("kisscurl-koli", "B");
    let row = nj_platform::devcaps::Refusal { codec: "eac3".into(), channels: 6, build: "B".into() };

    let (plain, body) = resolve_under(nj_platform::devcaps::effective_caps(&[]));
    assert_eq!(body["EnableDirectPlay"], true, "the unnarrowed attempt offers the original");
    assert_eq!(eac3_profile_limit(&body), None);
    assert!(plain.caps.as_deref() == Some(&boot));

    let narrowed = nj_platform::devcaps::effective_caps(std::slice::from_ref(&row));
    let (plan, body) = resolve_under(narrowed.clone());
    assert_eq!(body["EnableDirectPlay"], false, "the local gate no longer offers 5.1 E-AC3 natively");
    assert_eq!(eac3_profile_limit(&body).as_deref(), Some("2"), "the profile states the same ceiling");
    assert!(plan.caps.as_deref() == Some(&*narrowed), "the plan carries the snapshot it was resolved under");
    assert_eq!(*nj_platform::devcaps::caps(), boot, "boot capabilities stay immutable");

    let _other = nj_platform::devcaps::firmware_on_this_thread_for_test("kisscurl-koli", "C");
    assert_eq!(*nj_platform::devcaps::effective_caps(&[row]), boot, "another build's row is not evidence");
}

/// The snapshot a plan landed with is the one every later gate of the SAME attempt reads, even
/// after the install learns something new; the next play start reads the new value.
#[test]
fn an_attempt_keeps_its_snapshot_for_track_switches() {
    let mut ps = PlaybackSession::IDLE;
    let mut boot = nj_platform::devcaps::Caps::assumed();
    boot.audio_channels.insert("eac3".into(), 6);
    let _boot = nj_platform::devcaps::caps_on_this_thread_for_test(boot.clone());
    assert!(audio_track_direct_plays(&ps, "eac3", 6), "no plan yet: the start value");
    ps.attempt_caps = Some(std::sync::Arc::new(boot.without_audio("eac3", 6)));
    assert!(!audio_track_direct_plays(&ps, "eac3", 6));
    assert!(audio_track_direct_plays(&ps, "eac3", 2));
    assert!(audio_track_direct_plays(&ps.publication(), "eac3", 2), "a screen copy carries it");
    assert!(!audio_track_direct_plays(&ps.publication(), "eac3", 6));
    assert_eq!(*attempt_caps(&PlaybackSession::IDLE), boot);
}
