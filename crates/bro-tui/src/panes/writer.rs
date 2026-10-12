//! PTY input can block when a child stops reading. Keep that wait off the UI thread.

use std::io::{self, Write};
use std::sync::mpsc::{self, Sender};

pub struct Writer {
    tx: Sender<Vec<u8>>,
}

impl Writer {
    pub fn new(mut pipe: Box<dyn Write + Send>) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new().name("bro-pty-input".into()).spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if pipe.write_all(&bytes).and_then(|_| pipe.flush()).is_err() {
                    break;
                }
            }
        })?;
        Ok(Writer { tx })
    }
}

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.tx.send(bytes.to_vec()).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY input closed"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // The worker flushes each write in order; the UI never waits for the pipe.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Capture(Sender<Vec<u8>>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.send(bytes.to_vec()).unwrap();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.send(vec![]).unwrap();
            Ok(())
        }
    }

    #[test]
    fn queued_keys_and_terminal_replies_keep_their_order_and_flush() {
        let (tx, rx) = mpsc::channel();
        let mut writer = Writer::new(Box::new(Capture(tx))).unwrap();
        writer.write_all(b"hello").unwrap();
        writer.write_all(b"\x1b[12;3R").unwrap();
        writer.flush().unwrap();
        for expected in [b"hello".as_slice(), b"", b"\x1b[12;3R", b""] {
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), expected);
        }
    }

    struct Blocked {
        entered: Option<Sender<()>>,
        release: mpsc::Receiver<()>,
        written: Sender<(bool, Vec<u8>)>,
    }

    impl Write for Blocked {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let released = if let Some(entered) = self.entered.take() {
                entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(2)).is_ok()
            } else {
                true
            };
            self.written.send((released, bytes.to_vec())).unwrap();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }

    #[test]
    fn a_child_that_stops_reading_does_not_block_the_ui_producer() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (written_tx, written_rx) = mpsc::channel();
        let pipe = Blocked { entered: Some(entered_tx), release: release_rx, written: written_tx };
        let mut writer = Writer::new(Box::new(pipe)).unwrap();
        writer.write_all(b"first").unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        writer.write_all(b"second").unwrap();
        writer.flush().unwrap();
        let _ = release_tx.send(());
        assert_eq!(written_rx.recv_timeout(Duration::from_secs(2)).unwrap(), (true, b"first".to_vec()));
        assert_eq!(written_rx.recv_timeout(Duration::from_secs(2)).unwrap(), (true, b"second".to_vec()));
    }
}
