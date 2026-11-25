//! Project icons for the phone. bro's window keeps each project's icon as a small PNG in `~/.bro/project-icons/`
//! with an index ([`bro_core::project_icon::Kept`]); the bridge reads that index, never the projects themselves,
//! and tells clients what each project shows: `icon` (a versioned path under [`ROUTE`]), `iconTone` (what the
//! picture needs behind it: "symbol" is a drawn white symbol, set on a tile in the project's colour) and `hue` (that
//! colour, for its tile or its letter's). It serves the pictures too.
//!
//! Everything falls back to the letter tile. If the index is missing or won't parse, a project isn't in it, or its
//! picture is gone or too big, the project has no icon. The index is checked off the async threads every few
//! seconds and read again only when it changed. Building the project list costs one map lookup per project, and
//! nothing waits on the window.

use crate::model::TerminalProject;
use crate::state::AppState;
use axum::extract::{Path as UrlPath, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use bro_core::project_icon::{self, Kept};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where clients fetch a picture: `/api/project-icons/<file>?v=<version>`.
pub(crate) const ROUTE: &str = "/api/project-icons";
/// How often the index is checked for a change (a stat; it's read again only when it changed).
const EVERY: Duration = Duration::from_secs(3);
/// The largest picture served: the window keeps 72 px ones, a few KB each.
const MAX_BYTES: u64 = 1 << 20;
/// A picture still reading after this isn't served: the phone shows the letter instead.
const READ_LIMIT: Duration = Duration::from_secs(5);

/// What a client gets for a folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Look {
    pub icon: Option<String>,
    pub icon_tone: Option<String>,
    pub hue: Option<u16>,
}

/// One project's picture as the window keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shown {
    file: String,
    /// changes whenever the picture does, so a phone never shows a stale copy it cached
    version: String,
    tone: Option<String>,
}

#[derive(Default)]
struct Seen {
    /// the index as last read: when it changed, and its length
    index: Option<(SystemTime, u64)>,
    /// by project key
    shown: HashMap<String, Shown>,
}

pub(crate) struct ProjectIcons {
    dir: PathBuf,
    seen: Mutex<Seen>,
}

impl ProjectIcons {
    /// The window's kept icons for a bridge keeping its data in `data_root`: bro keeps it in `~/.bro/bridge`, next
    /// to `~/.bro/project-icons`.
    pub fn new(data_root: &Path) -> Self {
        let home = if data_root.file_name().is_some_and(|name| name == "bridge") { data_root.parent().unwrap_or(data_root) } else { data_root };
        ProjectIcons { dir: home.join(project_icon::KEPT_DIR), seen: Mutex::default() }
    }

    /// Read the index again if it changed. Blocking: a stat, then a small read only when it changed. True when
    /// what the projects show changed.
    pub fn refresh(&self) -> bool {
        let stat = std::fs::metadata(project_icon::kept_index(&self.dir)).ok().map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()));
        if self.seen.lock().index == stat {
            return false;
        }
        let shown: HashMap<String, Shown> = project_icon::read_kept(&self.dir)
            .into_iter()
            .filter_map(|(key, kept)| {
                let file = kept.picture()?.to_owned();
                Some((key, Shown { version: version(&kept), tone: kept.tone, file }))
            })
            .collect();
        let mut seen = self.seen.lock();
        seen.index = stat;
        if seen.shown == shown {
            return false;
        }
        seen.shown = shown;
        true
    }

    /// What a client shows for the project in `cwd`. Path arithmetic and a lookup only, no disk.
    pub fn look(&self, cwd: &str) -> Look {
        let cwd = cwd.trim();
        if cwd.is_empty() {
            return Look::default();
        }
        let key = bro_core::projects::key_for_root(Path::new(cwd)).key;
        let hue = Some(bro_core::projects::hue(&key).round() as u16 % 360);
        match self.seen.lock().shown.get(&key) {
            Some(s) => Look { icon: Some(format!("{ROUTE}/{}?v={}", s.file, s.version)), icon_tone: s.tone.clone(), hue },
            None => Look { hue, ..Look::default() },
        }
    }

    /// Give each project what it shows.
    pub fn decorate(&self, projects: &mut [TerminalProject]) {
        for project in projects {
            let look = self.look(&project.cwd);
            project.icon = look.icon;
            project.icon_tone = look.icon_tone;
            project.hue = look.hue;
        }
    }

    /// The kept picture `name`, while a project shows it.
    fn file(&self, name: &str) -> Option<PathBuf> {
        let shows = project_icon::is_kept_name(name) && self.seen.lock().shown.values().any(|s| s.file == name);
        shows.then(|| self.dir.join(name))
    }
}

/// A short fingerprint of what's kept: a new picture, source, tone or fitting gives a new one.
fn version(kept: &Kept) -> String {
    let text = format!("{}|{:?}|{}|{:?}|{:?}|{}", kept.kind, kept.source, kept.stamp, kept.file, kept.tone, kept.fit);
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{:08x}", hash >> 32)
}

/// While the bridge runs: notice the window keeping a new icon, and send clients the projects again.
pub(crate) async fn watch(state: AppState) {
    let mut stop = state.shutdown.subscribe();
    loop {
        let s = state.clone();
        // a slow disk only delays the next check; it never holds a request
        let _ = tokio::task::spawn_blocking(move || {
            if s.icons.refresh() {
                s.reconcile_projects();
            }
        })
        .await;
        tokio::select! {
            _ = tokio::time::sleep(EVERY) => {}
            _ = stop.changed() => return,
        }
    }
}

/// `GET /api/project-icons/{file}`: a kept picture. Not found when no project shows it (the phone shows the letter).
pub(crate) async fn picture(State(state): State<AppState>, UrlPath(name): UrlPath<String>) -> Response {
    let Some(path) = state.icons.file(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let read = async {
        let meta = tokio::fs::metadata(&path).await.ok()?;
        if !meta.is_file() || meta.len() > MAX_BYTES {
            return None;
        }
        tokio::fs::read(&path).await.ok()
    };
    match tokio::time::timeout(READ_LIMIT, read).await {
        Ok(Some(bytes)) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                // the URL changes with the picture
                (header::CACHE_CONTROL, "private, max-age=604800, immutable"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep(dir: &Path, index: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(project_icon::kept_index(dir), index).unwrap();
    }

    #[test]
    fn projects_show_what_the_window_kept_and_otherwise_their_letter() {
        let home = tempfile::tempdir().unwrap();
        let icons = ProjectIcons::new(&home.path().join("bridge"));
        let dir = home.path().join(project_icon::KEPT_DIR);
        let root = home.path().join("Site");
        let cwd = root.to_string_lossy().into_owned();
        let key = bro_core::projects::key_for_root(&root).key;

        // nothing kept yet: a letter in the window's own hue
        assert!(!icons.refresh());
        let letter = icons.look(&cwd);
        assert_eq!(letter.icon, None);
        assert_eq!(letter.hue, Some(bro_core::projects::hue(&key).round() as u16 % 360));
        assert_eq!(icons.look("  "), Look::default());

        // the window keeps a picture: the project shows it, versioned
        let kept = |stamp: u64| format!(r#"{{ {key:?}: {{ "kind": "found", "source": "x.svg", "stamp": {stamp}, "file": "00000000000000ab.png", "tone": "dark" }} }}"#);
        keep(&dir, &kept(1));
        std::fs::write(dir.join("00000000000000ab.png"), b"png").unwrap();
        assert!(icons.refresh());
        let first = icons.look(&cwd);
        let url = first.icon.clone().expect("an icon");
        assert!(url.starts_with("/api/project-icons/00000000000000ab.png?v="), "{url}");
        assert_eq!(first.icon_tone.as_deref(), Some("dark"));
        assert_eq!(icons.file("00000000000000ab.png"), Some(dir.join("00000000000000ab.png")));
        // nothing changed: nothing to send
        assert!(!icons.refresh());

        // a new picture gets a new address, so a phone's cached copy isn't shown
        keep(&dir, &format!("{}\n", kept(2)));
        assert!(icons.refresh());
        assert_ne!(icons.look(&cwd).icon, first.icon);

        // a picture nobody shows, or a name that isn't bro's own, is never served
        assert_eq!(icons.file("00000000000000cd.png"), None);
        assert_eq!(icons.file("../index.json"), None);

        // the index broken or gone: back to letters
        keep(&dir, "{ broken");
        assert!(icons.refresh());
        assert_eq!(icons.look(&cwd).icon, None);
        assert_eq!(icons.file("00000000000000ab.png"), None);
    }

    #[test]
    fn projects_are_decorated_in_place() {
        let home = tempfile::tempdir().unwrap();
        let icons = ProjectIcons::new(&home.path().join("bridge"));
        let mut projects = vec![TerminalProject { id: "a".into(), name: "A".into(), cwd: home.path().join("a").to_string_lossy().into_owned(), ..TerminalProject::default() }];
        icons.decorate(&mut projects);
        assert!(projects[0].hue.is_some());
        assert_eq!(projects[0].icon, None);
        let json = serde_json::to_value(&projects[0]).unwrap();
        assert!(json.get("hue").is_some() && json.get("icon").is_none() && json.get("iconTone").is_none(), "{json}");
    }
}
