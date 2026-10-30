//! `GET /api/media[?session=<id>]`: the phone's gallery, as the desktop app's media panel has it. The pictures,
//! videos, sounds and PDFs the sessions' agents sent you, made or looked at, the ones opened for you (`bro open`),
//! the ones you attached from the phone or pasted in, and the ones a reply names by its path. Newest first, once
//! each, only files still on disk; one session's, or every session's.
//!
//! Built from what bro already hands the bridge (each session's [`crate::AgentFiles`]), what the bridge saw itself
//! (attachments, opens: [`Noted`]) and each session's conversation, read from its transcript as `/conversation`
//! reads it. The same facts decide which media outside a session's folders clients may open at all ([`Access`]),
//! so everything listed here streams from `/raw`.

use crate::conversation::{Feeds, harness_of};
use crate::file_preview::{image_mime, is_media, media_mime};
use crate::insights::{AgentFiles, TouchKind};
use crate::rest::api_error;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bro_core::sessions::conversation::{Conversation, Item};
use bro_core::util::path_key;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::UNIX_EPOCH;

/// Most items one answer lists.
pub(crate) const MOST: usize = 300;
/// Notes kept per session.
const NOTED: usize = 100;

/// Why a file is in the gallery. Lower is a stronger reason: an entry shows its strongest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Source {
    /// `bro open`, or a client asked to show it
    Opened,
    /// the agent sent it to you (Claude's SendUserFile), or its reply names it
    Sent,
    /// the agent wrote, created, edited or moved it
    Made,
    /// the agent looked at it
    Viewed,
    /// you attached it from the phone
    Attached,
    /// it went with something you wrote (pasted, dropped)
    Pasted,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Opened => "opened",
            Source::Sent => "sent",
            Source::Made => "made",
            Source::Viewed => "viewed",
            Source::Attached => "attached",
            Source::Pasted => "pasted",
        }
    }
}

/// Something the bridge itself saw happen to a media file in a session.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Noted {
    pub path: PathBuf,
    pub source: Source,
    pub at_ms: i64,
}

/// One session's side of the gallery, read under the lock.
pub(crate) struct Sources {
    id: String,
    cwd: PathBuf,
    agent: Option<String>,
    files: AgentFiles,
    noted: Vec<Noted>,
}

#[derive(Deserialize)]
pub(crate) struct MediaQuery {
    session: Option<String>,
}

pub(crate) async fn media(Query(query): Query<MediaQuery>, State(state): State<AppState>) -> Response {
    let Some(sources) = state.media_sources(query.session.as_deref()) else {
        return api_error(StatusCode::NOT_FOUND, "Unknown terminal session.");
    };
    let feeds = state.conversations.clone();
    match tokio::task::spawn_blocking(move || list(&feeds, &sources)).await {
        Ok(items) => Json(json!({ "items": items })).into_response(),
        Err(_) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "The media could not be listed."),
    }
}

impl AppState {
    /// Note that `path` was attached or opened in a session (media only), for the gallery.
    pub(crate) fn note_media(&self, session_id: &str, path: PathBuf, source: Source) {
        if !is_media(&path) {
            return;
        }
        let mut inner = self.inner.lock();
        let Some(session) = inner.sessions.get_mut(session_id) else {
            return;
        };
        let key = path_key(&path);
        session.media.retain(|n| path_key(&n.path) != key);
        session.media.push(Noted { path, source, at_ms: chrono::Utc::now().timestamp_millis() });
        if session.media.len() > NOTED {
            session.media.remove(0);
        }
    }

    /// What the gallery is made from: `session`'s, or (None) every session's. None for an unknown session.
    pub(crate) fn media_sources(&self, session: Option<&str>) -> Option<Vec<Sources>> {
        let inner = self.inner.lock();
        let of = |id: &String, s: &crate::session::Session| Sources {
            id: id.clone(),
            cwd: s.summary.cwd.clone().into(),
            agent: s.summary.agent.clone().or_else(|| s.agent_hint.map(|a| a.id().to_owned())),
            files: s.agent_files.clone(),
            noted: s.media.clone(),
        };
        match session {
            Some(id) => inner.sessions.get_key_value(id).map(|(id, s)| vec![of(id, s)]),
            None => Some(inner.sessions.iter().map(|(id, s)| of(id, s)).collect()),
        }
    }
}

/// The media a conversation names, with when: pictures you sent with a message, files a reply names by their
/// path (`~/` expanded; a relative one is the caller's to place).
pub(crate) fn named_in(conversation: &Conversation) -> Vec<(PathBuf, Source, Option<i64>)> {
    let mut out = vec![];
    for item in &conversation.items {
        match item.as_ref() {
            Item::You { pictures, at_ms, .. } => out.extend(pictures.iter().map(|p| (PathBuf::from(p), Source::Pasted, *at_ms))),
            Item::Said { text, at_ms } => {
                for link in crate::file_links::find_file_links(text) {
                    let raw = link.path.trim().trim_start_matches("file:///");
                    let path = match raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
                        Some(rest) => bro_core::util::home_dir().join(rest),
                        None => PathBuf::from(raw),
                    };
                    if is_media(&path) {
                        out.push((path, Source::Sent, *at_ms));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Which media outside a session's own folders clients may open ([`crate::file_preview::resolve_media`]):
/// anything when bro lets media open from anywhere ([`crate::Bridge::set_media_anywhere`]), else only what's
/// tied to the work: in any session's folders, a folder bro knows you work in, or the phone's uploads; a file
/// an agent sent, made or looked at, or one attached or opened here; one a conversation names.
pub(crate) struct Access {
    anywhere: bool,
    /// as [`path_key`]s
    folders: Vec<String>,
    files: HashSet<String>,
    /// conversations that may name more: (session, transcript, harness)
    talks: Vec<(String, PathBuf, bro_core::Harness)>,
    feeds: Arc<Feeds>,
}

/// `key` is in `folder`, or is it (both [`path_key`]s).
fn under(key: &str, folder: &str) -> bool {
    !folder.is_empty() && key.strip_prefix(folder).is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || folder.ends_with('/'))
}

impl Access {
    /// May `path` (canonical) open? Blocking: a conversation not read yet is read.
    pub(crate) fn allows(&self, path: &Path) -> bool {
        if self.anywhere {
            return true;
        }
        let key = path_key(path);
        self.files.contains(&key)
            || self.folders.iter().any(|folder| under(&key, folder))
            || self.talks.iter().any(|(id, transcript, harness)| self.feeds.names(id, transcript, *harness, &key))
    }
}

impl AppState {
    /// What media clients may open right now ([`Access`]), read under the lock.
    pub(crate) fn media_access(&self) -> Access {
        let feeds = self.conversations.clone();
        if self.media_anywhere.load(Ordering::Relaxed) {
            return Access { anywhere: true, folders: vec![], files: HashSet::new(), talks: vec![], feeds };
        }
        let mut folders: Vec<String> = self.known_folders().iter().map(|f| path_key(Path::new(f))).collect();
        folders.push(path_key(&crate::rest_sessions::attachment_directory()));
        let mut files = HashSet::new();
        let mut talks = vec![];
        let inner = self.inner.lock();
        for (id, s) in &inner.sessions {
            let cwd = PathBuf::from(&s.summary.cwd);
            folders.extend((!s.summary.cwd.is_empty()).then(|| path_key(&cwd)));
            folders.extend(s.project_dir.as_deref().map(|p| path_key(Path::new(p))));
            let mut add = |p: &Path| {
                files.insert(path_key(&if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }));
            };
            s.agent_files.sent.iter().for_each(|f| add(&f.path));
            s.agent_files.touched.iter().filter(|f| f.kind != TouchKind::Delete).for_each(|f| add(&f.path));
            s.media.iter().for_each(|n| add(&n.path));
            if let Some(transcript) = &s.agent_files.transcript {
                let agent = s.summary.agent.clone().or_else(|| s.agent_hint.map(|a| a.id().to_owned()));
                talks.push((id.clone(), transcript.clone(), harness_of(agent.as_deref(), transcript)));
            }
        }
        Access { anywhere: false, folders, files, talks, feeds }
    }
}

/// A file as the gallery has it, before it's checked on disk.
struct Entry {
    path: PathBuf,
    source: Source,
    at_ms: Option<i64>,
    caption: Option<String>,
    session: String,
}

fn iso_ms(at: Option<&str>) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at?).ok().map(|d| d.timestamp_millis())
}

/// The gallery's items, newest first. Blocking: reads transcripts and checks each file on disk.
pub(crate) fn list(feeds: &Feeds, sources: &[Sources]) -> Vec<Value> {
    let mut found: Vec<Entry> = vec![];
    for s in sources {
        let mut add = |path: &Path, source: Source, at_ms: Option<i64>, caption: Option<String>| {
            let path = if path.is_absolute() { path.to_path_buf() } else { s.cwd.join(path) };
            if is_media(&path) {
                found.push(Entry { path, source, at_ms, caption, session: s.id.clone() });
            }
        };
        for f in &s.files.sent {
            add(&f.path, Source::Sent, iso_ms(f.at.as_deref()), f.caption.clone());
        }
        for f in &s.files.touched {
            let source = match f.kind {
                TouchKind::Delete => continue,
                TouchKind::Read => Source::Viewed,
                TouchKind::Edit | TouchKind::Write | TouchKind::Create | TouchKind::Move => Source::Made,
            };
            add(&f.path, source, iso_ms(f.at.as_deref()), None);
        }
        for n in &s.noted {
            add(&n.path, n.source, Some(n.at_ms), None);
        }
        let Some(transcript) = &s.files.transcript else { continue };
        let (_, conversation) = feeds.read(&s.id, transcript, harness_of(s.agent.as_deref(), transcript));
        for (path, source, at_ms) in named_in(&conversation) {
            add(&path, source, at_ms, None);
        }
    }

    // once each: the newest time, the strongest reason, a caption when any had one
    let mut merged: Vec<Entry> = vec![];
    let mut at: HashMap<String, usize> = HashMap::new();
    for e in found {
        let key = path_key(&e.path);
        let Some(&i) = at.get(&key) else {
            at.insert(key, merged.len());
            merged.push(e);
            continue;
        };
        let m = &mut merged[i];
        m.source = m.source.min(e.source);
        if m.caption.is_none() {
            m.caption = e.caption;
        }
        if e.at_ms > m.at_ms {
            m.at_ms = e.at_ms;
            m.session = e.session;
        }
    }

    let mut items: Vec<(i64, Value)> = merged
        .into_iter()
        .filter_map(|e| {
            let meta = std::fs::metadata(&e.path).ok().filter(|m| m.is_file())?;
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64);
            let at_ms = e.at_ms.or(modified).unwrap_or_default();
            let extension = e.path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            let (kind, mime) = match (image_mime(&extension), media_mime(&extension)) {
                (Some(mime), _) => ("image", mime),
                (_, Some(mime)) if mime.starts_with("video/") => ("video", mime),
                (_, Some(mime)) => ("audio", mime),
                _ => ("pdf", "application/pdf"),
            };
            let mut item = json!({
                "path": e.path.to_string_lossy(),
                "name": e.path.file_name().unwrap_or_default().to_string_lossy(),
                "kind": kind,
                "mimeType": mime,
                "source": e.source.as_str(),
                "at": chrono::DateTime::from_timestamp_millis(at_ms)
                    .unwrap_or_default()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "bytes": meta.len(),
                "sessionId": e.session,
            });
            if let Some(caption) = e.caption {
                item["caption"] = json!(caption);
            }
            Some((at_ms, item))
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    items.truncate(MOST);
    items.into_iter().map(|(_, item)| item).collect()
}
