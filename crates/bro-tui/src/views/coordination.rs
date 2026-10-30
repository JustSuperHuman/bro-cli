//! Durable outcomes alongside the orchestrator. All storage and bridge work runs on one background worker;
//! render paths only use bounded snapshots. A stopped agent is never presented as an accepted task.

#[cfg(feature = "gui")]
pub(crate) mod gui;

use crate::folder::{self, FolderPicker};
use crate::pane::{Cx, Pane, Waker};
use crate::ui::{self, fg, muted};
use bro_core::coordination::{
    Command, ContextPacket, Criterion, CriterionKind, NewIntent, NewTask, QualityMode, Snapshot,
    TaskContract, TaskState, TaskView,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

enum Job {
    Refresh,
    Commands(Vec<Command>),
    Context(String),
    Stop,
}
enum Update {
    Snapshot(Result<Snapshot, String>),
    Context(Result<ContextPacket, String>),
    Action(Result<(), String>),
    Refreshed,
}

struct Worker {
    tx: SyncSender<Job>,
    rx: Receiver<Update>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.try_send(Job::Stop);
    }
}

impl Worker {
    fn start(svc: crate::services::Services, waker: Waker) -> Self {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (updates, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut previous: Option<Result<Snapshot, String>> = None;
            let mut refreshing = false;
            loop {
                let next = svc.coordination_snapshot();
                if previous.as_ref() != Some(&next) {
                    previous = Some(next.clone());
                    if updates.send(Update::Snapshot(next)).is_err() {
                        break;
                    }
                    waker.wake();
                }
                if refreshing {
                    let _ = updates.send(Update::Refreshed);
                    waker.wake();
                    refreshing = false;
                }
                match jobs.recv_timeout(Duration::from_secs(3)) {
                    Ok(Job::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(Job::Commands(commands)) => {
                        let batch_id = uuid::Uuid::new_v4().to_string();
                        let result =
                            commands
                                .into_iter()
                                .enumerate()
                                .try_for_each(|(i, command)| {
                                    svc.coordination_execute(&format!("ui-{batch_id}-{i}"), command)
                                        .map(|_| ())
                                });
                        let _ = updates.send(Update::Action(result));
                        waker.wake();
                    }
                    Ok(Job::Context(id)) => {
                        let _ = updates.send(Update::Context(svc.coordination_context(&id)));
                        waker.wake();
                    }
                    Ok(Job::Refresh) => refreshing = true,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });
        Self { tx, rx }
    }
}

#[derive(Default, Clone, PartialEq)]
pub(crate) struct Counts {
    pub queued: usize,
    pub running: usize,
    pub blocked: usize,
    pub review: usize,
    pub accepted: usize,
    pub failed: usize,
}
impl Counts {
    fn from_snapshot(s: &Snapshot) -> Self {
        let mut c = Self::default();
        for task in &s.tasks {
            match task.state {
                TaskState::Pending | TaskState::Ready => c.queued += 1,
                TaskState::Assigned | TaskState::Running | TaskState::CancelRequested => {
                    c.running += 1
                }
                TaskState::Blocked | TaskState::Uncertain => c.blocked += 1,
                TaskState::Submitted | TaskState::Verifying => c.review += 1,
                TaskState::Verified => c.accepted += 1,
                TaskState::Failed => c.failed += 1,
                _ => {}
            }
        }
        c
    }
    fn full(&self) -> String {
        format!(
            "{} queued   {} working   {} waiting   {} review   {} accepted   {} failed",
            self.queued, self.running, self.blocked, self.review, self.accepted, self.failed
        )
    }
}

pub(super) struct Form {
    task: Option<TaskView>,
    pub goal: String,
    pub acceptance: String,
    /// The task's project folder. A new task starts from where New Session would open; an existing task keeps its own.
    pub project: String,
    pub focus: usize,
    pub quality: QualityMode,
    /// The shared project chooser, open over the form.
    pub(super) picker: Option<FolderPicker>,
}

pub struct CoordinationView {
    worker: Option<Worker>,
    pub(super) snapshot: Option<Snapshot>,
    pub(super) context: Option<ContextPacket>,
    pub(super) error: Option<String>,
    pub(super) notice: Option<String>,
    pub(super) selected: usize,
    pub(super) show_finished: bool,
    pub(super) context_page: bool,
    pub(super) technical: bool,
    pub(super) form: Option<Form>,
    pub(super) busy: bool,
    pub(super) detail_scroll: usize,
    pub(super) list_top: usize,
    owners: std::collections::HashMap<String, String>,
    owners_version: u64,
    row_indices: Vec<usize>,
    hits: Vec<(Rect, usize)>,
    demo: bool,
    /// The count line, rebuilt when the snapshot changes rather than every frame.
    pub(super) summary: String,
    pub(super) tally: Counts,
    /// The detail lines for the current selection; cleared by anything that changes them.
    detail_lines: Option<Vec<(String, bool)>>,
    /// Wrapped height of each detail line at one width and scale (the GUI fills it).
    pub(super) detail_heights: Option<((u32, u32), Vec<f32>)>,
}

impl CoordinationView {
    pub(super) fn action_enabled(&self, action: usize) -> bool {
        if self.busy {
            return false;
        }
        match action {
            1 | 2 | 3 | 7 | 8 | 10 | 11 | 21..=23 => true,
            15 => self.form.as_ref().is_some_and(|f| f.task.is_none()),
            4 => self.current().is_some(),
            5 => self.current().is_some_and(|t| {
                !matches!(
                    t.state,
                    TaskState::Verified
                        | TaskState::Cancelled
                        | TaskState::Stopped
                        | TaskState::CancelRequested
                )
            }),
            6 => self.current().is_some_and(|t| {
                matches!(
                    t.state,
                    TaskState::Failed
                        | TaskState::Stopped
                        | TaskState::Submitted
                        | TaskState::Verifying
                ) && self.snapshot.as_ref().is_some_and(|s| {
                    t.attempt_id
                        .as_ref()
                        .is_none_or(|id| !s.attempts.iter().any(|a| &a.id == id && !a.stopped))
                }) && t.attempts < t.budget.max_attempts
            }),
            12 => self
                .current()
                .is_some_and(|t| matches!(t.state, TaskState::Submitted | TaskState::Verifying)),
            13 => self.current().is_some_and(|t| {
                t.attempt_id.is_some()
                    && matches!(
                        t.state,
                        TaskState::Verified
                            | TaskState::Uncertain
                            | TaskState::CancelRequested
                            | TaskState::Failed
                    )
            }),
            14 => self
                .current()
                .is_some_and(|t| t.evidence.iter().any(|e| e.output_path.is_some())),
            _ => true,
        }
    }
    pub fn new() -> Self {
        Self {
            worker: None,
            snapshot: None,
            context: None,
            error: None,
            notice: None,
            selected: 0,
            show_finished: false,
            context_page: false,
            technical: false,
            form: None,
            busy: false,
            detail_scroll: 0,
            list_top: 0,
            owners: Default::default(),
            owners_version: 0,
            row_indices: vec![],
            hits: vec![],
            demo: false,
            summary: String::new(),
            tally: Counts::default(),
            detail_lines: None,
            detail_heights: None,
        }
    }

    /// Drop the cached detail lines (and their measured heights) after anything that changes them.
    pub(super) fn invalidate(&mut self) {
        self.detail_lines = None;
        self.detail_heights = None;
    }

    /// The detail lines for the current selection, built once per change instead of once per frame.
    pub(super) fn detail_cached(&mut self) -> &[(String, bool)] {
        if self.detail_lines.is_none() {
            self.detail_lines = Some(self.detail());
        }
        self.detail_lines.as_deref().unwrap_or_default()
    }

    /// Where a new task starts: the project New Session opens in, as the app last reported it.
    fn default_project(cx: &Cx) -> String {
        cx.svc
            .task_projects()
            .default
            .map(|dir| dir.display().to_string())
            .unwrap_or_default()
    }

    /// The open form's project folder (tests and callers that only read it).
    #[cfg(test)]
    pub(crate) fn form_project(&self) -> Option<String> {
        self.form.as_ref().map(|f| f.project.clone())
    }

    pub(crate) fn counts(&self) -> Counts {
        self.snapshot
            .as_ref()
            .map(Counts::from_snapshot)
            .unwrap_or_default()
    }

    pub(super) fn prepare(&mut self, cx: &Cx) {
        if self.worker.is_none() && self.snapshot.is_none() {
            self.demo = cx.svc.is_demo();
            if self.demo {
                self.snapshot = Some(demo_snapshot());
                self.rebuild_rows();
                self.tally = self.counts();
                self.summary = self.tally.full();
            } else {
                self.worker = Some(Worker::start(cx.svc.clone(), cx.waker()));
            }
        }
        self.take_updates();
        if let Some((version, owners)) = cx.svc.coordination_workers(self.owners_version) {
            self.owners = owners;
            self.owners_version = version;
            self.invalidate();
        }
    }

    fn take_updates(&mut self) {
        let updates: Vec<_> = self
            .worker
            .as_ref()
            .map(|w| w.rx.try_iter().collect())
            .unwrap_or_default();
        for update in updates {
            match update {
                Update::Snapshot(Ok(snapshot)) => {
                    let selected = self.current().map(|t| t.id.clone());
                    self.snapshot = Some(snapshot);
                    self.rebuild_rows();
                    self.tally = self.counts();
                self.summary = self.tally.full();
                    self.invalidate();
                    self.error = None;
                    if let Some(id) = selected {
                        let position = self.tasks().position(|t| t.id == id).unwrap_or(0);
                        self.selected = position;
                    }
                    self.selected = self.selected.min(self.row_indices.len().saturating_sub(1));
                    if self.context.as_ref().is_some_and(|packet| {
                        !self.current().is_some_and(|t| {
                            t.id == packet.task.id && t.revision == packet.task.revision
                        })
                    }) {
                        self.context = None;
                    }
                }
                Update::Snapshot(Err(e)) => self.error = Some(e),
                Update::Context(Ok(packet)) => {
                    self.invalidate();
                    if self.current().is_some_and(|t| {
                        t.id == packet.task.id && t.revision == packet.task.revision
                    }) {
                        self.context = Some(packet);
                    }
                    self.busy = false;
                }
                Update::Context(Err(e)) => {
                    self.invalidate();
                    self.error = Some(e);
                    self.busy = false;
                }
                Update::Action(Ok(())) => {
                    self.notice = Some("Saved. The scheduler will continue eligible work.".into());
                    self.busy = false;
                    self.form = None;
                }
                Update::Action(Err(e)) => {
                    self.error = Some(e);
                    self.busy = false;
                }
                Update::Refreshed => {
                    self.busy = false;
                    self.invalidate();
                }
            }
        }
    }

    fn rebuild_rows(&mut self) {
        self.row_indices = self
            .snapshot
            .as_ref()
            .map(|s| {
                s.tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| self.show_finished || !t.state.terminal())
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(s) = &self.snapshot {
            self.row_indices.sort_by_key(|i| {
                let t = &s.tasks[*i];
                (state_rank(t.state), t.created_at)
            });
        }
    }
    pub(super) fn tasks(&self) -> impl Iterator<Item = &TaskView> {
        self.row_indices
            .iter()
            .filter_map(|i| self.snapshot.as_ref()?.tasks.get(*i))
    }
    pub(super) fn current(&self) -> Option<&TaskView> {
        self.snapshot
            .as_ref()?
            .tasks
            .get(*self.row_indices.get(self.selected)?)
    }
    pub(super) fn select(&mut self, index: usize) {
        self.invalidate();
        self.selected = index.min(self.row_indices.len().saturating_sub(1));
        self.detail_scroll = 0;
        self.context = None;
        self.context_page = false;
    }

    pub(super) fn activate(&mut self, action: usize, cx: &mut Cx) {
        if !self.action_enabled(action) {
            return;
        }
        match action {
            1 => {
                if let Some(w) = &self.worker {
                    if w.tx.try_send(Job::Refresh).is_ok() {
                        self.busy = true;
                    }
                }
            }
            2 => {
                self.show_finished = !self.show_finished;
                self.rebuild_rows();
                self.select(0);
            }
            3 => {
                self.detail_scroll = 0;
                self.form = Some(Form {
                    task: None,
                    goal: String::new(),
                    acceptance: String::new(),
                    project: Self::default_project(cx),
                    focus: 0,
                    quality: QualityMode::Standard,
                    picker: None,
                });
                self.error = None;
            }
            4 => {
                self.detail_scroll = 0;
                if let Some(task) = self.current().cloned() {
                    self.form = Some(Form {
                        goal: task.contract.goal.clone(),
                        acceptance: task
                            .contract
                            .acceptance
                            .first()
                            .map(|c| c.description.clone())
                            .unwrap_or_default(),
                        project: task.project.clone(),
                        quality: task.contract.quality,
                        focus: 0,
                        picker: None,
                        task: Some(task),
                    });
                }
            }
            5 => {
                if let Some(task) = self.current() {
                    self.send(vec![Command::Cancel {
                        task_id: task.id.clone(),
                        reason: "Cancelled from Tasks".into(),
                    }]);
                }
            }
            6 => {
                if let Some(task) = self.current() {
                    self.send(vec![Command::Retry {
                        task_id: task.id.clone(),
                    }]);
                }
            }
            7 => {
                self.context_page = !self.context_page;
                self.detail_scroll = 0;
                if self.context_page && self.demo {
                    if let Some(id) = self.current().map(|t| t.id.clone()) {
                        match cx.svc.coordination_context(&id) {
                            Ok(packet) => {
                                self.context = Some(packet);
                                self.error = None;
                            }
                            Err(error) => self.error = Some(error),
                        }
                    }
                } else if self.context_page
                    && let (Some(w), Some(task)) = (&self.worker, self.current())
                {
                    if w.tx.try_send(Job::Context(task.id.clone())).is_ok() {
                        self.busy = true;
                    }
                }
            }
            8 => self.technical = !self.technical,
            9 => self.submit_form(cx),
            10 => self.form = None,
            11 => {
                if let Some(f) = &mut self.form {
                    f.quality = match f.quality {
                        QualityMode::Quick => QualityMode::Standard,
                        QualityMode::Standard => QualityMode::Critical,
                        QualityMode::Critical => QualityMode::Quick,
                    };
                }
            }
            12 | 13 => {
                if let Some(task) = self.current() {
                    cx.svc.coordination_task_action(
                        &task.id,
                        task.revision,
                        if action == 13 { "retire" } else { "review" },
                    );
                    self.notice = Some(if action == 13 { "Worker retirement was queued for the coordinator. Other sessions keep running." } else { "Review was queued for the coordinator. Acceptance still requires current evidence." }.into());
                }
            }
            14 => {
                if let Some(path) = self
                    .current()
                    .and_then(|t| t.evidence.iter().rev().find_map(|e| e.output_path.as_ref()))
                {
                    cx.act(crate::pane::Action::Preview(std::path::PathBuf::from(path)));
                }
            }
            15 => self.open_picker(cx),
            n @ 21..=23 => {
                if let Some(f) = &mut self.form {
                    f.quality = [QualityMode::Quick, QualityMode::Standard, QualityMode::Critical][n - 21];
                }
            }
            n if n >= 100 => self.select(n - 100),
            _ => {}
        }
        if matches!(action, 7 | 8) {
            self.invalidate();
        }
    }

    fn send(&mut self, commands: Vec<Command>) {
        self.error = None;
        self.notice = None;
        if let Some(w) = &self.worker {
            if w.tx.try_send(Job::Commands(commands)).is_ok() {
                self.busy = true;
            } else {
                self.error = Some(
                    "A task update is already pending. Wait for it to finish, then retry.".into(),
                );
            }
        } else {
            self.error = Some(
                "Demo tasks are examples. Open Tasks outside demo to change real work.".into(),
            );
        }
    }

    fn submit_form(&mut self, _cx: &Cx) {
        match self.form_commands() {
            Ok(commands) => self.send(commands),
            Err(e) => self.error = Some(e),
        }
    }

    /// Open the shared project chooser, listing the same folders New Session does. A new task only.
    fn open_picker(&mut self, cx: &Cx) {
        let Some(f) = self.form.as_mut().filter(|f| f.task.is_none()) else {
            return;
        };
        let known = cx.svc.task_projects();
        let current = std::path::PathBuf::from(f.project.trim());
        let dirs = folder::dedup(
            Some(current)
                .filter(|p| !p.as_os_str().is_empty())
                .into_iter()
                .chain(known.choices),
        );
        f.picker = Some(FolderPicker::new(dirs));
        self.error = None;
    }

    /// What the chooser decided: use an existing folder, or say why the folder can't be used.
    pub(super) fn picker_outcome(&mut self, outcome: folder::Outcome) {
        let Some(f) = self.form.as_mut() else {
            return;
        };
        match outcome {
            folder::Outcome::None => {}
            folder::Outcome::Close => f.picker = None,
            folder::Outcome::Open(dir) => match project_folder_error(&dir) {
                None => {
                    f.project = dir.display().to_string();
                    f.picker = None;
                    self.error = None;
                }
                Some(error) => {
                    if let Some(p) = &mut f.picker {
                        p.error = Some(error);
                    }
                }
            },
            folder::Outcome::Create(..) => {
                if let Some(p) = &mut f.picker {
                    p.error = Some(
                        "That folder doesn't exist yet. Choose an existing project folder, or create it from New Session first."
                            .into(),
                    );
                }
            }
        }
    }

    fn form_commands(&self) -> Result<Vec<Command>, String> {
        let f = self
            .form
            .as_ref()
            .ok_or_else(|| "No task form is open".to_owned())?;
        if f.goal.trim().is_empty() || f.acceptance.trim().is_empty() {
            return Err("Enter an outcome and an observable review check.".into());
        }
        if f.task.is_none() {
            if f.project.trim().is_empty() {
                return Err("Choose the project this task belongs to.".into());
            }
            if let Some(error) = project_folder_error(std::path::Path::new(f.project.trim())) {
                return Err(error);
            }
        }
        let mut contract = f
            .task
            .as_ref()
            .map(|t| t.contract.clone())
            .unwrap_or_default();
        contract.goal = f.goal.trim().into();
        contract.quality = f.quality;
        // Preserve all other criteria and source identities when steering an existing contract.
        if let Some(first) = contract.acceptance.first_mut() {
            first.description = f.acceptance.trim().into();
        } else {
            contract.acceptance.push(Criterion {
                id: "outcome".into(),
                description: f.acceptance.trim().into(),
                kind: CriterionKind::Review,
                ..Default::default()
            });
        }
        if f.quality == QualityMode::Critical
            && !contract
                .acceptance
                .iter()
                .any(|c| c.kind == CriterionKind::Review && c.independent)
        {
            contract.acceptance.push(Criterion {
                id: "independent-review".into(),
                description:
                    "Independent review confirms the outcome and failure/recovery behavior".into(),
                kind: CriterionKind::Review,
                independent: true,
                ..Default::default()
            });
        }
        let commands = if let Some(task) = &f.task {
            vec![Command::Steer {
                task_id: task.id.clone(),
                expected_revision: task.revision,
                contract,
            }]
        } else {
            let intent_id = uuid::Uuid::new_v4().to_string();
            let title = f.goal.trim().chars().take(120).collect::<String>();
            vec![Command::CreateWork {
                intent: NewIntent {
                    id: intent_id,
                    title: title.clone(),
                    description: f.goal.trim().into(),
                    authorization: bro_core::coordination::Authorization {
                        scope: f.project.trim().into(),
                        source: "User saved this outcome in Tasks".into(),
                        authorized_actions: vec![
                            "readFiles".into(),
                            "writeFiles".into(),
                            "runChecks".into(),
                        ],
                        recorded_at: crate::util::now_secs(),
                    },
                    ..Default::default()
                },
                tasks: vec![NewTask {
                    id: uuid::Uuid::new_v4().to_string(),
                    title,
                    project: f.project.trim().into(),
                    integration_owner: Some("desktop".into()),
                    contract,
                    ..Default::default()
                }],
            }]
        };
        Ok(commands)
    }

    pub(super) fn detail(&self) -> Vec<(String, bool)> {
        let mut lines = vec![];
        if self.context_page {
            if let Some(packet) = &self.context {
                lines.push(("Context for the next worker".into(), true));
                lines.push((packet.task.contract.goal.clone(), false));
                if let Some(c) = &packet.checkpoint {
                    lines.push(("Last checkpoint".into(), true));
                    lines.push((c.summary.clone(), false));
                    for step in &c.next_steps {
                        lines.push((format!("Next: {step}"), false));
                    }
                }
                for task in &packet.dependencies {
                    lines.push((
                        format!("Dependency: {} · {}", task.title, state_label(task.state)),
                        false,
                    ));
                }
                for memory in packet.memory.iter().filter(|m| m.valid).take(12) {
                    lines.push((memory.text.clone(), false));
                }
                if packet.checkpoint.is_none() {
                    lines.push(("No checkpoint has been recorded yet.".into(), false));
                }
            } else {
                lines.push((
                    if self.busy {
                        "Loading context…"
                    } else {
                        "Context is not loaded. Press c to return, then c to load the current context."
                    }
                    .into(),
                    false,
                ));
            }
            if let Some(s) = &self.snapshot {
                lines.push(("Recovery".into(), true));
                if s.recovery.is_empty() {
                    lines.push(("No recovery issues recorded.".into(), false));
                }
                for notice in s.recovery.iter().rev().take(8) {
                    lines.push((notice.detail.clone(), false));
                }
                lines.push(("Recorded outcomes".into(), true));
                lines.push((
                    format!(
                        "{} accepted · {} attempts · {} repairs · {} recoveries",
                        s.metrics.accepted,
                        s.metrics.attempts,
                        s.metrics.rework,
                        s.metrics.recoveries
                    ),
                    false,
                ));
                lines.push((
                    if s.metrics.cost_reports == 0 {
                        "Provider cost is not reported.".into()
                    } else {
                        format!(
                            "Reported cost ${:.3} · {} reported tokens (partial provider coverage)",
                            s.metrics.cost_reported_usd, s.metrics.tokens_reported
                        )
                    },
                    false,
                ));
            }
            return lines;
        }
        let Some(task) = self.current() else {
            return lines;
        };
        lines.push((task.title.clone(), true));
        lines.push((
            format!(
                "{} · {} quality",
                state_label(task.state),
                quality_label(task.contract.quality)
            ),
            false,
        ));
        lines.push((format!("Project: {}", task.project), false));
        lines.push((
            format!(
                "Worker: {}",
                task.session_id
                    .as_ref()
                    .map(|id| self
                        .owners
                        .get(id)
                        .map(String::as_str)
                        .unwrap_or("assigned session"))
                    .unwrap_or("unassigned")
            ),
            false,
        ));
        if !task.contract.goal.is_empty() {
            lines.push((task.contract.goal.clone(), false));
        }
        if let Some(b) = &task.blocker {
            lines.push((format!("Waiting: {}", b.message), true));
            lines.push((format!("Wakes when: {}", b.wake_condition), false));
        }
        if !task.blocked_by.is_empty() {
            lines.push((
                format!("Waiting for {} prerequisite task(s)", task.blocked_by.len()),
                false,
            ));
        }
        if let Some(p) = &task.progress {
            lines.push(("Latest progress".into(), true));
            lines.push((p.clone(), false));
        }
        if !task.next_action.is_empty() {
            let next = if !task.blocked_by.is_empty() {
                "Wait for prerequisite outcomes to be accepted"
            } else if task.next_action.starts_with("resource ") {
                "Wait for shared resource ownership to be reconciled"
            } else {
                &task.next_action
            };
            lines.push((format!("Next: {next}"), false));
        }
        if let Some(e) = &task.last_error {
            lines.push((format!("Issue: {e}"), false));
        }
        lines.push(("Acceptance".into(), true));
        for criterion in &task.contract.acceptance {
            let evidence = task
                .evidence
                .iter()
                .rev()
                .find(|e| e.criterion_id == criterion.id && e.revision == task.revision);
            let label = match evidence {
                Some(e) if e.passed => "Passed",
                Some(_) => "Failed",
                None => "Awaiting evidence",
            };
            lines.push((
                format!(
                    "{label}: {}{}",
                    criterion.description,
                    if criterion.independent {
                        " (independent)"
                    } else {
                        ""
                    }
                ),
                false,
            ));
            if let Some(e) = evidence {
                lines.push((
                    format!(
                        "Observed by {} · {}{}",
                        e.verifier,
                        e.command.as_deref().unwrap_or("artifact inspection"),
                        e.exit_code
                            .map(|c| format!(" · exit {c}"))
                            .unwrap_or_default()
                    ),
                    false,
                ));
                if let Some(path) = &e.output_path {
                    lines.push((
                        format!("Evidence log: {path} (e opens the latest log)"),
                        false,
                    ));
                }
            }
        }
        if let Some(r) = &task.result {
            lines.push(("Candidate result".into(), true));
            lines.push((r.summary.clone(), false));
            for gap in &r.unresolved {
                lines.push((format!("Unresolved: {gap}"), false));
            }
        }
        if !task.artifacts.is_empty() {
            lines.push(("Artifacts".into(), true));
            for a in task.artifacts.iter().take(12) {
                lines.push((a.path.clone(), false));
            }
        }
        if task.state != TaskState::Verified {
            lines.push((
                "Agent activity does not establish that this outcome is accepted.".into(),
                false,
            ));
        }
        if self.technical {
            lines.push(("Technical details".into(), true));
            lines.push((
                format!(
                    "Task {} · revision {} · epoch {}",
                    task.id, task.revision, task.epoch
                ),
                false,
            ));
            if let Some(s) = &task.session_id {
                lines.push((format!("Session {s}"), false));
            }
            if let Some(a) = &task.attempt_id {
                lines.push((format!("Attempt {a}"), false));
            }
            for e in &task.evidence {
                lines.push((
                    format!("{} · {} · digest {}", e.criterion_id, e.verifier, e.digest),
                    false,
                ));
            }
        }
        lines
    }
}

/// Why `dir` can't be a task's project, or None. One metadata call, run when a folder is chosen or saved.
fn project_folder_error(dir: &std::path::Path) -> Option<String> {
    if !dir.is_absolute() {
        return Some("Use an absolute project folder path.".into());
    }
    match std::fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => None,
        Ok(_) => Some("That path is a file. Choose a project folder.".into()),
        Err(_) => Some("That folder doesn't exist. Choose an existing project folder.".into()),
    }
}

pub(crate) fn state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Pending => "Queued",
        TaskState::Ready => "Ready",
        TaskState::Assigned => "Assigned",
        TaskState::Running => "Working",
        TaskState::Blocked => "Waiting",
        TaskState::Uncertain => "Needs recovery",
        TaskState::Submitted => "Needs review",
        TaskState::Verifying => "Checking",
        TaskState::Verified => "Accepted",
        TaskState::CancelRequested => "Stopping",
        TaskState::Cancelled => "Cancelled",
        TaskState::Failed => "Failed",
        TaskState::Stopped => "Stopped",
    }
}
pub(super) fn quality_label(q: QualityMode) -> &'static str {
    match q {
        QualityMode::Quick => "quick",
        QualityMode::Standard => "standard",
        QualityMode::Critical => "critical",
    }
}
fn state_rank(state: TaskState) -> u8 {
    match state {
        TaskState::Uncertain | TaskState::Failed => 0,
        TaskState::Submitted | TaskState::Verifying => 1,
        TaskState::Running | TaskState::Assigned => 2,
        TaskState::Blocked | TaskState::CancelRequested => 3,
        TaskState::Ready | TaskState::Pending => 4,
        _ => 5,
    }
}

impl Pane for CoordinationView {
    fn title(&self) -> String {
        "Tasks".into()
    }
    fn icon(&self) -> &'static str {
        "team"
    }
    fn view(&self) -> Option<&'static str> {
        Some("tasks")
    }
    fn poll(&mut self, cx: &mut Cx) {
        self.prepare(cx);
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        self.prepare(cx);
        self.hits.clear();
        let t = cx.theme;
        if area.width < 18 || area.height < 5 {
            return;
        }
        let area = ui::hint_line(
            f,
            area,
            &[
                ("n", "new"),
                ("s", "steer"),
                ("x", "cancel"),
                ("r", "retry"),
                ("v", "review"),
                ("z", "retire"),
                ("c", "context"),
                ("a", "finished"),
                ("d", "details"),
                ("e", "evidence"),
            ],
            t,
        );
        ui::line(
            f,
            Rect { height: 1, ..area },
            vec![Span::styled(
                if self.demo {
                    " Tasks · demo examples"
                } else {
                    " Tasks"
                },
                ui::bold_accent(t),
            )],
        );
        ui::line(
            f,
            Rect {
                y: area.y + 1,
                height: 1,
                ..area
            },
            vec![Span::styled(self.summary.as_str(), muted(t))],
        );
        let form_project = self.form.as_ref().map(|f| f.project.clone()).unwrap_or_default();
        if let Some(form) = &self.form {
            let title = if form.task.is_some() {
                "Steer this outcome"
            } else {
                "New outcome"
            };
            let lines = vec![
                Line::from(Span::styled(title, ui::bold_accent(t))),
                Line::from("Tab changes field · Ctrl+Enter saves · Esc returns"),
                Line::from(format!(
                    "{}Outcome: {}",
                    if form.focus == 0 { "> " } else { "  " },
                    form.goal
                )),
                Line::from(format!(
                    "{}Review check: {}",
                    if form.focus == 1 { "> " } else { "  " },
                    form.acceptance
                )),
                Line::from(format!(
                    "{}Project: {}{}",
                    if form.focus == 2 { "> " } else { "  " },
                    if form.project.is_empty() { "none chosen" } else { &form.project },
                    if form.task.is_some() {
                        " (stays with the task)"
                    } else if form.focus == 2 {
                        " · Enter chooses"
                    } else {
                        ""
                    }
                )),
                Line::from(format!(
                    "Quality: {} · Ctrl+q cycles quality",
                    quality_label(form.quality)
                )),
            ];
            let r = Rect {
                y: area.y + 3,
                height: area.height.saturating_sub(3),
                ..area
            };
            f.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((self.detail_scroll.min(u16::MAX as usize) as u16, 0)),
                r,
            );
            if let Some(e) = &self.error {
                ui::line(
                    f,
                    Rect {
                        y: area.bottom().saturating_sub(1),
                        height: 1,
                        ..area
                    },
                    vec![Span::styled(e.clone(), fg(t.danger))],
                );
            }
            if let Some(p) = self.form.as_mut().and_then(|f| f.picker.as_mut()) {
                let current = std::path::PathBuf::from(form_project.as_str());
                folder::draw_for_session(f, area, p, &current, t);
            }
            return;
        }
        let status = self.error.as_deref().or(self.notice.as_deref()).unwrap_or(
            if self.snapshot.is_none() {
                "Loading tasks…"
            } else {
                "Worker activity and accepted outcomes are tracked separately."
            },
        );
        ui::line(
            f,
            Rect {
                y: area.y + 2,
                height: 1,
                ..area
            },
            vec![Span::styled(
                status,
                if self.error.is_some() {
                    fg(t.danger)
                } else {
                    muted(t)
                },
            )],
        );
        let body = Rect {
            y: area.y + 4,
            height: area.height.saturating_sub(4),
            ..area
        };
        if self.row_indices.is_empty() && !self.context_page {
            let text = if self.snapshot.is_none() {
                "Loading durable outcomes…"
            } else if self.show_finished {
                "No tasks yet. Press n to define an outcome and how it will be checked."
            } else {
                "No active tasks. Press n for a new outcome, or a to see finished work."
            };
            f.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: true })
                    .style(muted(t)),
                body,
            );
            return;
        }
        let wide = body.width >= 100;
        let list_w = if wide {
            (body.width * 2 / 5).clamp(32, 58)
        } else {
            body.width
        };
        let list_h = if wide {
            body.height
        } else {
            body.height.min(8)
        };
        let rows = (list_h / 2).max(1) as usize;
        self.list_top = self.list_top.min(self.selected);
        if self.selected >= self.list_top + rows {
            self.list_top = self.selected + 1 - rows;
        }
        let entries: Vec<_> = self
            .tasks()
            .enumerate()
            .skip(self.list_top)
            .take(rows)
            .map(|(i, t)| (i, t.title.clone(), state_label(t.state), t.project.clone()))
            .collect();
        for (row, (i, title, state, project)) in entries.into_iter().enumerate() {
            let r = Rect::new(body.x, body.y + row as u16 * 2, list_w, 2.min(body.height));
            let on = i == self.selected;
            ui::line(
                f,
                Rect { height: 1, ..r },
                vec![Span::styled(
                    format!(
                        "{}{}",
                        if on { "> " } else { "  " },
                        ui::fit(&title, list_w.saturating_sub(2) as usize)
                    ),
                    if on { ui::bold_accent(t) } else { fg(t.fg) },
                )],
            );
            ui::line(
                f,
                Rect {
                    y: r.y + 1,
                    height: 1,
                    ..r
                },
                vec![Span::styled(
                    ui::fit(&format!("  {state} · {project}"), list_w as usize),
                    muted(t),
                )],
            );
            self.hits.push((r, i));
        }
        let detail = if wide {
            Rect::new(
                body.x + list_w + 2,
                body.y,
                body.width.saturating_sub(list_w + 2),
                body.height,
            )
        } else {
            Rect::new(
                body.x,
                body.y + list_h + 1,
                body.width,
                body.height.saturating_sub(list_h + 1),
            )
        };
        let lines = self
            .detail()
            .into_iter()
            .map(|(s, heading)| {
                Line::from(Span::styled(s, if heading { ui::bold() } else { fg(t.fg) }))
            })
            .collect::<Vec<_>>();
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .scroll((self.detail_scroll.min(u16::MAX as usize) as u16, 0)),
            detail,
        );
    }
    fn key(&mut self, k: KeyEvent, cx: &mut Cx) -> bool {
        if self.form.is_some() {
            if self.busy {
                return true;
            }
            if let Some(p) = self.form.as_mut().and_then(|f| f.picker.as_mut()) {
                let outcome = p.key(k);
                self.picker_outcome(outcome);
                return true;
            }
            match k.code {
                KeyCode::Esc => self.form = None,
                KeyCode::PageDown => self.detail_scroll = (self.detail_scroll + 6).min(24),
                KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(6),
                KeyCode::Enter if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.submit_form(cx)
                }
                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Down
                    if self.form.as_ref().is_some_and(|f| f.focus == 2 && f.task.is_none()) =>
                {
                    self.activate(15, cx)
                }
                KeyCode::Tab | KeyCode::Enter => {
                    if let Some(f) = &mut self.form {
                        f.focus = (f.focus + 1) % 3;
                    }
                }
                KeyCode::BackTab => {
                    if let Some(f) = &mut self.form {
                        f.focus = (f.focus + 2) % 3;
                    }
                }
                KeyCode::Char('q') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.activate(11, cx)
                }
                KeyCode::Backspace => {
                    if let Some(f) = &mut self.form {
                        match f.focus {
                            0 => Some(&mut f.goal),
                            1 => Some(&mut f.acceptance),
                            _ => None,
                        }
                        .map(String::pop);
                    }
                }
                KeyCode::Char(c)
                    if !k
                        .modifiers
                        .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
                {
                    if let Some(f) = &mut self.form {
                        let field = match f.focus {
                            0 => Some(&mut f.goal),
                            1 => Some(&mut f.acceptance),
                            _ => None,
                        };
                        if let Some(field) = field.filter(|field| field.len() < 8192) {
                            field.push(c);
                        }
                    }
                }
                _ => return false,
            }
            return true;
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.select(self.selected + 1),
            KeyCode::Up | KeyCode::Char('k') => self.select(self.selected.saturating_sub(1)),
            KeyCode::PageDown => self.detail_scroll += 8,
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(8),
            KeyCode::Char('n') => self.activate(3, cx),
            KeyCode::Char('s') => self.activate(4, cx),
            KeyCode::Char('x') => self.activate(5, cx),
            KeyCode::Char('r') => self.activate(6, cx),
            KeyCode::Char('v') => self.activate(12, cx),
            KeyCode::Char('z') => self.activate(13, cx),
            KeyCode::Char('c') => self.activate(7, cx),
            KeyCode::Char('a') => self.activate(2, cx),
            KeyCode::Char('d') => self.activate(8, cx),
            KeyCode::Char('e') => self.activate(14, cx),
            KeyCode::F(5) => self.activate(1, cx),
            _ => return false,
        }
        true
    }
    fn mouse(&mut self, e: MouseEvent, _area: Rect, _cx: &mut Cx) {
        match e.kind {
            MouseEventKind::Down(_) => {
                if let Some((_, i)) = self
                    .hits
                    .iter()
                    .find(|(r, _)| r.contains((e.column, e.row).into()))
                {
                    self.select(*i);
                }
            }
            MouseEventKind::ScrollDown => self.detail_scroll += 3,
            MouseEventKind::ScrollUp => self.detail_scroll = self.detail_scroll.saturating_sub(3),
            _ => {}
        }
    }
    fn paste(&mut self, text: &str, _cx: &mut Cx) {
        let Some(f) = &mut self.form else {
            return;
        };
        if let Some(p) = &mut f.picker {
            p.paste(text);
            return;
        }
        let field = match f.focus {
            0 => &mut f.goal,
            1 => &mut f.acceptance,
            _ => return,
        };
        let remaining = 8192usize.saturating_sub(field.len());
        field.extend(text.chars().take(remaining));
    }
}

/// Demo-only examples use the production model and rendering; real windows always read the live engine.
pub(crate) fn demo_snapshot() -> Snapshot {
    let tasks: Vec<TaskView> = [
        (
            "Release preferences",
            TaskState::Submitted,
            "Candidate ready for independent review",
            "Opening the app again keeps the selected preferences",
        ),
        (
            "Fix session reconnect",
            TaskState::Running,
            "Reproduced the disconnect; checking recovery",
            "A hosted session remains attached after reconnecting",
        ),
        (
            "Update onboarding",
            TaskState::Blocked,
            "Waiting for the approved wording",
            "The first-run instructions match the current setup",
        ),
        (
            "Add task shortcuts",
            TaskState::Ready,
            "Ready for an available worker",
            "Tasks opens from the keyboard and sidebar",
        ),
        (
            "Verify settings export",
            TaskState::Verified,
            "Export and restore checks passed",
            "Exported preferences restore correctly",
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (title, state, progress, check))| TaskView {
        id: format!("demo-task-{i}"),
        title: title.into(),
        project: "Bro".into(),
        state,
        revision: 1,
        attempts: u32::from(state != TaskState::Ready),
        progress: Some(progress.into()),
        next_action: if state == TaskState::Submitted {
            "Review the candidate and its evidence".into()
        } else {
            String::new()
        },
        session_id: (state == TaskState::Running).then(|| "demo-worker".into()),
        blocker: (state == TaskState::Blocked).then(|| bro_core::coordination::Blocker {
            message: "Approved wording is needed".into(),
            wake_condition: "The wording is supplied".into(),
            requires_user: true,
            ..Default::default()
        }),
        contract: TaskContract {
            goal: title.into(),
            acceptance: vec![Criterion {
                id: "outcome".into(),
                description: check.into(),
                ..Default::default()
            }],
            ..Default::default()
        },
        ..Default::default()
    })
    .collect();
    let metrics = bro_core::coordination::Metrics {
        accepted: tasks
            .iter()
            .filter(|t| t.state == TaskState::Verified)
            .count() as u64,
        attempts: tasks.iter().map(|t| u64::from(t.attempts)).sum(),
        blocked: tasks
            .iter()
            .filter(|t| t.state == TaskState::Blocked)
            .count() as u64,
        ..Default::default()
    };
    Snapshot {
        tasks,
        metrics,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_core::coordination::{Actor, ActorRole, Engine};

    #[test]
    fn demo_context_action_loads_the_selected_handoff_and_consistent_metrics_without_a_worker() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc =
            crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let theme = crate::theme::get("graphite");
        let mut actions = vec![];
        let mut cx = Cx {
            id: 1,
            theme: &theme,
            svc: &svc,
            tx: &tx,
            actions: &mut actions,
            focused: true,
        };
        let mut view = CoordinationView::new();
        view.prepare(&cx);
        assert!(
            view.worker.is_none(),
            "demo context must not start disk/network work"
        );
        view.activate(7, &mut cx);
        let packet = view
            .context
            .as_ref()
            .expect("Context button loads the selected demo outcome");
        assert_eq!(packet.task.id, view.current().unwrap().id);
        assert!(
            packet
                .checkpoint
                .as_ref()
                .unwrap()
                .summary
                .contains("Candidate ready")
        );
        let metrics = &view.snapshot.as_ref().unwrap().metrics;
        assert_eq!(metrics.accepted, view.counts().accepted as u64);
        assert_eq!(metrics.attempts, 4);
        assert!(
            view.detail()
                .iter()
                .any(|(text, _)| text.contains("1 accepted")),
            "context metrics agree with the task header"
        );
    }

    #[test]
    fn task_form_saves_through_actual_services_and_bridge_normalization() {
        crate::testkit::isolate();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "Keep the existing behavior working.\n",
        )
        .unwrap();
        let bridge = bro_bridge::Bridge::start_with(
            bro_bridge::BridgeConfig {
                port: 0,
                automatic_port: false,
                bind: "127.0.0.1".into(),
                data_root: dir.path().join("data"),
                web_interface: false,
            },
            bro_bridge::StartOptions {
                import_legacy_token: false,
            },
            Box::new(|_| {}),
        )
        .unwrap();
        let services = crate::services::Services::coordination_test_bridge(bridge.clone());
        let mut view = CoordinationView::new();
        view.form = Some(Form {
            task: None,
            goal: "Preserve preferences".into(),
            acceptance: "Reopen and inspect the setting".into(),
            project: dir.path().display().to_string(),
            focus: 0,
            quality: QualityMode::Standard,
            picker: None,
        });
        services
            .coordination_execute("saved-from-ui", view.form_commands().unwrap().remove(0))
            .unwrap();
        let snapshot = services.coordination_snapshot().unwrap();
        assert_eq!((snapshot.intents.len(), snapshot.tasks.len()), (1, 1));
        let task = &snapshot.tasks[0];
        assert_eq!(task.integration_owner.as_deref(), Some("desktop"));
        assert!(
            task.contract
                .sources
                .keys()
                .any(|s| s.ends_with("AGENTS.md")),
            "the bridge includes actual project instructions"
        );
        assert!(
            task.resources
                .iter()
                .any(|r| r.key.starts_with("workspace:")),
            "bridge mutation ownership applies to UI work"
        );
        assert!(
            snapshot.intents[0]
                .authorization
                .authorized_actions
                .contains(&"runChecks".into())
        );
        let mut contract = task.contract.clone();
        contract.acceptance.push(Criterion {
            id: "check".into(),
            description: "The project check succeeds".into(),
            kind: CriterionKind::Check,
            command: Some("cargo check".into()),
            ..Default::default()
        });
        services
            .coordination_execute(
                "steer-ui-check",
                Command::Steer {
                    task_id: task.id.clone(),
                    expected_revision: task.revision,
                    contract,
                },
            )
            .unwrap();
        assert!(
            services.coordination_snapshot().unwrap().intents[0]
                .authorization
                .authorized_actions
                .contains(&"runChecks".into()),
            "steering retains the user's implementation/check authorization"
        );
        bridge.shutdown();
    }

    #[test]
    fn task_form_creates_a_valid_atomic_contract_in_the_real_engine() {
        let dir = tempfile::tempdir().unwrap();
        for quality in [
            QualityMode::Quick,
            QualityMode::Standard,
            QualityMode::Critical,
        ] {
            let engine = Engine::open(dir.path().join(quality_label(quality))).unwrap();
            let mut view = CoordinationView::new();
            view.form = Some(Form {
                task: None,
                goal: "Keep preferences after reopening".into(),
                acceptance: "Close and reopen; inspect the selected setting".into(),
                project: dir.path().display().to_string(),
                focus: 0,
                quality,
                picker: None,
            });
            let commands = view.form_commands().unwrap();
            assert_eq!(commands.len(), 1, "task and intent must commit atomically");
            let actor = Actor {
                id: "desktop".into(),
                role: ActorRole::Coordinator,
            };
            engine
                .execute("from-ui", &actor, 100, commands.into_iter().next().unwrap())
                .unwrap();
            let snapshot = engine.snapshot().unwrap();
            assert_eq!((snapshot.intents.len(), snapshot.tasks.len()), (1, 1));
            let contract = &snapshot.tasks[0].contract;
            assert_eq!(contract.quality, quality);
            assert!(
                contract
                    .acceptance
                    .iter()
                    .all(|c| c.kind != CriterionKind::Check || c.command.is_some())
            );
            if quality == QualityMode::Critical {
                assert!(
                    contract
                        .acceptance
                        .iter()
                        .any(|c| c.required && c.independent && c.kind == CriterionKind::Review)
                );
            }
        }
    }

    #[test]
    fn project_choice_is_saved_and_bad_folders_are_explained() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        let mut view = CoordinationView::new();
        view.form = Some(Form {
            task: None,
            goal: "Goal".into(),
            acceptance: "Check".into(),
            project: String::new(),
            focus: 2,
            quality: QualityMode::Standard,
            picker: Some(FolderPicker::new(vec![])),
        });
        assert!(view.form_commands().unwrap_err().contains("Choose the project"));
        view.picker_outcome(folder::Outcome::Open(dir.path().join("missing")));
        let f = view.form.as_ref().unwrap();
        assert!(f.project.is_empty() && f.picker.as_ref().unwrap().error.is_some(), "a missing folder stays unchosen with a reason");
        view.picker_outcome(folder::Outcome::Open(file));
        assert!(view.form.as_ref().unwrap().project.is_empty());
        view.picker_outcome(folder::Outcome::Open(dir.path().to_path_buf()));
        let f = view.form.as_ref().unwrap();
        assert!(f.picker.is_none());
        let Command::CreateWork { tasks, .. } = view.form_commands().unwrap().remove(0) else { panic!("create") };
        assert_eq!(tasks[0].project, dir.path().display().to_string());
    }

    #[test]
    fn late_context_does_not_replace_the_selected_tasks_context() {
        let mut view = CoordinationView::new();
        view.snapshot = Some(demo_snapshot());
        view.rebuild_rows();
        let first = view.current().unwrap().clone();
        view.select(1);
        let (tx, jobs) = mpsc::sync_channel(1);
        let (updates, rx) = mpsc::channel();
        view.worker = Some(Worker { tx, rx });
        updates
            .send(Update::Context(Ok(ContextPacket {
                task: first,
                ..Default::default()
            })))
            .unwrap();
        view.take_updates();
        assert!(
            view.context.is_none(),
            "a stale response must not show the wrong task's handoff"
        );
        drop(jobs);
    }
}
