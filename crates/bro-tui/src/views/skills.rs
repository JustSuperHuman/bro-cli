//! The Skills view: every agent skill on this machine, where each came from, and where to get more. Its pages run
//! down the left as settings' sections do:
//!
//! - Installed: every skill your agents have, grouped by where it came from, with the agents that get each.
//! - Discover: Skill Issue's catalogue (<https://skill-issue.in>, see [`bro_core::skill_market`]) with the site's
//!   domain tabs and ranking (or most clicked, or by name), under the repos bro features (Theo's tstack).
//! - Disabled: the skills you took out. Removing a skill disables it: every copy moves out of the agents' reach into
//!   `~/.bro/disabled-skills` ([`bro_core::disabled_skills`]), to be enabled again (each copy goes back where it
//!   was) or deleted for good.
//! - A page per source: bro's own skills with their switches; each watched repo, every skill in it whether it's
//!   installed or not (tick the ones you want and install them everywhere or into a project; pull it now, its page,
//!   stop watching it); the skills installed from Skill Issue, yours, Codex's own.
//! - A page per sidebar project: its own skills (`.claude/skills`, `.codex/skills`, `.agents/skills`), then the ones
//!   every project gets: what an agent working there has.
//!
//! "Add repo" watches a repo ([`bro_core::skill_repos`]): bro clones it and lists its skills on its page, installing
//! none until you pick them. Installing puts a skill everywhere (every agent home) or into one project, and bro keeps
//! it in step with its repo; installing from Discover watches only that skill of its repo. Every skill has a menu (m,
//! or a right-click) with what can be done with it. The installed skills are looked at on a background thread
//! ([`bro_core::skill_inventory`]), again whenever something changes them; the catalogue is fetched on one (cached for
//! half a day). Both wake the pane.

pub mod gui;

use super::settings::{ago, spinner};
use crate::alerts::Kind;
use crate::pane::{Cx, Event, Pane};
use crate::services::skills::{Bundled, and_list, plural};
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::disabled_skills::Disabled;
use bro_core::skill_inventory::{Agent, Inventory, Origin, Place, Skill, one_line};
use bro_core::skill_market::{self, Catalog, DOMAINS, FEATURED, Featured, MarketSkill, Sort};
use bro_core::skill_repos::{self, RepoSkill, Target};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use parking_lot::Mutex;
use ratatui::{Frame, layout::Rect, style::{Modifier, Style}, text::Span};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the view shows: a page of the list down the left.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Page {
    /// every installed skill, grouped by where it came from
    #[default]
    Installed,
    /// Skill Issue's catalogue, and the repos bro features
    Discover,
    /// the skills that are disabled
    Disabled,
    /// the skills bro brings, with their switches
    Bro,
    /// a watched repo's (watched whole: its spec), every skill in it
    Repo(String),
    /// the single skills picked out of repos (what Skill Issue installs)
    Picks,
    /// yours
    Yours,
    /// Codex's own
    Codex,
    /// a sidebar project's (its root)
    Project(PathBuf),
}

/// A list's search, selection and scroll.
#[derive(Clone, Debug, Default)]
pub(crate) struct Browse {
    pub(crate) query: String,
    /// keys go to the search field
    pub(crate) typing: bool,
    pub(crate) sel: usize,
    /// lines (terminal UI) or pixels (desktop app)
    pub(crate) scroll: usize,
    /// the selection the desktop app last scrolled to (it follows the selection only when that moves)
    pub(crate) shown_sel: Option<usize>,
}

impl Browse {
    pub(crate) fn reset(&mut self) {
        (self.sel, self.scroll, self.shown_sel) = (0, 0, None);
    }

    /// A key while the search field is being typed in. False: not one it takes.
    fn type_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.typing = false;
                self.query.clear();
                self.reset();
            }
            KeyCode::Enter | KeyCode::Down => self.typing = false,
            KeyCode::Backspace => {
                self.query.pop();
                self.reset();
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.reset();
            }
            KeyCode::Char(c) if !ctrl && self.query.chars().count() < 60 => {
                self.query.push(c);
                self.reset();
            }
            _ => return false,
        }
        true
    }

    fn paste(&mut self, text: &str) {
        self.typing = true;
        self.query.push_str(text);
        while self.query.chars().count() > 60 {
            self.query.pop();
        }
        self.reset();
    }
}

/// A line of an installed page.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Entry {
    /// a group's heading
    Head(String),
    /// a skill of the inventory (its index)
    Skill(usize),
    /// one of bro's own, installed or not, with its switch
    Bro(Bundled),
    /// a quiet line under a heading (a project with no skills of its own)
    Note(String),
    /// a skill the watched repo holds, installed or not (its index in the repo's list)
    Repo(usize),
    /// a disabled skill (its index in the inventory's disabled ones)
    Disabled(usize),
}

impl Entry {
    /// A selection can land on it.
    pub(crate) fn selectable(&self) -> bool {
        matches!(self, Entry::Skill(_) | Entry::Bro(_) | Entry::Repo(_) | Entry::Disabled(_))
    }
}

/// The list down the left.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NavItem {
    /// a heading over the pages under it
    Head(&'static str),
    Page(Page, String, NavNote),
}

impl NavItem {
    fn page(self) -> Option<Page> {
        match self {
            NavItem::Page(p, ..) => Some(p),
            NavItem::Head(_) => None,
        }
    }
}

/// What a page in the list says at its right.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NavNote {
    Nothing,
    Count(usize),
    /// installed of what it holds (a watched repo: "3/9")
    Of(usize, usize),
    /// being looked at or pulled
    Busy,
    /// its last pull failed
    Failed,
}

impl NavNote {
    pub(crate) fn text(&self) -> String {
        match self {
            NavNote::Count(n) => n.to_string(),
            NavNote::Of(a, b) => format!("{a}/{b}"),
            _ => String::new(),
        }
    }
}

/// Where a catalogue skill stands in this bro.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum State {
    /// bro can't tell where it lives
    NoSource,
    /// installed; `via`: the whole repo it came with, when it came that way
    Installed { via: Option<String> },
    /// its install (or pull) is under way
    Installing(Instant),
    /// its last pull failed
    Failed(String),
    /// installed once, now disabled
    Disabled,
    /// a skill of yours by that name is in ~/.claude/skills
    Yours,
    Available,
}

/// What the add-a-repo dialog makes of what's typed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AddCheck {
    Empty,
    NotARepo,
    /// watched already, as settings spell it
    Watched(String),
    /// ready to watch
    Ready(String),
}

/// Where a skill of a watched repo stands, from one look at the inventory.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RepoStatus {
    /// its copy installed everywhere (an index into the inventory)
    pub(crate) everywhere: Option<usize>,
    /// the projects it's installed in (indexes into the inventory)
    pub(crate) projects: Vec<usize>,
    /// it's disabled (an index into the inventory's disabled ones)
    pub(crate) disabled: Option<usize>,
    /// a skill of that name that isn't this repo's holds its place in the agent homes (an index into the inventory)
    pub(crate) taken: Option<usize>,
}

impl RepoStatus {
    pub(crate) fn installed(&self) -> bool {
        self.everywhere.is_some() || !self.projects.is_empty()
    }

    /// It can be ticked to install: not installed, nor disabled (that one is enabled instead).
    pub(crate) fn tickable(&self) -> bool {
        !self.installed() && self.disabled.is_none()
    }
}

/// What a menu item does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MenuAct {
    /// a SKILL.md, in bro's preview
    Open(PathBuf),
    /// a folder, in the file manager
    Reveal(PathBuf),
    /// a path onto the clipboard
    Copy(PathBuf),
    /// a page on the web
    Web(String),
    Install { spec: String, names: Vec<String>, target: Target },
    Disable(Box<Skill>),
    Enable(Box<Disabled>),
    /// asks first, unless it was asked already (`true`)
    Delete(Box<Disabled>, bool),
    Pull(String),
    StopWatching { spec: String, disable: bool },
    Switch(Bundled),
    /// tick a skill of the repo shown (or untick it)
    Tick(String),
}

/// A menu item's icon (the desktop app draws it; the terminal UI has none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuIcon {
    Open,
    Folder,
    Copy,
    Web,
    GitHub,
    Everywhere,
    Project,
    Disable,
    Enable,
    Delete,
    Pull,
    Unwatch,
    Switch,
    Tick,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MenuItem {
    pub(crate) label: String,
    pub(crate) icon: Option<MenuIcon>,
    pub(crate) key: Option<&'static str>,
    /// None: a heading over the items under it (an empty label: a separator)
    pub(crate) act: Option<MenuAct>,
    pub(crate) danger: bool,
}

impl MenuItem {
    fn new(label: impl Into<String>, icon: MenuIcon, key: Option<&'static str>, act: MenuAct) -> MenuItem {
        MenuItem { label: label.into(), icon: Some(icon), key, act: Some(act), danger: false }
    }

    fn head(label: impl Into<String>) -> MenuItem {
        MenuItem { label: label.into(), icon: None, key: None, act: None, danger: false }
    }

    fn danger(self) -> MenuItem {
        MenuItem { danger: true, ..self }
    }
}

/// A skill's menu (m, a right-click, a row's ⋯), or the places an install can go.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Menu {
    /// what it's about, over the items ("council", "Install 3 skills into")
    pub(crate) title: String,
    pub(crate) items: Vec<MenuItem>,
    /// the item the keys are on
    pub(crate) sel: usize,
    /// where it opened: the desktop app's pixels, the terminal UI's cell; None: by the selection
    pub(crate) at: Option<(f32, f32)>,
    /// the item that asked to be pressed again (deleting for good)
    pub(crate) confirm: Option<usize>,
}

impl Menu {
    fn new(title: impl Into<String>, items: Vec<MenuItem>, at: Option<(f32, f32)>) -> Menu {
        let sel = items.iter().position(|i| i.act.is_some()).unwrap_or(0);
        Menu { title: title.into(), items, sel, at, confirm: None }
    }

    /// The next item that does something, `d` away (round past either end).
    pub(crate) fn step(&mut self, d: isize) {
        let n = self.items.len() as isize;
        if n == 0 {
            return;
        }
        let mut at = self.sel as isize;
        for _ in 0..n {
            at = (at + d).rem_euclid(n);
            if self.items[at as usize].act.is_some() {
                break;
            }
        }
        self.sel = at as usize;
        self.confirm = None;
    }

    /// An item's label as it reads now (the one asking to be pressed again says so).
    pub(crate) fn label(&self, i: usize) -> String {
        match (&self.items[i].act, self.confirm) {
            (Some(MenuAct::Delete(_, false)), Some(c)) if c == i => "Delete forever? Again to confirm".into(),
            _ => self.items[i].label.clone(),
        }
    }
}

#[derive(Default)]
pub struct SkillsView {
    pub(crate) page: Page,
    /// the installed pages' search and selection (one, kept from page to page)
    pub(crate) installed: Browse,
    /// Discover: Skill Issue's catalogue, as fetched
    pub(crate) catalog: Option<Catalog>,
    pub(crate) error: Option<String>,
    pub(crate) fetching: bool,
    /// when the view first drew (what its spinners turn from)
    opened: Option<Instant>,
    /// Discover's list
    pub(crate) market: Browse,
    /// the domain tab: None for all, else an index into [`DOMAINS`]
    pub(crate) domain: Option<usize>,
    pub(crate) sort: Sort,
    /// only what's installed
    pub(crate) mine: bool,
    /// a repo is being pulled, a skill installed or changed, or the skills looked at: keep spinners moving
    pub(crate) busy: bool,
    /// the add-a-repo dialog is open: what's typed in it
    pub(crate) adding: Option<String>,
    /// a repo's page: the skills ticked to install together, by name
    pub(crate) ticked: Vec<String>,
    /// the menu that's open
    pub(crate) menu: Option<Menu>,
    /// where the desktop app drew the selected row last (a menu opened with keys opens there)
    pub(crate) sel_at: Option<(f32, f32)>,
    inbox: Arc<Mutex<Option<Result<Catalog, String>>>>,
    /// (screen row, selectable index) of the list, (screen row, page) of the pages, from the last render, for clicks
    hits: Vec<(u16, usize)>,
    nav_hits: Vec<(u16, Page)>,
    /// the screen row of the list's "add repo", and the column the list ends at
    add_hit: Option<u16>,
    nav_end: u16,
    /// the open menu's items as last drawn (terminal UI), and its box
    menu_hits: Vec<(u16, usize)>,
    menu_box: Option<Rect>,
}

impl SkillsView {
    pub fn new() -> SkillsView {
        SkillsView::default()
    }

    /// The first draw looks at the installed skills, fetches the catalogue and reads what's watched.
    pub(crate) fn start(&mut self, cx: &Cx) {
        if self.opened.is_none() {
            self.opened = Some(Instant::now());
            self.fetch(false, cx);
            cx.svc.scan_skills();
            if !cx.svc.state().skills.loaded {
                cx.svc.load_skill_repos();
            }
        }
    }

    /// What a draw does besides drawing: the catalogue when it lands, whether anything is busy, the page still
    /// being there (a repo dropped, a project closed: back to Installed). The list down the left, as it stands.
    pub(crate) fn prepare(&mut self, cx: &Cx) -> Vec<NavItem> {
        self.start(cx);
        self.take_inbox();
        self.busy = {
            let st = cx.svc.state();
            st.skills.busy() || st.skills.scanning
        };
        let nav = self.nav(cx);
        if !nav.iter().any(|n| matches!(n, NavItem::Page(p, ..) if *p == self.page)) {
            self.go(Page::Installed);
        }
        nav
    }

    pub(crate) fn go(&mut self, page: Page) {
        if self.page != page {
            self.page = page;
            self.installed.reset();
            self.installed.typing = false;
            self.market.typing = false;
            self.ticked.clear();
            self.menu = None;
        }
    }

    /// Tab: the next page in the list (or the one before).
    fn step_page(&mut self, forward: bool, cx: &Cx) {
        let pages: Vec<Page> = self.nav(cx).into_iter().filter_map(NavItem::page).collect();
        let at = pages.iter().position(|p| *p == self.page).unwrap_or(0) as isize;
        let next = (at + if forward { 1 } else { -1 }).rem_euclid(pages.len().max(1) as isize) as usize;
        if let Some(p) = pages.get(next) {
            self.go(p.clone());
        }
    }

    /// The list down the left: Installed, Discover and Disabled, the sources, the sidebar's projects.
    pub(crate) fn nav(&self, cx: &Cx) -> Vec<NavItem> {
        let settings = cx.svc.settings();
        let (inv, repos, scanning) = {
            let st = cx.svc.state();
            (st.skills.inventory.clone(), st.skills.repos.clone(), st.skills.scanning)
        };
        let count = |keep: &dyn Fn(&Skill) -> bool| inv.as_ref().map_or(0, |i| i.skills.iter().filter(|s| keep(s)).count());
        let global = |s: &Skill| s.project.is_none();
        let installed = match &inv {
            None if scanning => NavNote::Busy,
            None => NavNote::Nothing,
            Some(i) => NavNote::Count(i.everywhere().count()),
        };
        let discover = match (&self.catalog, self.fetching) {
            (Some(c), _) => NavNote::Count(c.skills.len()),
            (None, true) => NavNote::Busy,
            _ => NavNote::Nothing,
        };
        let disabled = inv.as_ref().map_or(0, |i| i.disabled.len());
        let mut out = vec![
            NavItem::Page(Page::Installed, "Installed".into(), installed),
            NavItem::Page(Page::Discover, "Discover".into(), discover),
            NavItem::Page(Page::Disabled, "Disabled".into(), if disabled > 0 { NavNote::Count(disabled) } else { NavNote::Nothing }),
            NavItem::Head("Sources"),
            NavItem::Page(Page::Bro, "bro".into(), NavNote::Count(count(&|s| global(s) && s.origin == Origin::Bro))),
        ];
        let state = |specs: &[&String]| {
            let rs: Vec<_> = specs.iter().filter_map(|s| repos.get(*s)).collect();
            if rs.iter().any(|r| r.syncing.is_some()) {
                Some(NavNote::Busy)
            } else if rs.iter().any(|r| r.error.is_some()) {
                Some(NavNote::Failed)
            } else {
                None
            }
        };
        for spec in settings.skills.repos.iter().filter(|r| skill_repos::pick(r).is_none()) {
            // how many of its skills are installed, of all it holds
            let names = repos.get(spec).and_then(|r| r.skills.as_ref()).map(|s| &s.skills);
            let note = match (names, &inv) {
                (Some(names), Some(inv)) => NavNote::Of(names.iter().filter(|n| repo_status(spec, n, inv).installed()).count(), names.len()),
                (Some(names), None) => NavNote::Count(names.len()),
                (None, _) => NavNote::Count(count(&|s| global(s) && matches!(&s.origin, Origin::Repo(r) if skill_repos::same(r, spec)))),
            };
            out.push(NavItem::Page(Page::Repo(spec.clone()), skill_repos::label(spec), state(&[spec]).unwrap_or(note)));
        }
        let picks: Vec<&String> = settings.skills.repos.iter().filter(|r| skill_repos::pick(r).is_some()).collect();
        if !picks.is_empty() {
            out.push(NavItem::Page(Page::Picks, "Skill Issue".into(), state(&picks).unwrap_or(NavNote::Count(count(&|s| global(s) && is_pick(s))))));
        }
        let yours = count(&|s| global(s) && s.origin == Origin::Yours);
        if yours > 0 {
            out.push(NavItem::Page(Page::Yours, "Yours".into(), NavNote::Count(yours)));
        }
        let codex = count(&|s| s.origin == Origin::Codex);
        if codex > 0 {
            out.push(NavItem::Page(Page::Codex, "Codex".into(), NavNote::Count(codex)));
        }
        let projects = cx.svc.projects();
        if !projects.is_empty() {
            out.push(NavItem::Head("Projects"));
            for p in projects {
                let n = inv.as_ref().map_or(0, |i| i.of_project(&p.root).count());
                out.push(NavItem::Page(Page::Project(p.root), p.name, if n > 0 { NavNote::Count(n) } else { NavNote::Nothing }));
            }
        }
        out
    }

    /// The page's title and the line under it.
    pub(crate) fn heading(&self, cx: &Cx) -> (String, String) {
        let inv = cx.svc.state().skills.inventory.clone();
        match &self.page {
            Page::Installed => {
                let sub = match &inv {
                    None => "Looking at your skills…".to_string(),
                    Some(i) => {
                        let all = i.everywhere().count();
                        let more = i.skills.len() - all;
                        let mut s = format!("{} your agents can use in every project", plural(all, "skill"));
                        if more > 0 {
                            s.push_str(&format!(" · {more} more in your projects"));
                        }
                        s + " · new Claude and Codex sessions pick up changes"
                    }
                };
                ("Installed".into(), sub)
            }
            Page::Discover => {
                let sub = match self.catalog.as_ref().map_or(0, |c| c.skills.len()) {
                    0 => "Agent skills from Skill Issue, ranked by what people actually use".to_string(),
                    n => format!("{n} agent skills from Skill Issue, ranked by what people actually use · install one everywhere or into a project, and bro keeps it up to date"),
                };
                ("Discover".into(), sub)
            }
            Page::Disabled => {
                let n = inv.as_ref().map_or(0, |i| i.disabled.len());
                let what = if n == 0 { "Nothing disabled".to_string() } else { format!("{} out of your agents' reach", plural(n, "skill")) };
                ("Disabled".into(), format!("{what} · removing a skill disables it: every copy moves to ~/.bro/disabled-skills until you enable it again or delete it for good"))
            }
            Page::Bro => ("bro".into(), "The skills bro brings for its own features. Switch one off and bro takes it back out of every agent home.".into()),
            Page::Repo(spec) => (skill_repos::label(spec), self.repo_line(spec, cx)),
            Page::Picks => ("Skill Issue".into(), "Single skills installed from Skill Issue, each picked out of its repo and kept in step with it.".into()),
            Page::Yours => ("Yours".into(), "Skills you added yourself, by hand or with another tool. bro never changes them: disabling one moves it to Disabled, every copy, until you enable it again.".into()),
            Page::Codex => ("Codex".into(), "Skills Codex ships with and keeps up to date.".into()),
            Page::Project(root) => {
                let own = inv.as_ref().map_or(0, |i| i.of_project(root).count());
                let all = inv.as_ref().map_or(0, |i| i.everywhere().count());
                let mine = if own == 0 { "no skills of its own".to_string() } else { format!("{} of its own", plural(own, "skill")) };
                (project_name(root, cx), format!("{} · {mine} · agents working here also get the {all} installed everywhere", crate::folder::shorten_home(root)))
            }
        }
    }

    /// A watched repo's line: its skills, how many are installed, its commit, its last pull and how it stays in step.
    fn repo_line(&self, spec: &str, cx: &Cx) -> String {
        let st = cx.svc.state();
        let r = st.skills.repos.get(spec).cloned().unwrap_or_default();
        let mut parts = vec![match (&r.syncing, &r.skills) {
            (Some(_), None) => "pulling…".to_string(),
            (_, Some(s)) => plural(s.skills.len(), "skill"),
            (None, None) => "not pulled yet".to_string(),
        }];
        if let (Some(s), Some(inv)) = (&r.skills, st.skills.inventory.as_deref()) {
            let statuses: Vec<RepoStatus> = s.skills.iter().map(|n| repo_status(spec, n, inv)).collect();
            let installed = statuses.iter().filter(|s| s.installed()).count();
            let disabled = statuses.iter().filter(|s| !s.installed() && s.disabled.is_some()).count();
            parts.push(format!("{installed} installed"));
            if disabled > 0 {
                parts.push(format!("{disabled} disabled"));
            }
        }
        parts.extend(r.skills.as_ref().and_then(|s| s.commit.clone()).map(|c| format!("at {c}")));
        parts.extend(r.synced.map(|at| format!("pulled {}", ago(at))));
        parts.push(if st.settings.skills.auto_sync { "the ones you install stay in step with it (pulled every 6 hours)".into() } else { "auto-sync is off (settings)".into() });
        parts.join(" · ")
    }

    // ---------------------------------------------------------------------------------------- installed pages

    /// The skills a watched repo holds, as its clone was last read.
    pub(crate) fn repo_skills(&self, spec: &str, cx: &Cx) -> Vec<RepoSkill> {
        cx.svc.state().skills.repos.get(spec).and_then(|r| r.skills.as_ref()).map(|s| s.found.clone()).unwrap_or_default()
    }

    /// The installed page's lines, the search applied: group headings and skills, bro's own with their switches, a
    /// repo's skills, the disabled ones.
    pub(crate) fn installed_list(&self, cx: &Cx) -> Vec<Entry> {
        let q = self.installed.query.trim().to_lowercase();
        if self.page == Page::Bro {
            return Bundled::here().into_iter().filter(|b| q.is_empty() || b.title().to_lowercase().contains(&q) || b.about().to_lowercase().contains(&q) || b.names().iter().any(|n| n.contains(&q))).map(Entry::Bro).collect();
        }
        let Some(inv) = cx.svc.state().skills.inventory.clone() else { return vec![] };
        let matches = |name: &str, about: Option<&str>| q.is_empty() || name.to_lowercase().contains(&q) || about.is_some_and(|d| d.to_lowercase().contains(&q));
        let hit = |s: &Skill| matches(&s.name, s.description.as_deref());
        let rows = |keep: &dyn Fn(&Skill) -> bool| -> Vec<Entry> { inv.skills.iter().enumerate().filter(|(_, s)| keep(s) && hit(s)).map(|(i, _)| Entry::Skill(i)).collect() };
        let group = |out: &mut Vec<Entry>, head: String, rows: Vec<Entry>| {
            if !rows.is_empty() {
                out.push(Entry::Head(head));
                out.extend(rows);
            }
        };
        let global = |s: &Skill| s.project.is_none();
        let from = |s: &Skill, spec: &str| matches!(&s.origin, Origin::Repo(r) if skill_repos::same(r, spec));
        let mut out = vec![];
        match &self.page {
            Page::Installed => {
                group(&mut out, "Bundled with bro".into(), rows(&|s| global(s) && s.origin == Origin::Bro));
                let mut specs: Vec<&str> = vec![];
                for s in inv.everywhere() {
                    if let Origin::Repo(r) = &s.origin
                        && skill_repos::pick(r).is_none()
                        && !specs.contains(&r.as_str())
                    {
                        specs.push(r);
                    }
                }
                for spec in specs {
                    group(&mut out, skill_repos::label(spec), rows(&|s| global(s) && from(s, spec)));
                }
                group(&mut out, "From Skill Issue".into(), rows(&|s| global(s) && is_pick(s)));
                group(&mut out, "Built into Codex".into(), rows(&|s| global(s) && s.origin == Origin::Codex));
                group(&mut out, "Yours".into(), rows(&|s| global(s) && s.origin == Origin::Yours));
                let mut roots: Vec<&PathBuf> = vec![];
                for s in &inv.skills {
                    if let Some(p) = &s.project
                        && !roots.contains(&p)
                    {
                        roots.push(p);
                    }
                }
                for root in roots {
                    group(&mut out, format!("In {}", project_name(root, cx)), rows(&|s| s.project.as_ref() == Some(root)));
                }
            }
            Page::Disabled => out = inv.disabled.iter().enumerate().filter(|(_, d)| matches(&d.name, d.description.as_deref())).map(|(i, _)| Entry::Disabled(i)).collect(),
            Page::Repo(spec) => {
                // every skill in it: what's installed, what's waiting to be picked, what's disabled
                let found = self.repo_skills(spec, cx);
                let (mut installed, mut available, mut disabled) = (vec![], vec![], vec![]);
                for (k, f) in found.iter().enumerate().filter(|(_, f)| matches(&f.name, f.description.as_deref())) {
                    let st = repo_status(spec, &f.name, &inv);
                    if st.installed() {
                        installed.push(Entry::Repo(k));
                    } else if st.disabled.is_some() {
                        disabled.push(Entry::Repo(k));
                    } else {
                        available.push(Entry::Repo(k));
                    }
                }
                group(&mut out, "Installed".into(), installed);
                group(&mut out, "Not installed".into(), available);
                group(&mut out, "Disabled".into(), disabled);
            }
            Page::Picks => out = rows(&|s| global(s) && is_pick(s)),
            Page::Yours => out = rows(&|s| global(s) && s.origin == Origin::Yours),
            Page::Codex => out = rows(&|s| s.origin == Origin::Codex),
            Page::Project(root) => {
                let own = rows(&|s| s.project.as_deref().is_some_and(|p| bro_core::util::same_path(p, root)));
                out.push(Entry::Head("In this project".into()));
                if own.is_empty() {
                    let none = if q.is_empty() { "none of its own: agents working here get the skills installed everywhere" } else { "none of its own match" };
                    out.push(Entry::Note(none.into()));
                }
                out.extend(own);
                group(&mut out, "Everywhere".into(), rows(&global));
            }
            Page::Bro | Page::Discover => {}
        }
        out
    }

    /// The lines of `list` a selection can land on, as indexes into it.
    pub(crate) fn selectable(list: &[Entry]) -> Vec<usize> {
        list.iter().enumerate().filter(|(_, e)| e.selectable()).map(|(i, _)| i).collect()
    }

    /// The installed page's selected line.
    pub(crate) fn chosen(&self, cx: &Cx) -> Option<Entry> {
        let list = self.installed_list(cx);
        let at = Self::selectable(&list);
        at.get(self.installed.sel.min(at.len().saturating_sub(1))).map(|&i| list[i].clone())
    }

    /// A skill of the inventory.
    pub(crate) fn installed_skill(&self, i: usize, cx: &Cx) -> Option<Skill> {
        cx.svc.state().skills.inventory.as_ref()?.skills.get(i).cloned()
    }

    /// A disabled skill of the inventory.
    pub(crate) fn disabled_skill(&self, i: usize, cx: &Cx) -> Option<Disabled> {
        cx.svc.state().skills.inventory.as_ref()?.disabled.get(i).cloned()
    }

    /// A skill of the repo shown, and where it stands.
    pub(crate) fn repo_skill(&self, k: usize, cx: &Cx) -> Option<(String, RepoSkill, RepoStatus)> {
        let Page::Repo(spec) = &self.page else { return None };
        let f = self.repo_skills(spec, cx).into_iter().nth(k)?;
        let inv = cx.svc.state().skills.inventory.clone()?;
        let st = repo_status(spec, &f.name, &inv);
        Some((spec.clone(), f, st))
    }

    /// The installed copy of a skill of the repo shown: everywhere's, else a project's.
    fn repo_copy(&self, st: &RepoStatus, cx: &Cx) -> Option<Skill> {
        st.everywhere.or_else(|| st.projects.first().copied()).and_then(|i| self.installed_skill(i, cx))
    }

    /// Enter on an installed page: bro's own switches; a skill's SKILL.md opens in bro's preview.
    pub(crate) fn activate(&mut self, cx: &mut Cx) {
        let md = match self.chosen(cx) {
            Some(Entry::Bro(b)) => return self.switch(b, cx),
            Some(Entry::Skill(i)) => self.installed_skill(i, cx).map(|s| s.dir().join("SKILL.md")),
            Some(Entry::Repo(k)) => self.repo_skill(k, cx).map(|(_, f, st)| self.repo_copy(&st, cx).map_or_else(|| f.dir.join("SKILL.md"), |s| s.dir().join("SKILL.md"))),
            Some(Entry::Disabled(i)) => self.disabled_skill(i, cx).map(|d| d.skill_md()),
            _ => None,
        };
        if let Some(md) = md {
            cx.act(crate::pane::Action::Preview(md));
        }
    }

    /// One of bro's own: on or off (JustImagine's are written again).
    pub(crate) fn switch(&mut self, b: Bundled, cx: &mut Cx) {
        match cx.svc.bundled_on(b) {
            Some(on) => cx.svc.set_bundled(b, !on),
            None if b == Bundled::JustImagine => cx.svc.set_bundled(b, true),
            None => cx.toast(Kind::Info, format!("bro writes the {} skill when it's needed", b.title().to_lowercase())),
        }
    }

    /// The selected line's folder: a skill's, a disabled one's (where it's kept), a repo skill's (its installed copy,
    /// else the clone's); on a project's page with nothing chosen, the project's.
    fn chosen_dir(&self, cx: &Cx) -> Option<PathBuf> {
        match (self.chosen(cx), &self.page) {
            (Some(Entry::Skill(i)), _) => self.installed_skill(i, cx).map(|s| s.dir().to_path_buf()),
            (Some(Entry::Disabled(i)), _) => self.disabled_skill(i, cx).map(|d| d.dir()),
            (Some(Entry::Repo(k)), _) => self.repo_skill(k, cx).map(|(_, f, st)| self.repo_copy(&st, cx).map_or(f.dir, |s| s.dir().to_path_buf())),
            (_, Page::Project(root)) => Some(root.clone()),
            _ => None,
        }
    }

    /// o: the selected skill's folder in the file manager (a project's page: the project's).
    pub(crate) fn reveal(&mut self, cx: &mut Cx) {
        if let Some(dir) = self.chosen_dir(cx) {
            reveal(&dir);
        }
    }

    /// l: where the selected skill comes from on the web (its repo), or the repo page's own repo.
    pub(crate) fn open_web(&mut self, cx: &mut Cx) {
        let url = match (&self.page, self.chosen(cx)) {
            (Page::Repo(spec), Some(Entry::Repo(k))) => self.repo_skill(k, cx).map(|(_, f, _)| skill_web(spec, &f)),
            (Page::Repo(spec), _) => Some(skill_repos::web_url(spec)),
            (_, Some(Entry::Skill(i))) => match self.installed_skill(i, cx).map(|s| s.origin) {
                Some(Origin::Repo(spec)) => Some(skill_repos::web_url(&spec)),
                _ => None,
            },
            (_, Some(Entry::Disabled(i))) => match self.disabled_skill(i, cx).map(|d| d.origin) {
                Some(Origin::Repo(spec)) => Some(skill_repos::web_url(&spec)),
                _ => None,
            },
            _ => None,
        };
        match url {
            Some(u) => open_link(u, cx),
            None => cx.toast(Kind::Info, "it doesn't come from a repo bro watches"),
        }
    }

    /// d: disable the selected skill (every copy moves to Disabled); bro's own are switched off instead. A skill of the
    /// repo shown installed in more than one place offers each.
    pub(crate) fn disable_chosen(&mut self, cx: &mut Cx) {
        match self.chosen(cx) {
            Some(Entry::Bro(b)) if cx.svc.bundled_on(b) == Some(true) => cx.svc.set_bundled(b, false),
            Some(Entry::Bro(b)) => cx.toast(Kind::Info, format!("bro writes the {} skill only when it's needed", b.title().to_lowercase())),
            Some(Entry::Skill(i)) => {
                if let Some(s) = self.installed_skill(i, cx) {
                    disable(s, cx);
                }
            }
            Some(Entry::Repo(k)) => {
                let Some((_, f, st)) = self.repo_skill(k, cx) else { return };
                let copies: Vec<Skill> = st.everywhere.iter().chain(&st.projects).filter_map(|&i| self.installed_skill(i, cx)).collect();
                match copies.len() {
                    0 if st.disabled.is_some() => cx.toast(Kind::Info, format!("{} is disabled already · e enables it", f.name)),
                    0 => cx.toast(Kind::Info, format!("{} isn't installed", f.name)),
                    1 => disable(copies.into_iter().next().unwrap_or_else(|| unreachable!()), cx),
                    _ => self.open_menu(None, cx),
                }
            }
            Some(Entry::Disabled(i)) => {
                if let Some(d) = self.disabled_skill(i, cx) {
                    cx.toast(Kind::Info, format!("{} is disabled already · e enables it, x deletes it for good", d.name));
                }
            }
            _ => {}
        }
    }

    /// e: enable the selected disabled skill (each copy goes back where it was).
    pub(crate) fn enable_chosen(&mut self, cx: &mut Cx) {
        let d = match self.chosen(cx) {
            Some(Entry::Disabled(i)) => self.disabled_skill(i, cx),
            Some(Entry::Repo(k)) => self.repo_skill(k, cx).and_then(|(_, _, st)| st.disabled).and_then(|i| self.disabled_skill(i, cx)),
            _ => None,
        };
        match d {
            Some(d) => cx.svc.enable_skill(d),
            None => cx.toast(Kind::Info, "it isn't disabled"),
        }
    }

    /// x: delete the selected disabled skill for good, once asked.
    pub(crate) fn delete_chosen(&mut self, cx: &mut Cx) {
        let d = match self.chosen(cx) {
            Some(Entry::Disabled(i)) => self.disabled_skill(i, cx),
            Some(Entry::Repo(k)) => self.repo_skill(k, cx).and_then(|(_, _, st)| st.disabled).and_then(|i| self.disabled_skill(i, cx)),
            _ => None,
        };
        match d {
            Some(d) => {
                let title = format!("Delete {} for good?", d.name);
                let note = MenuItem::head(format!("its copy in {} goes", crate::folder::shorten_home(&d.dir())));
                self.menu = Some(Menu::new(title, vec![MenuItem::new("Delete forever", MenuIcon::Delete, None, MenuAct::Delete(Box::new(d), true)).danger(), note], None));
            }
            None => cx.toast(Kind::Info, "only a disabled skill is deleted: d disables one first"),
        }
    }

    /// space: tick the selected skill of the repo shown (or untick it), to install with the others.
    pub(crate) fn tick_chosen(&mut self, cx: &mut Cx) {
        if let Some(Entry::Repo(k)) = self.chosen(cx)
            && let Some((_, f, st)) = self.repo_skill(k, cx)
        {
            if st.tickable() {
                self.tick(&f.name);
            } else if st.disabled.is_some() {
                cx.toast(Kind::Info, format!("{} is disabled · e enables it", f.name));
            } else {
                cx.toast(Kind::Info, format!("{} is installed", f.name));
            }
        }
    }

    pub(crate) fn tick(&mut self, name: &str) {
        match self.ticked.iter().position(|n| n == name) {
            Some(at) => {
                self.ticked.remove(at);
            }
            None => self.ticked.push(name.to_string()),
        }
    }

    /// Tick every skill of the repo shown that isn't installed (or untick them all, when they are).
    pub(crate) fn tick_all(&mut self, cx: &Cx) {
        let Page::Repo(spec) = &self.page else { return };
        let Some(inv) = cx.svc.state().skills.inventory.clone() else { return };
        let all: Vec<String> = self.repo_skills(spec, cx).into_iter().filter(|f| repo_status(spec, &f.name, &inv).tickable()).map(|f| f.name).collect();
        if !all.is_empty() && all.iter().all(|n| self.ticked.contains(n)) {
            self.ticked.clear();
        } else {
            self.ticked = all;
        }
    }

    /// What an install from the repo shown takes: the ticked skills, else the selected one when it isn't installed.
    pub(crate) fn to_install(&self, cx: &Cx) -> Option<(String, Vec<String>)> {
        let Page::Repo(spec) = &self.page else { return None };
        if !self.ticked.is_empty() {
            return Some((spec.clone(), self.ticked.clone()));
        }
        match self.chosen(cx) {
            Some(Entry::Repo(k)) => self.repo_skill(k, cx).filter(|(_, _, st)| st.tickable()).map(|(spec, f, _)| (spec, vec![f.name])),
            _ => None,
        }
    }

    /// i: install the ticked skills of the repo shown (or the selected one) everywhere.
    pub(crate) fn install_here(&mut self, target: Target, cx: &mut Cx) {
        match self.to_install(cx) {
            Some((spec, names)) => {
                self.ticked.clear();
                cx.svc.install_skills(&spec, names, target);
            }
            None => cx.toast(Kind::Info, "tick the skills to install (space), or pick one that isn't installed"),
        }
    }

    /// p: where to install the ticked skills of the repo shown (or the selected one): everywhere, or a project.
    pub(crate) fn choose_target(&mut self, at: Option<(f32, f32)>, cx: &mut Cx) {
        match self.to_install(cx) {
            Some((spec, names)) => {
                let title = match &names[..] {
                    [one] => format!("Install {one}"),
                    _ => format!("Install {} skills", names.len()),
                };
                self.menu = Some(Menu::new(title, install_items(&spec, &names, &[], true, cx), at));
            }
            None => cx.toast(Kind::Info, "tick the skills to install (space), or pick one that isn't installed"),
        }
    }

    /// Stop watching `spec` (`disable`: and disable what was installed from it).
    pub(crate) fn drop_repo(&mut self, spec: &str, disable: bool, cx: &mut Cx) {
        cx.svc.remove_skill_repo(spec, disable);
        self.go(Page::Installed);
    }

    /// w on a repo's page: how to stop watching it, keeping what was installed from it or disabling that too.
    pub(crate) fn stop_menu(&mut self, at: Option<(f32, f32)>, cx: &Cx) {
        let Page::Repo(spec) = &self.page else { return };
        let n = cx.svc.state().skills.inventory.as_ref().map_or(0, |i| i.skills.iter().filter(|s| matches!(&s.origin, Origin::Repo(r) if skill_repos::same(r, spec))).count());
        let mut items = vec![MenuItem::new(if n == 0 { "Stop watching".to_string() } else { format!("Stop watching · keep its {} installed", plural(n, "skill")) }, MenuIcon::Unwatch, None, MenuAct::StopWatching { spec: spec.clone(), disable: false })];
        if n > 0 {
            items.push(MenuItem::new(format!("Stop watching and disable {}", if n == 1 { "it".to_string() } else { format!("all {n}") }), MenuIcon::Disable, None, MenuAct::StopWatching { spec: spec.clone(), disable: true }).danger());
        }
        self.menu = Some(Menu::new(format!("Stop watching {}?", skill_repos::label(spec)), items, at));
    }

    // ---------------------------------------------------------------------------------------- menus

    /// The menu for the selected line (m, a right-click at `at`), or for Discover's selected skill.
    pub(crate) fn open_menu(&mut self, at: Option<(f32, f32)>, cx: &Cx) {
        let menu = if self.page == Page::Discover { self.market_menu(cx) } else { self.chosen(cx).and_then(|e| self.entry_menu(&e, cx)) };
        self.menu = menu.map(|(title, items)| Menu::new(title, items, at));
    }

    /// What can be done with a line of an installed page.
    fn entry_menu(&self, e: &Entry, cx: &Cx) -> Option<(String, Vec<MenuItem>)> {
        let mut items = vec![];
        let file_items = |items: &mut Vec<MenuItem>, dir: &Path| {
            items.push(MenuItem::new("Open SKILL.md", MenuIcon::Open, Some("enter"), MenuAct::Open(dir.join("SKILL.md"))));
            items.push(MenuItem::new("Show folder", MenuIcon::Folder, Some("o"), MenuAct::Reveal(dir.to_path_buf())));
            items.push(MenuItem::new("Copy folder path", MenuIcon::Copy, None, MenuAct::Copy(dir.to_path_buf())));
        };
        let title = match e {
            Entry::Skill(i) => {
                let s = self.installed_skill(*i, cx)?;
                file_items(&mut items, s.dir());
                if let Origin::Repo(spec) = &s.origin {
                    items.push(MenuItem::head(""));
                    if cx.svc.settings().skills.repos.iter().any(|r| skill_repos::same(r, spec)) {
                        items.push(MenuItem::new("Update now", MenuIcon::Pull, None, MenuAct::Pull(spec.clone())));
                    }
                    items.push(web_item(spec, None));
                    // a repo's skill can go into the projects that don't have it yet
                    if s.project.is_none() {
                        let have: Vec<PathBuf> = cx.svc.state().skills.inventory.as_ref().map(|inv| inv.skills.iter().filter(|o| o.name == s.name && o.origin == s.origin).filter_map(|o| o.project.clone()).collect()).unwrap_or_default();
                        items.extend(install_items(spec, std::slice::from_ref(&s.name), &have, false, cx));
                    }
                }
                match &s.origin {
                    Origin::Bro => {
                        if let Some(b) = Bundled::here().into_iter().find(|b| b.names().contains(&s.name.as_str()))
                            && let Some(on) = cx.svc.bundled_on(b)
                        {
                            items.push(MenuItem::head(""));
                            items.push(MenuItem::new(if on { "Turn off" } else { "Turn on" }, MenuIcon::Switch, None, MenuAct::Switch(b)));
                        }
                    }
                    Origin::Codex => {}
                    _ => {
                        items.push(MenuItem::head(""));
                        items.push(MenuItem::new("Disable", MenuIcon::Disable, Some("d"), MenuAct::Disable(Box::new(s.clone()))));
                    }
                }
                s.name
            }
            Entry::Bro(b) => {
                if let Some(on) = cx.svc.bundled_on(*b) {
                    items.push(MenuItem::new(if on { "Turn off" } else { "Turn on" }, MenuIcon::Switch, Some("enter"), MenuAct::Switch(*b)));
                }
                if let Some(s) = cx.svc.state().skills.inventory.as_ref().and_then(|inv| inv.everywhere().find(|s| s.origin == Origin::Bro && b.names().contains(&s.name.as_str())).cloned()) {
                    items.push(MenuItem::new("Open SKILL.md", MenuIcon::Open, None, MenuAct::Open(s.dir().join("SKILL.md"))));
                    items.push(MenuItem::new("Show folder", MenuIcon::Folder, Some("o"), MenuAct::Reveal(s.dir().to_path_buf())));
                }
                b.title().to_string()
            }
            Entry::Repo(k) => {
                let (spec, f, st) = self.repo_skill(*k, cx)?;
                if st.tickable() {
                    items.extend(install_items(&spec, std::slice::from_ref(&f.name), &[], true, cx));
                    items.push(MenuItem::head(""));
                    let ticked = self.ticked.contains(&f.name);
                    items.push(MenuItem::new(if ticked { "Untick" } else { "Tick, to install with others" }, MenuIcon::Tick, Some("space"), MenuAct::Tick(f.name.clone())));
                    items.push(MenuItem::new("Open SKILL.md", MenuIcon::Open, Some("enter"), MenuAct::Open(f.dir.join("SKILL.md"))));
                } else if let Some(d) = st.disabled.filter(|_| !st.installed()).and_then(|i| self.disabled_skill(i, cx)) {
                    items.push(MenuItem::new("Enable", MenuIcon::Enable, Some("e"), MenuAct::Enable(Box::new(d.clone()))));
                    items.push(MenuItem::new("Open SKILL.md", MenuIcon::Open, Some("enter"), MenuAct::Open(d.skill_md())));
                    items.push(MenuItem::new("Delete forever", MenuIcon::Delete, Some("x"), MenuAct::Delete(Box::new(d), false)).danger());
                } else {
                    let copy = self.repo_copy(&st, cx)?;
                    file_items(&mut items, copy.dir());
                    let have: Vec<PathBuf> = st.projects.iter().filter_map(|&i| self.installed_skill(i, cx)).filter_map(|s| s.project).collect();
                    items.extend(install_items(&spec, std::slice::from_ref(&f.name), &have, st.everywhere.is_none(), cx));
                    items.push(MenuItem::head(""));
                    for i in st.everywhere.iter().chain(&st.projects) {
                        if let Some(s) = self.installed_skill(*i, cx) {
                            let label = match &s.project {
                                None if st.projects.is_empty() => "Disable".to_string(),
                                None => "Disable everywhere".to_string(),
                                Some(root) => format!("Disable in {}", project_name(root, cx)),
                            };
                            items.push(MenuItem::new(label, MenuIcon::Disable, (st.projects.len() + st.everywhere.iter().count() == 1).then_some("d"), MenuAct::Disable(Box::new(s))));
                        }
                    }
                }
                items.push(MenuItem::head(""));
                items.push(web_item(&spec, Some(&f)));
                f.name
            }
            Entry::Disabled(i) => {
                let d = self.disabled_skill(*i, cx)?;
                items.push(MenuItem::new("Enable", MenuIcon::Enable, Some("e"), MenuAct::Enable(Box::new(d.clone()))));
                items.push(MenuItem::new("Open SKILL.md", MenuIcon::Open, Some("enter"), MenuAct::Open(d.skill_md())));
                items.push(MenuItem::new("Show folder", MenuIcon::Folder, Some("o"), MenuAct::Reveal(d.dir())));
                items.push(MenuItem::new("Copy folder path", MenuIcon::Copy, None, MenuAct::Copy(d.dir())));
                if let Origin::Repo(spec) = &d.origin {
                    items.push(web_item(spec, None));
                }
                items.push(MenuItem::head(""));
                items.push(MenuItem::new("Delete forever", MenuIcon::Delete, Some("x"), MenuAct::Delete(Box::new(d.clone()), false)).danger());
                d.name
            }
            Entry::Head(_) | Entry::Note(_) => return None,
        };
        Some((title, items))
    }

    /// What can be done with Discover's selected skill.
    fn market_menu(&self, cx: &Cx) -> Option<(String, Vec<MenuItem>)> {
        let s = self.selected(cx).and_then(|i| self.skill(i)).cloned()?;
        let mut items = vec![];
        let state = self.state(&s, cx);
        match (&state, skill_market::source(&s)) {
            (State::Available | State::Failed(_), Some(spec)) => items.extend(install_items(&spec, &[], &[], true, cx)),
            (State::Installed { .. }, Some(_)) => {
                if let Some(k) = market_installed(&s, cx) {
                    items.push(MenuItem::new("Open SKILL.md", MenuIcon::Open, None, MenuAct::Open(k.dir().join("SKILL.md"))));
                    items.push(MenuItem::new("Show folder", MenuIcon::Folder, None, MenuAct::Reveal(k.dir().to_path_buf())));
                    items.push(MenuItem::new("Disable", MenuIcon::Disable, Some("d"), MenuAct::Disable(Box::new(k))));
                }
            }
            (State::Disabled, _) => {
                if let Some(d) = market_disabled(&s, cx) {
                    items.push(MenuItem::new("Enable", MenuIcon::Enable, Some("e"), MenuAct::Enable(Box::new(d))));
                }
            }
            _ => {}
        }
        if !items.is_empty() {
            items.push(MenuItem::head(""));
        }
        items.push(MenuItem::new("Its page on Skill Issue", MenuIcon::Web, Some("o"), MenuAct::Web(s.page_url())));
        if !s.link.is_empty() {
            let github = s.link.contains("github.com");
            items.push(MenuItem::new(if github { "Its source on GitHub" } else { "Its source" }, if github { MenuIcon::GitHub } else { MenuIcon::Web }, Some("l"), MenuAct::Web(s.link.clone())));
        }
        Some((s.name, items))
    }

    /// A key while a menu is open. It always takes it.
    fn menu_key(&mut self, key: KeyEvent, cx: &mut Cx) {
        let Some(m) = self.menu.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.menu = None,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => m.step(1),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => m.step(-1),
            KeyCode::Home => m.sel = m.items.iter().position(|i| i.act.is_some()).unwrap_or(0),
            KeyCode::End => m.sel = m.items.iter().rposition(|i| i.act.is_some()).unwrap_or(0),
            KeyCode::Enter => {
                let sel = m.sel;
                self.run_menu(sel, cx);
            }
            // an item's own key runs it
            KeyCode::Char(c) => {
                let k = if c == ' ' { "space".to_string() } else { c.to_string() };
                if let Some(i) = m.items.iter().position(|i| i.act.is_some() && i.key == Some(k.as_str())) {
                    self.run_menu(i, cx);
                }
            }
            _ => {}
        }
    }

    /// Item `i` of the open menu was chosen: it runs (deleting for good asks for a second press) and the menu closes.
    pub(crate) fn run_menu(&mut self, i: usize, cx: &mut Cx) {
        let Some(m) = self.menu.as_mut() else { return };
        let Some(act) = m.items.get(i).and_then(|item| item.act.clone()) else { return };
        if matches!(act, MenuAct::Delete(_, false)) && m.confirm != Some(i) {
            (m.sel, m.confirm) = (i, Some(i));
            return;
        }
        self.menu = None;
        self.run_act(act, cx);
    }

    fn run_act(&mut self, act: MenuAct, cx: &mut Cx) {
        match act {
            MenuAct::Open(md) => cx.act(crate::pane::Action::Preview(md)),
            MenuAct::Reveal(dir) => reveal(&dir),
            MenuAct::Copy(path) => {
                let text = path.display().to_string();
                if !cfg!(test) {
                    crate::clip::copy(&text);
                }
                cx.toast(Kind::Info, format!("copied {}", crate::folder::shorten_home(&path)));
            }
            MenuAct::Web(url) => open_link(url, cx),
            MenuAct::Install { spec, names, target } => {
                self.ticked.retain(|n| !names.contains(n));
                cx.svc.install_skills(&spec, names, target);
            }
            MenuAct::Disable(s) => disable(*s, cx),
            MenuAct::Enable(d) => cx.svc.enable_skill(*d),
            MenuAct::Delete(d, _) => cx.svc.delete_disabled(*d),
            MenuAct::Pull(spec) => cx.svc.sync_skill_repo(&spec, true),
            MenuAct::StopWatching { spec, disable } => self.drop_repo(&spec, disable, cx),
            MenuAct::Switch(b) => self.switch(b, cx),
            MenuAct::Tick(name) => self.tick(&name),
        }
    }

    // ---------------------------------------------------------------------------------------- adding a repo

    /// What the dialog makes of what's typed.
    pub(crate) fn add_check(&self, cx: &Cx) -> AddCheck {
        let text = self.adding.as_deref().unwrap_or("").trim();
        if text.is_empty() {
            return AddCheck::Empty;
        }
        let Some(spec) = skill_repos::normalize(text) else { return AddCheck::NotARepo };
        match cx.svc.settings().skills.repos.into_iter().find(|r| skill_repos::same(r, &spec)) {
            Some(w) => AddCheck::Watched(w),
            None => AddCheck::Ready(spec),
        }
    }

    /// The repo the dialog suggests: the first featured one not watched yet.
    pub(crate) fn suggestion(&self, cx: &Cx) -> Option<&'static Featured> {
        let watched = cx.svc.settings().skills.repos;
        FEATURED.iter().find(|f| !watched.iter().any(|w| skill_repos::same(w, f.repo)))
    }

    /// Enter in the dialog: watch what's typed and show its page (a repo watched already: just show it).
    pub(crate) fn add_repo(&mut self, cx: &mut Cx) {
        match self.add_check(cx) {
            AddCheck::Empty => {}
            AddCheck::NotARepo => cx.toast(Kind::Error, format!("'{}' isn't a repo: paste owner/repo, a GitHub link or a git URL", self.adding.as_deref().unwrap_or("").trim())),
            AddCheck::Watched(spec) => {
                self.adding = None;
                self.show_spec(spec);
            }
            AddCheck::Ready(_) => {
                let typed = self.adding.clone().unwrap_or_default();
                self.watch(&typed, cx);
            }
        }
    }

    /// Watch `input` (what was typed, a featured repo) and show its page, where its skills wait to be picked.
    pub(crate) fn watch(&mut self, input: &str, cx: &mut Cx) {
        match cx.svc.add_skill_repo(input) {
            Ok(spec) => {
                self.adding = None;
                cx.toast(Kind::Info, format!("watching {} · pulling it, then pick the skills you want", skill_repos::label(&spec)));
                self.show_spec(spec);
            }
            Err(e) => cx.toast(Kind::Error, e),
        }
    }

    /// A featured repo: watch it, or show it when it's watched already.
    pub(crate) fn feature(&mut self, f: &Featured, cx: &mut Cx) {
        match cx.svc.settings().skills.repos.into_iter().find(|w| skill_repos::same(w, f.repo)) {
            Some(spec) => self.show_spec(spec),
            None => self.watch(f.repo, cx),
        }
    }

    fn show_spec(&mut self, spec: String) {
        self.go(if skill_repos::pick(&spec).is_some() { Page::Picks } else { Page::Repo(spec) });
    }

    // ---------------------------------------------------------------------------------------- Discover

    /// The catalogue (from its cache while it's fresh, unless `force`).
    pub(crate) fn fetch(&mut self, force: bool, cx: &Cx) {
        if self.fetching {
            return;
        }
        // the demo shows a slice of the catalogue that ships with bro
        if cx.svc.is_demo() {
            if self.catalog.is_none() {
                self.catalog = Some(crate::services::demo::skill_catalog());
            }
            return;
        }
        self.fetching = true;
        let inbox = self.inbox.clone();
        let waker = cx.waker();
        std::thread::spawn(move || {
            let r = skill_market::load(force).map_err(|e| format!("{e:#}"));
            *inbox.lock() = Some(r);
            waker.wake();
        });
    }

    pub(crate) fn take_inbox(&mut self) {
        if let Some(r) = self.inbox.lock().take() {
            self.fetching = false;
            match r {
                Ok(c) => (self.catalog, self.error) = (Some(c), None),
                Err(e) => self.error = Some(e),
            }
        }
    }

    pub(crate) fn domain_slug(&self) -> Option<&'static str> {
        self.domain.map(|d| DOMAINS[d].0)
    }

    /// Discover's rows, as indexes into the catalogue.
    pub(crate) fn discover_list(&self, cx: &Cx) -> Vec<usize> {
        let Some(cat) = &self.catalog else { return vec![] };
        let all = cat.list(self.domain_slug(), &self.market.query, self.sort);
        if !self.mine {
            return all;
        }
        let st = cx.svc.state();
        all.into_iter().filter(|&i| matches!(state_of(&cat.skills[i], &st), State::Installed { .. } | State::Installing(_) | State::Failed(_))).collect()
    }

    pub(crate) fn skill(&self, i: usize) -> Option<&MarketSkill> {
        self.catalog.as_ref()?.skills.get(i)
    }

    pub(crate) fn selected(&self, cx: &Cx) -> Option<usize> {
        let all = self.discover_list(cx);
        all.get(self.market.sel.min(all.len().saturating_sub(1))).copied()
    }

    /// Where `s` stands: installed (itself, or with its whole repo), being installed, failed, disabled, yours, or not
    /// here.
    pub(crate) fn state(&self, s: &MarketSkill, cx: &Cx) -> State {
        state_of(s, &cx.svc.state())
    }

    /// The watched entry that is `s` itself (as settings spell it).
    fn watched(&self, s: &MarketSkill, cx: &Cx) -> Option<String> {
        let spec = skill_market::source(s)?;
        cx.svc.settings().skills.repos.into_iter().find(|w| skill_repos::same(w, &spec))
    }

    /// Enter (or the Install button, at `at`): where to install the selected skill, when there are projects to choose
    /// from (else everywhere); or say where it stands.
    pub(crate) fn install(&mut self, at: Option<(f32, f32)>, cx: &mut Cx) {
        let Some(s) = self.selected(cx).and_then(|i| self.skill(i)).cloned() else { return };
        match self.state(&s, cx) {
            State::Available if !cx.svc.projects().is_empty() => {
                let spec = skill_market::source(&s).unwrap_or_default();
                self.menu = Some(Menu::new(format!("Install {}", s.name), install_items(&spec, &[], &[], true, cx), at));
            }
            _ => self.install_market(Target::Everywhere, cx),
        }
    }

    /// i: install the selected skill into `target` straight away (or say where it stands).
    pub(crate) fn install_market(&mut self, target: Target, cx: &mut Cx) {
        let Some(s) = self.selected(cx).and_then(|i| self.skill(i)).cloned() else { return };
        match self.state(&s, cx) {
            State::NoSource => cx.toast(Kind::Info, format!("{}: bro can't tell which repo it's in · l opens its page", s.name)),
            State::Installed { via: None } => cx.toast(Kind::Info, format!("{} is installed · d disables it", s.name)),
            State::Installed { via: Some(repo) } => cx.toast(Kind::Info, format!("{} came with {repo}, which bro watches", skill_repos::label(&repo))),
            State::Installing(_) => {}
            State::Disabled => cx.toast(Kind::Info, format!("{} is disabled · e enables it", s.name)),
            State::Yours => cx.toast(Kind::Info, format!("you have a skill named {} in ~/.claude/skills already · bro leaves it alone", s.name)),
            State::Failed(_) => {
                // try again
                if let Some(spec) = self.watched(&s, cx) {
                    cx.svc.install_skills(&spec, vec![], target);
                }
            }
            State::Available => cx.svc.install_skills(&skill_market::source(&s).unwrap_or_default(), vec![], target),
        }
    }

    /// d: disable the selected skill (it was installed); e: enable it (it was disabled).
    pub(crate) fn remove(&mut self, cx: &mut Cx) {
        let Some(s) = self.selected(cx).and_then(|i| self.skill(i)).cloned() else { return };
        match market_installed(&s, cx) {
            Some(k) => disable(k, cx),
            None => cx.toast(Kind::Info, "not installed"),
        }
    }

    fn enable_market(&mut self, cx: &mut Cx) {
        let Some(s) = self.selected(cx).and_then(|i| self.skill(i)).cloned() else { return };
        match market_disabled(&s, cx) {
            Some(d) => cx.svc.enable_skill(d),
            None => cx.toast(Kind::Info, "it isn't disabled"),
        }
    }

    /// o: its page on skill-issue.in; l: where it lives.
    pub(crate) fn open(&mut self, source: bool, cx: &mut Cx) {
        let Some(s) = self.selected(cx).and_then(|i| self.skill(i)) else { return };
        let url = if source { s.link.clone() } else { s.page_url() };
        open_link(url, cx);
    }

    pub(crate) fn set_domain(&mut self, d: Option<usize>) {
        if self.domain != d {
            self.domain = d;
            self.market.reset();
        }
    }

    /// The next (or previous) domain tab, round from All.
    pub(crate) fn step_domain(&mut self, forward: bool) {
        let n = DOMAINS.len() as isize + 1;
        let at = self.domain.map_or(0, |d| d as isize + 1);
        let next = (at + if forward { 1 } else { -1 }).rem_euclid(n);
        self.set_domain((next > 0).then(|| next as usize - 1));
    }

    pub(crate) fn cycle_sort(&mut self) {
        self.set_sort(self.sort.next());
    }

    pub(crate) fn set_sort(&mut self, s: Sort) {
        if self.sort != s {
            self.sort = s;
            self.market.reset();
        }
    }

    pub(crate) fn toggle_mine(&mut self) {
        self.mine = !self.mine;
        self.market.reset();
    }

    /// What a row says about where the skill stands, and whether that's good news.
    pub(crate) fn state_label(state: &State) -> (String, Option<bool>) {
        match state {
            State::NoSource => ("no source".into(), None),
            State::Installed { via: None } => ("installed".into(), Some(true)),
            State::Installed { via: Some(_) } => ("with its repo".into(), Some(true)),
            State::Installing(at) => (format!("{} installing", spinner(*at)), None),
            State::Failed(_) => ("✗ didn't pull".into(), Some(false)),
            State::Disabled => ("disabled".into(), None),
            State::Yours => ("yours".into(), None),
            State::Available => (String::new(), None),
        }
    }

    // ---------------------------------------------------------------------------------------- both

    /// The list the keys move in: Discover's, or the installed page's.
    fn browse(&mut self) -> &mut Browse {
        if self.page == Page::Discover { &mut self.market } else { &mut self.installed }
    }

    fn move_sel(&mut self, d: isize, cx: &Cx) {
        let n = if self.page == Page::Discover { self.discover_list(cx).len() } else { Self::selectable(&self.installed_list(cx)).len() };
        let b = self.browse();
        b.sel = if n == 0 { 0 } else { (b.sel as isize).saturating_add(d).clamp(0, n as isize - 1) as usize };
    }

    /// The keys the page takes, for the hint line.
    pub(crate) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.menu.is_some() {
            return vec![("↑↓", "choose"), ("enter", "run"), ("esc", "close")];
        }
        if self.adding.is_some() {
            return vec![("enter", "watch it"), ("tab", "suggestion"), ("esc", "cancel"), ("ctrl+u", "clear")];
        }
        let mut h = match &self.page {
            Page::Discover => vec![("enter", "install"), ("p", "into a project"), ("/", "search"), ("←/→", "domain"), ("s", "sort"), ("f", "installed"), ("d", "disable"), ("o", "page"), ("l", "source"), ("m", "menu")],
            Page::Bro => vec![("enter", "switch"), ("m", "menu"), ("/", "search")],
            Page::Repo(_) => vec![("space", "tick"), ("i", "install"), ("p", "into a project"), ("d", "disable"), ("e", "enable"), ("m", "menu"), ("r", "pull now"), ("w", "stop watching"), ("l", "GitHub"), ("/", "search")],
            Page::Disabled => vec![("e", "enable"), ("enter", "open"), ("o", "folder"), ("x", "delete forever"), ("m", "menu"), ("/", "search")],
            _ => vec![("enter", "open"), ("o", "folder"), ("d", "disable"), ("m", "menu"), ("/", "search"), ("r", "look again")],
        };
        h.extend([("a", "add repo"), ("tab", "next page")]);
        h
    }

    // ---------------------------------------------------------------------------------------- terminal UI

    fn render_nav(&mut self, f: &mut Frame, r: Rect, nav: &[NavItem], t: &Theme) {
        ui::line(f, Rect { height: 1, ..r }, vec![Span::styled(" SKILLS", ui::bold_accent(t))]);
        let w = r.width as usize;
        let foot = r.bottom().saturating_sub(1);
        let mut y = r.y + 2;
        for item in nav {
            if y + 1 >= foot {
                break;
            }
            match item {
                NavItem::Head(h) => {
                    y += 1;
                    ui::line(f, Rect { y, height: 1, ..r }, vec![Span::styled(format!("  {}", h.to_lowercase()), muted(t))]);
                }
                NavItem::Page(p, label, note) => {
                    let on = *p == self.page;
                    let right = match note {
                        NavNote::Nothing => Span::raw(""),
                        NavNote::Count(_) | NavNote::Of(..) => Span::styled(note.text(), muted(t)),
                        NavNote::Busy => Span::styled(spinner(self.opened.unwrap_or_else(Instant::now)), fg(t.shine)),
                        NavNote::Failed => Span::styled("✗", fg(t.danger)),
                    };
                    let left = vec![Span::styled(if on { "▌" } else { " " }, fg(t.accent)), Span::styled(format!(" {}", ui::fit(label, w.saturating_sub(9))), if on { ui::bold() } else { Style::default() })];
                    ui::line_lr(f, Rect { y, height: 1, ..r }, left, vec![right, Span::raw(" ")]);
                    self.nav_hits.push((y, p.clone()));
                }
            }
            y += 1;
        }
        ui::line(f, Rect { y: foot, height: 1, ..r }, vec![Span::styled("  + add repo", fg(t.inline)), Span::styled("  a", muted(t))]);
        self.add_hit = Some(foot);
    }

    /// The page's title line, and the search line under it.
    fn render_heading(&self, f: &mut Frame, area: Rect, browse: &Browse, cx: &Cx) -> u16 {
        let t = cx.theme;
        let (title, sub) = self.heading(cx);
        let mut lines = vec![vec![Span::styled(format!(" {} ", title.to_uppercase()), ui::bold_accent(t)), Span::styled(format!("  {sub}"), muted(t))]];
        if let Page::Repo(spec) = &self.page
            && let Some(e) = cx.svc.state().skills.repos.get(spec).and_then(|r| r.error.clone())
        {
            lines.push(vec![Span::styled(format!("  ✗ couldn't pull: {e} · r tries again"), fg(t.danger))]);
        }
        let search = if browse.typing || !browse.query.is_empty() {
            Span::styled(format!("  / {}{}", browse.query, if browse.typing { "▏" } else { "" }), fg(t.inline))
        } else {
            Span::styled("  / to search", muted(t))
        };
        let mut line = vec![search];
        if !self.ticked.is_empty() {
            line.push(Span::styled(format!("    {} ticked: {} · i installs everywhere, p into a project", self.ticked.len(), and_list(&self.ticked)), fg(t.accent)));
        }
        lines.push(line);
        lines.push(vec![]);
        for (n, spans) in lines.iter().enumerate() {
            ui::line(f, Rect { y: area.y + n as u16, height: 1, ..area }, spans.clone());
        }
        lines.len() as u16
    }

    /// Lines into `list`, the selection kept in view; their screen rows are noted for clicks.
    fn draw_lines(&mut self, f: &mut Frame, list: Rect, lines: Vec<(Option<usize>, Vec<Span<'static>>)>, discover: bool, t: &Theme) {
        let sel = if discover { self.market.sel } else { self.installed.sel };
        let room = list.height as usize;
        let at = lines.iter().position(|(s, _)| *s == Some(sel)).unwrap_or(0);
        let b = self.browse();
        if at < b.scroll {
            b.scroll = at.saturating_sub(1);
        } else if at >= b.scroll + room {
            b.scroll = at + 1 - room;
        }
        b.scroll = b.scroll.min(lines.len().saturating_sub(room));
        let scroll = b.scroll;
        for (n, (s, spans)) in lines.into_iter().skip(scroll).take(room).enumerate() {
            let r = Rect { y: list.y + n as u16, height: 1, ..list };
            if let Some(s) = s {
                self.hits.push((r.y, s));
            }
            let spans = if s == Some(sel) { spans.into_iter().map(|s| Span::styled(s.content, s.style.bg(t.frame))).collect() } else { spans };
            ui::line(f, r, spans);
        }
    }

    fn render_installed(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        let top = self.render_heading(f, area, &self.installed.clone(), cx);
        const DETAIL: u16 = 5;
        let list = Rect { y: area.y + top, height: area.height.saturating_sub(top + DETAIL), ..area };
        let detail = Rect { y: list.y + list.height, height: area.height.saturating_sub(top).min(DETAIL), ..area };
        let entries = self.installed_list(cx);
        let at = Self::selectable(&entries);
        self.installed.sel = self.installed.sel.min(at.len().saturating_sub(1));
        let (inv, pending) = {
            let st = cx.svc.state();
            (st.skills.inventory.clone(), st.skills.pending.clone())
        };
        let spin = |name: &str| pending.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, at)| spinner(*at));
        let mut lines: Vec<(Option<usize>, Vec<Span<'static>>)> = vec![];
        if at.is_empty() && !entries.iter().any(|e| matches!(e, Entry::Note(_))) {
            lines.push((None, vec![Span::styled(format!("  {}", self.empty_note(cx)), muted(t))]));
        }
        let wide = area.width >= 100;
        let found = match &self.page {
            Page::Repo(spec) => self.repo_skills(spec, cx),
            _ => vec![],
        };
        let mut k = 0;
        for e in &entries {
            let on = k == self.installed.sel;
            let marker = Span::raw(format!("{} ", if on { "▌" } else { " " }));
            let name_style = if on { ui::bold() } else { Style::default() };
            let line = match e {
                Entry::Head(h) => (None, vec![Span::styled(format!("  {h}"), fg(t.fg).add_modifier(Modifier::BOLD))]),
                Entry::Note(n) => (None, vec![Span::styled(format!("    {n}"), muted(t))]),
                Entry::Skill(i) => {
                    let Some(s) = inv.as_ref().and_then(|v| v.skills.get(*i)) else { continue };
                    let mut spans = vec![marker, Span::styled(ui::pad(&s.name, 28), name_style)];
                    spans.extend(agent_spans(&s.agents(), t));
                    if wide && matches!(self.page, Page::Project(_)) {
                        spans.push(Span::styled(format!(" {}", ui::pad(&origin_tag(&s.origin), 16)), muted(t)));
                    }
                    match spin(&s.name) {
                        Some(sp) => spans.push(Span::styled(format!(" {sp} "), fg(t.shine))),
                        None => spans.push(Span::raw(" ")),
                    }
                    spans.push(Span::styled(s.summary().unwrap_or_default(), muted(t)));
                    (Some(k), spans)
                }
                Entry::Bro(b) => {
                    let (state, style) = match cx.svc.bundled_on(*b) {
                        Some(true) => ("● on", fg(t.good)),
                        Some(false) => ("○ off", muted(t)),
                        None if *b == Bundled::JustImagine => ("write ›", fg(t.inline)),
                        None => ("when needed", muted(t)),
                    };
                    let spans = vec![marker, Span::styled(ui::pad(b.title(), 16), name_style), Span::styled(ui::pad(state, 13), style), Span::styled(b.about().to_string(), muted(t))];
                    (Some(k), spans)
                }
                Entry::Repo(n) => {
                    let (Some(f), Some(inv), Page::Repo(spec)) = (found.get(*n), inv.as_deref(), &self.page) else { continue };
                    let st = repo_status(spec, &f.name, inv);
                    let (mark, mark_style) = match (spin(&f.name), &st) {
                        (Some(sp), _) => (format!(" {sp} "), fg(t.shine)),
                        (None, st) if st.installed() => (" ✓ ".into(), fg(t.good)),
                        (None, st) if st.disabled.is_some() => (" ◌ ".into(), muted(t)),
                        (None, _) if self.ticked.contains(&f.name) => ("[x]".into(), fg(t.accent)),
                        _ => ("[ ]".into(), muted(t)),
                    };
                    let place = repo_place(&st, inv, cx);
                    let spans = vec![marker, Span::styled(mark, mark_style), Span::raw(" "), Span::styled(ui::pad(&f.name, 26), name_style), Span::styled(ui::pad(&place, 22), if st.taken.is_some() && !st.installed() { fg(t.inline) } else { muted(t) }), Span::styled(f.description.as_deref().map(one_line).unwrap_or_default(), muted(t))];
                    (Some(k), spans)
                }
                Entry::Disabled(i) => {
                    let Some(d) = inv.as_ref().and_then(|v| v.disabled.get(*i)) else { continue };
                    let mut spans = vec![marker, Span::styled(ui::pad(&d.name, 28), name_style)];
                    spans.extend(agent_spans(&d.agents(), t));
                    match spin(&d.name) {
                        Some(sp) => spans.push(Span::styled(format!(" {sp} {}", ui::pad("", 12)), fg(t.shine))),
                        None => spans.push(Span::styled(format!(" {}", ui::pad(&since(d.at), 14)), muted(t))),
                    }
                    spans.push(Span::styled(d.description.as_deref().map(one_line).unwrap_or_default(), muted(t)));
                    (Some(k), spans)
                }
            };
            if line.0.is_some() {
                k += 1;
            }
            lines.push(line);
        }
        self.draw_lines(f, list, lines, false, t);

        // the selected one: what it does, where it came from, who gets it, where it lives
        let mut out: Vec<Vec<Span<'static>>> = vec![vec![]];
        match self.chosen(cx) {
            Some(Entry::Skill(i)) => {
                let Some(s) = inv.as_ref().and_then(|v| v.skills.get(i)) else { return };
                out.push(vec![Span::styled(format!("  {}", s.name), ui::bold()), Span::styled(format!("  {}", origin_line(s, cx)), muted(t))]);
                out.push(vec![Span::styled(format!("  {}", s.summary().unwrap_or_else(|| "no description in its SKILL.md".into())), fg(t.inline))]);
                out.push(vec![Span::styled(format!("  {}", places_line(&s.places)), muted(t))]);
                out.push(vec![Span::styled(format!("  {}", crate::folder::shorten_home(s.dir())), muted(t))]);
            }
            Some(Entry::Bro(b)) => {
                let homes = inv.as_ref().map_or(0, |v| v.everywhere().filter(|s| s.origin == Origin::Bro && b.names().contains(&s.name.as_str())).map(|s| s.places.len()).max().unwrap_or(0));
                out.push(vec![Span::styled(format!("  {}", b.title()), ui::bold()), Span::styled(format!("  writes {}", b.names().join(", ")), muted(t))]);
                out.push(vec![Span::styled(format!("  {}", b.about()), fg(t.inline))]);
                out.push(vec![Span::styled(format!("  in {} now", plural(homes, "agent home")), muted(t))]);
            }
            Some(Entry::Repo(n)) => {
                let (Some(f), Some(inv), Page::Repo(spec)) = (found.get(n), inv.as_deref(), &self.page) else { return };
                let st = repo_status(spec, &f.name, inv);
                out.push(vec![Span::styled(format!("  {}", f.name), ui::bold()), Span::styled(format!("  in {} · {}", skill_repos::label(spec), repo_standing(&st, inv, cx)), muted(t))]);
                out.push(vec![Span::styled(format!("  {}", f.description.as_deref().map(one_line).unwrap_or_else(|| "no description in its SKILL.md".into())), fg(t.inline))]);
                let copy = st.everywhere.or_else(|| st.projects.first().copied()).and_then(|i| inv.skills.get(i));
                match copy {
                    Some(s) => {
                        out.push(vec![Span::styled(format!("  {}", places_line(&s.places)), muted(t))]);
                        out.push(vec![Span::styled(format!("  {}", crate::folder::shorten_home(s.dir())), muted(t))]);
                    }
                    None if st.disabled.is_some() => out.push(vec![Span::styled("  e enables it again · x deletes it for good", muted(t))]),
                    None => out.push(vec![Span::styled("  i installs it everywhere · p into a project · space ticks it to install with others", muted(t))]),
                }
            }
            Some(Entry::Disabled(i)) => {
                let Some(d) = inv.as_ref().and_then(|v| v.disabled.get(i)) else { return };
                out.push(vec![Span::styled(format!("  {}", d.name), ui::bold()), Span::styled(format!("  disabled {} · {}", since(d.at), origin_text(&d.origin, d.project.as_deref(), cx)), muted(t))]);
                out.push(vec![Span::styled(format!("  {}", d.description.as_deref().map(one_line).unwrap_or_else(|| "no description in its SKILL.md".into())), fg(t.inline))]);
                out.push(vec![Span::styled("  e enables it, x deletes it for good", fg(t.inline)), Span::styled(format!(" · was in: {}", places_line(&d.places)), muted(t))]);
                out.push(vec![Span::styled(format!("  kept in {}", crate::folder::shorten_home(&d.dir())), muted(t))]);
            }
            _ => {}
        }
        for (n, spans) in out.into_iter().take(detail.height as usize).enumerate() {
            ui::line(f, Rect { y: detail.y + n as u16, height: 1, ..detail }, spans);
        }
    }

    /// Why an installed page has nothing to show.
    pub(crate) fn empty_note(&self, cx: &Cx) -> String {
        let st = cx.svc.state();
        if st.skills.inventory.is_none() {
            return "looking at your skills…".into();
        }
        if !self.installed.query.trim().is_empty() {
            return format!("nothing matches “{}”", self.installed.query.trim());
        }
        match &self.page {
            Page::Repo(spec) => match st.skills.repos.get(spec) {
                Some(r) if r.syncing.is_some() => format!("pulling {}…", skill_repos::label(spec)),
                Some(r) if r.error.is_some() => "its last pull didn't work · r tries again".into(),
                Some(r) if r.skills.is_some() => "no skills in it: a folder with a SKILL.md at its root, one folder down, or under skills/".into(),
                _ => "not pulled yet · r pulls it".into(),
            },
            Page::Disabled => "nothing disabled · d on a skill disables it: every copy moves here, out of your agents' reach".into(),
            _ => "no skills yet · a adds a repo, and Discover finds more".into(),
        }
    }

    fn render_discover(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        let mut top = self.render_heading(f, area, &self.market.clone(), cx);
        // the domain tabs, with counts while they fit; installed only and the sort
        let tab_label = |d: Option<usize>, counts: bool| {
            let label = d.map_or("All", |d| DOMAINS[d].1);
            match self.catalog.as_ref().map(|c| c.count(d.map(|d| DOMAINS[d].0))).filter(|_| counts) {
                Some(n) => format!(" {label} {n} "),
                None => format!(" {label} "),
            }
        };
        let tabs: Vec<Option<usize>> = std::iter::once(None).chain((0..DOMAINS.len()).map(Some)).collect();
        let counts = tabs.iter().map(|d| ui::width(&tab_label(*d, true)) + 1).sum::<usize>() + 2 <= area.width as usize;
        let mut bar = vec![Span::raw("  ")];
        for d in &tabs {
            let on = *d == self.domain;
            bar.push(Span::styled(tab_label(*d, counts || on), if on { ui::bold_accent(t).add_modifier(Modifier::REVERSED) } else { muted(t) }));
            bar.push(Span::raw(" "));
        }
        let mut opts = vec![Span::styled("  sort ", muted(t)), Span::styled(self.sort.label().to_string(), fg(t.inline))];
        if self.mine {
            opts.push(Span::styled("   installed only", fg(t.good)));
        }
        let mut head = vec![bar, opts];
        // the repos bro features, while there's no search
        if self.market.query.trim().is_empty() {
            let watched = cx.svc.settings().skills.repos;
            for feat in FEATURED {
                let state = if watched.iter().any(|w| skill_repos::same(w, feat.repo)) { Span::styled("  ✓ watching", fg(t.good)) } else { Span::styled("  a, then tab: add it", fg(t.inline)) };
                head.push(vec![Span::styled(format!("  ★ {} ", feat.title), fg(t.shine).add_modifier(Modifier::BOLD)), Span::styled(format!("by {} · {} · {}", feat.by, feat.repo, plural(feat.skills.len(), "skill")), muted(t)), state]);
            }
        }
        head.push(vec![]);
        for (n, spans) in head.iter().enumerate() {
            ui::line(f, Rect { y: area.y + top + n as u16, height: 1, ..area }, spans.clone());
        }
        top += head.len() as u16;
        // the selected skill's details, pinned under the list
        const DETAIL: u16 = 4;
        let list = Rect { y: area.y + top, height: area.height.saturating_sub(top + DETAIL), ..area };
        let detail = Rect { y: list.y + list.height, height: area.height.saturating_sub(top).min(DETAIL), ..area };

        let entries = self.discover_list(cx);
        self.market.sel = self.market.sel.min(entries.len().saturating_sub(1));
        let mut lines: Vec<(Option<usize>, Vec<Span<'static>>)> = vec![];
        if entries.is_empty() {
            let why = match (&self.catalog, &self.error) {
                (None, Some(e)) => format!("couldn't fetch the marketplace: {e} · r tries again"),
                (None, None) => format!("{} fetching skill-issue.in…", spinner(Instant::now())),
                _ if self.mine => "nothing installed from here yet · f shows them all".into(),
                _ => "nothing matches".into(),
            };
            lines.push((None, vec![Span::styled(format!("  {why}"), muted(t))]));
        }
        let wide = area.width >= 110;
        for (row, &i) in entries.iter().enumerate() {
            let Some(s) = self.skill(i) else { continue };
            let sel = row == self.market.sel;
            let state = self.state(s, cx);
            let (label, good) = SkillsView::state_label(&state);
            let (mark, mark_style) = match good {
                Some(true) => ("✓", fg(t.good)),
                Some(false) => ("✗", fg(t.danger)),
                None if matches!(state, State::Installing(_)) => ("·", fg(t.shine)),
                None => (" ", muted(t)),
            };
            let clicks = self.catalog.as_ref().map_or(0, |c| c.clicks_of(&s.id));
            let mut spans = vec![
                Span::raw(format!("{} ", if sel { "▌" } else { " " })),
                Span::styled(format!("{mark} "), mark_style),
                Span::styled(ui::pad(&s.name, 30), if sel { ui::bold() } else { Style::default() }),
                Span::styled(format!(" {}", ui::pad(&s.popularity().unwrap_or_default(), 15)), fg(t.inline)),
                Span::styled(format!(" {}", ui::pad(&clicks_label(clicks), 12)), muted(t)),
            ];
            let state_style = match good {
                Some(true) => fg(t.good),
                Some(false) => fg(t.danger),
                None if matches!(state, State::Installing(_)) => fg(t.shine),
                None => muted(t),
            };
            spans.push(Span::styled(format!(" {}", ui::pad(&label, 14)), state_style));
            if wide {
                let place = match &s.niche {
                    Some(n) if self.domain.is_none() => format!("{} · {n}", s.domain),
                    Some(n) => n.clone(),
                    None => s.domain.clone(),
                };
                spans.push(Span::styled(format!(" {}", ui::pad(&place, 36)), muted(t)));
            }
            spans.push(Span::styled(format!(" {}", one_line(&s.use_case)), muted(t)));
            lines.push((Some(row), spans));
        }
        self.draw_lines(f, list, lines, true, t);

        // the selected one: what it does, who made it, where it comes from and how it installs
        let Some(s) = self.selected(cx).and_then(|i| self.skill(i)).cloned() else { return };
        let state = self.state(&s, cx);
        let mut out: Vec<Vec<Span<'static>>> = vec![vec![]];
        let verified = if s.link_verified { "" } else { " · link not checked by hand" };
        out.push(vec![Span::styled(format!("  {}", s.name), ui::bold()), Span::styled(format!("  by {} · {}{}", s.creator, s.niche.clone().unwrap_or_else(|| s.domain.clone()), verified), muted(t))]);
        out.push(vec![Span::styled(format!("  {}", one_line(&s.use_case)), fg(t.inline))]);
        let how = match (&state, skill_market::source(&s)) {
            (State::Failed(e), _) => vec![Span::styled(format!("  ✗ {e} · enter tries again"), fg(t.danger))],
            (State::Yours, _) => vec![Span::styled(format!("  ~/.claude/skills has a {} of yours: bro won't replace it", s.name), muted(t))],
            (State::Disabled, _) => vec![Span::styled("  disabled · e enables it again", muted(t))],
            (_, Some(spec)) => vec![Span::styled("  from ", muted(t)), Span::styled(skill_repos::label(&spec), fg(t.fg)), Span::styled(format!("  {}", s.link), muted(t))],
            (_, None) => vec![Span::styled(format!("  {} · l opens it", s.link), muted(t))],
        };
        out.push(how);
        for (n, spans) in out.into_iter().take(detail.height as usize).enumerate() {
            ui::line(f, Rect { y: detail.y + n as u16, height: 1, ..detail }, spans);
        }
    }

    /// The add dialog, at the foot of the page: what's typed, what bro makes of it, and a suggestion.
    fn render_dialog(&mut self, f: &mut Frame, r: Rect, cx: &Cx) {
        let t = cx.theme;
        let text = self.adding.clone().unwrap_or_default();
        let check = self.add_check(cx);
        let rule = "─".repeat(r.width as usize);
        let (what, style) = match &check {
            AddCheck::Empty => ("paste a GitHub link, owner/repo or any git URL: bro lists the skills in it, and you install the ones you want".to_string(), muted(t)),
            AddCheck::NotARepo => ("✗ not a repo yet: github.com/owner/repo, owner/repo or a git URL".to_string(), fg(t.danger)),
            AddCheck::Watched(spec) => (format!("already watching {} · enter shows it", skill_repos::label(spec)), fg(t.inline)),
            AddCheck::Ready(spec) => (format!("✓ {} · enter watches it", skill_repos::label(spec)), fg(t.good)),
        };
        let mut lines = vec![
            vec![Span::styled(rule, fg(t.frame))],
            vec![Span::styled(" ADD A REPO ", ui::bold_accent(t).add_modifier(Modifier::REVERSED)), Span::styled(format!("  › {text}▏"), fg(t.fg))],
            vec![Span::styled(format!("  {what}"), style)],
        ];
        if let Some(feat) = self.suggestion(cx).filter(|f| !matches!(&check, AddCheck::Ready(s) if skill_repos::same(s, f.repo))) {
            lines.push(vec![Span::styled(format!("  suggested: {} ({}, by {}) · tab fills it in", feat.repo, feat.title, feat.by), muted(t))]);
        }
        for (n, spans) in lines.into_iter().take(r.height as usize).enumerate() {
            ui::line(f, Rect { y: r.y + n as u16, height: 1, ..r }, spans);
        }
    }

    /// The open menu, as a box by the selection (or where it was right-clicked); its rows are noted for clicks.
    fn render_menu(&mut self, f: &mut Frame, body: Rect, t: &Theme) {
        self.menu_hits.clear();
        self.menu_box = None;
        let Some(m) = &self.menu else { return };
        // as wide as its labels read now (one asking to be pressed again says more), each with its key: "› label  key "
        let key_w = |i: &MenuItem| i.key.map_or(0, |k| ui::width(k) + 2);
        let need = m.items.iter().enumerate().map(|(n, i)| ui::width(&m.label(n)) + key_w(i) + 4).max().unwrap_or(10).max(ui::width(&m.title) + 6);
        let w = (need as u16 + 2).clamp(24, 64).min(body.width);
        let h = (m.items.len() as u16 + 2).min(body.height);
        if w < 12 || h < 3 {
            return;
        }
        // by the selected row, else where it was clicked
        let sel_row = self.hits.iter().find(|(_, i)| *i == if self.page == Page::Discover { self.market.sel } else { self.installed.sel }).map(|(y, _)| *y);
        let (x, y) = match (m.at, sel_row) {
            (Some((x, y)), _) => (x as u16, y as u16 + 1),
            (None, Some(y)) => (body.x + 6, y + 1),
            _ => (body.x + 6, body.y + 3),
        };
        let y = if y + h > body.bottom() { y.saturating_sub(h + 1).max(body.y) } else { y };
        let r = Rect::new(x.clamp(body.x, body.right().saturating_sub(w)), y.clamp(body.y, body.bottom().saturating_sub(h)), w, h);
        ui::clear(f, r, t);
        let inner = ui::frame(f, r, &m.title, None, true, t);
        for (row, i) in (0..m.items.len()).take(inner.height as usize).enumerate() {
            let item = &m.items[i];
            let rr = Rect { y: inner.y + row as u16, height: 1, ..inner };
            match &item.act {
                None if item.label.is_empty() => ui::line(f, rr, vec![Span::styled("─".repeat(rr.width as usize), fg(t.frame))]),
                None => ui::line(f, rr, vec![Span::styled(format!(" {}", item.label), muted(t))]),
                Some(_) => {
                    let on = i == m.sel;
                    let style = match (item.danger, on) {
                        (true, _) => fg(t.danger).add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }),
                        (false, true) => ui::bold_accent(t),
                        _ => fg(t.fg),
                    };
                    let room = (rr.width as usize).saturating_sub(4 + key_w(item));
                    let left = vec![Span::styled(if on { "›" } else { " " }, fg(t.accent)), Span::styled(format!(" {}", ui::fit(&m.label(i), room)), style)];
                    ui::line_lr(f, rr, left, vec![Span::styled(item.key.map(|k| format!("{k} ")).unwrap_or_default(), muted(t))]);
                    self.menu_hits.push((rr.y, i));
                }
            }
        }
        self.menu_box = Some(r);
    }
}

/// A skill picked out of its repo (what Skill Issue installs).
fn is_pick(s: &Skill) -> bool {
    matches!(&s.origin, Origin::Repo(r) if skill_repos::pick(r).is_some())
}

/// Where a skill of watched repo `spec` named `name` stands: installed everywhere, in which projects, disabled, or
/// held in the agent homes by a skill of yours of that name.
pub(crate) fn repo_status(spec: &str, name: &str, inv: &Inventory) -> RepoStatus {
    let ours = |o: &Origin| matches!(o, Origin::Repo(r) if skill_repos::same(r, spec));
    let mut st = RepoStatus::default();
    for (i, s) in inv.skills.iter().enumerate().filter(|(_, s)| s.name.eq_ignore_ascii_case(name)) {
        match (&s.project, ours(&s.origin)) {
            (None, true) => st.everywhere = Some(i),
            (Some(_), true) => st.projects.push(i),
            (None, false) if s.origin != Origin::Codex => st.taken = Some(i),
            _ => {}
        }
    }
    st.disabled = inv.disabled.iter().position(|d| d.name.eq_ignore_ascii_case(name) && ours(&d.origin));
    st
}

/// Where a repo skill is, in a few words (its row's middle column).
fn repo_place(st: &RepoStatus, inv: &Inventory, cx: &Cx) -> String {
    let projects: Vec<String> = st.projects.iter().filter_map(|&i| inv.skills.get(i)?.project.as_deref().map(|p| project_name(p, cx))).collect();
    match (st.everywhere.is_some(), &projects[..]) {
        (true, []) => "everywhere".into(),
        (true, p) => format!("everywhere + {}", plural(p.len(), "project")),
        (false, [one]) => format!("in {one}"),
        (false, p) if !p.is_empty() => format!("in {}", plural(p.len(), "project")),
        _ if st.disabled.is_some() => "disabled".into(),
        _ if st.taken.is_some() => "name taken".into(),
        _ => String::new(),
    }
}

/// Where a repo skill stands, as its details say it.
pub(crate) fn repo_standing(st: &RepoStatus, inv: &Inventory, cx: &Cx) -> String {
    let projects: Vec<String> = st.projects.iter().filter_map(|&i| inv.skills.get(i)?.project.as_deref().map(|p| project_name(p, cx))).collect();
    match (st.everywhere.is_some(), projects.is_empty()) {
        (true, true) => "installed everywhere".into(),
        (true, false) => format!("installed everywhere, and in {}", and_list(&projects)),
        (false, false) => format!("installed in {}", and_list(&projects)),
        _ if st.disabled.is_some() => "disabled".into(),
        _ if st.taken.is_some() => "not installed: another skill in your agents' homes has its name, so installing it everywhere leaves it out".into(),
        _ => "not installed".into(),
    }
}

/// The items that install `names` of `spec`: everywhere (when `everywhere`), then each sidebar project not in
/// `skip`, under a heading.
fn install_items(spec: &str, names: &[String], skip: &[PathBuf], everywhere: bool, cx: &Cx) -> Vec<MenuItem> {
    let install = |target: Target| MenuAct::Install { spec: spec.to_string(), names: names.to_vec(), target };
    let mut items = vec![];
    if everywhere {
        items.push(MenuItem::new("Install everywhere", MenuIcon::Everywhere, Some("i"), install(Target::Everywhere)));
    }
    let projects: Vec<_> = cx.svc.projects().into_iter().filter(|p| !skip.iter().any(|s| bro_core::util::same_path(s, &p.root))).collect();
    if !projects.is_empty() {
        items.push(MenuItem::head(if everywhere { "Or into a project" } else { "Also into a project" }));
        items.extend(projects.into_iter().map(|p| MenuItem::new(p.name, MenuIcon::Project, None, install(Target::Project(p.root)))));
    }
    items
}

/// A repo's page (or a skill's folder in it) on the web.
fn web_item(spec: &str, skill: Option<&RepoSkill>) -> MenuItem {
    let url = match skill {
        Some(f) => skill_web(spec, f),
        None => skill_repos::web_url(spec),
    };
    let github = skill_repos::is_github(spec);
    MenuItem::new(if github { "Open on GitHub" } else { "Open its repo" }, if github { MenuIcon::GitHub } else { MenuIcon::Web }, Some("l"), MenuAct::Web(url))
}

/// A repo skill's folder on the web (GitHub's tree view), else its repo's page.
fn skill_web(spec: &str, f: &RepoSkill) -> String {
    let clone = skill_repos::cache_dir(spec);
    match f.dir.strip_prefix(&clone).ok().map(|p| p.to_string_lossy().replace('\\', "/")) {
        Some(rel) if !rel.is_empty() && skill_repos::is_github(spec) => skill_repos::web_url(&format!("{}#{rel}", skill_repos::base(spec))),
        _ => skill_repos::web_url(spec),
    }
}

/// Disable an installed skill: bro's own are switched off instead, and Codex keeps its own.
fn disable(s: Skill, cx: &mut Cx) {
    match &s.origin {
        Origin::Bro => match Bundled::here().into_iter().find(|b| b.names().contains(&s.name.as_str())) {
            Some(b) if cx.svc.bundled_on(b) == Some(true) => cx.svc.set_bundled(b, false),
            _ => cx.toast(Kind::Info, format!("{} is bro's own: bro writes it when it's needed", s.name)),
        },
        Origin::Codex => cx.toast(Kind::Info, format!("{} comes with Codex, which keeps it", s.name)),
        _ => cx.svc.disable_skill(s),
    }
}

/// The installed copy of a catalogue skill (from its own spec, or its whole repo).
fn market_installed(s: &MarketSkill, cx: &Cx) -> Option<Skill> {
    let spec = skill_market::source(s)?;
    let st = cx.svc.state();
    let inv = st.skills.inventory.as_deref()?;
    inv.skills.iter().find(|k| k.project.is_none() && market_origin(s, &spec, &k.origin, &k.name)).cloned()
}

/// The disabled copy of a catalogue skill.
fn market_disabled(s: &MarketSkill, cx: &Cx) -> Option<Disabled> {
    let spec = skill_market::source(s)?;
    let st = cx.svc.state();
    let inv = st.skills.inventory.as_deref()?;
    inv.disabled.iter().find(|d| market_origin(s, &spec, &d.origin, &d.name)).cloned()
}

/// A skill named `name` from `origin` is catalogue skill `s` (its spec `spec`): installed from that spec, or with its
/// whole repo under its name.
fn market_origin(s: &MarketSkill, spec: &str, origin: &Origin, name: &str) -> bool {
    match origin {
        Origin::Repo(r) if skill_repos::same(r, spec) => true,
        Origin::Repo(r) => skill_repos::pick(spec).is_some() && skill_repos::same(r, skill_repos::base(spec)) && name.eq_ignore_ascii_case(&s.name),
        _ => false,
    }
}

/// A project's name as the sidebar shows it, else its folder's.
pub(crate) fn project_name(root: &Path, cx: &Cx) -> String {
    cx.svc.project_label(root)
}

/// Where a skill came from, in a word or two (beside it on a project's page).
pub(crate) fn origin_tag(o: &Origin) -> String {
    match o {
        Origin::Bro => "bro".into(),
        Origin::Repo(spec) if skill_repos::pick(spec).is_some() => "Skill Issue".into(),
        Origin::Repo(spec) => skill_repos::base(spec).rsplit('/').next().unwrap_or(spec).to_string(),
        Origin::Codex => "Codex".into(),
        Origin::Yours => "yours".into(),
    }
}

/// Where a skill came from, as its details say it.
pub(crate) fn origin_line(s: &Skill, cx: &Cx) -> String {
    origin_text(&s.origin, s.project.as_deref(), cx)
}

pub(crate) fn origin_text(origin: &Origin, project: Option<&Path>, cx: &Cx) -> String {
    let base = match origin {
        Origin::Bro => "bundled with bro".to_string(),
        Origin::Repo(spec) if skill_repos::pick(spec).is_some() => format!("from Skill Issue · {}", skill_repos::base(spec)),
        Origin::Repo(spec) => format!("from {}", skill_repos::label(spec)),
        Origin::Codex => "built into Codex".into(),
        Origin::Yours => "yours: bro leaves it alone".into(),
    };
    match project {
        Some(root) => format!("{base} · only in {}", project_name(root, cx)),
        None => base,
    }
}

/// Who gets a skill, and from where: "Claude in ~/.claude and 2 accounts · Codex in ~/.codex".
pub(crate) fn places_line(places: &[Place]) -> String {
    let mut agents: Vec<Agent> = places.iter().map(|p| p.agent).collect();
    agents.sort();
    agents.dedup();
    agents
        .into_iter()
        .map(|a| {
            let dirs: Vec<&Path> = places.iter().filter(|p| p.agent == a).map(|p| home_of(&p.dir)).collect();
            let first = dirs.first().map(|d| crate::folder::shorten_home(d)).unwrap_or_default();
            let more = match (a, dirs.len().saturating_sub(1)) {
                (_, 0) => String::new(),
                (Agent::Claude, n) => format!(" and {}", plural(n, "account")),
                (Agent::Codex, n) => format!(" and {}", plural(n, "profile")),
                (_, n) => format!(" and {n} more"),
            };
            format!("{} in {first}{more}", a.label())
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The home (or project folder) a skill's copy is in: the folder holding its `skills`.
pub(crate) fn home_of(dir: &Path) -> &Path {
    dir.ancestors().find(|a| a.file_name().is_some_and(|n| n == "skills")).and_then(Path::parent).unwrap_or(dir)
}

/// How long ago `at` (seconds since 1970) was: "just now", "5 min ago", "3 h ago", "2 days ago".
pub(crate) fn since(at: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match now.saturating_sub(at) {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s if s < 2 * 86_400 => "a day ago".into(),
        s if s < 60 * 86_400 => format!("{} days ago", s / 86_400),
        s => format!("{} months ago", s / (30 * 86_400)),
    }
}

/// The agents that get a skill, as the terminal UI marks them (their glyphs), in a slot of the same width.
fn agent_spans(agents: &[Agent], t: &Theme) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = agents
        .iter()
        .map(|a| match a.harness() {
            Some(h) => Span::styled(format!("{} ", ui::harness_glyph(Some(h))), fg(ui::harness_color(Some(h), t))),
            None => Span::styled("◇ ", muted(t)),
        })
        .collect();
    out.push(Span::raw(" ".repeat(10usize.saturating_sub(agents.len() * 2))));
    out
}

/// A folder shown in the file manager (tests show nothing).
fn reveal(dir: &Path) {
    if !cfg!(test) {
        crate::panes::links::reveal(dir);
    }
}

/// A link opened for the view (the app opens it off the UI thread; tests open nothing).
fn open_link(url: String, cx: &Cx) {
    let _ = cx.tx.send(Event::OpenLink(url));
}

/// Where `s` stands, read from one look at bro's state (the list asks for every row each frame): its install under
/// way or failed, then what the inventory has of it (installed, disabled, a skill of yours of that name).
pub(crate) fn state_of(s: &MarketSkill, st: &crate::services::State) -> State {
    let Some(spec) = skill_market::source(s) else { return State::NoSource };
    let watched = &st.settings.skills.repos;
    let repo = skill_repos::base(&spec);
    let key = watched.iter().find(|w| skill_repos::same(w, &spec)).or_else(|| watched.iter().find(|w| skill_repos::same(w, repo)));
    match key.and_then(|k| st.skills.repos.get(k)) {
        Some(r) if r.syncing.is_some() && st.skills.pending(&s.name).is_some() => return State::Installing(r.syncing.unwrap_or_else(Instant::now)),
        Some(r) if r.error.is_some() && skill_repos::same(key.map_or("", String::as_str), &spec) => return State::Failed(r.error.clone().unwrap_or_default()),
        _ => {}
    }
    let yours = || st.skills.own.iter().any(|o| o.eq_ignore_ascii_case(&s.name));
    // before the skills were looked at: what's watched
    let Some(inv) = st.skills.inventory.as_deref() else {
        return match key {
            Some(k) if skill_repos::same(k, &spec) => State::Installed { via: None },
            _ if yours() => State::Yours,
            _ => State::Available,
        };
    };
    if let Some(k) = inv.skills.iter().find(|k| k.project.is_none() && market_origin(s, &spec, &k.origin, &k.name)) {
        return match &k.origin {
            Origin::Repo(r) if !skill_repos::same(r, &spec) => State::Installed { via: Some(r.clone()) },
            _ => State::Installed { via: None },
        };
    }
    if inv.disabled.iter().any(|d| market_origin(s, &spec, &d.origin, &d.name)) {
        return State::Disabled;
    }
    if yours() { State::Yours } else { State::Available }
}

/// "1.2k" clicks.
pub(crate) fn clicks_label(n: u64) -> String {
    if n == 0 { String::new() } else { format!("{} clicks", skill_market::compact(n as f64)) }
}

impl Pane for SkillsView {
    fn title(&self) -> String {
        "skills".into()
    }
    fn icon(&self) -> &'static str {
        "skills"
    }
    fn view(&self) -> Option<&'static str> {
        Some("skills")
    }
    fn poll(&mut self, _cx: &mut Cx) {
        self.take_inbox();
    }
    fn tick_every(&self) -> Option<Duration> {
        (self.fetching || self.busy).then_some(Duration::from_millis(120))
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        let nav = self.prepare(cx);
        let area = ui::hint_line(f, area, &self.hints(), t);
        // the pages down the left, when there's room for them beside the page
        let nav_w: u16 = if area.width >= 84 { 26 } else { 0 };
        self.nav_hits.clear();
        self.hits.clear();
        self.add_hit = None;
        self.nav_end = area.x + nav_w;
        if nav_w > 0 {
            self.render_nav(f, Rect { width: nav_w - 2, ..area }, &nav, t);
        }
        let body = Rect { x: area.x + nav_w, width: area.width.saturating_sub(nav_w), ..area };
        // the add dialog takes the foot of the page
        let (page, dialog) = match self.adding {
            Some(_) if body.height > 8 => (Rect { height: body.height - 4, ..body }, Some(Rect { y: body.bottom() - 4, height: 4, ..body })),
            _ => (body, None),
        };
        if self.page == Page::Discover {
            self.render_discover(f, page, cx);
        } else {
            self.render_installed(f, page, cx);
        }
        if let Some(d) = dialog {
            self.render_dialog(f, d, cx);
        }
        self.render_menu(f, body, t);
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.menu.is_some() {
            self.menu_key(key, cx);
            return true;
        }
        if self.adding.is_some() {
            let suggest = self.suggestion(cx).map(|f| f.repo);
            match key.code {
                KeyCode::Esc => self.adding = None,
                KeyCode::Enter => self.add_repo(cx),
                KeyCode::Tab => {
                    if let (Some(s), Some(text)) = (suggest, self.adding.as_mut()) {
                        *text = s.to_string();
                    }
                }
                KeyCode::Backspace => {
                    self.adding.as_mut().map(String::pop);
                }
                KeyCode::Char('u') if ctrl => self.adding = Some(String::new()),
                KeyCode::Char(c) if !ctrl && self.adding.as_ref().is_some_and(|t| t.chars().count() < 300) => self.adding.as_mut().into_iter().for_each(|t| t.push(c)),
                _ => return false,
            }
            return true;
        }
        if self.browse().typing {
            return self.browse().type_key(key);
        }
        match key.code {
            KeyCode::Tab => self.step_page(true, cx),
            KeyCode::BackTab => self.step_page(false, cx),
            KeyCode::Char('a') => self.adding = Some(String::new()),
            KeyCode::Char('/') => self.browse().typing = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1, cx),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1, cx),
            KeyCode::PageDown => self.move_sel(10, cx),
            KeyCode::PageUp => self.move_sel(-10, cx),
            KeyCode::Char('g') | KeyCode::Home => self.move_sel(-(isize::MAX / 2), cx),
            KeyCode::Char('G') | KeyCode::End => self.move_sel(isize::MAX / 2, cx),
            // the menu: m, the menu key, shift+F10
            KeyCode::Char('m') | KeyCode::Menu => self.open_menu(None, cx),
            KeyCode::F(10) if key.modifiers.contains(KeyModifiers::SHIFT) => self.open_menu(None, cx),
            KeyCode::Esc if !self.ticked.is_empty() => self.ticked.clear(),
            KeyCode::Esc if !self.browse().query.is_empty() => {
                let b = self.browse();
                b.query.clear();
                b.reset();
            }
            _ if self.page == Page::Discover => return self.discover_key(key, cx),
            _ => return self.installed_key(key, cx),
        }
        true
    }
    fn mouse(&mut self, ev: MouseEvent, _area: Rect, cx: &mut Cx) {
        // a menu that's open takes the mouse: a click on an item runs it, anywhere else closes it
        if self.menu.is_some() {
            match ev.kind {
                MouseEventKind::Down(MouseButton::Left) => match self.menu_hits.iter().find(|(y, _)| *y == ev.row).map(|(_, i)| *i).filter(|_| self.menu_box.is_some_and(|b| ev.column >= b.x && ev.column < b.right())) {
                    Some(i) => self.run_menu(i, cx),
                    None => self.menu = None,
                },
                MouseEventKind::Down(_) => self.menu = None,
                MouseEventKind::ScrollDown => self.menu.as_mut().into_iter().for_each(|m| m.step(1)),
                MouseEventKind::ScrollUp => self.menu.as_mut().into_iter().for_each(|m| m.step(-1)),
                MouseEventKind::Moved => {
                    if let (Some(i), Some(m)) = (self.menu_hits.iter().find(|(y, _)| *y == ev.row).map(|(_, i)| *i), self.menu.as_mut()) {
                        m.sel = i;
                    }
                }
                _ => {}
            }
            return;
        }
        match ev.kind {
            MouseEventKind::ScrollDown => self.move_sel(1, cx),
            MouseEventKind::ScrollUp => self.move_sel(-1, cx),
            MouseEventKind::Down(MouseButton::Left) if ev.column < self.nav_end => {
                if self.add_hit == Some(ev.row) {
                    self.adding = Some(String::new());
                } else if let Some((_, p)) = self.nav_hits.iter().find(|(y, _)| *y == ev.row).cloned() {
                    self.go(p);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, i)) = self.hits.iter().find(|(y, _)| *y == ev.row) {
                    if self.browse().sel == i {
                        if self.page == Page::Discover { self.install(Some((ev.column as f32, ev.row as f32)), cx) } else { self.activate(cx) }
                    }
                    self.browse().sel = i;
                }
            }
            // a right-click: that line's menu, where it was clicked
            MouseEventKind::Down(MouseButton::Right) if ev.column >= self.nav_end => {
                if let Some(&(_, i)) = self.hits.iter().find(|(y, _)| *y == ev.row) {
                    self.browse().sel = i;
                    self.open_menu(Some((ev.column as f32, ev.row as f32)), cx);
                }
            }
            _ => {}
        }
    }
    fn paste(&mut self, text: &str, _cx: &mut Cx) {
        let line = text.lines().next().unwrap_or_default().trim();
        match &mut self.adding {
            Some(t) => t.push_str(line),
            None => self.browse().paste(line),
        }
    }
}

impl SkillsView {
    /// Keys only Discover takes.
    fn discover_key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        match key.code {
            KeyCode::Right | KeyCode::Char('L') | KeyCode::Char(']') => self.step_domain(true),
            KeyCode::Left | KeyCode::Char('H') | KeyCode::Char('[') => self.step_domain(false),
            KeyCode::Char('0') => self.set_domain(None),
            KeyCode::Char(c @ '1'..='9') => self.set_domain(Some(c as usize - '1' as usize).filter(|d| *d < DOMAINS.len())),
            KeyCode::Enter => self.install(None, cx),
            KeyCode::Char('i') => self.install_market(Target::Everywhere, cx),
            KeyCode::Char('p') => match self.selected(cx).and_then(|i| self.skill(i)).cloned() {
                Some(s) if self.state(&s, cx) == State::Available => {
                    let spec = skill_market::source(&s).unwrap_or_default();
                    let items = install_items(&spec, &[], &[], false, cx);
                    if items.is_empty() {
                        cx.toast(Kind::Info, "no projects in the sidebar to install into");
                    } else {
                        self.menu = Some(Menu::new(format!("Install {} into", s.name), items, None));
                    }
                }
                _ => self.install_market(Target::Everywhere, cx),
            },
            KeyCode::Char('d') | KeyCode::Delete => self.remove(cx),
            KeyCode::Char('e') => self.enable_market(cx),
            KeyCode::Char('s') => self.cycle_sort(),
            KeyCode::Char('f') => self.toggle_mine(),
            KeyCode::Char('o') => self.open(false, cx),
            KeyCode::Char('l') => self.open(true, cx),
            KeyCode::Char('r') => {
                self.fetch(true, cx);
                cx.svc.load_skill_repos();
            }
            _ => return false,
        }
        true
    }

    /// Keys only the installed pages take.
    fn installed_key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        let repo = matches!(self.page, Page::Repo(_));
        match key.code {
            KeyCode::Enter => self.activate(cx),
            KeyCode::Char(' ') if repo => self.tick_chosen(cx),
            KeyCode::Char(' ') => self.activate(cx),
            KeyCode::Char('i') if repo => self.install_here(Target::Everywhere, cx),
            KeyCode::Char('p') if repo => self.choose_target(None, cx),
            KeyCode::Char('w') if repo => self.stop_menu(None, cx),
            KeyCode::Char('*') if repo => self.tick_all(cx),
            KeyCode::Char('o') => self.reveal(cx),
            KeyCode::Char('l') => self.open_web(cx),
            KeyCode::Char('d') | KeyCode::Delete => self.disable_chosen(cx),
            KeyCode::Char('e') => self.enable_chosen(cx),
            KeyCode::Char('x') => self.delete_chosen(cx),
            KeyCode::Char('r') => match &self.page {
                Page::Repo(spec) => cx.svc.sync_skill_repo(spec, true),
                _ => {
                    cx.svc.scan_skills();
                    cx.svc.load_skill_repos();
                }
            },
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_tabs_go_round_from_all() {
        let mut v = SkillsView::new();
        v.step_domain(false);
        assert_eq!(v.domain_slug(), Some("automation-tools"));
        v.step_domain(true);
        assert_eq!(v.domain, None);
        v.step_domain(true);
        assert_eq!(v.domain_slug(), Some("design"));
        assert_eq!(Sort::Ranked.next(), Sort::Clicks);
        assert_eq!(Sort::Name.next(), Sort::Ranked);
        assert_eq!(clicks_label(9833), "9.8k clicks");
        assert_eq!(clicks_label(0), "");
    }

    #[test]
    fn where_a_skill_lives_reads_short() {
        let home = PathBuf::from("/h");
        let place = |agent, dir: &str| Place { agent, dir: home.join(dir) };
        let s = Skill {
            name: "council".into(),
            description: None,
            origin: Origin::Repo("t3dotgg/tstack".into()),
            project: None,
            places: vec![place(Agent::Claude, ".claude/skills/council"), place(Agent::Claude, "pool/a/skills/council"), place(Agent::Claude, "pool/b/skills/council"), place(Agent::Codex, ".codex/skills/council")],
        };
        let line = places_line(&s.places);
        assert!(line.starts_with("Claude in ") && line.contains(".claude and 2 accounts") && line.contains("Codex in "), "{line}");
        assert_eq!(origin_tag(&s.origin), "tstack");
        assert_eq!(origin_tag(&Origin::Repo("o/r#skills/x".into())), "Skill Issue");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!((since(now), since(now - 600), since(now - 3 * 86_400)), ("just now".into(), "10 min ago".into(), "3 days ago".into()));
    }

    #[test]
    fn a_repo_skill_is_installed_disabled_or_waiting() {
        let skill = |name: &str, origin: Origin, project: Option<&str>| Skill { name: name.into(), description: None, origin, project: project.map(PathBuf::from), places: vec![Place { agent: Agent::Claude, dir: PathBuf::from("/h/.claude/skills").join(name) }] };
        let tstack = || Origin::Repo("t3dotgg/tstack".into());
        let inv = Inventory {
            skills: vec![skill("council", tstack(), None), skill("council", tstack(), Some("/code/app")), skill("file-pr", Origin::Yours, None), skill("full-send", tstack(), Some("/code/app"))],
            disabled: vec![Disabled { id: "sol-loop".into(), name: "sol-loop".into(), description: None, origin: tstack(), project: None, places: vec![], at: 0 }],
        };
        let council = repo_status("t3dotgg/tstack", "council", &inv);
        assert_eq!((council.everywhere, council.projects.clone(), council.installed(), council.tickable()), (Some(0), vec![1], true, false));
        assert_eq!(repo_status("T3dotgg/tstack", "full-send", &inv).projects, [3]);
        let sol = repo_status("t3dotgg/tstack", "sol-loop", &inv);
        assert!(sol.disabled == Some(0) && !sol.installed() && !sol.tickable());
        // a skill of yours with its name: still to pick, but it'd stay out of the homes
        let file_pr = repo_status("t3dotgg/tstack", "file-pr", &inv);
        assert!(file_pr.taken == Some(2) && file_pr.tickable());
        assert!(repo_status("t3dotgg/tstack", "takeover", &inv).tickable());
    }

    #[test]
    fn a_menu_steps_over_its_headings_and_asks_before_deleting() {
        let d = Disabled { id: "x".into(), name: "x".into(), description: None, origin: Origin::Yours, project: None, places: vec![], at: 0 };
        let mut m = Menu::new(
            "x",
            vec![MenuItem::head("first"), MenuItem::new("Enable", MenuIcon::Enable, Some("e"), MenuAct::Enable(Box::new(d.clone()))), MenuItem::head(""), MenuItem::new("Delete forever", MenuIcon::Delete, Some("x"), MenuAct::Delete(Box::new(d), false)).danger()],
            None,
        );
        assert_eq!(m.sel, 1, "the first item that does something");
        m.step(1);
        assert_eq!(m.sel, 3);
        m.step(1);
        assert_eq!(m.sel, 1, "round past the end");
        m.confirm = Some(3);
        assert!(m.label(3).contains("Again to confirm") && m.label(1) == "Enable");
    }
}
