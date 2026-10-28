//! Real HTTP queue contract, using invented series GUIDs and no household data.
use super::*;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

fn guid(index: usize) -> String { format!("9aab{index:09x}0000000000000000000") }
fn episode(index: usize) -> serde_json::Value {
    serde_json::json!({"Id": guid(index), "Type": "Episode", "Name": "Synthetic episode",
        "SeriesId": guid(100), "SeriesName": "Synthetic series", "IndexNumber": index,
        "ParentIndexNumber": 1, "MediaSources": [{"Id":guid(index),"Container":"mkv"}]})
}
struct Server { port: i32, invalid: Arc<AtomicBool>, stop: Arc<AtomicBool>, join: Option<std::thread::JoinHandle<()>> }
impl Server {
    fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port() as i32;
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let invalid = Arc::new(AtomicBool::new(false));
        let bad = invalid.clone();
        let join = std::thread::spawn(move || while !stopped.load(Ordering::Relaxed) {
            match nj_base::testnet::accept(&listener) {
                Ok((mut socket, _)) => {
                    socket.set_nonblocking(false).unwrap();
                    socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                    let mut reader = BufReader::new(socket.try_clone().unwrap());
                    let mut request = String::new();
                    reader.read_line(&mut request).unwrap();
                    loop { let mut header = String::new(); reader.read_line(&mut header).unwrap();
                        if header == "\r\n" || header.is_empty() { break; } }
                    let path = request.split_whitespace().nth(1).unwrap();
                    let body = if path.starts_with("/Items/") {
                        let id = path.split('/').nth(2).unwrap().split('?').next().unwrap();
                        episode((1..=4).find(|&i| guid(i) == id).unwrap())
                    } else {
                        assert!(path.contains("AdjacentTo="), "bounded neighbour query");
                        assert!(path.contains("Limit=3") && path.contains("IsMissing=false"));
                        let i = (1..=4).find(|&i| path.contains(&format!("AdjacentTo={}", guid(i)))).unwrap();
                        serde_json::json!({"Items": if bad.load(Ordering::Relaxed) { vec![episode(100)] }
                            else { ((i.saturating_sub(1).max(1))..=(i+1).min(4)).map(episode).collect::<Vec<_>>() }})
                    }.to_string();
                    write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(2)),
                Err(e) => panic!("accept: {e}"),
            }
        });
        Self { port, invalid, stop, join: Some(join) }
    }
}
impl Drop for Server { fn drop(&mut self) {
    self.stop.store(true, Ordering::Relaxed);
    self.join.take().unwrap().join().unwrap();
} }

#[test]
fn refreshed_neighbourhood_keeps_previous_after_next_and_marks_the_selected_row() {
    let _g = nj_base::testlock::serial();
    let server = Server::new();
    let sid = crate::catalog::register_for_test("navigation-fixture", "127.0.0.1", server.port, "fixture", "fixture");
    let client = crate::catalog::client_for(sid).unwrap();
    super::super::seat::register_with(client.origin(), super::super::seat::Seat { user_id: guid(99), ..Default::default() });
    let rk2 = ids::intern(&guid(2)).to_string();
    let q = client.create_play_queue(&rk2, true).unwrap();
    assert_eq!(q.items.len(), 3);
    let at = crate::catalog::queue_index_of(&q.items, q.selected_item_id, sid, &rk2).unwrap();
    assert_eq!(at, 1, "middle episode remains selected");
    assert_eq!(q.items[0].index, 1);
    let next = q.next.unwrap();
    assert_eq!(next.index, 3);
    let q3 = client.create_play_queue(&next.rk, true).unwrap();
    let at3 = crate::catalog::queue_index_of(&q3.items, q3.selected_item_id, sid, &next.rk).unwrap();
    assert_eq!(q3.items[at3-1].rk, rk2, "Previous returns to the outgoing episode");
    assert_eq!(q3.items[at3-1].item_id, q.selected_item_id, "stable client queue identity");
    let lone = client.create_play_queue(&rk2, false).unwrap();
    assert_eq!(lone.items.len(), 1, "extras do not acquire neighbours");
    let first = client.create_play_queue(&ids::intern(&guid(1)).to_string(), true).unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.items[0].item_id, first.selected_item_id);
    let last = client.create_play_queue(&ids::intern(&guid(4)).to_string(), true).unwrap();
    assert!(last.next.is_none());
    assert_eq!(last.items[1].item_id, last.selected_item_id);
    server.invalid.store(true, Ordering::Relaxed);
    let missing = client.create_play_queue(&rk2, true).unwrap();
    assert_eq!(missing.items.len(), 1, "a missing selection cannot become a different episode");
    assert_eq!(missing.items[0].rk, rk2);
    assert!(missing.next.is_none());
    super::super::seat::forget(client.origin());
}
