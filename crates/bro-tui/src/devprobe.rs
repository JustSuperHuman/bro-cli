//! Is anything listening behind the localhost addresses sessions mention (a dev server's
//! `http://localhost:5173/`)? The UI asks [`Probe::up`] and gets the last answer at once; a background thread
//! connects to each port asked about lately, every couple of seconds, and wakes the loop when an answer
//! changes. Nothing here blocks the TUI thread.

use crate::pane::Event;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// How often a port being asked about is checked again.
const EVERY: Duration = Duration::from_secs(2);
/// How long one connection attempt may take.
const CONNECT: Duration = Duration::from_millis(150);
/// A port nobody asked about for this long isn't checked any more.
const FORGET: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
struct Entry {
    up: Option<bool>,
    asked: Instant,
}

/// Shared answers, keyed by (host, port).
#[derive(Clone, Default)]
pub struct Probe {
    ports: Arc<Mutex<HashMap<(String, u16), Entry>>>,
}

impl Probe {
    /// Start checking in the background, waking the loop through `tx` when an answer changes.
    pub fn start(tx: Sender<Event>) -> Probe {
        let me = Probe::default();
        let ports = me.ports.clone();
        let _ = std::thread::Builder::new().name("bro-devprobe".into()).spawn(move || run(ports, tx));
        me
    }

    /// Whether something answers at `url`'s host and port: the last answer (None until the first check).
    pub fn up(&self, url: &str) -> Option<bool> {
        let key = key(url)?;
        let mut ports = self.ports.lock();
        let e = ports.entry(key).or_insert(Entry { up: None, asked: Instant::now() });
        e.asked = Instant::now();
        e.up
    }

    #[cfg(test)]
    pub fn set(&self, url: &str, up: bool) {
        if let Some(k) = key(url) {
            self.ports.lock().insert(k, Entry { up: Some(up), asked: Instant::now() });
        }
    }
}

fn key(url: &str) -> Option<(String, u16)> {
    let u = url::Url::parse(url).ok()?;
    Some((u.host_str()?.trim_matches(['[', ']']).to_string(), u.port_or_known_default()?))
}

fn listening(host: &str, port: u16) -> bool {
    // "localhost" may be either family: any address that takes the connection counts
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
    addrs.iter().any(|a| TcpStream::connect_timeout(a, CONNECT).is_ok())
}

fn run(ports: Arc<Mutex<HashMap<(String, u16), Entry>>>, tx: Sender<Event>) {
    loop {
        std::thread::sleep(EVERY);
        let due: Vec<(String, u16)> = {
            let mut p = ports.lock();
            p.retain(|_, e| e.asked.elapsed() < FORGET);
            p.keys().cloned().collect()
        };
        let mut changed = false;
        for (host, port) in due {
            let up = listening(&host, port);
            if let Some(e) = ports.lock().get_mut(&(host, port))
                && e.up != Some(up)
            {
                e.up = Some(up);
                changed = true;
            }
        }
        if changed && tx.send(Event::Tick).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_by_host_and_port() {
        let p = Probe::default();
        assert_eq!(p.up("http://localhost:5173/"), None, "not checked yet");
        p.set("http://localhost:5173/x", true);
        assert_eq!(p.up("http://localhost:5173/other"), Some(true), "same port, same answer");
        assert_eq!(p.up("http://localhost:3000"), None);
        assert_eq!(key("http://[::1]:8080/"), Some(("::1".into(), 8080)));
        assert_eq!(key("https://example.com"), Some(("example.com".into(), 443)));
    }

    #[test]
    fn a_listening_port_answers() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(listening("127.0.0.1", port));
        drop(l);
        assert!(!listening("127.0.0.1", port));
    }
}
