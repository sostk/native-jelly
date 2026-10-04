#!/usr/bin/env python3
"""Phase 1 live-server spikes against a real Jellyfin server.

Every request this client will make is exercised once and its raw answer written to
`tests/fixtures/jf/live/` (gitignored: it holds the server's own library titles, file paths and
ids, which do not belong in a public repository). The committed fixtures under
`tests/fixtures/jf/` are sanitized, hand-reduced copies of these.

Credentials come from the environment, never from argv or a tracked file:

    JF_URL=http://127.0.0.1:8096 JF_USER=… JF_PASS=… python3 tools/jf-spike.py

Use a dedicated TEST user: S5 starts real playback sessions and reports progress for it.
"""
from __future__ import annotations

import hashlib
import json
import os
import pathlib
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "tests" / "fixtures" / "jf" / "live"
CLIENT = "PlxJF-Spike"
VERSION = "0.0.1"


def device_id(user: str) -> str:
    # One token per DeviceId on Jellyfin: derive it per user so two users on one set can coexist.
    return hashlib.sha256(f"spike-device:{user}".encode()).hexdigest()[:32]


class Server:
    def __init__(self, base: str, user: str):
        self.base = base.rstrip("/")
        self.user = user
        self.token = ""
        self.user_id = ""

    def auth_header(self) -> str:
        fields = {
            "Client": CLIENT,
            "Device": "spike",
            "DeviceId": device_id(self.user),
            "Version": VERSION,
        }
        if self.token:
            fields["Token"] = self.token
        return "MediaBrowser " + ", ".join(
            f'{k}="{urllib.parse.quote(v, safe="")}"' for k, v in fields.items()
        )

    def request(self, method: str, path: str, body=None, headers=None, raw=False, timeout=20):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method)
        req.add_header("Authorization", self.auth_header())
        req.add_header("Accept", "application/json")
        if data is not None:
            req.add_header("Content-Type", "application/json")
        for k, v in (headers or {}).items():
            req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                payload = r.read()
                return r.status, dict(r.headers), payload if raw else _json(payload)
        except urllib.error.HTTPError as e:
            return e.code, dict(e.headers), e.read() if raw else None


def _json(b: bytes):
    if not b:
        return None
    try:
        return json.loads(b)
    except ValueError:
        return b.decode("utf-8", "replace")


def save(name: str, obj) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{name}.json").write_text(json.dumps(obj, indent=2, ensure_ascii=False))


def main() -> int:
    base = os.environ.get("JF_URL", "http://127.0.0.1:8096")
    user = os.environ.get("JF_USER")
    pw = os.environ.get("JF_PASS")
    if not user or pw is None:
        print("set JF_USER and JF_PASS (a dedicated test user)", file=sys.stderr)
        return 2
    s = Server(base, user)
    report: dict[str, object] = {}

    # ---- S1 auth ----
    st, _, info = s.request("GET", "/System/Info/Public")
    save("system_info_public", info)
    report["server_version"] = info.get("Version")
    report["legacy_emby_prefix_status"] = s.request("GET", "/emby/System/Info/Public")[0]
    st, _, auth = s.request("POST", "/Users/AuthenticateByName", {"Username": user, "Pw": pw})
    if st != 200:
        print(f"sign-in failed: {st}", file=sys.stderr)
        return 1
    s.token, s.user_id = auth["AccessToken"], auth["User"]["Id"]
    auth["AccessToken"] = "<redacted>"
    save("authenticate_by_name", auth)
    tok = s.token
    s.token = ""
    report["apikey_query_status"] = s.request("GET", f"/Users/Me?ApiKey={tok}")[0]
    report["legacy_api_key_query_status"] = s.request("GET", f"/Users/Me?api_key={tok}")[0]
    report["legacy_x_emby_token_status"] = s.request(
        "GET", "/Users/Me", headers={"X-Emby-Token": tok}
    )[0]
    s.token = tok
    report["quickconnect_enabled"] = s.request("GET", "/QuickConnect/Enabled")[2]
    report["quickconnect_get_initiate_status"] = s.request("GET", "/QuickConnect/Initiate")[0]
    report["public_user_count"] = len(s.request("GET", "/Users/Public")[2] or [])

    # ---- S7 browse ----
    _, _, views = s.request("GET", f"/UserViews?userId={s.user_id}")
    save("user_views", views)
    libs = {v["CollectionType"]: v["Id"] for v in views["Items"] if v.get("CollectionType")}
    fields = (
        "Overview,Genres,People,MediaSources,MediaStreams,Chapters,ProviderIds,"
        "Taglines,Studios,ParentId,RecursiveItemCount,ChildCount,DateCreated,"
        "PrimaryImageAspectRatio,OriginalTitle"
    )
    movie = series = None
    if "movies" in libs:
        q = urllib.parse.urlencode({
            "userId": s.user_id, "parentId": libs["movies"], "includeItemTypes": "Movie",
            "recursive": "true", "sortBy": "SortName", "sortOrder": "Ascending",
            "startIndex": 0, "limit": 5, "fields": fields, "enableTotalRecordCount": "true",
            "enableImageTypes": "Primary,Backdrop,Thumb", "imageTypeLimit": 1,
        })
        _, _, items = s.request("GET", f"/Items?{q}")
        save("items_movies_page", items)
        report["movies_total"] = items.get("TotalRecordCount")
        movie = next((i for i in items["Items"] if i.get("MediaSources")), None)
        _, _, letters = s.request(
            "GET",
            f"/Items?userId={s.user_id}&parentId={libs['movies']}&includeItemTypes=Movie"
            "&recursive=true&nameStartsWith=A&limit=0&enableTotalRecordCount=true",
        )
        report["movies_starting_with_A"] = letters.get("TotalRecordCount")
        _, _, filters = s.request(
            "GET", f"/Items/Filters2?userId={s.user_id}&parentId={libs['movies']}"
            "&includeItemTypes=Movie"
        )
        save("filters2_movies", filters)
    if "tvshows" in libs:
        _, _, shows = s.request(
            "GET", f"/Items?userId={s.user_id}&parentId={libs['tvshows']}"
            f"&includeItemTypes=Series&recursive=true&limit=5&fields={fields}"
        )
        save("items_series_page", shows)
        series = shows["Items"][0] if shows["Items"] else None
    for name, path in [
        ("resume", f"/UserItems/Resume?userId={s.user_id}&limit=12&fields={fields}"),
        ("next_up", f"/Shows/NextUp?userId={s.user_id}&limit=12&fields={fields}"),
        ("latest_movies", f"/Items/Latest?userId={s.user_id}&parentId={libs.get('movies','')}&limit=12"),
    ]:
        save(name, s.request("GET", path)[2])

    # ---- detail ----
    if movie:
        mid = movie["Id"]
        _, _, detail = s.request("GET", f"/Items/{mid}?userId={s.user_id}")
        save("item_movie_detail", detail)
        save("similar", s.request("GET", f"/Items/{mid}/Similar?userId={s.user_id}&limit=6")[2])
        save("special_features", s.request("GET", f"/Items/{mid}/SpecialFeatures?userId={s.user_id}")[2])
        report["movie_runtime_ticks"] = detail.get("RunTimeTicks")
        report["movie_has_blurhash"] = bool(detail.get("ImageBlurHashes"))
    if series:
        sid = series["Id"]
        _, _, seasons = s.request("GET", f"/Shows/{sid}/Seasons?userId={s.user_id}")
        save("seasons", seasons)
        if seasons and seasons.get("Items"):
            season_id = seasons["Items"][0]["Id"]
            _, _, eps = s.request(
                "GET", f"/Shows/{sid}/Episodes?userId={s.user_id}&seasonId={season_id}&fields={fields}"
            )
            save("episodes", eps)
            ep = (eps or {}).get("Items", [None])[0]
            if ep:
                # ---- S6 segments ----
                st, _, seg = s.request("GET", f"/MediaSegments/{ep['Id']}")
                save("media_segments_episode", seg)
                report["segments_status"] = st
                report["segment_types"] = sorted({x["Type"] for x in (seg or {}).get("Items", [])})

    # ---- S3/S4 playback ----
    if movie:
        mid = movie["Id"]
        src = movie["MediaSources"][0]
        _, _, pbi = s.request("POST", f"/Items/{mid}/PlaybackInfo", {
            "UserId": s.user_id, "MediaSourceId": src["Id"], "MaxStreamingBitrate": 120_000_000,
            "EnableDirectPlay": True, "EnableDirectStream": True, "EnableTranscoding": True,
            "AutoOpenLiveStream": False,
            "DeviceProfile": {
                "Name": "spike", "MaxStreamingBitrate": 120_000_000,
                "DirectPlayProfiles": [{"Type": "Video", "Container": "mkv,mp4",
                                        "VideoCodec": "h264,hevc", "AudioCodec": "aac,ac3,eac3"}],
                "TranscodingProfiles": [{"Type": "Video", "Container": "mkv", "Protocol": "http",
                                         "VideoCodec": "h264", "AudioCodec": "ac3",
                                         "Context": "Streaming"}],
                "SubtitleProfiles": [{"Format": "srt", "Method": "External"},
                                     {"Format": "ass", "Method": "External"},
                                     {"Format": "pgssub", "Method": "Embed"}],
            },
        })
        save("playback_info_movie", pbi)
        ms = pbi["MediaSources"][0]
        report["pbi_supports"] = {k: ms.get(k) for k in
                                  ("SupportsDirectPlay", "SupportsDirectStream", "SupportsTranscoding")}
        report["pbi_transcoding_url"] = (ms.get("TranscodingUrl") or "").split("?")[0]
        report["pbi_transcoding_url_auth_param"] = [
            k for k in urllib.parse.parse_qs(urllib.parse.urlsplit(ms.get("TranscodingUrl") or "").query)
            if k.lower() in ("api_key", "apikey")
        ]
        # ---- S2 range ----
        url = (f"/Videos/{mid}/stream?static=true&MediaSourceId={src['Id']}"
               f"&PlaySessionId={pbi.get('PlaySessionId','')}&ApiKey={s.token}")
        s_tok = s.token
        s.token = ""
        st, hdrs, body = s.request("GET", url, headers={"Range": "bytes=1000-1999"}, raw=True)
        s.token = s_tok
        report["direct_play_range"] = {"status": st, "len": len(body or b""),
                                       "content_range": hdrs.get("Content-Range"),
                                       "accept_ranges": hdrs.get("Accept-Ranges"),
                                       "content_type": hdrs.get("Content-Type")}
        # ---- S1 anonymous images ----
        tag = (movie.get("ImageTags") or {}).get("Primary")
        if tag:
            s.token = ""
            st, hdrs, img = s.request("GET", f"/Items/{mid}/Images/Primary?maxWidth=200&tag={tag}", raw=True)
            s.token = s_tok
            report["anonymous_image"] = {"status": st, "type": hdrs.get("Content-Type"), "len": len(img or b"")}
        # ---- ticks ----
        ps = pbi.get("PlaySessionId", "")
        start = {"ItemId": mid, "MediaSourceId": src["Id"], "PlaySessionId": ps,
                 "PositionTicks": 0, "PlayMethod": "DirectPlay", "CanSeek": True}
        report["playing_start"] = s.request("POST", "/Sessions/Playing", start)[0]
        prog = dict(start, PositionTicks=435_781_990, IsPaused=False)
        report["playing_progress"] = s.request("POST", "/Sessions/Playing/Progress", prog)[0]
        report["playing_ping"] = s.request("POST", f"/Sessions/Playing/Ping?playSessionId={ps}")[0]
        report["playing_stopped"] = s.request("POST", "/Sessions/Playing/Stopped",
                                              dict(start, PositionTicks=435_781_990))[0]
        time.sleep(0.5)
        _, _, after = s.request("GET", f"/UserItems/{mid}/UserData?userId={s.user_id}")
        save("user_data_after_progress", after)
        report["resume_ticks_after_report"] = (after or {}).get("PlaybackPositionTicks")
        # leave the test user's record as it was
        s.request("POST", f"/UserItems/{mid}/UserData?userId={s.user_id}",
                  {"PlaybackPositionTicks": 0})

    save("_report", report)
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
