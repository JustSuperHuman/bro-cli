//! Safe session context for the independently hosted, genuinely moving Orbit.
use super::App;
use crate::pane::Activity;
use crate::panes::term::Term;
use serde_json::json;

// A missing window suspends containment; it never grants desktop permission.
fn placement(allow_escape: bool) -> (&'static str, Option<serde_json::Value>) {
    ("attached", allow_escape.then(|| json!({"schema":1,"policy":"auto"})))
}

pub(super) fn generation(term: &Term) -> String {
    term.orbit_generation()
}

impl App {
    pub(crate) fn publish_orbit_context(&self) {
        let Some(handle) = self.svc.orbit_handle() else {
            return;
        };
        let settings = self.svc.settings().orbit_voice;
        if !settings.enabled {
            return;
        }
        let focused = self
            .focused()
            .and_then(|id| self.panes.get(&id))
            .and_then(|pane| pane.as_term_ref())
            .map(|term| term.meta.sid.clone());
        let sessions: Vec<_> = self
            .panes
            .values()
            .filter_map(|pane| {
                let term = pane.as_term_ref()?;
                let activity = match pane.activity() {
                    Some(Activity::Working) => "working",
                    Some(Activity::Blocked) => "waiting",
                    _ => "idle",
                };
                let context = term
                    .insights()
                    .and_then(|snapshot| snapshot.facts)
                    .and_then(|facts| {
                        facts
                            .reply
                            .as_ref()
                            .map(|reply| reply.text.chars().take(4000).collect::<String>())
                    });
                Some(json!({
                    "id": term.meta.sid,
                    "generation": generation(term),
                    "scope": bro_core::orbit_voice::project_scope_id(&term.meta.project.root),
                    "title": term.display_name(),
                    "agent": term.meta.harness.map(|h| h.label()),
                    "activity": activity,
                    "context": context,
                    "orchestrator": term.meta.orchestrator,
                }))
            })
            .take(128)
            .collect();
        #[cfg(windows)]
        let window = {
            let hwnd = crate::instance::APP_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
            (hwnd != 0).then(|| json!({"handle": hwnd.to_string(), "pid": std::process::id()}))
        };
        #[cfg(not(windows))]
        let window: Option<serde_json::Value> = None;
        let (mode, roam) = placement(settings.allow_escape);
        self.svc.publish_orbit_host(json!({
            "protocol": 1,
            "instanceId": handle.instance_id(),
            "name": "bro",
            "capabilities": ["sessions.read", "sessions.focus", "orchestrator.submit", "requests.observe", "requests.cancel-queued", "avatar.desktop", "avatar.attached"],
            "mode": mode,
            "avatarRoam": roam,
            "avatarPlacement": {"schema":1,"allowEscape":settings.allow_escape},
            "window": window,
            "focusedSessionId": focused,
            "sessions": sessions,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment_is_default_and_only_explicit_permission_grants_escape() {
        assert_eq!(placement(false), ("attached", None));
        assert_eq!(placement(true), ("attached", Some(json!({"schema":1,"policy":"auto"}))));
        // Native window availability is deliberately not a permission input.
        for legacy in ["auto", "attached", "desktop", "unknown"] {
            let settings: bro_core::config::OrbitSettings = serde_json::from_value(json!({"mode":legacy})).unwrap();
            assert_eq!(placement(settings.allow_escape), ("attached", None));
        }
    }
}
