//! Watchdogs in the app: the overlay (prefix w) and the prompt editor, waking a session's watchdog when its
//! trigger fires, and telling you what the reviewers did.

use super::App;
use super::overlays::Overlay;
use crate::alerts::Kind;
use crate::layout::PaneId;
use crate::pane::Activity;
use crate::watchdog::prompt_editor::{self, PromptEditor};
use crate::watchdog::view::{Out, WatchdogView};
use crate::watchdog::{Seen, Trigger, pick_reviewer, reviewer_launch};
use bro_bridge::{WatchdogUpdate, WatchdogWake, WatchedSession};
use bro_core::config::WatchdogSettings;
use crossterm::event::KeyEvent;
use std::time::Instant;

impl App {
    /// The name a session goes by in watchdog toasts.
    fn watched_name(&self, pane: PaneId) -> String {
        self.panes.get(&pane).and_then(|p| p.as_term_ref()).map(|t| crate::ui::fit(&t.display_name(), 32)).unwrap_or_else(|| "a session".into())
    }

    /// "claude · home · opus · 72% left": who reviews if the watchdog woke now.
    fn reviewer_hint(&self, cfg: &WatchdogSettings) -> String {
        let st = self.svc.state();
        match st.profiles.ready().and_then(|ps| pick_reviewer(ps, &st.usage, cfg)) {
            Some((p, left)) => {
                let label = reviewer_launch(p, cfg).label;
                match left {
                    Some(l) => format!("{label} · {l:.0}% left"),
                    None => label,
                }
            }
            None => "no signed-in login has usage left".into(),
        }
    }

    /// Prefix w: the focused session's watchdog (its status page), or the form to start one.
    pub(crate) fn open_watchdog(&mut self) {
        let Some(id) = self.focused() else {
            self.toast(Kind::Info, "focus an agent session first: a watchdog keeps one session going");
            return;
        };
        let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()).filter(|t| t.meta.harness.is_some()) else {
            self.toast(Kind::Info, "watchdogs watch agent sessions: focus a Claude or Codex session and try again");
            return;
        };
        let (sid, title, agent) = (t.meta.sid.clone(), crate::ui::fit(&t.display_name(), 48), t.meta.label.clone());
        // the copy names it when it has a name of its own ("Mal"), not "claude · work · opus"
        let name = t.display_title().map(|n| crate::ui::fit(&n, 28));
        let cfg = self.svc.settings().watchdog;
        let dog = self.svc.watchdog(&sid);
        let reviewer = self.reviewer_hint(&cfg);
        self.overlay = Overlay::Watchdog(Box::new(WatchdogView::new(id, sid, title, name, agent, dog.as_ref(), &cfg, reviewer)));
    }

    pub(crate) fn open_watchdog_prompt(&mut self) {
        let current = self.svc.settings().watchdog.prompt;
        self.overlay = Overlay::WatchdogPrompt(Box::new(PromptEditor::new(current.as_deref())));
    }

    pub(super) fn watchdog_key(&mut self, k: KeyEvent) {
        let Overlay::Watchdog(v) = &mut self.overlay else { return };
        let dog = self.svc.watchdog(&v.sid);
        let out = v.key(k, dog.as_ref());
        self.watchdog_outcome(out);
    }

    pub(super) fn watchdog_click(&mut self, pos: ratatui::layout::Position) {
        let Overlay::Watchdog(v) = &mut self.overlay else { return };
        let dog = self.svc.watchdog(&v.sid);
        let out = v.click(pos, dog.as_ref());
        self.watchdog_outcome(out);
    }

    pub(super) fn watchdog_outcome(&mut self, out: Out) {
        let (pane, sid) = match &self.overlay {
            Overlay::Watchdog(v) => (v.pane, v.sid.clone()),
            _ => return,
        };
        let name = self.watched_name(pane);
        match out {
            Out::None => {}
            Out::Close => self.overlay = Overlay::None,
            Out::Start(plan) => {
                let idle = plan.idle_secs;
                if let Err(e) = self.svc.watchdog_start(&sid, plan) {
                    if let Overlay::Watchdog(v) = &mut self.overlay {
                        v.set_error(e);
                    }
                    return;
                }
                self.overlay = Overlay::None;
                self.watch.insert(pane, Trigger::new(false));
                let busy = self.panes.get(&pane).is_some_and(|p| p.busy());
                if busy {
                    self.toast(Kind::Info, format!("{name} · watchdog on · it reviews once {name} stops and sits idle {}", crate::watchdog::human(idle)));
                } else {
                    self.wake_watchdog(pane, "the user just started the watchdog while the session was idle: get the first milestone going".into(), true);
                }
            }
            Out::Save(plan) => {
                self.svc.watchdog_set_plan(&sid, plan);
                self.toast(Kind::Info, format!("{name} · watchdog plan saved · the next reviewer works from it"));
            }
            Out::Wake => self.wake_watchdog(pane, "the user asked for a review now".into(), true),
            Out::Pause(paused) => {
                self.svc.watchdog_pause(&sid, paused);
                self.toast(Kind::Info, if paused { format!("{name} · watchdog paused") } else { format!("{name} · watchdog watching again") });
            }
            Out::Stop => {
                self.svc.watchdog_stop(&sid);
                self.watch.remove(&pane);
                self.overlay = Overlay::None;
                self.toast(Kind::Info, format!("{name} · watchdog stopped · the session carries on as it is"));
            }
        }
    }

    pub(super) fn watchdog_prompt_key(&mut self, k: KeyEvent) {
        let Overlay::WatchdogPrompt(e) = &mut self.overlay else { return };
        let out = e.key(k);
        self.watchdog_prompt_outcome(out);
    }

    /// What the prompt editor asked for (a key or, in the desktop app, a click).
    pub(super) fn watchdog_prompt_outcome(&mut self, out: prompt_editor::Out) {
        match out {
            prompt_editor::Out::None => {}
            prompt_editor::Out::Close => self.overlay = Overlay::None,
            prompt_editor::Out::Save(prompt) => {
                self.overlay = Overlay::None;
                let mut s = self.svc.settings();
                let custom = prompt.is_some();
                s.watchdog.prompt = prompt;
                self.svc.save_settings(s);
                self.toast(Kind::Info, if custom { "watchdog prompt saved · reviewers use it from the next wake-up" } else { "watchdog prompt: back to bro's own" });
            }
        }
    }

    /// Wake `pane`'s watchdog now: the orchestrator reviews it (started if none runs). `manual`: you asked (a
    /// refusal is news, not an error).
    pub(crate) fn wake_watchdog(&mut self, pane: PaneId, reason: String, manual: bool) {
        let Some(t) = self.panes.get(&pane).and_then(|p| p.as_term_ref()) else { return };
        if t.meta.orchestrator {
            return;
        }
        let session = WatchedSession { title: t.display_name(), agent: t.meta.label.clone(), cwd: t.meta.cwd.clone() };
        let name = self.watched_name(pane);
        let cfg = self.svc.settings().watchdog;
        let prompt = cfg.prompt.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| bro_bridge::watchdog::DEFAULT_PROMPT.to_string());
        let reviewer = bro_bridge::watchdog::ReviewerLaunch {
            kind: bro_bridge::watchdog::ReviewerKind::Claude,
            program: String::new(),
            env: vec![],
            env_remove: vec![],
            model: None,
            label: "the orchestrator".into(),
        };
        self.watchdog_to_orchestrator(pane, WatchdogWake { reason, reviewer, prompt, session }, &name, manual);
    }

    /// Each batch of events: wake the watchdogs whose sessions stopped and sat idle long enough.
    pub(super) fn tick_watchdogs(&mut self) {
        if self.svc.is_demo() {
            return;
        }
        let dogs = {
            let st = self.svc.state();
            if st.watchdogs.is_empty() {
                drop(st);
                self.watch.clear();
                return;
            }
            st.watchdogs.clone()
        };
        let now = Instant::now();
        let mut wakes = vec![];
        for (id, p) in &self.panes {
            let Some(t) = p.as_term_ref() else { continue };
            let Some(dog) = dogs.get(&t.meta.sid) else { continue };
            let helpers_busy = dog.helpers.iter().any(|h| self.panes.values().any(|hp| hp.as_term_ref().is_some_and(|ht| ht.meta.sid == *h) && hp.busy()));
            let blocked = p.activity() == Some(Activity::Blocked);
            let seen = Seen { busy: p.busy() && !blocked, question: blocked.then(|| t.prompt_title().unwrap_or("a question on screen")), helpers_busy };
            let trigger = self.watch.entry(*id).or_insert_with(|| Trigger::new(true));
            if let Some(reason) = trigger.step(now, &seen, dog) {
                wakes.push((*id, reason));
            }
        }
        let live: Vec<PaneId> = self.panes.keys().copied().collect();
        self.watch.retain(|id, _| live.contains(id));
        for (id, reason) in wakes {
            self.wake_watchdog(id, reason, false);
        }
    }

    /// What a reviewer did, as a toast.
    pub(super) fn watchdog_event(&mut self, u: WatchdogUpdate) {
        let name = |app: &App, sid: &str| app.term_id(sid).map(|p| app.watched_name(p)).unwrap_or_else(|| "a closed session".into());
        match u {
            WatchdogUpdate::Started { session_id, reviewer, .. } => {
                let n = name(self, &session_id);
                self.toast(Kind::Info, format!("{n} · watchdog woke · {reviewer} is reviewing"));
            }
            WatchdogUpdate::Finished { session_id, summary, ok } => {
                let n = name(self, &session_id);
                if ok {
                    self.raise(Kind::AgentDone, format!("{n} · watchdog: {}", crate::services::brief(&summary)));
                } else {
                    self.raise(Kind::Error, format!("{n} · watchdog review failed: {}", crate::ui::fit(&summary, 90)));
                }
            }
            WatchdogUpdate::StoodDown { session_id, why } => {
                let n = name(self, &session_id);
                self.raise(Kind::AgentDone, format!("{n} · watchdog stood down: {why}"));
            }
            WatchdogUpdate::Step { .. } | WatchdogUpdate::Changed { .. } => {}
        }
    }
}
