//! PTY input can block when a child stops reading. Keep that wait off the UI thread.

use std::io::{self, Write};
use std::sync::mpsc::{self, Sender};

pub struct Writer {
    tx: Sender<Request>,
    #[cfg(test)]
    direct: Option<Box<dyn Write + Send>>,
}

struct Request {
    bytes: Vec<u8>,
    receipt: Option<Sender<io::Result<()>>>,
}

pub type Receipt = mpsc::Receiver<io::Result<()>>;

impl Writer {
    pub fn new(mut pipe: Box<dyn Write + Send>) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Request>();
        std::thread::Builder::new().name("bro-pty-input".into()).spawn(move || {
            while let Ok(request) = rx.recv() {
                let result = pipe.write_all(&request.bytes).and_then(|_| pipe.flush());
                let failed = result.is_err();
                if let Some(receipt) = request.receipt {
                    let _ = receipt.send(result);
                }
                if failed { break; }
            }
        })?;
        Ok(Writer { tx, #[cfg(test)] direct: None })
    }

    /// A paste's settle timer starts once the worker has actually written and flushed it.
    pub fn write_receipt(&mut self, bytes: &[u8]) -> io::Result<Receipt> {
        let (tx, rx) = mpsc::channel();
        #[cfg(test)]
        if let Some(pipe) = &mut self.direct {
            let _ = tx.send(pipe.write_all(bytes).and_then(|_| pipe.flush()));
            return Ok(rx);
        }
        self.tx.send(Request { bytes: bytes.to_vec(), receipt: Some(tx) })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY input closed"))?;
        Ok(rx)
    }

    #[cfg(test)]
    pub fn direct(pipe: Box<dyn Write + Send>) -> Self {
        let (tx, _rx) = mpsc::channel();
        Self { tx, direct: Some(pipe) }
    }
}

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(test)]
        if let Some(pipe) = &mut self.direct { return pipe.write(bytes); }
        if bytes.is_empty() {
            return Ok(0);
        }
        self.tx.send(Request { bytes: bytes.to_vec(), receipt: None }).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY input closed"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        #[cfg(test)]
        if let Some(pipe) = &mut self.direct { return pipe.flush(); }
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

    #[test]
    fn a_write_receipt_waits_until_the_pipe_has_finished() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (written_tx, written_rx) = mpsc::channel();
        let pipe = Blocked { entered: Some(entered_tx), release: release_rx, written: written_tx };
        let mut writer = Writer::new(Box::new(pipe)).unwrap();
        let receipt = writer.write_receipt(b"paste").unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(receipt.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release_tx.send(()).unwrap();
        assert_eq!(written_rx.recv_timeout(Duration::from_secs(2)).unwrap(), (true, b"paste".to_vec()));
        receipt.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    }
}
