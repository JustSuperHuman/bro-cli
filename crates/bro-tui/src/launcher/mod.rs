//! The launcher: choose a project, an agent, and where it runs.
//!
//! 1. **Project** — the path bar shows the current location and opens the project picker.
//! 2. **Harness** — tabs across the top (←/→): claude, codex, pi, omp, and ★ orchestrator. The orchestrator
//!    is bro's one session that drives all the others; on its tab a second row picks the harness it runs on
//!    (shift+←/→), and the list starts on the smartest login for it (your last orchestrator combo unless that
//!    login is nearly out, else the ready login with the most left).
//! 3. **Run on** — one list: recent combos for this harness, then its own logins (no model to pick — the CLI
//!    uses its default and `/model` works inside), then other logins it can use through bro-proxy, then API
//!    providers (OpenRouter's live catalogue, DeepSeek, …).
//! 4. **Model** — only when the choice needs one (providers, cross-family logins): a second, filterable list.
//!
//! Sessions start in the current project. The project row lets you switch to an open project or enter a new
//! path with the folder picker's completion; typing on the run-on list opens that picker with what you typed
//! (Esc there comes back to the list). Enter on a row that needs a model moves to the model list;
//! Enter there (or on anything else) launches. This file is the model + keys (pure, testable);
//! `view.rs` draws it.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

pub mod view;

use crate::fuzzy;
use crate::folder::{self, FolderPicker};
use crate::continue_picker::{self, ContinuePicker};
use crate::pane::Place;
use crate::recents::Recent;
use bro_core::Harness;
use bro_core::browser::BrowserMode;
use bro_core::catalogue::ModelRow;
use bro_core::launch::{LaunchSpec, Permission};
use bro_core::profiles::{Profile, ProfileKind};
use bro_core::providers::{Provider, ProviderMode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Which list has the keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    /// the "run on" list
    List,
    Models,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionTab {
    New,
    Continue,
}

/// Who a session runs as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountKind {
    /// a Claude / Codex login ("claude:work")
    Profile(String),
    /// an API provider from the catalogue ("openrouter")
    Provider(String),
    /// the Claude account pool
    Pool,
    /// pi / omp with their own configured login
    Native,
}

/// A "run on" entry.
#[derive(Clone, Debug)]
pub struct Account {
    pub kind: AccountKind,
    pub label: String,
    pub detail: String,
    /// logged in / has a key
    pub ready: bool,
    /// % left of the 5h window (weekly when that's the only reported window)
    pub left: Option<f64>,
}

/// One row of the "run on" list.
#[derive(Clone, Debug)]
pub enum Item {
    Header(&'static str),
    /// index into `Data::recents`
    Recent(usize),
    Account(Account),
}

/// What the launcher offers, updated as service readings arrive.
#[derive(Clone, Debug, Default)]
pub struct Data {
    pub profiles: Vec<Profile>,
    pub providers: Vec<Provider>,
    /// profile id → % used of its tighter window (the 5h one capped by the week), as the sidebar reads it
    pub usage: BTreeMap<String, f32>,
    /// Window label and refresh state shown alongside a login's last reading.
    pub usage_status: BTreeMap<String, String>,
    /// harness → installed on PATH
    pub installed: Vec<(Harness, bool)>,
    pub recents: Vec<Recent>,
    /// provider id (plus "codex", "claude") → models
    pub models: BTreeMap<String, Vec<ModelRow>>,
    /// provider ids with a key configured (or that need none)
    pub keyed: Vec<String>,
    /// The orchestrator already running, by its label: launching it again goes to that one.
    pub orchestrator_running: Option<String>,
}

/// Something clickable in the launcher (recorded while drawing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Profiles,
    Project,
    Tab(SessionTab),
    ContinueScope(bool),
    ContinueRow(usize),
    Harness(Harness),
    /// the ★ orchestrator tab
    Orchestrator,
    /// the harness the orchestrator runs on
    Brain(Harness),
    /// index into the selectable run-on rows
    Row(usize),
    /// index into the filtered models
    Model(usize),
    Perm(Permission),
    Browser,
    Place,
    Launch,
    /// "install it" under an agent that isn't installed
    Install,
    /// the × in the corner
    Close,
}

/// What a key asks for.
pub enum Outcome {
    None,
    Close,
    Profiles,
    Project(PathBuf),
    CreateProject(PathBuf, uuid::Uuid),
    Launch(LaunchSpec, Place),
    Resume(usize),
    /// Resume several earlier sessions, one after another.
    ResumeMany(Vec<usize>),
    Archive(String, bool),
    UndoArchive,
    /// Install this agent (it isn't on PATH).
    Install(Harness),
    /// Sign in to Gemini with Google first (the selected provider has no API key).
    SignIn,
}

/// Recent combos shown per harness.
const RECENTS_SHOWN: usize = 4;
/// A login with less than this % left isn't the orchestrator's first pick.
const NEARLY_OUT: f64 = 10.0;

/// The launcher state.
pub struct Launcher {
    pub data: Data,
    /// The agent; on the orchestrator tab, the harness the orchestrator runs on.
    pub harness: Harness,
    /// The ★ orchestrator tab is open.
    pub orchestrator: bool,
    pub focus: Focus,
    pub list_filter: String,
    pub model_filter: String,
    /// index into the *selectable* rows of the run-on list
    pub list_sel: usize,
    /// index into the filtered models
    pub model_sel: usize,
    pub dir: PathBuf,
    /// Open projects and recent folders offered by the project picker.
    pub projects: Vec<PathBuf>,
    pub project_picker: Option<FolderPicker>,
    pub tab: SessionTab,
    pub continue_picker: ContinuePicker,
    pub permission: Permission,
    pub browser: BrowserMode,
    pub place: Place,
    /// clickable regions from the last draw, and the popup's own rect
    pub hits: Vec<(ratatui::layout::Rect, Hit)>,
    pub area: ratatui::layout::Rect,
}

impl Launcher {
    /// Replace service data while keeping the user's account, model and typed filters.
    pub fn set_data(&mut self, data: Data) {
        let current = self.current();
        let recent = match &current {
            Some(Item::Recent(i)) => self.data.recents.get(*i).cloned(),
            _ => None,
        };
        let model = self.model().map(|m| m.id);
        self.data = data;
        let items = self.items();
        let selectable = Self::selectable(&items);
        let selected = selectable.iter().position(|&i| match (&current, &items[i]) {
            (Some(Item::Account(a)), Item::Account(b)) => a.kind == b.kind,
            (Some(Item::Recent(_)), Item::Recent(i)) => recent.as_ref().is_some_and(|r| self.data.recents.get(*i).is_some_and(|other| r.same(other))),
            _ => false,
        });
        self.list_sel = selected.unwrap_or(self.list_sel.min(selectable.len().saturating_sub(1)));
        if current.is_some() && selected.is_none() {
            self.reset_models();
            self.focus = Focus::List;
        } else {
            let models = self.models_view();
            self.model_sel = model.and_then(|id| models.iter().position(|&i| self.models()[i].id == id)).unwrap_or(self.model_sel.min(models.len().saturating_sub(1)));
        }
        self.hits.clear();
    }

    /// Open on the most recent harness, in `cwd` (the current project; else the most recent one).
    pub fn new(data: Data, cwd: Option<PathBuf>, place: Place) -> Launcher {
        let recent = data.recents.first().cloned();
        let harness = recent.as_ref().map(|r| r.harness).unwrap_or(Harness::Claude);
        let dir = cwd
            .or_else(|| recent.as_ref().map(|r| r.cwd.clone()))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let browser = recent.map(|r| r.browser).unwrap_or_default();
        let permission = Permission::Skip;
        Launcher {
            data,
            harness,
            orchestrator: false,
            focus: Focus::List,
            list_filter: String::new(),
            model_filter: String::new(),
            list_sel: 0,
            model_sel: 0,
            dir,
            projects: vec![],
            project_picker: None,
            tab: SessionTab::New,
            continue_picker: ContinuePicker::new(vec![], None),
            permission,
            browser,
            place,
            hits: vec![],
            area: ratatui::layout::Rect::default(),
        }
    }

    pub fn with_projects(mut self, projects: Vec<PathBuf>) -> Launcher {
        self.projects = projects;
        self
    }

    pub fn with_continue(mut self, entries: Vec<continue_picker::Entry>, project: Option<(String, String)>) -> Launcher {
        self.continue_picker = ContinuePicker::new(entries, project);
        self
    }

    pub fn show_continue(&mut self) {
        self.tab = SessionTab::Continue;
        self.hits.clear();
    }

    pub fn set_continue_entries(&mut self, entries: Vec<continue_picker::Entry>) {
        self.continue_picker.set_entries(entries);
    }

    fn continue_outcome(outcome: continue_picker::Outcome) -> Outcome {
        match outcome {
            continue_picker::Outcome::None => Outcome::None,
            continue_picker::Outcome::Close => Outcome::Close,
            continue_picker::Outcome::Resume(idx) => Outcome::Resume(idx),
            continue_picker::Outcome::ResumeMany(idx) => Outcome::ResumeMany(idx),
            continue_picker::Outcome::Archive(id, on) => Outcome::Archive(id, on),
            continue_picker::Outcome::Undo => Outcome::UndoArchive,
        }
    }

    fn open_project_picker(&mut self) {
        let dirs = folder::dedup(std::iter::once(self.dir.clone()).chain(self.projects.iter().cloned()));
        self.project_picker = Some(FolderPicker::new(dirs));
    }

    fn project_outcome(&mut self, outcome: folder::Outcome) -> Outcome {
        match outcome {
            folder::Outcome::None => Outcome::None,
            folder::Outcome::Close => {
                self.project_picker = None;
                Outcome::None
            }
            folder::Outcome::Open(dir) => {
                self.select_project(dir.clone());
                Outcome::Project(dir)
            }
            folder::Outcome::Create(dir, request) => Outcome::CreateProject(dir, request),
        }
    }

    pub fn select_project(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.project_picker = None;
        self.hits.clear();
    }

    /// Start on the permission mode picked last time instead of skip.
    pub fn with_permission(mut self, p: Permission) -> Launcher {
        self.permission = p;
        self
    }

    /// Open on `h` instead of the most recent harness.
    pub fn with_harness(mut self, h: Harness) -> Launcher {
        self.orchestrator = false;
        self.harness = h;
        self.list_filter.clear();
        self.list_sel = 0;
        self.reset_models();
        self
    }

    /// Select an exact login without matching a recent combo or a similarly named account.
    pub fn with_profile(mut self, id: &str) -> Option<Launcher> {
        self.list_filter.clear();
        let items = self.items();
        self.list_sel = Self::selectable(&items).iter().position(|&i|
            matches!(&items[i], Item::Account(Account { kind: AccountKind::Profile(profile), .. }) if profile == id))?;
        self.reset_models();
        Some(self)
    }

    /// What a one-click "new claude / codex" should run: the top of `h`'s list (your latest combo for it, else
    /// its first login). None when that still needs a decision (not installed, nothing to run on, a model to pick).
    pub fn quick_spec(&self) -> Option<LaunchSpec> {
        if !self.installed(self.harness) {
            return None;
        }
        match self.current()? {
            Item::Account(a) if !a.ready || self.needs_model() => None,
            _ => self.spec(),
        }
    }

    /// Open on the ★ orchestrator tab.
    pub fn with_orchestrator(mut self) -> Launcher {
        self.enter_orchestrator();
        self
    }

    /// Switch to the orchestrator tab on its smartest harness and login.
    fn enter_orchestrator(&mut self) {
        self.orchestrator = true;
        self.harness = self.brain_default();
        self.list_filter.clear();
        self.focus = Focus::List;
        self.list_sel = self.smart_sel();
        self.reset_models();
    }

    /// The orchestrator's harness to start on: the one it last ran on, else Claude Code (its tools are native
    /// there), else Codex, else whatever is installed.
    pub fn brain_default(&self) -> Harness {
        let last = self.data.recents.iter().find(|r| r.orchestrator).map(|r| r.harness).filter(|h| self.installed(*h));
        last.or_else(|| [Harness::Claude, Harness::Codex, Harness::Pi, Harness::Omp].into_iter().find(|h| self.installed(*h))).unwrap_or(Harness::Claude)
    }

    /// Pick the harness the orchestrator runs on, landing on its smartest login.
    fn set_brain(&mut self, h: Harness) {
        self.harness = h;
        self.list_filter.clear();
        if self.focus == Focus::Models {
            self.focus = Focus::List;
        }
        self.list_sel = self.smart_sel();
        self.reset_models();
    }

    /// % left on the login a profile id names.
    fn left_of(&self, profile: Option<&str>) -> Option<f64> {
        profile.and_then(|p| self.data.usage.get(p)).map(|u| (100.0 - *u as f64).clamp(0.0, 100.0))
    }

    /// Where the cursor starts on the orchestrator tab: your last orchestrator combo on this harness unless its
    /// login is nearly out, else the ready login of this harness's own family with the most left, else the first
    /// ready row. It runs for hours and every turn reads the whole workspace, so headroom matters most.
    pub fn smart_sel(&self) -> usize {
        let items = self.items();
        let sel = Self::selectable(&items);
        let fine = |left: Option<f64>| left.is_none_or(|l| l >= NEARLY_OUT);
        for (k, &i) in sel.iter().enumerate() {
            if let Item::Recent(r) = &items[i] {
                let r = &self.data.recents[*r];
                if fine(self.left_of(r.profile_id.as_deref().or(r.provider_id.as_deref()))) {
                    return k;
                }
            }
        }
        let own = |a: &Account| match &a.kind {
            AccountKind::Profile(p) => match self.harness {
                Harness::Claude => p.starts_with("claude:"),
                Harness::Codex => p.starts_with("codex:"),
                _ => false,
            },
            AccountKind::Native | AccountKind::Pool => true,
            AccountKind::Provider(_) => false,
        };
        let mut best: Option<(usize, f64)> = None;
        for (k, &i) in sel.iter().enumerate() {
            if let Item::Account(a) = &items[i]
                && a.ready
                && own(a)
            {
                // an unread meter counts as half full: a known roomy login beats it, a nearly empty one doesn't
                let score = a.left.unwrap_or(50.0);
                if best.is_none_or(|(_, s)| score > s) {
                    best = Some((k, score));
                }
            }
        }
        if let Some((k, _)) = best {
            return k;
        }
        sel.iter().position(|&i| matches!(&items[i], Item::Account(a) if a.ready)).unwrap_or(0)
    }

    pub fn installed(&self, h: Harness) -> bool {
        self.data.installed.iter().find(|x| x.0 == h).map(|x| x.1).unwrap_or(true)
    }

    // ------------------------------------------------------------------ the run-on list

    fn profile_account(&self, p: &Profile, via_proxy: bool) -> Account {
        let family = if p.is_claude() { "claude" } else { "chatgpt" };
        let mut detail = vec![];
        if via_proxy {
            detail.push(family.to_string());
        }
        if let Some(plan) = &p.plan {
            detail.push(plan.clone());
        }
        if !p.authenticated {
            detail.push("not logged in".into());
        }
        if let Some(status) = self.data.usage_status.get(&p.id) {
            detail.push(status.clone());
        }
        Account {
            kind: AccountKind::Profile(p.id.clone()),
            label: p.name.clone(),
            detail: detail.join(" · "),
            ready: p.authenticated,
            left: self.data.usage.get(&p.id).map(|u| (100.0 - *u as f64).clamp(0.0, 100.0)),
        }
    }

    fn pool_account(&self) -> Option<Account> {
        let n = self.data.profiles.iter().filter(|p| p.kind == ProfileKind::ClaudeAccount && p.authenticated).count();
        (n > 0).then(|| Account { kind: AccountKind::Pool, label: "pool".into(), detail: format!("{n} claude accounts · failover"), ready: true, left: None })
    }

    fn provider_account(&self, p: &Provider) -> Account {
        let n = self.data.models.get(&p.id).map(Vec::len).unwrap_or(p.models.len());
        let keyed = p.no_key || self.data.keyed.iter().any(|k| k == &p.id);
        let mut detail = format!("{n} model{}", if n == 1 { "" } else { "s" });
        if p.is_google_login() {
            detail.push_str(if keyed { " · google ai subscription" } else { " · enter signs in with Google" });
        } else if !keyed {
            detail.push_str(" · no key");
        }
        Account { kind: AccountKind::Provider(p.id.clone()), label: p.id.clone(), detail, ready: keyed, left: None }
    }

    /// The full run-on list for the current harness (headers included), filtered.
    pub fn items(&self) -> Vec<Item> {
        let h = self.harness;
        let claude: Vec<&Profile> = self.data.profiles.iter().filter(|p| p.is_claude()).collect();
        let codex: Vec<&Profile> = self.data.profiles.iter().filter(|p| p.is_codex()).collect();
        let mut groups: Vec<(&'static str, Vec<Item>)> = vec![];

        let recents: Vec<Item> = self.recents_for(h).into_iter().map(Item::Recent).collect();
        groups.push(("recent", recents));

        let (own, via): (Vec<Account>, Vec<Account>) = match h {
            Harness::Claude => (
                claude.iter().map(|p| self.profile_account(p, false)).chain(self.pool_account()).collect(),
                codex.iter().map(|p| self.profile_account(p, true)).collect(),
            ),
            Harness::Codex => (
                codex.iter().map(|p| self.profile_account(p, false)).collect(),
                claude.iter().map(|p| self.profile_account(p, true)).chain(self.pool_account()).collect(),
            ),
            Harness::Pi | Harness::Omp => (
                vec![Account { kind: AccountKind::Native, label: format!("{}'s own login", h.label()), detail: "whatever it's configured with".into(), ready: true, left: None }],
                claude.iter().chain(codex.iter()).map(|p| self.profile_account(p, true)).chain(self.pool_account()).collect(),
            ),
        };
        groups.push(("your logins", own.into_iter().map(Item::Account).collect()));
        groups.push(("other logins · via proxy", via.into_iter().map(Item::Account).collect()));
        let providers = self.data.providers.iter().filter(|p| p.mode != ProviderMode::Native).map(|p| Item::Account(self.provider_account(p))).collect();
        groups.push(("providers", providers));

        let q = self.list_filter.trim();
        let mut out = vec![];
        for (name, items) in groups {
            let keep: Vec<Item> = if q.is_empty() {
                items
            } else {
                let idx = fuzzy::filter(q, &items, |i| self.item_text(i));
                idx.into_iter().map(|i| items[i].clone()).collect()
            };
            if !keep.is_empty() {
                out.push(Item::Header(name));
                out.extend(keep);
            }
        }
        out
    }

    fn item_text(&self, i: &Item) -> String {
        match i {
            Item::Header(_) => String::new(),
            Item::Recent(r) => {
                let r = &self.data.recents[*r];
                format!("{} {} {}", r.profile_id.clone().unwrap_or_default(), r.provider_id.clone().unwrap_or_default(), r.model.clone().unwrap_or_default())
            }
            Item::Account(a) => format!("{} {}", a.label, a.detail),
        }
    }

    /// Recent combos for a harness, newest first, one per (login/provider, model).
    fn recents_for(&self, h: Harness) -> Vec<usize> {
        let mut out: Vec<usize> = vec![];
        for (i, r) in self.data.recents.iter().enumerate() {
            if r.harness != h || r.orchestrator != self.orchestrator {
                continue;
            }
            let dup = out.iter().any(|&j| {
                let o = &self.data.recents[j];
                o.profile_id == r.profile_id && o.provider_id == r.provider_id && o.model == r.model
            });
            if !dup {
                out.push(i);
            }
            if out.len() == RECENTS_SHOWN {
                break;
            }
        }
        out
    }

    /// Positions (in `items()`) of the rows the cursor can land on.
    fn selectable(items: &[Item]) -> Vec<usize> {
        items.iter().enumerate().filter(|(_, i)| !matches!(i, Item::Header(_))).map(|(k, _)| k).collect()
    }

    /// The row under the cursor.
    pub fn current(&self) -> Option<Item> {
        let items = self.items();
        let sel = Self::selectable(&items);
        sel.get(self.list_sel).map(|&k| items[k].clone())
    }

    /// Position of the cursor in `items()` (for drawing).
    pub fn cursor_row(&self, items: &[Item]) -> Option<usize> {
        Self::selectable(items).get(self.list_sel).copied()
    }

    /// Does the current row need a model picked?
    pub fn needs_model(&self) -> bool {
        match self.current() {
            Some(Item::Account(a)) => match &a.kind {
                AccountKind::Provider(_) => true,
                AccountKind::Profile(p) => !matches!((self.harness, p.starts_with("claude:")), (Harness::Claude, true) | (Harness::Codex, false)),
                AccountKind::Pool => self.harness != Harness::Claude,
                AccountKind::Native => false,
            },
            _ => false,
        }
    }

    /// Models for the current row (empty when none is needed).
    pub fn models(&self) -> &[ModelRow] {
        if !self.needs_model() {
            return &[];
        }
        let key = match self.current() {
            Some(Item::Account(a)) => match a.kind {
                AccountKind::Provider(p) => p,
                AccountKind::Profile(p) if p.starts_with("codex:") => "codex".into(),
                _ => "claude".into(),
            },
            _ => return &[],
        };
        self.data.models.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Filtered model indices.
    pub fn models_view(&self) -> Vec<usize> {
        fuzzy::filter(&self.model_filter, self.models(), |m| format!("{} {}", m.id, m.name))
    }

    pub fn model(&self) -> Option<ModelRow> {
        let idx = self.models_view();
        idx.get(self.model_sel).map(|&i| self.models()[i].clone())
    }

    /// The spec Enter would launch (None when a model is still needed or nothing is selected).
    pub fn spec(&self) -> Option<LaunchSpec> {
        let base = |profile_id: Option<String>, provider_id: Option<String>, model: Option<String>| LaunchSpec {
            harness: self.harness,
            profile_id,
            provider_id,
            model,
            cwd: self.dir.clone(),
            resume: None,
            permission: self.permission,
            browser: self.browser,
            extra_args: vec![],
            orchestrator: self.orchestrator,
        };
        match self.current()? {
            Item::Header(_) => None,
            Item::Recent(i) => {
                let r = &self.data.recents[i];
                Some(base(r.profile_id.clone(), r.provider_id.clone(), r.model.clone()))
            }
            Item::Account(a) => {
                let model = if self.needs_model() { Some(self.model()?.id) } else { None };
                Some(match a.kind {
                    AccountKind::Profile(p) => base(Some(p), None, model),
                    AccountKind::Provider(p) => base(None, Some(p), model),
                    AccountKind::Pool => base(None, Some("pool".into()), model),
                    AccountKind::Native => base(None, None, model),
                })
            }
        }
    }

    // ------------------------------------------------------------------ keys

    /// ←/→ across the tabs: the agents, then ★ orchestrator.
    fn set_harness(&mut self, d: i32) {
        let all = Harness::ALL;
        let n = all.len() as i32 + 1;
        let i = if self.orchestrator { all.len() as i32 } else { all.iter().position(|h| *h == self.harness).unwrap_or(0) as i32 };
        let next = (i + d).rem_euclid(n) as usize;
        if next == all.len() {
            self.enter_orchestrator();
            return;
        }
        self.orchestrator = false;
        self.harness = all[next];
        self.list_filter.clear();
        self.list_sel = 0;
        self.reset_models();
        if self.focus == Focus::Models {
            self.focus = Focus::List;
        }
    }

    /// New row under the cursor: forget the model choice, but start on the last model used with it.
    fn reset_models(&mut self) {
        self.model_filter.clear();
        self.model_sel = 0;
        let Some(Item::Account(a)) = self.current() else { return };
        let (profile, provider) = match &a.kind {
            AccountKind::Profile(p) => (Some(p.clone()), None),
            AccountKind::Provider(p) => (None, Some(p.clone())),
            AccountKind::Pool => (None, Some("pool".to_string())),
            AccountKind::Native => (None, None),
        };
        let last = self.data.recents.iter().find(|r| r.profile_id == profile && r.provider_id == provider && r.model.is_some()).and_then(|r| r.model.clone());
        if let Some(m) = last
            && let Some(k) = self.models_view().iter().position(|&i| self.models()[i].id == m)
        {
            self.model_sel = k;
        }
    }

    fn move_sel(&mut self, d: i32) {
        match self.focus {
            Focus::List => {
                let n = Self::selectable(&self.items()).len();
                if n > 0 {
                    let before = self.list_sel;
                    self.list_sel = (self.list_sel as i32 + d).clamp(0, n as i32 - 1) as usize;
                    if self.list_sel != before {
                        self.reset_models();
                    }
                }
            }
            Focus::Models => {
                let n = self.models_view().len();
                if n > 0 {
                    self.model_sel = (self.model_sel as i32 + d).clamp(0, n as i32 - 1) as usize;
                }
            }
        }
    }

    fn filter_mut(&mut self) -> &mut String {
        match self.focus {
            Focus::List => &mut self.list_filter,
            Focus::Models => &mut self.model_filter,
        }
    }

    fn filter_changed(&mut self) {
        match self.focus {
            Focus::List => {
                self.list_sel = 0;
                self.reset_models();
            }
            Focus::Models => self.model_sel = 0,
        }
    }

    /// Paste into the focused filter.
    pub fn paste(&mut self, s: &str) {
        if let Some(p) = &mut self.project_picker {
            p.paste(s);
            return;
        }
        if self.tab == SessionTab::Continue {
            self.continue_picker.paste(s);
            return;
        }
        // the run-on list is picked with the arrows; text is a project
        if self.focus == Focus::List {
            self.open_project_picker();
            if let Some(p) = &mut self.project_picker {
                p.paste(s);
            }
            return;
        }
        self.filter_mut().push_str(s.trim());
        self.filter_changed();
    }

    /// The selected provider needs a Google sign-in before it can run anything.
    fn sign_in_needed(&self) -> bool {
        let Some(Item::Account(Account { kind: AccountKind::Provider(id), ready: false, .. })) = self.current() else { return false };
        self.data.providers.iter().any(|p| p.id == id && p.is_google_login())
    }

    fn launch(&self) -> Outcome {
        if self.sign_in_needed() {
            return Outcome::SignIn;
        }
        match self.spec() {
            // the orchestrator always gets its own session
            Some(s) if s.orchestrator => Outcome::Launch(s, Place::Tab),
            Some(s) => Outcome::Launch(s, self.place),
            None => Outcome::None,
        }
    }

    /// A mouse click. Clicking the selected row again acts like Enter; clicking outside closes.
    /// Shift / ctrl on an earlier session picks several.
    pub fn click(&mut self, pos: ratatui::layout::Position, mods: KeyModifiers) -> Outcome {
        if let Some(p) = &mut self.project_picker {
            let clicked_row = p.hits.iter().any(|(r, _)| r.contains(pos));
            let mut outcome = p.click(pos);
            if clicked_row && matches!(outcome, folder::Outcome::None) && p.error.is_none() {
                outcome = p.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
            return self.project_outcome(outcome);
        }
        if !self.area.contains(pos) {
            return Outcome::Close;
        }
        let Some(&(_, hit)) = self.hits.iter().find(|(r, _)| r.contains(pos)) else { return Outcome::None };
        match hit {
            Hit::Profiles => return Outcome::Profiles,
            Hit::Project => self.open_project_picker(),
            Hit::Tab(tab) => self.tab = tab,
            Hit::ContinueScope(all) => self.continue_picker.set_all(all),
            Hit::ContinueRow(_) => return Self::continue_outcome(self.continue_picker.click(pos, mods)),
            Hit::Harness(h) => {
                if self.orchestrator || self.harness != h {
                    self.orchestrator = false;
                    self.harness = h;
                    self.set_harness(0);
                }
            }
            Hit::Orchestrator => {
                if !self.orchestrator {
                    self.enter_orchestrator();
                }
            }
            Hit::Brain(h) => {
                if self.harness != h {
                    self.set_brain(h);
                }
            }
            Hit::Row(i) => {
                if self.focus == Focus::List && self.list_sel == i {
                    return self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
                self.focus = Focus::List;
                if self.list_sel != i {
                    self.list_sel = i;
                    self.reset_models();
                }
            }
            Hit::Model(k) => {
                if self.focus == Focus::Models && self.model_sel == k {
                    return self.launch();
                }
                self.focus = Focus::Models;
                self.model_sel = k;
            }
            Hit::Perm(p) => self.permission = p,
            Hit::Browser => {
                self.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
            }
            Hit::Place => {
                self.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
            }
            Hit::Install => return Outcome::Install(self.harness),
            Hit::Launch => return self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Hit::Close => return Outcome::Close,
        }
        Outcome::None
    }

    /// Mouse wheel over the launcher: move in the list under the pointer.
    pub fn scroll(&mut self, pos: ratatui::layout::Position, down: bool) {
        if let Some(p) = &mut self.project_picker {
            for _ in 0..3 {
                p.key(KeyEvent::new(if down { KeyCode::Down } else { KeyCode::Up }, KeyModifiers::NONE));
            }
            return;
        }
        if self.tab == SessionTab::Continue {
            for _ in 0..3 {
                self.continue_picker.key(KeyEvent::new(if down { KeyCode::Down } else { KeyCode::Up }, KeyModifiers::NONE));
            }
            return;
        }
        if let Some(&(_, hit)) = self.hits.iter().find(|(r, _)| r.contains(pos)) {
            match hit {
                Hit::Model(_) => self.focus = Focus::Models,
                Hit::Row(_) => self.focus = Focus::List,
                _ => {}
            }
        }
        self.move_sel(if down { 3 } else { -3 });
    }

    /// Handle a key. The launcher is modal: it takes every key.
    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        if let Some(p) = &mut self.project_picker {
            let outcome = p.key(k);
            return self.project_outcome(outcome);
        }
        if matches!(k.code, KeyCode::Char('p' | 'P')) && k.modifiers.contains(KeyModifiers::ALT) {
            self.open_project_picker();
            return Outcome::None;
        }
        if k.code == KeyCode::Char('o') && k.modifiers == KeyModifiers::ALT {
            return Outcome::Profiles;
        }
        if k.modifiers.contains(KeyModifiers::ALT) {
            match k.code {
                KeyCode::Char('n' | 'N') => { self.tab = SessionTab::New; return Outcome::None; }
                KeyCode::Char('r' | 'R') => { self.show_continue(); return Outcome::None; }
                _ => {}
            }
        }
        if self.tab == SessionTab::Continue {
            return Self::continue_outcome(self.continue_picker.key(k));
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => {
                if !self.filter_mut().is_empty() {
                    self.filter_mut().clear();
                    self.filter_changed();
                } else if self.focus != Focus::List {
                    self.focus = Focus::List;
                } else {
                    return Outcome::Close;
                }
            }
            KeyCode::Enter if !self.installed(self.harness) => return Outcome::Install(self.harness),
            KeyCode::Enter => match self.focus {
                Focus::List if self.sign_in_needed() => return self.launch(),
                Focus::List if self.needs_model() => {
                    self.focus = Focus::Models;
                }
                _ => return self.launch(),
            },
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = if self.focus == Focus::List && self.needs_model() { Focus::Models } else { Focus::List };
            }
            KeyCode::Right | KeyCode::Left if self.orchestrator && k.modifiers.contains(KeyModifiers::SHIFT) => {
                let all = Harness::ALL;
                let i = all.iter().position(|h| *h == self.harness).unwrap_or(0) as i32;
                let d = if k.code == KeyCode::Right { 1 } else { -1 };
                self.set_brain(all[(i + d).rem_euclid(all.len() as i32) as usize]);
            }
            KeyCode::Right => self.set_harness(1),
            KeyCode::Left => self.set_harness(-1),
            KeyCode::Down => self.move_sel(1),
            KeyCode::Up => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(8),
            KeyCode::PageUp => self.move_sel(-8),
            KeyCode::Char('n' | 'j') if ctrl => self.move_sel(1),
            KeyCode::Char('p' | 'k') if ctrl => self.move_sel(-1),
            KeyCode::Char('e') if ctrl => {
                self.permission = match self.permission {
                    Permission::Default => Permission::Auto,
                    Permission::Auto => Permission::Skip,
                    Permission::Skip => Permission::Default,
                }
            }
            KeyCode::Char('b') if ctrl => {
                self.browser = match self.browser {
                    BrowserMode::Off => BrowserMode::Auto,
                    BrowserMode::Auto => BrowserMode::Edge,
                    BrowserMode::Edge => BrowserMode::Chrome,
                    BrowserMode::Chrome => BrowserMode::Off,
                }
            }
            KeyCode::Char('s') if ctrl && !self.orchestrator => self.place = if self.place == Place::Tab { Place::Split } else { Place::Tab },
            KeyCode::Char('u') if ctrl => {
                self.filter_mut().clear();
                self.filter_changed();
            }
            KeyCode::Backspace => {
                self.filter_mut().pop();
                self.filter_changed();
            }
            // typing on the run-on list picks the project (esc there comes back here)
            KeyCode::Char(_) if !ctrl && self.focus == Focus::List => {
                self.open_project_picker();
                if let Some(p) = &mut self.project_picker {
                    p.key(k);
                }
            }
            KeyCode::Char(ch) if !ctrl => {
                self.filter_mut().push(ch);
                self.filter_changed();
            }
            _ => {}
        }
        Outcome::None
    }
}

#[cfg(test)]
mod tests;
