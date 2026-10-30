//! One durable inbox owner. The lifetime kernel lock prevents overlapping Bro hosts from loading and
//! dispatching the same request. Only this worker performs file I/O; saves coalesce before acknowledgment.

use crate::pane::Event;
use parking_lot::{Condvar, Mutex};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, mpsc::Sender};
use std::time::Duration;

pub(crate) enum Update {
    Loaded {
        owner: String,
        saved: super::voice::SavedRoute,
    },
    Saved {
        owner: String,
        generation: u64,
    },
    Error {
        owner: String,
        message: String,
    },
}

#[derive(Default)]
struct Pending {
    save: Option<(u64, super::voice::SavedRoute)>,
    stop: bool,
    saved_generation: u64,
    error: Option<String>,
}

pub(crate) struct Store {
    pub owner: String,
    shared: Arc<(Mutex<Pending>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Store {
    pub fn start(path: PathBuf, tx: Sender<Event>) -> Self {
        let owner = uuid::Uuid::new_v4().to_string();
        let shared = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let state = shared.clone();
        let identity = owner.clone();
        let thread = std::thread::spawn(move || {
            let post = |update| {
                let _ = tx.send(Event::OrchestratorInbox(Box::new(update)));
            };
            let mut last_error = String::new();
            let _guard: File = loop {
                if state.0.lock().stop {
                    return;
                }
                match acquire(&path) {
                    Ok(guard) => break guard,
                    Err(e) => {
                        let message = format!(
                            "Orchestrator inbox is waiting for its saved-state owner: {e}. Queued work will not be sent twice."
                        );
                        if message != last_error {
                            post(Update::Error {
                                owner: identity.clone(),
                                message: message.clone(),
                            });
                            last_error = message;
                        }
                        let mut pending = state.0.lock();
                        state.1.wait_for(&mut pending, Duration::from_secs(1));
                    }
                }
            };
            let bytes = match std::fs::read(&path) {
                Ok(bytes) if bytes.len() <= 8 * 1024 * 1024 => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Ok(_) => {
                    post(Update::Error {owner:identity,message:"The saved orchestrator inbox exceeds its size limit. The file was preserved for recovery.".into()});
                    return;
                }
                Err(e) => {
                    post(Update::Error {
                        owner: identity,
                        message: format!(
                            "The orchestrator inbox could not be read: {e}. The file was preserved."
                        ),
                    });
                    return;
                }
            };
            let saved: super::voice::SavedRoute =
                match bytes.map(|b| serde_json::from_slice(&b)).transpose() {
                    Ok(saved) => saved.unwrap_or_default(),
                    Err(e) => {
                        post(Update::Error {
                            owner: identity,
                            message: format!(
                                "The saved inbox needs recovery: {e}. The file was preserved."
                            ),
                        });
                        return;
                    }
                };
            if let Err(e) = saved.validate() {
                post(Update::Error {
                    owner: identity,
                    message: format!(
                        "The saved inbox needs recovery: {e}. The file was preserved."
                    ),
                });
                return;
            }
            post(Update::Loaded {
                owner: identity.clone(),
                saved,
            });
            loop {
                let save = {
                    let mut pending = state.0.lock();
                    while pending.save.is_none() && !pending.stop {
                        state.1.wait(&mut pending);
                    }
                    if pending.stop && pending.save.is_none() {
                        break;
                    }
                    pending.save.take()
                };
                if let Some((generation, value)) = save {
                    let result = serde_json::to_vec(&value)
                        .map_err(std::io::Error::other)
                        .and_then(|bytes| durable_write(&path, &bytes));
                    match result {
                        Ok(()) => {
                            {
                                let mut pending = state.0.lock();
                                pending.saved_generation = generation;
                                pending.error = None;
                            }
                            post(Update::Saved {
                                owner: identity.clone(),
                                generation,
                            });
                        }
                        Err(e) => {
                            post(Update::Error {
                                owner: identity.clone(),
                                message: format!(
                                    "The orchestrator request was not sent because its inbox could not be saved: {e}"
                                ),
                            });
                            let mut pending = state.0.lock();
                            pending.error = Some(e.to_string());
                            if pending.stop {
                                break;
                            }
                            if pending.save.is_none() {
                                pending.save = Some((generation, value));
                            }
                            state.1.wait_for(&mut pending, Duration::from_secs(1));
                        }
                    }
                }
            }
        });
        Self {
            owner,
            shared,
            thread: Some(thread),
        }
    }

    pub fn save(&self, generation: u64, value: super::voice::SavedRoute) {
        self.shared.0.lock().save = Some((generation, value));
        self.shared.1.notify_one();
    }

    /// Called only while the app is closing; ordinary input and rendering never wait for disk.
    pub fn finish(&mut self, required: u64) -> Result<(), String> {
        self.shared.0.lock().stop = true;
        self.shared.1.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let pending = self.shared.0.lock();
        if pending.saved_generation >= required {
            Ok(())
        } else {
            Err(pending
                .error
                .clone()
                .unwrap_or_else(|| "The inbox closed before queued work could be saved".into()))
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.shared.0.lock().stop = true;
        self.shared.1.notify_one();
    }
}

fn acquire(path: &std::path::Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("owner.lock"))?;
    file.try_lock().map_err(std::io::Error::other)?;
    Ok(file)
}

fn durable_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("pending-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inbox_ownership_is_exclusive_and_survives_save_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inbox.json");
        let owner = acquire(&path).unwrap();
        crate::util::atomic_write(&path, b"{}").unwrap();
        assert!(
            acquire(&path).is_err(),
            "a second host must not load or dispatch this inbox"
        );
        drop(owner);
        assert!(
            acquire(&path).is_ok(),
            "OS releases ownership after the host stops"
        );
    }

    #[test]
    fn inbox_save_acknowledges_actual_durable_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inbox.json");
        let (tx, rx) = std::sync::mpsc::channel();
        let store = Store::start(path.clone(), tx);
        assert!(matches!(
            *match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::OrchestratorInbox(u) => u,
                _ => panic!(),
            },
            Update::Loaded { .. }
        ));
        store.save(
            7,
            serde_json::from_value(serde_json::json!({"completed":["request-a"]})).unwrap(),
        );
        let Event::OrchestratorInbox(update) = rx.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!()
        };
        assert!(matches!(*update, Update::Saved { generation: 7, .. }));
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["completed"][0], "request-a");
    }
}
