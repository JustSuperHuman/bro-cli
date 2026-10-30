use super::model::*;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const SCHEMA: u32 = 1;
const COMPACT_BYTES: u64 = 4 * 1024 * 1024;
const COMPACT_RECORDS: usize = 64;
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const RECEIPTS: usize = 512;

/// Each mutation reloads under a kernel file lock. Clones/processes share one authority.
/// No process-local cache can silently overwrite another bridge's state.
#[derive(Clone, Debug)]
pub struct Engine {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    schema: u32,
    head: String,
    checksum: String,
    state: State,
}

#[derive(Serialize, Deserialize)]
struct Record {
    schema: u32,
    seq: u64,
    previous: String,
    checksum: String,
    delta: Delta,
}

/// Only changed entities enter the journal. Lists here have fixed retention bounds.
#[derive(Serialize, Deserialize)]
struct Delta {
    seq: u64,
    retained_bytes: u64,
    intents: BTreeMap<String, Option<Intent>>,
    tasks: BTreeMap<String, Option<TaskView>>,
    attempts: BTreeMap<String, Option<Attempt>>,
    deliveries: BTreeMap<String, Option<Delivery>>,
    resources: BTreeMap<String, Option<ResourceLease>>,
    adapters: BTreeMap<String, Option<Adapter>>,
    checkpoints: BTreeMap<String, Option<Checkpoint>>,
    memory: BTreeMap<String, Option<Memory>>,
    receipts: BTreeMap<String, Option<Receipt>>,
    metrics: Metrics,
    policy: Policy,
    events: Vec<Event>,
    recovery: Vec<RecoveryNotice>,
}

fn patch<T: Clone + PartialEq>(
    a: &BTreeMap<String, T>,
    b: &BTreeMap<String, T>,
) -> BTreeMap<String, Option<T>> {
    let mut p = BTreeMap::new();
    for (key, value) in b {
        if a.get(key) != Some(value) {
            p.insert(key.clone(), Some(value.clone()));
        }
    }
    for key in a.keys() {
        if !b.contains_key(key) {
            p.insert(key.clone(), None);
        }
    }
    p
}
fn apply_patch<T>(map: &mut BTreeMap<String, T>, patch: BTreeMap<String, Option<T>>) {
    for (key, value) in patch {
        match value {
            Some(value) => {
                map.insert(key, value);
            }
            None => {
                map.remove(&key);
            }
        }
    }
}
impl Delta {
    fn between(a: &State, b: &State) -> Self {
        Self {
            seq: b.seq,
            retained_bytes: b.retained_bytes,
            intents: patch(&a.intents, &b.intents),
            tasks: patch(&a.tasks, &b.tasks),
            attempts: patch(&a.attempts, &b.attempts),
            deliveries: patch(&a.deliveries, &b.deliveries),
            resources: patch(&a.resources, &b.resources),
            adapters: patch(&a.adapters, &b.adapters),
            checkpoints: patch(&a.checkpoints, &b.checkpoints),
            memory: patch(&a.memory, &b.memory),
            receipts: patch(&a.receipts, &b.receipts),
            metrics: b.metrics.clone(),
            policy: b.policy.clone(),
            events: b.events.clone(),
            recovery: b.recovery.clone(),
        }
    }
    fn apply(self, state: &mut State) {
        state.seq = self.seq;
        state.retained_bytes = self.retained_bytes;
        apply_patch(&mut state.intents, self.intents);
        apply_patch(&mut state.tasks, self.tasks);
        apply_patch(&mut state.attempts, self.attempts);
        apply_patch(&mut state.deliveries, self.deliveries);
        apply_patch(&mut state.resources, self.resources);
        apply_patch(&mut state.adapters, self.adapters);
        apply_patch(&mut state.checkpoints, self.checkpoints);
        apply_patch(&mut state.memory, self.memory);
        apply_patch(&mut state.receipts, self.receipts);
        state.metrics = self.metrics;
        state.policy = self.policy;
        state.events = self.events;
        state.recovery = self.recovery;
    }
}

struct Loaded {
    state: State,
    head: String,
    records: usize,
}

impl Engine {
    pub fn open(data_root: impl AsRef<Path>) -> Result<Self> {
        let engine = Self {
            root: data_root.as_ref().join("coordination").join("v1"),
        };
        fs::create_dir_all(&engine.root).context("create coordination store")?;
        // Verify/recover now so corrupted authority never becomes an empty workspace.
        let _guard = engine.lock()?;
        engine.load()?;
        Ok(engine)
    }

    pub fn storage_path(&self) -> &Path {
        &self.root
    }

    fn lock(&self) -> Result<File> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("authority.lock"))?;
        f.lock().context("lock coordination authority")?;
        Ok(f)
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let _guard = self.lock()?;
        let loaded = self.load()?;
        Ok(super::scheduler::snapshot(&loaded.state))
    }

    pub fn task(&self, id: &str) -> Result<Option<TaskView>> {
        let _guard = self.lock()?;
        let loaded = self.load()?;
        Ok(loaded
            .state
            .tasks
            .get(id)
            .map(|task| super::scheduler::view(&loaded.state, task)))
    }

    pub fn context(&self, task_id: &str, now: i64) -> Result<ContextPacket> {
        let _guard = self.lock()?;
        super::context::packet(&self.load()?.state, task_id, now)
    }

    /// Fair ranked candidates; unknown required capabilities are not treated as demonstrated support.
    pub fn ready_tasks(&self, adapter_id: Option<&str>, now: i64) -> Result<Vec<TaskView>> {
        let _guard = self.lock()?;
        Ok(super::scheduler::ready(
            &self.load()?.state,
            adapter_id,
            now,
        ))
    }

    /// UI refresh omits dispatch payloads and old attempt histories; detail queries remain available.
    pub fn compact_snapshot(&self) -> Result<Snapshot> {
        let mut snapshot = self.snapshot()?;
        for delivery in &mut snapshot.deliveries {
            delivery.message.clear();
        }
        snapshot.attempts.retain(|a| !a.stopped);
        snapshot.events = snapshot.events.into_iter().rev().take(32).collect();
        Ok(snapshot)
    }

    pub fn execute(
        &self,
        command_id: &str,
        actor: &Actor,
        now: i64,
        command: Command,
    ) -> Result<Outcome> {
        super::transition::identifier(command_id)?;
        super::transition::identifier(&actor.id)?;
        ensure!(now >= 0, "invalid command timestamp");
        let fingerprint = super::evidence::digest_bytes(&serde_json::to_vec(&(actor, &command))?);
        let _guard = self.lock()?;
        let mut loaded = self.load()?;
        let archived_receipt = self.root.join("receipts").join(format!(
            "{}.json",
            super::evidence::digest_bytes(command_id.as_bytes())
        ));
        let receipt = match loaded.state.receipts.get(command_id) {
            Some(receipt) => Some(receipt.clone()),
            None if archived_receipt.exists() => Some(
                serde_json::from_slice::<Receipt>(&read_bounded(&archived_receipt)?)
                    .context("read archived command receipt")?,
            ),
            None => None,
        };
        if let Some(receipt) = receipt {
            ensure!(
                receipt.fingerprint == fingerprint,
                "command id was already used for a different command or actor"
            );
            let mut outcome = receipt.outcome;
            outcome.duplicate = true;
            return Ok(outcome);
        }
        let before = loaded.state.clone();
        let mut outcome = super::transition::apply(&mut loaded.state, actor, now, &command)?;
        loaded.state.seq = before
            .seq
            .checked_add(1)
            .context("coordination sequence exhausted")?;
        outcome.seq = loaded.state.seq;
        loaded.state.events.push(Event {
            seq: loaded.state.seq,
            at: now,
            actor: actor.id.clone(),
            kind: super::transition::command_name(&command).into(),
            task_id: outcome.task_id.clone(),
            attempt_id: outcome.attempt_id.clone(),
            detail: String::new(),
        });
        let excess = loaded.state.events.len().saturating_sub(256);
        loaded.state.events.drain(..excess);
        loaded.state.receipts.insert(
            command_id.into(),
            Receipt {
                fingerprint,
                outcome: outcome.clone(),
            },
        );
        while loaded.state.receipts.len() > RECEIPTS {
            let oldest = loaded
                .state
                .receipts
                .iter()
                .min_by_key(|(_, r)| r.outcome.seq)
                .map(|(id, _)| id.clone())
                .unwrap();
            let receipt = loaded.state.receipts.get(&oldest).unwrap();
            // Archive only receipts for transactions already committed. A failed new append cannot invent success.
            ensure!(
                receipt.outcome.seq <= before.seq,
                "receipt retention invariant"
            );
            durable_write(
                &self.root.join("receipts").join(format!(
                    "{}.json",
                    super::evidence::digest_bytes(oldest.as_bytes())
                )),
                &serde_json::to_vec(receipt)?,
            )?;
            loaded.state.receipts.remove(&oldest);
        }
        if let Command::ArchiveIntent { intent_id } = &command {
            let tasks: Vec<_> = before
                .tasks
                .values()
                .filter(|t| &t.intent_id == intent_id)
                .collect();
            let ids: std::collections::BTreeSet<_> = tasks.iter().map(|t| t.id.as_str()).collect();
            let archive = serde_json::json!({ "schema": SCHEMA, "archivedAt": now, "intent": before.intents.get(intent_id),
                "tasks": tasks, "attempts": before.attempts.values().filter(|a| ids.contains(a.task_id.as_str())).collect::<Vec<_>>(),
                "events": before.events.iter().filter(|e| e.task_id.as_deref().is_some_and(|id| ids.contains(id))).collect::<Vec<_>>() });
            durable_write(
                &self.root.join("archives").join(format!(
                    "{}-{}.json",
                    super::evidence::digest_bytes(intent_id.as_bytes()),
                    loaded.state.seq
                )),
                &serde_json::to_vec(&archive)?,
            )?;
        }
        self.append(&mut loaded, &before)?;
        Ok(outcome)
    }

    fn append(&self, loaded: &mut Loaded, before: &State) -> Result<()> {
        // Incremental retained-size accounting serializes changed entities only. Leave headroom for
        // JSON map framing, bounded events and snapshot metadata; never write an unreadable authority.
        let mut estimate = before.retained_bytes as i128;
        macro_rules! account {
            ($field:ident) => {
                for (key, value) in &loaded.state.$field {
                    if before.$field.get(key) != Some(value) {
                        estimate += serde_json::to_vec(&(key, value))?.len() as i128;
                        if let Some(old) = before.$field.get(key) {
                            estimate -= serde_json::to_vec(&(key, old))?.len() as i128;
                        }
                    }
                }
                for (key, old) in &before.$field {
                    if !loaded.state.$field.contains_key(key) {
                        estimate -= serde_json::to_vec(&(key, old))?.len() as i128;
                    }
                }
            };
        }
        account!(intents);
        account!(tasks);
        account!(attempts);
        account!(deliveries);
        account!(resources);
        account!(adapters);
        account!(checkpoints);
        account!(memory);
        account!(receipts);
        ensure!(
            (0..48 * 1024 * 1024).contains(&estimate),
            "coordination active storage limit reached; archive finished work"
        );
        loaded.state.retained_bytes = estimate as u64;
        let delta = Delta::between(before, &loaded.state);
        let checksum = super::evidence::digest_bytes(&serde_json::to_vec(&(
            SCHEMA,
            loaded.state.seq,
            &loaded.head,
            &delta,
        ))?);
        let record = Record {
            schema: SCHEMA,
            seq: loaded.state.seq,
            previous: loaded.head.clone(),
            checksum: checksum.clone(),
            delta,
        };
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() as u64 <= MAX_FILE_BYTES,
            "transaction exceeds store size limit"
        );
        let mut journal = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("journal.jsonl"))?;
        journal
            .write_all(&bytes)
            .context("append coordination transaction; outcome may need reconciliation")?;
        journal
            .sync_all()
            .context("sync coordination transaction; outcome may need reconciliation")?;
        #[cfg(unix)]
        {
            File::open(&self.root)?
                .sync_all()
                .context("sync coordination directory")?;
        }
        loaded.head = checksum;
        loaded.records += 1;
        if loaded.records >= COMPACT_RECORDS || journal.metadata()?.len() >= COMPACT_BYTES {
            // Commit is already durable. A compaction failure must not report that it failed to commit.
            if let Err(error) = self.compact(loaded) {
                // The intact journal remains the authority. Surface via a durable notice on next recovery.
                let _ = durable_write(
                    &self.root.join("maintenance-error.txt"),
                    error.to_string().as_bytes(),
                );
            }
        }
        Ok(())
    }

    fn compact(&self, loaded: &Loaded) -> Result<()> {
        let checksum = super::evidence::digest_bytes(&serde_json::to_vec(&(
            SCHEMA,
            &loaded.head,
            &loaded.state,
        ))?);
        let saved = Saved {
            schema: SCHEMA,
            head: loaded.head.clone(),
            checksum,
            state: loaded.state.clone(),
        };
        let bytes = serde_json::to_vec(&saved)?;
        ensure!(
            bytes.len() as u64 <= MAX_FILE_BYTES,
            "coordination snapshot exceeds size limit; journal remains authoritative"
        );
        durable_write(&self.root.join("snapshot.json"), &bytes)?;
        // If killed after the snapshot rename, old journal entries are safely skipped on reload.
        let journal = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(self.root.join("journal.jsonl"))?;
        journal.sync_all()?;
        Ok(())
    }

    fn load(&self) -> Result<Loaded> {
        let snapshot = self.root.join("snapshot.json");
        let mut loaded = if snapshot.exists() {
            let saved: Saved = serde_json::from_slice(&read_bounded(&snapshot)?)
                .context("invalid coordination snapshot; refusing to discard state")?;
            ensure!(
                saved.schema == SCHEMA,
                "unsupported coordination snapshot schema"
            );
            let checksum = super::evidence::digest_bytes(&serde_json::to_vec(&(
                saved.schema,
                &saved.head,
                &saved.state,
            ))?);
            ensure!(
                checksum == saved.checksum,
                "coordination snapshot checksum mismatch; recovery required"
            );
            Loaded {
                state: saved.state,
                head: saved.head,
                records: 0,
            }
        } else {
            Loaded {
                state: State::default(),
                head: "genesis".into(),
                records: 0,
            }
        };
        let path = self.root.join("journal.jsonl");
        if !path.exists() {
            return Ok(loaded);
        }
        let bytes = read_bounded(&path)?;
        let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        for line in bytes[..complete]
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
        {
            let record: Record = serde_json::from_slice(line)
                .context("corrupt committed coordination journal; recovery required")?;
            ensure!(
                record.schema == SCHEMA,
                "unsupported coordination journal schema"
            );
            let checksum = super::evidence::digest_bytes(&serde_json::to_vec(&(
                record.schema,
                record.seq,
                &record.previous,
                &record.delta,
            ))?);
            ensure!(
                checksum == record.checksum,
                "coordination journal checksum mismatch; recovery required"
            );
            if record.seq <= loaded.state.seq {
                continue;
            }
            ensure!(
                record.seq == loaded.state.seq + 1 && record.delta.seq == record.seq,
                "coordination journal sequence gap"
            );
            ensure!(
                record.previous == loaded.head,
                "coordination journal chain mismatch"
            );
            loaded.head = record.checksum;
            record.delta.apply(&mut loaded.state);
            loaded.records += 1;
        }
        if complete != bytes.len() {
            // A record without its commit newline was never committed. Keep its exact bytes before repair.
            let now = crate::util::now_secs();
            let preserved = self.root.join(format!(
                "torn-tail-{}-{:016x}.bin",
                now,
                rand::random::<u64>()
            ));
            durable_write(&preserved, &bytes[complete..])?;
            let file = OpenOptions::new().write(true).open(&path)?;
            file.set_len(complete as u64)?;
            file.sync_all()?;
            let before = loaded.state.clone();
            loaded.state.seq += 1;
            loaded.state.metrics.recoveries += 1;
            loaded.state.recovery.push(RecoveryNotice {
                at: now,
                kind: "uncommittedTail".into(),
                detail: "Recovered committed transactions; preserved the incomplete final record."
                    .into(),
                preserved_path: Some(preserved.to_string_lossy().into_owned()),
            });
            let excess = loaded.state.recovery.len().saturating_sub(32);
            loaded.state.recovery.drain(..excess);
            self.append(&mut loaded, &before)?;
        }
        if let Ok(note) = fs::read_to_string(self.root.join("maintenance-error.txt")) {
            let excess = loaded.state.recovery.len().saturating_sub(31);
            loaded.state.recovery.drain(..excess);
            loaded.state.recovery.push(RecoveryNotice {
                at: crate::util::now_secs(),
                kind: "maintenance".into(),
                detail: note,
                preserved_path: None,
            });
        }
        Ok(loaded)
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_FILE_BYTES,
        "coordination file exceeds size limit: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "coordination file grew beyond size limit"
    );
    Ok(bytes)
}

/// Sync the new content before replacement; Unix also syncs directory metadata.
fn durable_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("coordination file has no parent")?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".pending-{}-{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    let mut last = None;
    for i in 0..8 {
        match fs::rename(&tmp, path) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    File::open(parent)?.sync_all()?;
                }
                return Ok(());
            }
            Err(error) => {
                last = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(5 * (i + 1)));
            }
        }
    }
    let _ = fs::remove_file(&tmp);
    bail!(
        "could not replace coordination file {}: {}",
        path.display(),
        last.unwrap()
    );
}
