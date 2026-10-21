//! Decoding off the UI thread: thumbnails, full-size images (animated ones keep every frame) and opening video
//! players (Media Foundation can take a moment to start). Results come back on a channel; the window is woken
//! so it picks them up at once.
//!
//! Each job runs on a thread of its own under a time limit: a decoder stuck on a hostile or enormous file (or
//! one that panics) is left to finish by itself, its result dropped, and the panel says why instead of waiting
//! forever. The decoders bound their own memory (`bro_media`).

use bro_media::{Image, Player, Rgba};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// The last number in `Image` and `Player` is the request's: a result for an older one is stale.
pub enum Job {
    Thumb(PathBuf),
    Image(PathBuf, u32, u64),
    Player(PathBuf, u32, u64),
}

pub enum Done {
    Thumb(PathBuf, Result<Rgba, String>),
    Image(PathBuf, u64, Result<Image, String>),
    Player(PathBuf, u64, Result<Player, String>),
}

/// Thumbnails fit inside this many pixels (shown at up to half of it, crisp on 2x screens).
pub const THUMB: u32 = 256;

/// How long a picture may take to decode (generous: a 100-megapixel photo or a long animation in a debug build).
const IMAGE_LIMIT: Duration = Duration::from_secs(25);
/// Opening a player has its own 30 s limit inside; this only catches a decoder that never returns.
const PLAYER_LIMIT: Duration = Duration::from_secs(45);
pub const TOO_SLOW: &str = "this file took too long to decode";
const CRASHED: &str = "the decoder gave up on this file";

impl Job {
    fn limit(&self) -> Duration {
        match self {
            Job::Player(..) => PLAYER_LIMIT,
            _ => IMAGE_LIMIT,
        }
    }

    /// The result to report when this job fails without one.
    fn failed(&self, e: &str) -> Done {
        match self {
            Job::Thumb(p) => Done::Thumb(p.clone(), Err(e.into())),
            Job::Image(p, _, g) => Done::Image(p.clone(), *g, Err(e.into())),
            Job::Player(p, _, g) => Done::Player(p.clone(), *g, Err(e.into())),
        }
    }

    fn run(self) -> Done {
        match self {
            Job::Thumb(p) => {
                let r = bro_media::thumbnail(&p, THUMB);
                Done::Thumb(p, r)
            }
            Job::Image(p, max, g) => {
                let r = bro_media::load_image(&p, max);
                Done::Image(p, g, r)
            }
            Job::Player(p, max, g) => {
                let r = Player::open(&p, max);
                Done::Player(p, g, r)
            }
        }
    }
}

/// Run `job` on a thread of its own and wait for it at most its limit.
fn run_limited(job: Job) -> Done {
    let limit = job.limit();
    run_within(job, limit, Job::run)
}

fn run_within(job: Job, limit: Duration, work: impl FnOnce(Job) -> Done + Send + 'static) -> Done {
    let (fallback_slow, fallback_crash) = (job.failed(TOO_SLOW), job.failed(CRASHED));
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new().name("bro-media-decode".into()).spawn(move || {
        let _ = tx.send(work(job));
    });
    if spawned.is_err() {
        return fallback_crash;
    }
    match rx.recv_timeout(limit) {
        Ok(done) => done,
        // still decoding: it finishes (or not) on its own; nobody waits for it
        Err(RecvTimeoutError::Timeout) => fallback_slow,
        // the decoder panicked
        Err(RecvTimeoutError::Disconnected) => fallback_crash,
    }
}

pub struct Loader {
    jobs: Sender<Job>,
    pub done: Receiver<Done>,
}

impl Loader {
    /// `wake` is called after each result (from a worker thread).
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Loader {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel::<Done>();
        let rx = Arc::new(parking_lot::Mutex::new(rx));
        // two workers: a slow video open doesn't hold up the thumbnails
        for i in 0..2 {
            let (rx, tx, wake) = (rx.clone(), tx.clone(), wake.clone());
            let _ = std::thread::Builder::new().name(format!("bro-media-load-{i}")).spawn(move || {
                loop {
                    let job = { rx.lock().recv() };
                    let Ok(job) = job else { break };
                    if tx.send(run_limited(job)).is_err() {
                        break;
                    }
                    wake();
                }
            });
        }
        Loader { jobs, done }
    }

    pub fn ask(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stuck_or_crashing_decoder_reports_instead_of_hanging() {
        let started = std::time::Instant::now();
        // a stand-in for a decoder stuck on a hostile file: its thread outlives the wait
        let stuck = run_within(Job::Image(PathBuf::from("x.png"), 64, 3), Duration::from_millis(60), |job| {
            std::thread::sleep(Duration::from_secs(3));
            job.run()
        });
        assert!(matches!(stuck, Done::Image(_, 3, Err(ref e)) if e == TOO_SLOW));
        assert!(started.elapsed() < Duration::from_secs(2), "waited for the stuck decoder");
        // a decoder that panics doesn't take the worker with it
        let crashed = run_within(Job::Thumb(PathBuf::from("y.png")), Duration::from_secs(5), |_| panic!("bad file"));
        assert!(matches!(crashed, Done::Thumb(_, Err(ref e)) if e == CRASHED));
    }

    #[test]
    fn results_carry_the_request_they_answer() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("missing.png");
        match run_limited(Job::Image(p.clone(), 64, 7)) {
            Done::Image(path, 7, Err(e)) => {
                assert_eq!(path, p);
                assert!(e.contains("doesn't exist"), "{e}");
            }
            _ => panic!("the image job answers with its own path and number"),
        }
    }
}
