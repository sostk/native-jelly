#!/usr/bin/env python3
"""A small Jellyfin REST mock for Native Jelly (password + Quick Connect + browse + PlaybackInfo).

Synthetic item ids are 32-hex strings. Titles stay in the closed alphabet (`s` + 8 hex digits)
so committed fixtures remain scrub-free. Serve until Ctrl-C:

    python3 tests/mock_jf.py --port 8096
"""
from __future__ import annotations

import argparse
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

USER_ID = "a" * 32
SERVER_ID = "b" * 32
TOKEN = "mock-jf-token"
MOVIE_ID = "c" * 32
SHOW_ID = "d" * 32
LIBRARY_ID = "e" * 32


def item(iid: str, itype: str, name: str, **extra) -> dict:
    row = {
        "Id": iid,
        "Name": name,
        "Type": itype,
        "ServerId": SERVER_ID,
        "ImageTags": {"Primary": "tag1"},
        "UserData": {"PlaybackPositionTicks": 0, "PlayCount": 0, "Played": False},
        "RunTimeTicks": 5_400_000_0000,
        "ProviderIds": {},
    }
    row.update(extra)
    return row


CATALOG = [
    item(LIBRARY_ID, "CollectionFolder", "s11111111", CollectionType="movies"),
    item(MOVIE_ID, "Movie", "s3fa90c12", ProductionYear=2011, PremiereDate="2011-03-14"),
    item(SHOW_ID, "Series", "s8bb12c00", ChildCount=1),
]


def json_bytes(obj) -> bytes:
    return json.dumps(obj).encode()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        print(f"mock_jf: {fmt % args}", flush=True)

    def _send(self, status: int, body: bytes, ctype="application/json"):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = urlparse(self.path).path.rstrip("/") or "/"
        if path == "/System/Info/Public":
            return self._send(200, json_bytes({
                "Id": SERVER_ID, "ServerName": "s00000001", "Version": "10.11.0",
                "StartupWizardCompleted": True,
            }))
        if path == "/Users/Me":
            return self._send(200, json_bytes({
                "Id": USER_ID, "Name": "suser0001", "ServerId": SERVER_ID,
            }))
        if path == "/UserViews":
            return self._send(200, json_bytes({"Items": [CATALOG[0]], "TotalRecordCount": 1}))
        if path == "/Items":
            movies = [CATALOG[1]]
            return self._send(200, json_bytes({"Items": movies, "TotalRecordCount": 1}))
        if path.startswith("/Items/") and path.endswith("/SpecialFeatures"):
            return self._send(200, json_bytes([]))
        if path.startswith("/Items/") and "/Images/" not in path:
            iid = path.split("/")[2]
            found = next((x for x in CATALOG if x["Id"] == iid), CATALOG[1])
            return self._send(200, json_bytes(found))
        if path in ("/UserItems/Resume", "/Shows/NextUp", "/Items/Latest"):
            return self._send(200, json_bytes({"Items": [CATALOG[1]], "TotalRecordCount": 1}
                                             if path != "/Items/Latest" else json_bytes([CATALOG[1]])))
        if path == "/Items/Filters2":
            return self._send(200, json_bytes({"Genres": []}))
        if path == "/QuickConnect/Enabled":
            return self._send(200, b"true")
        if path == "/QuickConnect/Connect":
            return self._send(200, json_bytes({"Authenticated": False}))
        if path.startswith("/Videos/") and "/stream" in path:
            self._send(200, b"\x00" * 16, "application/octet-stream")
            return
        self._send(404, b"{}")

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0") or 0)
        _ = self.rfile.read(length) if length else b""
        path = urlparse(self.path).path.rstrip("/") or "/"
        if path == "/Users/AuthenticateByName":
            return self._send(200, json_bytes({
                "AccessToken": TOKEN,
                "User": {"Id": USER_ID, "Name": "suser0001"},
                "SessionInfo": {"Id": "sess1"},
            }))
        if path == "/QuickConnect/Initiate":
            return self._send(200, json_bytes({"Secret": "sec", "Code": "123456"}))
        if path.endswith("/PlaybackInfo"):
            return self._send(200, json_bytes({
                "MediaSources": [{
                    "Id": MOVIE_ID,
                    "SupportsDirectPlay": True,
                    "SupportsDirectStream": True,
                    "SupportsTranscoding": True,
                    "Path": "file.mkv",
                    "Container": "mkv",
                    "MediaStreams": [],
                }],
                "PlaySessionId": "play1",
            }))
        if path.startswith("/Sessions/Playing"):
            return self._send(204, b"")
        self._send(404, b"{}")

    def do_DELETE(self):
        self._send(204, b"")


def serve(host: str, port: int):
    httpd = ThreadingHTTPServer((host, port), Handler)
    print(f"mock_jf: serving on http://{host}:{httpd.server_address[1]}", flush=True)
    httpd.serve_forever()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=8096)
    args = p.parse_args()
    serve(args.host, args.port)


if __name__ == "__main__":
    main()
