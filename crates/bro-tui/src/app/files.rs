//! File links: the preview overlay for a clicked path or an agent's `bro open`, its actions (editor, system app,
//! copy, mention), and remote "open on desktop" requests.

use super::App;
use super::overlays::Overlay;
use crate::alerts::Kind;
use crate::editor::{EditorPane, Plan};
use crate::file_view::{FileView, Loaded, Outcome};
use crate::layout::PaneId;
use crate::pane::{Event, Place};
use crate::panes::term::{Meta, Spawn, Term};
use bro_bridge::file_preview::ResolveRoots;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The last file an agent opened (`bro open`), for prefix f when it arrived in another session.
#[derive(Clone, Debug)]
pub(crate) struct Opened {
    pub pane: PaneId,
    pub path: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// `src/app.rs` relative to the session when it's inside, else the full path.
fn short(path: &Path, roots: &ResolveRoots) -> String {
    let inside = [Some(roots.cwd.as_path()), roots.project.as_deref()].into_iter().flatten().find_map(|root| {
        let root = crate::util::strip_verbatim(root.to_path_buf());
        path.strip_prefix(&root).ok().map(|r| r.to_string_lossy().replace('\\', "/")).filter(|r| !r.is_empty())
    });
    inside.unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn located(name: &str, line: Option<u32>) -> String {
    match line {
        Some(l) => format!("{name}:{l}"),
        None => name.to_string(),
    }
}

impl App {
    fn roots_for(&self, pane: PaneId) -> Option<ResolveRoots> {
        self.panes.get(&pane).and_then(|p| p.as_term_ref()).map(|t| t.file_roots())
    }

    /// A `file:` link in a terminal was clicked: preview it (links bro can't preview go to the OS as before).
    pub(crate) fn open_file_link(&mut self, pane: PaneId, uri: &str) {
        let (Some((path, line, column)), Some(roots)) = (crate::panes::links::parse_file_uri(uri), self.roots_for(pane)) else {
            let _ = self.tx.send(Event::OpenLink(uri.to_string()));
            return;
        };
        self.open_file_view(Some(pane), roots, path.to_string_lossy().into_owned(), line, column, Some(uri.to_string()));
    }

    pub(crate) fn open_file_view(&mut self, pane: Option<PaneId>, roots: ResolveRoots, target: String, line: Option<u32>, column: Option<u32>, fallback: Option<String>) {
        self.sel = None;
        self.overlay = Overlay::File(Box::new(FileView::open(pane, roots, target, line, column, fallback, &self.theme, self.tx.clone())));
    }

    pub(crate) fn file_view_loaded(&mut self, loaded: Loaded) {
        let Overlay::File(v) = &mut self.overlay else { return };
        let o = v.apply(loaded);
        self.file_view_outcome(o);
    }

    pub(crate) fn file_view_outcome(&mut self, o: Outcome) {
        let Overlay::File(v) = &self.overlay else { return };
        let path = v.path().map(Path::to_path_buf);
        let (pane, line, column, mention) = (v.pane, v.line, v.column, v.mention().map(str::to_string));
        match o {
            Outcome::None => {}
            Outcome::ToggleRendered => {
                if let Overlay::File(v) = &mut self.overlay {
                    v.toggle_rendered();
                }
            }
            Outcome::Close => self.overlay = Overlay::None,
            Outcome::Fallback(uri) => {
                self.overlay = Overlay::None;
                let _ = self.tx.send(Event::OpenLink(uri));
            }
            Outcome::Copy => {
                if let Some(p) = path {
                    let text = p.to_string_lossy().into_owned();
                    crate::clip::copy(&text);
                    self.toast(Kind::Info, format!("copied {text}"));
                }
            }
            Outcome::Mention => {
                let Some(mention) = mention else { return };
                match pane.filter(|p| self.panes.contains_key(p)) {
                    Some(p) => {
                        self.overlay = Overlay::None;
                        self.focus_pane(p);
                        self.leave_sidebar();
                        self.with_pane(p, |pane, cx| pane.paste(&format!("{mention} "), cx));
                    }
                    None => self.toast(Kind::Info, "that session has closed"),
                }
            }
            Outcome::Editor => {
                if let Some(p) = path {
                    self.overlay = Overlay::None;
                    self.open_in_editor(pane, p, line, column);
                }
            }
            Outcome::System => {
                if let Some(uri) = path.as_deref().and_then(|p| crate::panes::links::file_uri(p, None, None)) {
                    self.overlay = Overlay::None;
                    let _ = self.tx.send(Event::OpenLink(uri));
                }
            }
        }
    }

    /// Open `path` at `line` in your editor. Deciding how (PATH lookups) and starting a GUI editor happen on a
    /// worker; a terminal editor comes back as [`Event::EditorPane`].
    pub(crate) fn open_in_editor(&mut self, pane: Option<PaneId>, path: PathBuf, line: Option<u32>, column: Option<u32>) {
        let cwd = pane.and_then(|p| self.roots_for(p)).map(|r| r.cwd).or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
        if cfg!(test) {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let result = crate::services::guard("editor::open", || match crate::editor::plan_here(&path, line, column) {
                Plan::Gui { program, args } => crate::editor::spawn_gui(&program, &args).map(|()| Some(format!("opened {} in {}", located(&name, line), program_name(&program)))),
                Plan::Terminal { program, args } => {
                    let _ = tx.send(Event::EditorPane(Box::new(EditorPane { pane, cwd, program, args, file: name.clone() })));
                    Ok(None)
                }
                Plan::System => crate::panes::links::file_uri(&path, None, None).ok_or_else(|| "unsupported path".to_string()).and_then(|uri| crate::panes::links::open(&uri)).map(|()| None),
            });
            match result {
                Ok(Ok(Some(note))) => {
                    let _ = tx.send(Event::Toast(Kind::Info, note));
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) | Err(e) => {
                    let _ = tx.send(Event::Toast(Kind::Error, e));
                }
            }
        });
    }

    /// A terminal editor: a pane split beside the session it was opened from.
    pub(crate) fn editor_pane(&mut self, e: EditorPane) {
        let label = program_name(&e.program);
        let meta = Meta { sid: uuid::Uuid::new_v4().to_string(), harness: None, profile: None, store: None, model: None, label: label.clone(), name: Some(format!("{label} {}", e.file)), renamed: false, project: self.svc.project_for(&e.cwd), cwd: e.cwd, started: Instant::now(), route_id: None, cleanup: vec![], orchestrator: false };
        let term = Term::new(meta, Spawn { program: e.program, args: e.args, ..Spawn::default() }, self.svc.clone());
        if let Some(p) = e.pane.filter(|p| self.panes.contains_key(p)) {
            self.focus_pane(p);
        }
        self.leave_sidebar();
        let place = if self.tabs.is_empty() { Place::Tab } else { Place::Split };
        self.open(Box::new(term), place);
    }

    /// The bridge says an agent (`bro open`) or a remote client opened a file in session `sid`: show it when
    /// you're looking at that session, else say so (prefix f shows it).
    pub(crate) fn agent_opened_file(&mut self, sid: &str, path: PathBuf, line: Option<u32>, column: Option<u32>) {
        let Some(pane) = self.term_id(sid) else { return };
        let Some(roots) = self.roots_for(pane) else { return };
        self.last_opened = Some(Opened { pane, path: path.clone(), line, column });
        let watching = self.focused() == Some(pane) && self.visible().contains(&pane) && !self.side_focus && self.renaming.is_none() && matches!(self.overlay, Overlay::None | Overlay::File(_));
        if watching {
            self.open_file_view(Some(pane), roots, path.to_string_lossy().into_owned(), line, column, None);
            return;
        }
        let who = self.panes.get(&pane).map(|p| p.title()).unwrap_or_default();
        let key = self.keymap.primary(crate::keymap::Act::OpenedFile);
        let view = if key.is_empty() { String::new() } else { format!(" · {key} to view") };
        self.toast(Kind::Info, format!("{who} opened {}{view}", located(&short(&path, &roots), line)));
    }

    /// prefix F: the focused agent's files (sent to you, touched).
    pub(crate) fn show_agent_files(&mut self) {
        let Some(pane) = self.focused() else { return };
        let Some(t) = self.panes.get(&pane).and_then(|p| p.as_term_ref()) else {
            self.toast(Kind::Info, "agent files: focus an agent session first");
            return;
        };
        let roots = t.file_roots();
        let snap = t.insights();
        let items = crate::agent_files::items(snap.as_ref(), &roots);
        let empty = match (t.meta.harness, &snap) {
            (Some(bro_core::Harness::Claude | bro_core::Harness::Codex), Some(s)) if s.transcript.is_some() => "nothing sent or touched yet".to_string(),
            (Some(bro_core::Harness::Claude | bro_core::Harness::Codex), _) => "waiting for the agent's transcript (it appears once the agent has answered)".to_string(),
            (Some(h), _) => format!("{} sessions don't report their files", h.label()),
            (None, _) => "a shell has no agent files".to_string(),
        };
        self.sel = None;
        self.overlay = Overlay::AgentFiles(Box::new(crate::agent_files::AgentFilesView::new(pane, roots, t.display_name(), items, empty)));
    }

    pub(crate) fn agent_files_outcome(&mut self, o: crate::agent_files::Outcome) {
        let Overlay::AgentFiles(v) = &self.overlay else { return };
        match o {
            crate::agent_files::Outcome::None => {}
            crate::agent_files::Outcome::Close => self.overlay = Overlay::None,
            crate::agent_files::Outcome::Open(i) => {
                let Some(it) = v.items.get(i).cloned() else { return };
                let (pane, roots) = (v.pane, v.roots.clone());
                let pane = self.panes.contains_key(&pane).then_some(pane);
                self.open_file_view(pane, roots, it.path.to_string_lossy().into_owned(), it.line, None, None);
            }
        }
    }

    /// prefix f: the file an agent last opened.
    pub(crate) fn show_last_opened(&mut self) {
        let Some(o) = self.last_opened.clone() else {
            self.toast(Kind::Info, "no file opened yet · agents open files with `bro open path:line`");
            return;
        };
        let Some(roots) = self.roots_for(o.pane) else {
            self.toast(Kind::Info, "that session has closed");
            return;
        };
        self.open_file_view(Some(o.pane), roots, o.path.to_string_lossy().into_owned(), o.line, o.column, None);
    }
}

fn program_name(program: &str) -> String {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    [".exe", ".cmd", ".bat"].iter().find_map(|e| name.strip_suffix(e)).unwrap_or(name).to_string()
}
