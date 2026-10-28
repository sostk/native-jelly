//! Optional trickplay jobs on the existing poster workers. No disk entries or full-sheet uploads.
use crate::catalog::{ArtFetch, Client};
use crate::jf::trickplay::{self, LayoutId, Request, Spec};

/// One compressed sheet per worker (at most two workers), each bounded to 4 MiB.
/// Repeated tiles in the same sheet reuse network bytes. Decoded sheets are not retained.
const SHEET_BYTES_MAX: usize = 4 * 1024 * 1024;
const MANIFEST_BYTES_MAX: usize = 512 * 1024;
#[derive(Default)]
pub(super) struct Cache {
    owner: Option<LayoutId>,
    sheet: Option<(u64, Vec<u8>)>,
}
pub(super) enum Got {
    Ready(u32, u32, Vec<u8>),
    Final,
    Transient,
    Cancelled,
}
fn transient(status: i32) -> bool {
    status == 408 || status == 429 || status >= 500
}

impl Cache {
    pub(super) fn load(
        &mut self,
        c: &Client,
        id: LayoutId,
        req: &Request,
        current: impl Fn() -> bool,
    ) -> Got {
        if !current() {
            return Got::Cancelled;
        }
        if self.owner.as_ref() != Some(&id) {
            self.owner = Some(id.clone());
            self.sheet = None;
        }
        let Some(j) = c.jf() else {
            return Got::Final;
        };
        let spec = match trickplay::known(&id) {
            Some(s) => s,
            None => {
                let Some(uid) = j.user_id() else {
                    return Got::Final;
                };
                let path = format!(
                    "/Items/{}?Fields=Trickplay&userId={}",
                    req.owner.item,
                    crate::catalog::urlenc_str(&uid)
                );
                let value = match j.preview_bytes(&path, MANIFEST_BYTES_MAX) {
                    ArtFetch::Bytes(b) => serde_json::from_slice::<serde_json::Value>(&b).ok(),
                    ArtFetch::Status(s) if !transient(s) => None,
                    _ => return Got::Transient,
                };
                if !current() {
                    return Got::Cancelled;
                }
                let spec = value
                    .filter(|v| {
                        v.get("Id")
                            .and_then(|v| v.as_str())
                            .is_some_and(|g| crate::jf::ids::normalize(g) == req.owner.item)
                    })
                    .and_then(|v| trickplay::select(&v, &req.owner));
                trickplay::publish(id.clone(), spec);
                spec
            }
        };
        let Some(spec) = spec else {
            return Got::Final;
        };
        self.tile(c, &id, req, spec, current)
    }
    fn tile(
        &mut self,
        c: &Client,
        id: &LayoutId,
        req: &Request,
        spec: Spec,
        current: impl Fn() -> bool,
    ) -> Got {
        let Some(tile) = spec.tile(req.ms) else {
            return Got::Final;
        };
        if !current() {
            return Got::Cancelled;
        }
        if self.sheet.as_ref().is_none_or(|(n, _)| *n != tile.sheet) {
            self.sheet = None;
            let j = c.jf().expect("checked by load");
            let path = trickplay::sheet_path(&req.owner, spec, tile.sheet);
            let bytes = match j.preview_bytes(&path, SHEET_BYTES_MAX) {
                ArtFetch::Bytes(b) => b,
                ArtFetch::Status(s) if !transient(s) => return Got::Final,
                _ => return Got::Transient,
            };
            if !current() {
                return Got::Cancelled;
            }
            self.sheet = Some((tile.sheet, bytes));
        }
        if c.token_gen() != id.token || !current() {
            return Got::Cancelled;
        }
        match nj_gfx::img::img_decode_region(
            &self.sheet.as_ref().unwrap().1,
            spec.width * spec.tile_width,
            spec.height * spec.tile_height,
            (tile.x, tile.y, tile.w, tile.h),
        ) {
            Some((w, h, px)) => Got::Ready(w, h, px),
            None => {
                self.sheet = None;
                Got::Final
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };
    const ITEM: &str = "0123456789abcdef0123456789abcdef";
    const SOURCE: &str = "11111111111111111111111111111111";
    struct Server {
        port: i32,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
    }
    impl Server {
        fn new(manifest: serde_json::Value) -> Self {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port() as i32;
            l.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let got = requests.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            // Two coloured cells, plus a partial final sheet. The selected region must be exact.
            let sheet = nj_gfx::img::img_encode_png(
                4,
                2,
                &[
                    255, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 255, 0, 0, 255,
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255,
                ],
            )
            .unwrap();
            let join = std::thread::spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    match nj_base::testnet::accept(&l) {
                        Ok((mut socket, _)) => {
                            socket.set_nonblocking(false).unwrap();
                            socket
                                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                                .unwrap();
                            let mut reader = BufReader::new(socket.try_clone().unwrap());
                            let mut request = String::new();
                            reader.read_line(&mut request).unwrap();
                            let mut headers = String::new();
                            loop {
                                let mut h = String::new();
                                reader.read_line(&mut h).unwrap();
                                if h == "\r\n" || h.is_empty() {
                                    break;
                                }
                                headers.push_str(&h);
                            }
                            assert!(
                                headers.contains("Authorization: MediaBrowser "),
                                "authenticated thumbnail request"
                            );
                            assert!(headers.contains("Token=\"fixture\""));
                            let path = request.split_whitespace().nth(1).unwrap().to_owned();
                            assert!(!path.contains("ApiKey") && !path.contains("X-Plex-Token"));
                            got.lock().unwrap().push(path.clone());
                            let body = if path.starts_with("/Items/") {
                                manifest.to_string().into_bytes()
                            } else {
                                assert!(path.contains(&format!("mediaSourceId={SOURCE}")));
                                sheet.clone()
                            };
                            write!(socket,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                            socket.write_all(&body).unwrap();
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(2))
                        }
                        Err(e) => panic!("accept: {e}"),
                    }
                }
            });
            Self {
                port,
                requests,
                stop,
                join: Some(join),
            }
        }
        fn client(&self) -> &'static Client {
            let sid = crate::catalog::register_for_test(
                "trickplay-fixture",
                "127.0.0.1",
                self.port,
                "fixture",
                "fixture",
            );
            let c = crate::catalog::client_for(sid).unwrap();
            crate::jf::seat::register_with(
                c.origin(),
                crate::jf::seat::Seat {
                    user_id: ITEM.into(),
                    ..Default::default()
                },
            );
            c
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.join.take().unwrap().join().unwrap();
        }
    }
    fn manifest() -> serde_json::Value {
        serde_json::json!({"Id":ITEM,"Trickplay":{SOURCE:{"2":{
        "Width":2,"Height":2,"TileWidth":2,"TileHeight":1,"ThumbnailCount":4,"Interval":10_000}}}})
    }
    fn request(c: &Client, ms: i64) -> (LayoutId, Request) {
        let owner = trickplay::Owner::new(ITEM, SOURCE, "HTTP fixture").unwrap();
        (
            LayoutId {
                srv: 0,
                token: c.token_gen(),
                cache: nj_platform::imgcache::generation(),
                owner: owner.clone(),
            },
            Request { owner, ms },
        )
    }
    #[test]
    fn trickplay_worker_authenticates_crops_and_reuses_a_compressed_sheet() {
        let _g = nj_base::testlock::serial();
        let server = Server::new(manifest());
        let c = server.client();
        let mut cache = Cache::default();
        for (ms, rgb) in [
            (0, [255, 0, 0]),
            (10_000, [0, 255, 0]),
            (20_000, [255, 0, 0]),
        ] {
            let (id, req) = request(c, ms);
            match cache.load(c, id, &req, || true) {
                Got::Ready(w, h, p) => {
                    assert_eq!((w, h), (2, 2));
                    assert!(p.chunks_exact(4).all(|v| v[..3] == rgb));
                }
                _ => panic!("expected decoded tile"),
            }
        }
        let got = server.requests.lock().unwrap();
        assert_eq!(
            got.len(),
            3,
            "one manifest and two sheets, not one request per still"
        );
        assert!(got[0].contains("Fields=Trickplay"));
        assert!(got[1].contains("/2/0.jpg"));
        assert!(got[2].contains("/2/1.jpg"));
    }
    #[test]
    fn trickplay_worker_missing_manifest_is_quiet_and_cancellation_skips_the_sheet() {
        let _g = nj_base::testlock::serial();
        {
            let server = Server::new(serde_json::json!({"Id":ITEM}));
            let c = server.client();
            let mut cache = Cache::default();
            for ms in [0, 10_000] {
                let (id, req) = request(c, ms);
                assert!(matches!(cache.load(c, id, &req, || true), Got::Final));
            }
            assert_eq!(server.requests.lock().unwrap().len(), 1);
        }
        {
            let server = Server::new(manifest());
            let c = server.client();
            let mut cache = Cache::default();
            let (id, req) = request(c, 0);
            let checks = AtomicUsize::new(0);
            assert!(matches!(
                cache.load(c, id, &req, || checks.fetch_add(1, Ordering::Relaxed) == 0),
                Got::Cancelled
            ));
            assert_eq!(
                server.requests.lock().unwrap().len(),
                1,
                "cancelled after metadata, before thumbnail fetch"
            );
        }
    }
    #[test]
    fn trickplay_source_cancellation_invalidates_queued_and_late_loading_results() {
        use crate::app::adapters::poster as source;
        use crate::ui::tex::Source;
        let _g = nj_base::testlock::serial();
        let server = Server::new(manifest());
        let c = server.client();
        let (id, req) = request(c, 0);
        let sid = crate::catalog::current_server();
        let path = req.owner.key(0);
        source::reset_key_memo();
        crate::ui::tex::reset_for_test(crate::ui::tex::TEX_RESIDENT_BYTES_MAX);
        for state in [
            source::P_WANT,
            source::P_LOADING,
            source::P_DECODED,
            source::P_READY,
        ] {
            {
                let mut g = source::store();
                g.slots = [source::Pslot::ZERO; source::PT_CAP];
                g.slots[0] = source::Pslot {
                    srv: sid,
                    state,
                    gen: 91,
                    cache_gen: id.cache,
                    token_gen: id.token,
                    ..source::Pslot::ZERO
                };
                source::set_key(&mut g.slots[0], &path);
            }
            source::PosterSource.forget(sid.raw(), &path, 320, 180, false);
            let g = source::store();
            assert_eq!(g.slots[0].state, source::P_EMPTY);
            assert_ne!(
                g.slots[0].gen, 91,
                "a delayed worker owns an obsolete generation"
            );
        }
        source::store().slots = [source::Pslot::ZERO; source::PT_CAP];
        source::reset_key_memo();
    }
}
