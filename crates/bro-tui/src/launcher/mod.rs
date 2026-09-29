//! The launcher: start an agent session in three decisions.
//!
//! 1. **Harness** — tabs across the top (←/→): claude, codex, pi, omp.
//! 2. **Run on** — one list: recent combos for this harness, then its own logins (no model to pick — the CLI
//!    uses its default and `/model` works inside), then other logins it can use through bro-proxy, then API
//!    providers (OpenRouter's live catalogue, DeepSeek, …).
//! 3. **Model** — only when the choice needs one (providers, cross-family logins): a second, filterable list.
//!
//! Sessions start in the current project (the one selected in the sidebar). Enter on a row that needs a
//! model moves to the model list; Enter there (or on anything else) launches. This file is the model + keys (pure, testable);
//! `view.rs` draws it.

pub mod view;

use crate::fuzzy;
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
    /// % left of the 5h window, for logins we have meters for
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

/// Snapshot of what the launcher offers (taken from services when it opens).
#[derive(Clone, Debug, Default)]
pub struct Data {
    pub profiles: Vec<Profile>,
    pub providers: Vec<Provider>,
    /// profile id → 5h used %
    pub usage: BTreeMap<String, f32>,
    /// harness → installed on PATH
    pub installed: Vec<(Harness, bool)>,
    pub recents: Vec<Recent>,
    /// provider id (plus "codex", "claude") → models
    pub models: BTreeMap<String, Vec<ModelRow>>,
    /// provider ids with a key configured (or that need none)
    pub keyed: Vec<String>,
}

/// Something clickable in the launcher (recorded while drawing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Harness(Harness),
    /// index into the selectable run-on rows
    Row(usize),
    /// index into the filtered models
    Model(usize),
    Perm(Permission),
    Browser,
    Place,
    Launch,
    /// the × in the corner
    Close,
}

/// What a key asks for.
pub enum Outcome {
    None,
    Close,
    Launch(LaunchSpec, Place),
}

/// Recent combos shown per harness.
const RECENTS_SHOWN: usize = 4;

/// The launcher state.
pub struct Launcher {
    pub data: Data,
    pub harness: Harness,
    pub focus: Focus,
    pub list_filter: String,
    pub model_filter: String,
    /// index into the *selectable* rows of the run-on list
    pub list_sel: usize,
    /// index into the filtered models
    pub model_sel: usize,
    pub dir: PathBuf,
    pub permission: Permission,
    pub browser: BrowserMode,
    pub place: Place,
    /// clickable regions from the last draw, and the popup's own rect
    pub hits: Vec<(ratatui::layout::Rect, Hit)>,
    pub area: ratatui::layout::Rect,
}

impl Launcher {
    /// Open on the most recent harness, in `cwd` (the current project; else the most recent one).
    pub fn new(data: Data, cwd: Option<PathBuf>, place: Place) -> Launcher {
        let recent = data.recents.first().cloned();
        let harness = recent.as_ref().map(|r| r.harness).unwrap_or(Harness::Claude);
        let dir = cwd
            .or_else(|| recent.as_ref().map(|r| r.cwd.clone()))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let (permission, browser) = recent.map(|r| (r.permission, r.browser)).unwrap_or_default();
        Launcher {
            data,
            harness,
            focus: Focus::List,
            list_filter: String::new(),
            model_filter: String::new(),
            list_sel: 0,
            model_sel: 0,
            dir,
            permission,
            browser,
            place,
            hits: vec![],
            area: ratatui::layout::Rect::default(),
        }
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
        if !keyed {
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
            if r.harness != h {
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

    fn set_harness(&mut self, d: i32) {
        let all = Harness::ALL;
        let i = all.iter().position(|h| *h == self.harness).unwrap_or(0) as i32;
        self.harness = all[(i + d).rem_euclid(all.len() as i32) as usize];
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
        self.filter_mut().push_str(s.trim());
        self.filter_changed();
    }

    fn launch(&self) -> Outcome {
        match self.spec() {
            Some(s) => Outcome::Launch(s, self.place),
            None => Outcome::None,
        }
    }

    /// A mouse click. Clicking the selected row again acts like Enter; clicking outside closes.
    pub fn click(&mut self, pos: ratatui::layout::Position) -> Outcome {
        if !self.area.contains(pos) {
            return Outcome::Close;
        }
        let Some(&(_, hit)) = self.hits.iter().find(|(r, _)| r.contains(pos)) else { return Outcome::None };
        match hit {
            Hit::Harness(h) => {
                while self.harness != h {
                    self.set_harness(1);
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
            Hit::Launch => return self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Hit::Close => return Outcome::Close,
        }
        Outcome::None
    }

    /// Mouse wheel over the launcher: move in the list under the pointer.
    pub fn scroll(&mut self, pos: ratatui::layout::Position, down: bool) {
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
            KeyCode::Enter => match self.focus {
                Focus::List if self.needs_model() => {
                    self.focus = Focus::Models;
                }
                _ => return self.launch(),
            },
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = if self.focus == Focus::List && self.needs_model() { Focus::Models } else { Focus::List };
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
            KeyCode::Char('s') if ctrl => self.place = if self.place == Place::Tab { Place::Split } else { Place::Tab },
            KeyCode::Char('u') if ctrl => {
                self.filter_mut().clear();
                self.filter_changed();
            }
            KeyCode::Backspace => {
                self.filter_mut().pop();
                self.filter_changed();
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
