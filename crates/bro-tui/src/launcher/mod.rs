//! The launcher: a centered modal to start an agent session. Columns harness × account (profiles, key-based
//! providers, the Claude pool) × model × project dir, each fuzzy-filterable; recent combos on top; permission
//! mode, browser and placement toggles. Enter builds a `LaunchSpec`.
//!
//! This file is the model + key handling (pure, testable); `view.rs` draws it.

pub mod view;

use crate::fuzzy;
use crate::pane::Place;
use crate::recents::Recent;
use bro_core::Harness;
use bro_core::browser::BrowserMode;
use bro_core::launch::{LaunchSpec, Permission};
use bro_core::profiles::{Profile, ProfileKind};
use bro_core::providers::{Provider, ProviderMode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A column of the launcher.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Recent = 0,
    Harness = 1,
    Account = 2,
    Model = 3,
    Dir = 4,
}

impl Col {
    const ALL: [Col; 5] = [Col::Recent, Col::Harness, Col::Account, Col::Model, Col::Dir];
    pub fn label(self) -> &'static str {
        ["recent", "harness", "account", "model", "project"][self as usize]
    }
}

/// Who a session runs as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountKind {
    Profile(String),
    Provider(String),
    Pool,
}

/// One entry of the account column.
#[derive(Clone, Debug)]
pub struct Account {
    pub kind: AccountKind,
    pub label: String,
    pub detail: String,
    /// logged in / has what it needs
    pub ready: bool,
    /// 5h usage %, for profiles we have meters for
    pub five_hour: Option<f32>,
}

/// One entry of the model column (`id` None = the account's default).
#[derive(Clone, Debug)]
pub struct ModelChoice {
    pub id: Option<String>,
    pub label: String,
}

/// One entry of the project column.
#[derive(Clone, Debug)]
pub struct DirChoice {
    pub path: PathBuf,
    pub typed: bool,
}

/// Snapshot of what the launcher offers (taken from services when it opens).
#[derive(Clone, Debug, Default)]
pub struct Data {
    pub profiles: Vec<Profile>,
    pub providers: Vec<Provider>,
    /// profile id → 5h used %
    pub usage: BTreeMap<String, f32>,
    /// harness → installed on PATH
    pub installed: Vec<(Harness, bool)>,
    /// candidate dirs, best first (selected project, live projects, recents, past)
    pub dirs: Vec<PathBuf>,
    pub recents: Vec<Recent>,
}

/// What Enter asks for.
pub enum Outcome {
    None,
    Close,
    Launch(LaunchSpec, Place),
}

/// The launcher state.
pub struct Launcher {
    pub data: Data,
    pub col: Col,
    pub filters: [String; 5],
    /// cursor per column, as an index into that column's *filtered* list
    pub sel: [usize; 5],
    pub permission: Permission,
    pub browser: BrowserMode,
    pub place: Place,
}

impl Launcher {
    /// Open, preselecting the most recent combo (but `cwd` if given).
    pub fn new(data: Data, cwd: Option<PathBuf>, place: Place) -> Launcher {
        let mut l = Launcher { data, col: Col::Harness, filters: Default::default(), sel: [0; 5], permission: Permission::Default, browser: BrowserMode::Off, place };
        if let Some(r) = l.data.recents.first().cloned() {
            l.apply_recent(&r);
        }
        if let Some(c) = cwd {
            l.data.dirs.retain(|d| d != &c);
            l.data.dirs.insert(0, c);
            l.sel[Col::Dir as usize] = 0;
        }
        l
    }

    fn apply_recent(&mut self, r: &Recent) {
        if let Some(i) = self.harnesses().iter().position(|h| *h == r.harness) {
            self.sel[Col::Harness as usize] = i;
        }
        let want = match (&r.provider_id, &r.profile_id) {
            (Some(p), _) if p == "pool" => Some(AccountKind::Pool),
            (Some(p), _) => Some(AccountKind::Provider(p.clone())),
            (None, Some(p)) => Some(AccountKind::Profile(p.clone())),
            _ => None,
        };
        if let Some(w) = want
            && let Some(i) = self.accounts().iter().position(|a| a.kind == w) {
                self.sel[Col::Account as usize] = i;
            }
        if let Some(i) = self.models().iter().position(|m| m.id == r.model) {
            self.sel[Col::Model as usize] = i;
        }
        if let Some(i) = self.data.dirs.iter().position(|d| *d == r.cwd) {
            self.sel[Col::Dir as usize] = i;
        }
        self.permission = r.permission;
        self.browser = r.browser;
    }

    // ------------------------------------------------------------------ columns

    /// All harnesses (installed first is not enforced: order is fixed for muscle memory).
    pub fn harnesses(&self) -> Vec<Harness> {
        Harness::ALL.to_vec()
    }

    pub fn installed(&self, h: Harness) -> bool {
        self.data.installed.iter().find(|x| x.0 == h).map(|x| x.1).unwrap_or(true)
    }

    fn filtered<T>(&self, col: Col, items: &[T], text: impl Fn(&T) -> String) -> Vec<usize> {
        fuzzy::filter(&self.filters[col as usize], items, text)
    }

    /// The selected harness.
    pub fn harness(&self) -> Harness {
        let hs = self.harnesses();
        let idx = self.filtered(Col::Harness, &hs, |h| h.label().to_string());
        idx.get(self.sel[Col::Harness as usize]).map(|&i| hs[i]).unwrap_or(Harness::Claude)
    }

    /// Accounts that make sense for the selected harness: its own logins first, then the pool, then providers.
    pub fn accounts(&self) -> Vec<Account> {
        let h = self.harness();
        let d = &self.data;
        let prof = |p: &Profile, via: &str| Account {
            kind: AccountKind::Profile(p.id.clone()),
            label: p.name.clone(),
            detail: [p.plan.clone().unwrap_or_default(), via.to_string()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "),
            ready: p.authenticated,
            five_hour: d.usage.get(&p.id).copied(),
        };
        let claude: Vec<&Profile> = d.profiles.iter().filter(|p| p.is_claude()).collect();
        let codex: Vec<&Profile> = d.profiles.iter().filter(|p| p.is_codex()).collect();
        let pool_n = d.profiles.iter().filter(|p| p.kind == ProfileKind::ClaudeAccount && p.authenticated).count();
        let pool = Account { kind: AccountKind::Pool, label: "pool".into(), detail: format!("{pool_n} claude accounts · failover"), ready: pool_n > 0, five_hour: None };
        let providers = d.providers.iter().filter(|p| p.mode != ProviderMode::Native).map(|p| {
            let direct = matches!((h, p.mode), (Harness::Claude, ProviderMode::Anthropic) | (Harness::Codex, ProviderMode::Openai) | (Harness::Pi, _) | (Harness::Omp, _));
            Account { kind: AccountKind::Provider(p.id.clone()), label: p.id.clone(), detail: if direct { p.name.clone() } else { format!("{} · via proxy", p.name) }, ready: true, five_hour: None }
        });
        let mut v = vec![];
        match h {
            Harness::Claude => {
                v.extend(claude.iter().map(|p| prof(p, "")));
                v.push(pool);
                v.extend(codex.iter().map(|p| prof(p, "chatgpt via proxy")));
                v.extend(providers);
            }
            Harness::Codex => {
                v.extend(codex.iter().map(|p| prof(p, "")));
                v.extend(claude.iter().map(|p| prof(p, "claude via proxy")));
                v.push(pool);
                v.extend(providers);
            }
            Harness::Pi | Harness::Omp => {
                v.extend(providers);
                v.push(pool);
                v.extend(claude.iter().map(|p| prof(p, "via proxy")));
                v.extend(codex.iter().map(|p| prof(p, "via proxy")));
            }
        }
        v
    }

    /// Filtered account list.
    pub fn accounts_view(&self) -> (Vec<Account>, Vec<usize>) {
        let a = self.accounts();
        let idx = self.filtered(Col::Account, &a, |x| format!("{} {}", x.label, x.detail));
        (a, idx)
    }

    pub fn account(&self) -> Option<Account> {
        let (a, idx) = self.accounts_view();
        idx.get(self.sel[Col::Account as usize]).map(|&i| a[i].clone())
    }

    /// Models for the selected account (first is always "default").
    pub fn models(&self) -> Vec<ModelChoice> {
        let d = &self.data;
        let from = |pid: &str| -> Vec<ModelChoice> {
            d.providers.iter().find(|p| p.id == pid).map(|p| p.models.iter().filter(|m| !m.id.is_empty()).map(|m| ModelChoice { id: Some(m.id.clone()), label: m.name.clone().unwrap_or_else(|| m.id.clone()) }).collect()).unwrap_or_default()
        };
        let fixed = |ids: &[(&str, &str)]| ids.iter().map(|(id, n)| ModelChoice { id: Some(id.to_string()), label: n.to_string() }).collect::<Vec<_>>();
        let mut v = vec![ModelChoice { id: None, label: "default".into() }];
        let mut rest = match self.account().map(|a| a.kind) {
            Some(AccountKind::Provider(p)) => from(&p),
            Some(AccountKind::Profile(p)) if p.starts_with("codex:") => {
                let m = from("openai");
                if m.is_empty() { fixed(&[("gpt-5.2-codex", "GPT-5.2 Codex"), ("gpt-5.2", "GPT-5.2")]) } else { m }
            }
            Some(_) => {
                let m = from("anthropic");
                if m.is_empty() { fixed(&[("claude-opus-5", "Claude Opus 5"), ("claude-sonnet-5", "Claude Sonnet 5"), ("claude-haiku-4-5-20251001", "Claude Haiku 4.5")]) } else { m }
            }
            None => vec![],
        };
        let mut seen = std::collections::HashSet::new();
        rest.retain(|m| seen.insert(m.id.clone()));
        v.extend(rest);
        v
    }

    pub fn models_view(&self) -> (Vec<ModelChoice>, Vec<usize>) {
        let m = self.models();
        let idx = self.filtered(Col::Model, &m, |x| format!("{} {}", x.label, x.id.clone().unwrap_or_default()));
        (m, idx)
    }

    pub fn model(&self) -> Option<ModelChoice> {
        let (m, idx) = self.models_view();
        idx.get(self.sel[Col::Model as usize]).map(|&i| m[i].clone())
    }

    /// Project dirs; a typed path becomes the first entry.
    pub fn dirs_view(&self) -> Vec<DirChoice> {
        let f = self.filters[Col::Dir as usize].trim();
        let mut out = vec![];
        if looks_like_path(f) {
            out.push(DirChoice { path: expand(f), typed: true });
        }
        let idx = fuzzy::filter(if looks_like_path(f) { "" } else { f }, &self.data.dirs, |p| p.to_string_lossy().to_string());
        out.extend(idx.into_iter().map(|i| DirChoice { path: self.data.dirs[i].clone(), typed: false }));
        out
    }

    pub fn dir(&self) -> Option<PathBuf> {
        self.dirs_view().get(self.sel[Col::Dir as usize]).map(|d| d.path.clone())
    }

    pub fn recents_view(&self) -> Vec<usize> {
        self.filtered(Col::Recent, &self.data.recents, |r| format!("{} {} {} {} {}", r.harness.label(), r.profile_id.clone().unwrap_or_default(), r.provider_id.clone().unwrap_or_default(), r.model.clone().unwrap_or_default(), r.cwd.display()))
    }

    fn len(&self, col: Col) -> usize {
        match col {
            Col::Recent => self.recents_view().len(),
            Col::Harness => self.filtered(Col::Harness, &self.harnesses(), |h| h.label().to_string()).len(),
            Col::Account => self.accounts_view().1.len(),
            Col::Model => self.models_view().1.len(),
            Col::Dir => self.dirs_view().len(),
        }
    }

    /// The spec Enter would launch (None when something required is missing).
    pub fn spec(&self) -> Option<LaunchSpec> {
        if self.col == Col::Recent {
            let r = &self.data.recents[*self.recents_view().get(self.sel[0])?];
            return Some(LaunchSpec { harness: r.harness, profile_id: r.profile_id.clone(), provider_id: r.provider_id.clone(), model: r.model.clone(), cwd: r.cwd.clone(), resume: None, permission: r.permission, browser: r.browser, extra_args: vec![] });
        }
        let acct = self.account()?;
        let (profile_id, provider_id) = match acct.kind {
            AccountKind::Profile(p) => (Some(p), None),
            AccountKind::Provider(p) => (None, Some(p)),
            AccountKind::Pool => (None, Some("pool".to_string())),
        };
        Some(LaunchSpec {
            harness: self.harness(),
            profile_id,
            provider_id,
            model: self.model().and_then(|m| m.id),
            cwd: self.dir()?,
            resume: None,
            permission: self.permission,
            browser: self.browser,
            extra_args: vec![],
        })
    }

    // ------------------------------------------------------------------ keys

    fn cols(&self) -> Vec<Col> {
        Col::ALL.iter().copied().filter(|c| *c != Col::Recent || !self.data.recents.is_empty()).collect()
    }

    fn move_col(&mut self, d: i32) {
        let cols = self.cols();
        let i = cols.iter().position(|c| *c == self.col).unwrap_or(0) as i32;
        self.col = cols[(i + d).rem_euclid(cols.len() as i32) as usize];
    }

    fn move_sel(&mut self, d: i32) {
        let n = self.len(self.col);
        let c = self.col as usize;
        if n == 0 {
            self.sel[c] = 0;
            return;
        }
        self.sel[c] = (self.sel[c] as i32 + d).clamp(0, n as i32 - 1) as usize;
        self.clamp_after(self.col);
    }

    /// Changing a column resets the ones that depend on it.
    fn clamp_after(&mut self, col: Col) {
        if col == Col::Harness {
            self.sel[Col::Account as usize] = 0;
            self.filters[Col::Account as usize].clear();
        }
        if matches!(col, Col::Harness | Col::Account) {
            self.sel[Col::Model as usize] = 0;
            self.filters[Col::Model as usize].clear();
        }
    }

    /// Handle a key. The launcher is modal: it takes every key.
    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let c = self.col as usize;
        match k.code {
            KeyCode::Esc if !self.filters[c].is_empty() => {
                self.filters[c].clear();
                self.sel[c] = 0;
            }
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Enter => {
                return match self.spec() {
                    Some(s) => Outcome::Launch(s, self.place),
                    None => Outcome::None,
                };
            }
            KeyCode::Tab | KeyCode::Right => self.move_col(1),
            KeyCode::BackTab | KeyCode::Left => self.move_col(-1),
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
            KeyCode::Char('s') if ctrl => self.place = if self.place == Place::Tab { Place::Split } else { Place::Tab },
            KeyCode::Char('u') if ctrl => {
                self.filters[c].clear();
                self.sel[c] = 0;
            }
            KeyCode::Backspace => {
                self.filters[c].pop();
                self.sel[c] = 0;
                self.clamp_after(self.col);
            }
            KeyCode::Char(ch) if !ctrl => {
                self.filters[c].push(ch);
                self.sel[c] = 0;
                self.clamp_after(self.col);
            }
            _ => {}
        }
        Outcome::None
    }
}

/// True for input that is clearly a path ("~/x", "C:\x", "/x", "./x").
pub fn looks_like_path(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('~') || s.starts_with('/') || s.starts_with('.') || s.starts_with('\\') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// Expand a leading `~`.
pub fn expand(s: &str) -> PathBuf {
    match s.strip_prefix('~') {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest.trim_start_matches(['/', '\\'])),
        None => PathBuf::from(s),
    }
}

/// The candidate dirs, deduplicated, in the given priority order.
pub fn dedup_dirs(groups: Vec<Vec<PathBuf>>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for p in groups.into_iter().flatten() {
        if !out.iter().any(|o| same_path(o, &p)) {
            out.push(p);
        }
    }
    out
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() } else { a == b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{Services, fallback_settings};

    fn data() -> Data {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx);
        let st = svc.state();
        Data {
            profiles: st.profiles.ready().cloned().unwrap_or_default(),
            providers: st.providers.ready().cloned().unwrap_or_default(),
            usage: BTreeMap::new(),
            installed: vec![],
            dirs: vec![PathBuf::from("/code/bro"), PathBuf::from("/code/justgains"), PathBuf::from("/code/terminal")],
            recents: vec![],
        }
    }

    fn key(l: &mut Launcher, code: KeyCode) -> Outcome {
        l.key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn typ(l: &mut Launcher, s: &str) {
        for c in s.chars() {
            key(l, KeyCode::Char(c));
        }
    }

    #[test]
    fn filtering_each_column() {
        let mut l = Launcher::new(data(), None, Place::Tab);
        assert_eq!(l.col, Col::Harness, "no recents: start on harness");
        typ(&mut l, "cod");
        assert_eq!(l.harness(), Harness::Codex);
        key(&mut l, KeyCode::Tab);
        assert_eq!(l.col, Col::Account);
        // codex: its own profiles first
        assert_eq!(l.account().unwrap().kind, AccountKind::Profile("codex:local".into()));
        typ(&mut l, "team");
        assert_eq!(l.account().unwrap().kind, AccountKind::Profile("codex:team".into()));
        key(&mut l, KeyCode::Tab);
        typ(&mut l, "mini");
        assert_eq!(l.model().unwrap().id.as_deref(), Some("gpt-5-mini"));
        key(&mut l, KeyCode::Tab);
        typ(&mut l, "just");
        assert_eq!(l.dir(), Some(PathBuf::from("/code/justgains")));
        match key(&mut l, KeyCode::Enter) {
            Outcome::Launch(s, Place::Tab) => {
                assert_eq!(s.harness, Harness::Codex);
                assert_eq!(s.profile_id.as_deref(), Some("codex:team"));
                assert_eq!(s.model.as_deref(), Some("gpt-5-mini"));
                assert_eq!(s.cwd, PathBuf::from("/code/justgains"));
            }
            _ => panic!("expected a launch"),
        }
    }

    #[test]
    fn typed_paths_pool_and_toggles() {
        let mut l = Launcher::new(data(), Some(PathBuf::from("/code/terminal")), Place::Tab);
        assert_eq!(l.dir(), Some(PathBuf::from("/code/terminal")), "cwd preselected");
        // claude → pool
        key(&mut l, KeyCode::Tab);
        typ(&mut l, "pool");
        let s = l.spec().unwrap();
        assert_eq!(s.provider_id.as_deref(), Some("pool"));
        assert!(s.profile_id.is_none());
        // a typed path wins
        key(&mut l, KeyCode::Tab);
        key(&mut l, KeyCode::Tab);
        typ(&mut l, "~/new-thing");
        assert!(l.dirs_view()[0].typed);
        assert!(l.dir().unwrap().ends_with("new-thing"));
        l.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        l.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(l.permission, Permission::Skip);
        l.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(l.place, Place::Split);
        // esc clears the filter first, then closes
        assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::None));
        assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::Close));
    }

    #[test]
    fn recents_preselect_and_relaunch() {
        let mut d = data();
        d.recents = vec![Recent { harness: Harness::Pi, profile_id: None, provider_id: Some("openrouter".into()), model: Some("qwen/qwen3-coder".into()), cwd: PathBuf::from("/code/justgains"), permission: Permission::Auto, browser: BrowserMode::Off, at: 1 }];
        let mut l = Launcher::new(d, None, Place::Tab);
        assert_eq!(l.harness(), Harness::Pi);
        assert_eq!(l.account().unwrap().kind, AccountKind::Provider("openrouter".into()));
        assert_eq!(l.model().unwrap().id.as_deref(), Some("qwen/qwen3-coder"));
        assert_eq!(l.dir(), Some(PathBuf::from("/code/justgains")));
        assert_eq!(l.permission, Permission::Auto);
        key(&mut l, KeyCode::BackTab);
        assert_eq!(l.col, Col::Recent);
        let s = l.spec().unwrap();
        assert_eq!(s.provider_id.as_deref(), Some("openrouter"));
        // changing harness resets account + model
        key(&mut l, KeyCode::Tab);
        key(&mut l, KeyCode::Up);
        key(&mut l, KeyCode::Up);
        assert_eq!(l.sel[Col::Account as usize], 0);
        assert!(l.model().unwrap().id.is_none());
    }

    #[test]
    fn paths_and_dedup() {
        assert!(looks_like_path("~/x") && looks_like_path("C:\\x") && looks_like_path("/x") && !looks_like_path("bro"));
        let d = dedup_dirs(vec![vec![PathBuf::from("/a"), PathBuf::from("/b")], vec![PathBuf::from("/a"), PathBuf::from("/c")]]);
        assert_eq!(d, vec![PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/c")]);
    }
}
