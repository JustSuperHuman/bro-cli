//! The launcher: choose a project, an agent, and where it runs.
//!
//! 1. **Project** — the path bar shows the current location and opens the project picker.
//! 2. **Harness** — tabs across the top (←/→): claude, codex, pi, omp, and ★ orchestrator. The orchestrator
//!    coordinates the selected project; on its tab a second row picks the harness it runs on
//!    (shift+←/→), and the list starts on the smartest login for it (your last orchestrator combo unless that
//!    login is nearly out, else the ready login with the most left).
//! 3. **Providers** — one list. The main ones first: the harness's own logins (no model to pick — the CLI uses
//!    its default and `/model` works inside), the Just Superhuman account, the other family's logins through
//!    bro-proxy, Gemini. In proxy mode a family is one row, not a row per login. Then **connected**: API providers
//!    with a key (OpenRouter's live catalogue, …). The ones without a key fold away under **other providers**
//!    (Enter or a click opens it). The cursor starts on what this harness ran last, its model picked.
//! 4. **Model** — only when the choice needs one (providers, cross-family logins): a second, filterable list.
//!
//! Sessions start in the current project. The project row lets you switch to an open project or enter a new
//! path with the folder picker's completion; typing on the run-on list opens that picker with what you typed
//! (Esc there comes back to the list). Enter on a row that needs a model moves to the model list;
//! Enter there (or on anything else) launches. This file is the model + keys (pure, testable);
//! `view.rs` draws it.
//!
//! A third tab, **Create Slop** (alt+s), sets a session up for making video and media: a cast and the media
//! tools the machine has (`studio.rs`).

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;
pub mod studio;
/// The Create Slop tab in the desktop app.
#[cfg(feature = "gui")]
pub(crate) mod studio_gui;

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
    /// Create Slop: a session cast and tooled up for video and media
    Slop,
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
    /// proxy mode: every signed-in login of the Claude family (`true`) or the Codex one
    Shared(bool),
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
    /// why it can't run yet, in a few words ("no key", "not signed in")
    pub blocked: Option<&'static str>,
    /// % left of the 5h window (weekly when that's the only reported window)
    pub left: Option<f64>,
}

/// One row of the "run on" list.
#[derive(Clone, Debug)]
pub enum Item {
    Header(&'static str),
    Account(Account),
    /// The fold over the providers with no key yet: how many it holds, and whether it's open.
    Others { count: usize, open: bool },
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
    /// Project key → running coordinator's launch label.
    pub orchestrator_running: BTreeMap<String, String>,
    /// Proxy mode: sessions run on all of a family's logins, so the list offers those instead of each login.
    pub shared: bool,
    /// The Just Superhuman account as last read, while signed in (its row shows the credits).
    pub jsh: Option<bro_core::jsh::Account>,
    /// Create Slop: who can be cast and what the machine has for making media (empty until its tab is opened).
    pub studio: crate::services::studio::StudioState,
}

/// Something clickable in the launcher (recorded while drawing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Profiles,
    Project,
    NoProject,
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
    Launch,
    /// "install it" under an agent that isn't installed
    Install,
    /// Create Slop: a character, by its place in the filtered cast
    Cast(usize),
    /// Create Slop: a tool, by its place in the toolkit
    Tool(usize),
    /// Create Slop: fetch the roster again
    StudioRefresh,
    /// Create Slop: where it runs (the New tab is where that's chosen)
    RunsOn,
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
    /// Create Slop: install a media tool the machine lacks (`true`: it is installed and stopped, start it again).
    InstallTool(bro_core::quick_install::Tool, bool),
    /// Create Slop: keep this key for a media tool (its id, the key).
    SaveKey(&'static str, String),
}

/// A login with less than this % left isn't the orchestrator's first pick.
const NEARLY_OUT: f64 = 10.0;

/// The launcher state.
pub struct Launcher {
    pub data: Data,
    /// The agent; on the orchestrator tab, the harness the orchestrator runs on.
    pub harness: Harness,
    /// The ★ orchestrator tab is open.
    pub orchestrator: bool,
    /// Coordinate across projects from the dedicated workspace.
    pub no_project: bool,
    pub focus: Focus,
    pub list_filter: String,
    pub model_filter: String,
    /// index into the *selectable* rows of the run-on list
    pub list_sel: usize,
    /// "other providers" is unfolded
    pub others_open: bool,
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
    /// The Create Slop tab: the cast and the tools switched off.
    pub studio: studio::Studio,
    /// clickable regions from the last draw, and the popup's own rect
    pub hits: Vec<(ratatui::layout::Rect, Hit)>,
    pub area: ratatui::layout::Rect,
}

impl Launcher {
    /// Replace service data while keeping the user's account, model and typed filters.
    pub fn set_data(&mut self, data: Data) {
        let current = self.current();
        let model = self.model().map(|m| m.id);
        self.data = data;
        let items = self.items();
        let selectable = Self::selectable(&items);
        let selected = selectable.iter().position(|&i| match (&current, &items[i]) {
            (Some(Item::Account(a)), Item::Account(b)) => a.kind == b.kind,
            (Some(Item::Others { .. }), Item::Others { .. }) => true,
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
        self.studio_sync();
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
        // Browser tools are managed with their installation, not a per-session switch.
        let browser = BrowserMode::Off;
        let permission = Permission::Skip;
        let mut l = Launcher {
            data,
            harness,
            orchestrator: false,
            no_project: false,
            focus: Focus::List,
            list_filter: String::new(),
            model_filter: String::new(),
            list_sel: 0,
            others_open: false,
            model_sel: 0,
            dir,
            projects: vec![],
            project_picker: None,
            tab: SessionTab::New,
            continue_picker: ContinuePicker::new(vec![], None),
            permission,
            browser,
            place,
            studio: studio::Studio::default(),
            hits: vec![],
            area: ratatui::layout::Rect::default(),
        };
        l.list_sel = l.last_sel();
        l.reset_models();
        l
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

    pub(crate) fn open_project_picker(&mut self) {
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
        self.no_project = false;
        self.project_picker = None;
        // another project: the cast it last cooked with
        self.studio_sync();
        self.hits.clear();
    }

    pub fn select_no_project(&mut self) {
        self.no_project = true;
        self.project_picker = None;
        self.hits.clear();
    }

    pub fn launch_dir(&self) -> PathBuf {
        if self.orchestrator && self.no_project { bro_core::orchestrator::workspace() } else { self.dir.clone() }
    }

    pub fn running_orchestrator(&self) -> Option<&String> {
        self.data.orchestrator_running.get(&bro_core::orchestrator::scope_key(&self.launch_dir()))
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
        self.list_sel = self.last_sel();
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

    /// Select a provider's row (the Just Superhuman account, say). None when the list doesn't offer it.
    pub fn with_provider(mut self, id: &str) -> Option<Launcher> {
        self.list_filter.clear();
        let kind = AccountKind::Provider(id.into());
        // one with no key yet sits under "other providers": open it
        if self.row_of(&kind).is_none() {
            self.others_open = true;
        }
        self.list_sel = self.row_of(&kind)?;
        self.reset_models();
        Some(self)
    }

    /// Where an account sits among the selectable rows.
    fn row_of(&self, kind: &AccountKind) -> Option<usize> {
        let items = self.items();
        Self::selectable(&items).iter().position(|&i| matches!(&items[i], Item::Account(a) if a.kind == *kind))
    }

    /// Select proxy mode's row for a family's logins together (None outside proxy mode, or with none signed in).
    pub fn with_shared(mut self, claude: bool) -> Option<Launcher> {
        self.list_filter.clear();
        let items = self.items();
        self.list_sel = Self::selectable(&items).iter().position(|&i| matches!(&items[i], Item::Account(a) if a.kind == AccountKind::Shared(claude)))?;
        self.reset_models();
        Some(self)
    }

    /// What a one-click "new claude / codex" should run: where the cursor starts (your latest combo for the
    /// harness, else its first login). None when that still needs a decision (not installed, nothing to run on, a
    /// model to pick that isn't the one you ran last).
    pub fn quick_spec(&self) -> Option<LaunchSpec> {
        if !self.installed(self.harness) {
            return None;
        }
        match self.current()? {
            Item::Account(a) if !a.ready => None,
            Item::Account(_) if self.needs_model() && !self.on_last_run() => None,
            Item::Account(_) => self.spec(),
            _ => None,
        }
    }

    /// The combos this harness ran, newest first (on the orchestrator tab, the orchestrator's).
    fn runs(&self) -> impl Iterator<Item = &Recent> {
        self.data.recents.iter().filter(|r| r.harness == self.harness && r.orchestrator == self.orchestrator)
    }

    /// The newest of them that the list still offers, and its row among the selectable ones. In proxy mode a
    /// combo naming one login (or the pool) has no row; neither has a provider folded under "other providers".
    fn last_run(&self) -> Option<(&Recent, usize)> {
        let items = self.items();
        let sel = Self::selectable(&items);
        self.runs().find_map(|r| {
            let kind = self.recent_kind(r);
            sel.iter().position(|&i| matches!(&items[i], Item::Account(a) if a.kind == kind)).map(|k| (r, k))
        })
    }

    /// Where the cursor starts: on what this harness ran last, else the top of the list.
    fn last_sel(&self) -> usize {
        self.last_run().map_or(0, |(_, k)| k)
    }

    /// The cursor and the model are the combo this harness ran last.
    fn on_last_run(&self) -> bool {
        self.last_run().is_some_and(|(r, k)| k == self.list_sel && r.model.is_some() && self.model().map(|m| m.id) == r.model)
    }

    /// Open on the ★ orchestrator tab.
    pub fn with_orchestrator(mut self) -> Launcher {
        self.enter_orchestrator();
        self
    }

    /// Switch to the orchestrator tab on its smartest harness and login.
    fn enter_orchestrator(&mut self) {
        self.orchestrator = true;
        self.no_project |= bro_core::orchestrator::is_global(&self.dir);
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
        for r in self.runs() {
            let kind = self.recent_kind(r);
            let row = sel.iter().position(|&i| matches!(&items[i], Item::Account(a) if a.kind == kind));
            if let Some(k) = row
                && fine(self.left_of(r.profile_id.as_deref().or(r.provider_id.as_deref())))
            {
                return k;
            }
        }
        let own = |a: &Account| match &a.kind {
            AccountKind::Profile(p) => match self.harness {
                Harness::Claude => p.starts_with("claude:"),
                Harness::Codex => p.starts_with("codex:"),
                _ => false,
            },
            AccountKind::Native | AccountKind::Pool => true,
            AccountKind::Shared(claude) => self.own_family() == Some(*claude),
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
        let mut detail = vec![];
        // a Google subscription is the Gemini row; an API key is named for its profile
        let label = if p.is_gemini_subscription() {
            detail.push(p.plan.clone().unwrap_or_else(|| "Google AI subscription".into()));
            "Gemini".to_string()
        } else if p.is_gemini_api() {
            detail.push(if p.is_local() { "API key on this computer" } else { "Gemini API key" }.to_string());
            if p.is_local() { "Gemini API".to_string() } else { p.name.clone() }
        } else {
            if via_proxy {
                detail.push(if p.is_claude() { "Claude" } else { "ChatGPT" }.to_string());
            }
            if let Some(plan) = &p.plan {
                detail.push(plan.clone());
            }
            p.name.clone()
        };
        if let Some(status) = self.data.usage_status.get(&p.id) {
            detail.push(status.clone());
        }
        Account {
            kind: AccountKind::Profile(p.id.clone()),
            label,
            detail: detail.join(" · "),
            ready: p.authenticated,
            blocked: (!p.authenticated).then_some("not signed in"),
            left: self.data.usage.get(&p.id).map(|u| (100.0 - *u as f64).clamp(0.0, 100.0)),
        }
    }

    fn pool_account(&self) -> Option<Account> {
        let n = self.data.profiles.iter().filter(|p| p.kind == ProfileKind::ClaudeAccount && p.authenticated).count();
        (n > 0).then(|| Account { kind: AccountKind::Pool, label: "Claude pool".into(), detail: format!("{n} accounts · moves on when one runs out"), ready: true, blocked: None, left: None })
    }

    /// The family a harness signs in to itself: Claude (`true`) or Codex; None for Pi and omp.
    fn own_family(&self) -> Option<bool> {
        match self.harness {
            Harness::Claude => Some(true),
            Harness::Codex => Some(false),
            Harness::Pi | Harness::Omp | Harness::Antigravity => None,
        }
    }

    /// Proxy mode's row for a family, in place of a row per login: how many logins share the work, and the % they
    /// have left together (the average over them, as the sidebar sums a family up).
    fn shared_account(&self, claude: bool) -> Option<Account> {
        let logins = bro_core::profiles::subscription_logins_in(&self.data.profiles, claude);
        if logins.is_empty() {
            return None;
        }
        let read: Vec<f64> = logins.iter().filter_map(|p| self.data.usage.get(&p.id)).map(|u| (100.0 - *u as f64).clamp(0.0, 100.0)).collect();
        let left = (!read.is_empty()).then(|| read.iter().sum::<f64>() / read.len() as f64);
        let n = logins.len();
        let detail = format!("{n} login{} · proxy mode", if n == 1 { "" } else { "s" });
        Some(Account { kind: AccountKind::Shared(claude), label: if claude { "Claude" } else { "ChatGPT" }.into(), detail, ready: true, blocked: None, left })
    }

    /// The row a recent combo ran on.
    pub fn recent_kind(&self, rc: &Recent) -> AccountKind {
        match (rc.provider_id.as_deref(), rc.profile_id.as_deref()) {
            (Some("pool"), _) => AccountKind::Pool,
            (Some(bro_core::launch::LOGINS_CLAUDE), _) => AccountKind::Shared(true),
            (Some(bro_core::launch::LOGINS_CODEX), _) => AccountKind::Shared(false),
            (Some(p), _) => AccountKind::Provider(p.into()),
            (None, Some(p)) => AccountKind::Profile(p.into()),
            (None, None) => match self.own_family().filter(|_| self.data.shared) {
                Some(claude) => AccountKind::Shared(claude),
                None => AccountKind::Native,
            },
        }
    }

    /// The Just Superhuman account's row: the organization that pays and its credits (or why Claude can't run
    /// there), and what's left of the month's allowance as a login's meter.
    fn jsh_account(&self, p: &Provider) -> Account {
        let keyed = p.no_key || self.data.keyed.iter().any(|k| k == &p.id);
        let name = bro_core::jsh::PROVIDER_NAME;
        let (detail, left) = match &self.data.jsh {
            Some(a) => match a.paused_reason() {
                Some(why) => (why.to_string(), None),
                None => {
                    // an organization named for the service itself would only say the row's name twice
                    let same = |o: &str| o.replace(' ', "").eq_ignore_ascii_case(&name.replace(' ', ""));
                    let org = Some(a.organization.name.trim()).filter(|o| !o.is_empty() && !same(o));
                    let credits = a.credits_text();
                    (org.map_or(credits.clone(), |o| format!("{o} · {credits}")), a.credits.month_used_pct().map(|used| (100.0 - used as f64).clamp(0.0, 100.0)))
                }
            },
            None => ("your team's credits".into(), None),
        };
        Account { kind: AccountKind::Provider(p.id.clone()), label: name.into(), detail, ready: keyed, blocked: (!keyed).then_some("not signed in"), left }
    }

    /// An API provider's row: its name ("Z.ai (GLM)" reads as Z.ai, then GLM) and how many models it has.
    fn provider_account(&self, p: &Provider) -> Account {
        if p.id == bro_core::jsh::PROVIDER_ID {
            return self.jsh_account(p);
        }
        let n = self.data.models.get(&p.id).map(Vec::len).unwrap_or(p.models.len());
        let keyed = p.no_key || self.data.keyed.iter().any(|k| k == &p.id);
        let (name, note) = match p.display_name().split_once(" (") {
            Some((name, note)) if note.ends_with(')') && !name.trim().is_empty() => (name.trim(), Some(note.trim_end_matches(')'))),
            _ => (p.display_name(), None),
        };
        let mut detail: Vec<String> = note.map(str::to_string).into_iter().collect();
        detail.push(format!("{n} model{}", if n == 1 { "" } else { "s" }));
        let blocked = if keyed {
            None
        } else if p.is_google_login() {
            Some("sign in with Google")
        } else {
            Some("no key")
        };
        Account { kind: AccountKind::Provider(p.id.clone()), label: name.to_string(), detail: detail.join(" · "), ready: keyed, blocked, left: None }
    }

    /// The full run-on list for the current harness (headings and the fold included), filtered.
    pub fn items(&self) -> Vec<Item> {
        let h = self.harness;
        let claude: Vec<&Profile> = self.data.profiles.iter().filter(|p| p.is_claude()).collect();
        let codex: Vec<&Profile> = self.data.profiles.iter().filter(|p| p.is_codex()).collect();

        let native = || Account { kind: AccountKind::Native, label: format!("{}'s own login", h.label()), detail: "whatever it's set up with".into(), ready: true, blocked: None, left: None };
        if h == Harness::Antigravity {
            return vec![Item::Account(Account {
                label: "Antigravity account".into(),
                detail: "Configured in agy".into(),
                ..native()
            })];
        }
        let (own, via): (Vec<Account>, Vec<Account>) = if self.data.shared {
            // proxy mode: no login to pick, each family's logins together
            match self.own_family() {
                Some(claude) => (self.shared_account(claude).into_iter().collect(), self.shared_account(!claude).into_iter().collect()),
                None => (vec![native()], [true, false].into_iter().filter_map(|c| self.shared_account(c)).collect()),
            }
        } else {
            match h {
            Harness::Claude => (
                claude.iter().map(|p| self.profile_account(p, false)).chain(self.pool_account()).collect(),
                codex.iter().map(|p| self.profile_account(p, true)).collect(),
            ),
            Harness::Codex => (
                codex.iter().map(|p| self.profile_account(p, false)).collect(),
                claude.iter().map(|p| self.profile_account(p, true)).chain(self.pool_account()).collect(),
            ),
            Harness::Pi | Harness::Omp | Harness::Antigravity => (
                vec![native()],
                claude.iter().chain(codex.iter()).map(|p| self.profile_account(p, true)).chain(self.pool_account()).collect(),
            ),
            }
        };
        // The main providers: your own logins first (Pi and omp keep theirs in proxy mode: only Claude and Codex
        // share), the Just Superhuman account's credits straight after, then the other family through the proxy
        // and Google's.
        let mut main = own;
        main.extend(self.data.providers.iter().filter(|p| p.id == bro_core::jsh::PROVIDER_ID).map(|p| self.jsh_account(p)));
        main.extend(via);
        main.extend(self.data.profiles.iter().filter(|p| p.is_gemini()).map(|p| self.profile_account(p, true)));
        // the API providers: the ones with a key are connected, the rest fold away. Google's sign-in is a main
        // one (until a subscription is signed in, which has its own row).
        let subscribed = self.data.profiles.iter().any(|p| p.is_gemini_subscription());
        let (mut connected, mut other) = (vec![], vec![]);
        for p in self.data.providers.iter().filter(|p| p.id != bro_core::jsh::PROVIDER_ID && p.mode != ProviderMode::Native) {
            let a = self.provider_account(p);
            if p.is_google_login() {
                if !subscribed {
                    main.push(a);
                }
            } else if a.ready {
                connected.push(a);
            } else {
                other.push(a);
            }
        }

        let q = self.list_filter.trim();
        let keep = |rows: Vec<Account>| -> Vec<Item> {
            if q.is_empty() {
                return rows.into_iter().map(Item::Account).collect();
            }
            fuzzy::filter(q, &rows, |a| self.account_text(a)).into_iter().map(|i| Item::Account(rows[i].clone())).collect()
        };
        let mut out = keep(main);
        let connected = keep(connected);
        if !connected.is_empty() {
            out.push(Item::Header("connected"));
            out.extend(connected);
        }
        if q.is_empty() {
            if !other.is_empty() {
                out.push(Item::Others { count: other.len(), open: self.others_open });
                if self.others_open {
                    out.extend(other.into_iter().map(Item::Account));
                }
            }
        } else {
            // a filter looks under the fold too
            let other = keep(other);
            if !other.is_empty() {
                out.push(Item::Header("other providers"));
                out.extend(other);
            }
        }
        out
    }

    /// What a filter matches a row on: its name and detail, and a provider's id ("openrouter" for OpenRouter).
    fn account_text(&self, a: &Account) -> String {
        match &a.kind {
            AccountKind::Provider(id) => format!("{} {} {id}", a.label, a.detail),
            _ => format!("{} {}", a.label, a.detail),
        }
    }

    /// Fold or unfold "other providers" (the cursor is on it).
    fn toggle_others(&mut self) {
        self.others_open = !self.others_open;
        self.hits.clear();
    }

    /// The cursor is on the "other providers" fold.
    pub fn on_others(&self) -> bool {
        self.focus == Focus::List && matches!(self.current(), Some(Item::Others { .. }))
    }

    /// A click on selectable row `i`: when that's the fold, the cursor goes to it and it opens or closes (true).
    pub(crate) fn pick_fold(&mut self, i: usize) -> bool {
        let items = self.items();
        let fold = Self::selectable(&items).get(i).is_some_and(|&k| matches!(items[k], Item::Others { .. }));
        if !fold {
            return false;
        }
        self.focus = Focus::List;
        if self.list_sel != i {
            self.list_sel = i;
            self.reset_models();
        }
        self.toggle_others();
        true
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
                AccountKind::Profile(p) => !((self.harness == Harness::Claude && p.starts_with("claude:")) || (self.harness == Harness::Codex && p.starts_with("codex:"))),
                AccountKind::Pool => self.harness != Harness::Claude,
                AccountKind::Shared(claude) => self.own_family() != Some(*claude),
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
                AccountKind::Profile(p) if bro_core::profiles::is_gemini_id(&p) => p,
                AccountKind::Profile(p) if p.starts_with("codex:") => "codex".into(),
                AccountKind::Shared(false) => "codex".into(),
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
            cwd: self.launch_dir(),
            resume: None,
            permission: self.permission,
            browser: self.browser,
            extra_args: vec![],
            orchestrator: self.orchestrator,
            // the Create Slop tab launches with its kit
            studio: (self.tab == SessionTab::Slop).then(|| self.studio_kit()),
        };
        // Focusing a running coordinator does not need a new account or model selection.
        if self.orchestrator && self.running_orchestrator().is_some() {
            return Some(base(None, None, None));
        }
        match self.current()? {
            Item::Header(_) | Item::Others { .. } => None,
            Item::Account(a) => {
                let model = if self.needs_model() { Some(self.model()?.id) } else { None };
                Some(match a.kind {
                    AccountKind::Profile(p) => base(Some(p), None, model),
                    AccountKind::Provider(p) => base(None, Some(p), model),
                    AccountKind::Pool => base(None, Some("pool".into()), model),
                    // its own family's logins: proxy mode picks the login it runs as
                    AccountKind::Shared(claude) if self.own_family() == Some(claude) => base(None, None, model),
                    AccountKind::Shared(claude) => base(None, Some(bro_core::launch::logins_upstream(claude).into()), model),
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
        let pinned = match self.current() {
            Some(Item::Account(Account { kind: AccountKind::Profile(p), .. })) if bro_core::profiles::is_gemini_id(&p) => Some((p, self.model().map(|m| m.id))),
            _ => None,
        };
        self.orchestrator = false;
        self.harness = all[next];
        self.list_filter.clear();
        self.list_sel = self.last_sel();
        let mut chosen_model = None;
        if let Some((p, model)) = pinned {
            chosen_model = model;
            let items = self.items();
            if let Some(i) = Self::selectable(&items).iter().position(|&i| matches!(&items[i], Item::Account(Account { kind: AccountKind::Profile(id), .. }) if id == &p)) { self.list_sel = i; }
        }
        self.reset_models();
        if let Some(model) = chosen_model {
            if let Some(index) = self.models_view().iter().position(|&i| self.models()[i].id == model) { self.model_sel = index; }
        }
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
            AccountKind::Shared(claude) if self.own_family() == Some(*claude) => (None, None),
            AccountKind::Shared(claude) => (None, Some(bro_core::launch::logins_upstream(*claude).to_string())),
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
        if self.tab == SessionTab::Slop {
            self.studio_paste(s);
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
        if self.sign_in_needed() && !(self.orchestrator && self.running_orchestrator().is_some()) {
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
            Hit::NoProject if self.orchestrator => self.select_no_project(),
            Hit::NoProject => {},
            Hit::Tab(tab) => self.set_tab(tab),
            Hit::Cast(i) => {
                self.studio.focus = studio::StudioFocus::Cast;
                let cast = self.studio_cast();
                if let Some(c) = cast.get(i).and_then(|&k| self.data.studio.cast.get(k)).cloned() {
                    self.studio.sel = i;
                    self.studio_toggle(c);
                }
            }
            Hit::Tool(i) => {
                if let Some(t) = bro_core::studio::TOOLS.get(i) {
                    self.studio.focus = studio::StudioFocus::Tools;
                    self.studio.key = None;
                    // a tool that can't be used yet: the click gets it going (the selected one; the first
                    // click selects)
                    if self.studio_fix(t.id).is_some() {
                        if self.studio.tool_sel == i {
                            return self.studio_act(i);
                        }
                        self.studio.tool_sel = i;
                    } else {
                        self.studio.tool_sel = i;
                        self.studio_toggle_tool(t.id);
                    }
                }
            }
            Hit::StudioRefresh => self.studio.ask = Some(true),
            Hit::RunsOn => self.set_tab(SessionTab::New),
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
                if self.pick_fold(i) {
                    return Outcome::None;
                }
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
        if self.tab == SessionTab::Slop {
            self.studio.focus = studio::StudioFocus::Cast;
            self.studio_move(0, if down { 1 } else { -1 });
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
                KeyCode::Char('g' | 'G') if self.orchestrator => { self.select_no_project(); return Outcome::None; }
                KeyCode::Char('n' | 'N') => { self.set_tab(SessionTab::New); return Outcome::None; }
                KeyCode::Char('r' | 'R') => { self.show_continue(); return Outcome::None; }
                KeyCode::Char('s' | 'S') => { self.set_tab(SessionTab::Slop); return Outcome::None; }
                _ => {}
            }
        }
        if self.tab == SessionTab::Continue {
            return Self::continue_outcome(self.continue_picker.key(k));
        }
        if self.tab == SessionTab::Slop {
            return self.studio_key(k);
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
            // the fold opens and closes under the cursor
            KeyCode::Enter | KeyCode::Char(' ') if !ctrl && self.on_others() => self.toggle_others(),
            KeyCode::Enter if self.orchestrator && self.running_orchestrator().is_some() => return self.launch(),
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
