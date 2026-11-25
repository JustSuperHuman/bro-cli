//! The computers on the Just Superhuman account (JustConnect): which there are, whether each can run sessions
//! right now and what stands in the way when it can't, and the sessions on the one being looked at. Every
//! request crosses the internet, so all of it runs on background threads and publishes into [`ComputersState`];
//! the views only read.
//!
//! What was last known stays on screen while a new look is under way and when one fails: the list is as old as
//! [`ComputersState::looked`] says, and whether the account can be reached right now is [`ComputersState::failure`]'s
//! to say. Mixing the two ("can't connect, so show nothing") hides the computers exactly when the person wants
//! to know what state they were in.

use super::Services;
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::justconnect::{self as jc, health};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// This Bro's version, for telling an older one on another computer.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// A list older than this is looked at again while the view is open.
pub const STALE: Duration = Duration::from_secs(15);
/// A computer's sessions older than this are read again while it is selected.
pub const SESSIONS_STALE: Duration = Duration::from_secs(4);

/// One computer on the account.
#[derive(Clone, Debug, PartialEq)]
pub struct Computer {
    pub id: String,
    pub name: String,
    /// "Windows", "macOS", "Linux"
    pub system: String,
    /// The computer Bro is running on.
    pub here: bool,
    pub bro_version: Option<String>,
    /// Whether it can run sessions now, and what's in the way when it can't.
    pub health: health::Diagnosis,
}

/// A session running on another computer, as its Bro describes it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RemoteSession {
    pub id: String,
    pub title: String,
    /// The project it works in, when it is in one.
    pub project: Option<String>,
    /// "claude", "codex"…; None for a plain shell.
    pub agent: Option<String>,
    /// "working", "awaiting", "idle".
    pub activity: Option<String>,
    /// In a sentence: what it is doing or waiting for.
    pub summary: String,
    /// "active 5s ago", "idle 12m".
    pub last_output: String,
}

/// The sessions on one computer.
#[derive(Clone, Debug, Default)]
pub enum Sessions {
    /// Not asked for yet.
    #[default]
    Unasked,
    /// The first answer is on its way.
    Loading(Instant),
    /// What the computer's Bro said, how long the round trip took and when.
    Ready { list: Vec<RemoteSession>, took: Duration, at: Instant, refreshing: bool },
    /// Why they couldn't be read.
    Failed(health::Diagnosis, Instant),
}

/// What Bro knows about the account's computers.
#[derive(Clone, Debug, Default)]
pub struct ComputersState {
    /// False until the first look has come back.
    pub checked: bool,
    /// Since when a look is under way.
    pub loading: Option<Instant>,
    /// Someone is signed in to Just Superhuman here.
    pub signed_in: bool,
    pub list: Vec<Computer>,
    /// When `list` was read.
    pub looked: Option<Instant>,
    /// When the last look ended, whether it got through or not (the next one is paced from here).
    pub tried: Option<Instant>,
    /// Why the last look didn't get through (the list is then what an earlier one read).
    pub failure: Option<health::Diagnosis>,
    /// The computer `bro ctl` runs on (None: this one).
    pub ctl: Option<String>,
    /// Each computer's sessions, by computer id.
    pub sessions: HashMap<String, Sessions>,
    /// Something being done to a computer: its id, what, since when.
    pub busy: Option<(String, &'static str, Instant)>,
}

impl ComputersState {
    pub fn computer(&self, id: &str) -> Option<&Computer> {
        self.list.iter().find(|c| c.id == id)
    }
}

impl Computer {
    /// Its desktop can be opened from here: another computer, on the account with access on. Bro doesn't have
    /// to be running there (the monitoring service shows the screen).
    pub fn desktop_reachable(&self) -> bool {
        !self.here && matches!(self.health.stopped_at, None | Some(health::Hop::Bro))
    }

    /// `bro ctl` can run on it: another computer whose Bro is open.
    pub fn runs_ctl(&self) -> bool {
        !self.here && self.health.ready()
    }

    /// What installing Bro on it from here would be called, when that can be done: "Install Bro there" on a
    /// Windows computer Bro has never run on, "Update Bro there" on one whose Bro is older than this one. None
    /// otherwise (installing from here is Windows-only, and this computer updates itself).
    pub fn bro_install(&self) -> Option<&'static str> {
        if self.here || self.system != "Windows" {
            return None;
        }
        match &self.bro_version {
            Some(version) if health::older(version, VERSION) => Some("Update Bro there"),
            Some(_) => None,
            None if self.health.stopped_at == Some(health::Hop::Bro) => Some("Install Bro there"),
            None => None,
        }
    }
}

fn system(platform: &str) -> String {
    match platform {
        "windows" => "Windows".into(),
        "darwin" => "macOS".into(),
        "linux" => "Linux".into(),
        other => other.to_string(),
    }
}

/// The account's list as [`Computer`]s: this computer first, then the ones that can run sessions, then by name.
pub(crate) fn computers(answer: &Value, here: &str, now_ms: i64) -> Vec<Computer> {
    let mut list: Vec<Computer> = answer["computers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            let id = c["id"].as_str().filter(|id| jc::valid_id(id))?.to_string();
            Some(Computer {
                here: id == here,
                name: c["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or("Computer").to_string(),
                system: system(c["platform"].as_str().unwrap_or_default()),
                bro_version: c["broVersion"].as_str().filter(|v| !v.is_empty()).map(str::to_string),
                health: health::of_computer(c, None, now_ms, VERSION),
                id,
            })
        })
        .collect();
    list.sort_by(|a, b| b.here.cmp(&a.here).then(b.health.ready().cmp(&a.health.ready())).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    list
}

/// What a computer's Bro answered `list_sessions` with (a JSON list of records), as [`RemoteSession`]s: the ones
/// that need someone first, then the ones working, then the rest; Bro's own orchestrator left out.
pub(crate) fn remote_sessions(output: &str) -> Vec<RemoteSession> {
    let records: Vec<Value> = serde_json::from_str(output).unwrap_or_default();
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let mut list: Vec<RemoteSession> = records
        .iter()
        .filter(|r| r["orchestrator"] != true && r["status"].as_str() != Some("exited"))
        .filter_map(|r| {
            Some(RemoteSession {
                id: text(&r["id"])?,
                title: text(&r["title"]).unwrap_or_else(|| "Session".into()),
                project: text(&r["project"]),
                agent: text(&r["agent"]),
                activity: text(&r["agentActivity"]),
                summary: text(&r["summary"]).unwrap_or_default(),
                last_output: text(&r["lastOutput"]).unwrap_or_default(),
            })
        })
        .collect();
    let rank = |s: &RemoteSession| match s.activity.as_deref() {
        Some("awaiting") => 0,
        Some("working") => 1,
        _ => 2,
    };
    list.sort_by_key(rank);
    list
}

impl Services {
    /// Read the account's computers again (the list stays as it is until the answer is in).
    pub fn refresh_computers(&self) {
        if self.is_demo() {
            return;
        }
        {
            let mut st = self.inner.state.write();
            if st.computers.loading.is_some() {
                return;
            }
            st.computers.loading = Some(Instant::now());
        }
        self.wake();
        let svc = self.clone();
        std::thread::spawn(move || {
            let signed_in = bro_core::jsh::signed_in();
            let here = jc::load().computer_id;
            let answer = if signed_in { Some(jc::call("GET", "/computers", json!({}))) } else { None };
            let ctl = jc::selected();
            let now_ms = chrono::Utc::now().timestamp_millis();
            svc.update(|st| {
                let c = &mut st.computers;
                c.checked = true;
                c.loading = None;
                c.tried = Some(Instant::now());
                c.signed_in = signed_in;
                c.ctl = ctl;
                match answer {
                    Some(Ok(answer)) => {
                        c.list = computers(&answer, &here, now_ms);
                        c.looked = Some(Instant::now());
                        c.failure = None;
                        // sessions of computers that left the account go with them
                        let ids: Vec<String> = c.list.iter().map(|x| x.id.clone()).collect();
                        c.sessions.retain(|id, _| ids.contains(id));
                    }
                    Some(Err(error)) => c.failure = Some(health::of_failure(&error, None)),
                    None => {
                        c.list.clear();
                        c.sessions.clear();
                        c.failure = None;
                    }
                }
            });
        });
    }

    /// Read the sessions on computer `id` (what was read before stays up until the new answer is in).
    pub fn refresh_remote_sessions(&self, id: &str) {
        if self.is_demo() {
            return;
        }
        {
            let mut st = self.inner.state.write();
            let Some(name) = st.computers.computer(id).filter(|c| c.health.ready() && !c.here).map(|c| c.name.clone()) else { return };
            let _ = name;
            let slot = st.computers.sessions.entry(id.to_string()).or_default();
            match slot {
                Sessions::Loading(_) | Sessions::Ready { refreshing: true, .. } => return,
                Sessions::Ready { refreshing, .. } => *refreshing = true,
                _ => *slot = Sessions::Loading(Instant::now()),
            }
        }
        self.wake();
        let (svc, id) = (self.clone(), id.to_string());
        std::thread::spawn(move || {
            let name = svc.state().computers.computer(&id).map(|c| c.name.clone());
            let read = jc::remote_tool_timed(&id, "list_sessions", json!({}));
            let next = match read {
                Ok((job, took)) if job["state"] == "completed" => Sessions::Ready { list: remote_sessions(job["result"]["output"].as_str().unwrap_or("[]")), took, at: Instant::now(), refreshing: false },
                // handed over and not answered: Bro there closed between the list and the ask
                Ok((job, _)) => {
                    let why = match job["state"].as_str().unwrap_or_default() {
                        "expired" => "Bro there didn't pick the request up. It may have just closed.",
                        "cancelled" => "The request was cancelled: access to that computer changed.",
                        _ => "Bro there took the request and never answered.",
                    };
                    Sessions::Failed(
                        health::Diagnosis { stopped_at: Some(health::Hop::Bro), name: name.clone(), headline: why.to_string(), detail: "no answer".into(), steps: vec!["Look again in a moment. If it keeps happening, open Bro on that computer.".into()], ..Default::default() },
                        Instant::now(),
                    )
                }
                Err(error) => Sessions::Failed(health::of_failure(&error, name.as_deref()), Instant::now()),
            };
            svc.update(|st| {
                st.computers.sessions.insert(id, next);
            });
        });
    }

    /// Choose where `bro ctl` runs: computer `id`, or this one (None).
    pub fn use_computer_for_ctl(&self, id: Option<String>) {
        if self.is_demo() || !self.begin_computer(id.clone().unwrap_or_default(), "choosing") {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            let result = jc::select(id.as_deref().unwrap_or("local"));
            let name = id.as_deref().and_then(|id| svc.state().computers.computer(id).map(|c| c.name.clone()));
            let ctl = jc::selected();
            svc.update(|st| {
                st.computers.busy = None;
                st.computers.ctl = ctl;
            });
            svc.send(match (result, name) {
                (Ok(_), Some(name)) => Event::Toast(Kind::Info, format!("`bro ctl` now runs on {name}. Pick this computer again to come back.")),
                (Ok(_), None) => Event::Toast(Kind::Info, "`bro ctl` runs on this computer again.".into()),
                (Err(error), _) => Event::Toast(Kind::Error, health::of_failure(&error, None).headline),
            });
        });
    }

    /// Open computer `id`'s desktop in the browser.
    pub fn open_computer_desktop(&self, id: &str) {
        if self.is_demo() || !self.begin_computer(id.to_string(), "opening its desktop") {
            return;
        }
        let (svc, id) = (self.clone(), id.to_string());
        std::thread::spawn(move || {
            let name = svc.state().computers.computer(&id).map(|c| c.name.clone());
            let opened = jc::call("POST", &format!("/computers/{id}/desktop"), json!({})).and_then(|answer| {
                let url = answer["url"].as_str().ok_or_else(|| anyhow::anyhow!("The desktop link didn't come back. Try again."))?.to_string();
                if url::Url::parse(&url)?.scheme() != "https" {
                    anyhow::bail!("The desktop link that came back isn't secure, so it wasn't opened.");
                }
                crate::panes::links::open(&url).map_err(|e| anyhow::anyhow!(e.to_string()))
            });
            svc.update(|st| st.computers.busy = None);
            svc.send(match opened {
                Ok(()) => Event::Toast(Kind::Info, format!("{}'s desktop is opening in your browser. The link works for 20 minutes.", name.as_deref().unwrap_or("That computer"))),
                Err(error) if health::explains(&error) => Event::Toast(Kind::Error, health::of_failure(&error, name.as_deref()).headline),
                Err(error) => Event::Toast(Kind::Error, error.to_string()),
            });
        });
    }

    /// Install Bro on computer `id`, or bring it up to date (Windows; its sessions keep running).
    pub fn install_bro_on(&self, id: &str) {
        if self.is_demo() || !self.begin_computer(id.to_string(), "installing Bro") {
            return;
        }
        let (svc, id) = (self.clone(), id.to_string());
        std::thread::spawn(move || {
            let name = svc.state().computers.computer(&id).map(|c| c.name.clone()).unwrap_or_else(|| "that computer".into());
            let done = jc::call("POST", &format!("/computers/{id}/bro/install"), json!({"requestId":uuid::Uuid::new_v4().to_string()}));
            svc.update(|st| st.computers.busy = None);
            svc.send(match done {
                Ok(answer) if answer["state"] == "completed" => Event::Toast(Kind::Info, format!("Bro is up to date on {name}. Sessions running there carried on.")),
                // sent, and no word back within the time allowed: it may still be installing
                Ok(_) => Event::Toast(Kind::Info, format!("The install was sent to {name} and hasn't reported back yet. Look again in a minute.")),
                Err(error) if health::explains(&error) => Event::Toast(Kind::Error, health::of_failure(&error, Some(&name)).headline),
                Err(error) => Event::Toast(Kind::Error, error.to_string()),
            });
            svc.refresh_computers();
        });
    }

    /// Take the one slot for something being done to a computer. False when another is under way.
    fn begin_computer(&self, id: String, what: &'static str) -> bool {
        let mut st = self.inner.state.write();
        if st.computers.busy.is_some() {
            return false;
        }
        st.computers.busy = Some((id, what, Instant::now()));
        drop(st);
        self.wake();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const HERE: &str = "aaaaaaaa-0da5-444f-a10c-9c17afcd48e0";

    #[test]
    fn this_computer_leads_then_the_ones_that_can_run_sessions() {
        let answer = json!({"computers":[
            {"id":"cccccccc-0da5-444f-a10c-9c17afcd48e0","name":"Zed's laptop","platform":"darwin","enabled":true,"provisioned":true,"broOnline":true,"broLastSeen":NOW,"broVersion":"2.1.40"},
            {"id":"bbbbbbbb-0da5-444f-a10c-9c17afcd48e0","name":"Build box","platform":"linux","enabled":true,"provisioned":true,"broOnline":false,"broLastSeen":NOW - 7_200_000},
            {"id":HERE,"name":"Studio workstation","platform":"windows","enabled":true,"provisioned":true,"broOnline":true,"broLastSeen":NOW},
            {"id":"not-an-id","name":"Bogus"},
        ]});
        let list = computers(&answer, HERE, NOW);
        assert_eq!(list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Studio workstation", "Zed's laptop", "Build box"]);
        assert!(list[0].here && !list[1].here);
        assert_eq!(list[1].system, "macOS");
        // installing from here: Windows only, never this computer, only when there is something to install
        assert_eq!(list.iter().map(|c| c.bro_install()).collect::<Vec<_>>(), [None, None, None]);
        let windows = |version: Option<&str>, online: bool| {
            let record = json!({"id":"dddddddd-0da5-444f-a10c-9c17afcd48e0","name":"Front desk","platform":"windows","enabled":true,"provisioned":true,"broOnline":online,"broLastSeen":if version.is_some() { json!(NOW) } else { json!(null) },"broVersion":version});
            computers(&json!({"computers":[record]}), HERE, NOW).remove(0)
        };
        assert_eq!(windows(Some("2.0.1"), true).bro_install(), Some("Update Bro there"));
        assert_eq!(windows(Some("99.0.0"), true).bro_install(), None);
        assert_eq!(windows(None, false).bro_install(), Some("Install Bro there"));
        // what can be done with each: this computer needs neither; a closed Bro still shows its desktop
        let list = computers(&answer, HERE, NOW);
        assert_eq!(list.iter().map(|c| (c.desktop_reachable(), c.runs_ctl())).collect::<Vec<_>>(), [(false, false), (true, true), (true, false)]);
        let off = computers(&json!({"computers":[{"id":"eeeeeeee-0da5-444f-a10c-9c17afcd48e0","name":"Front desk","platform":"windows","enabled":false}]}), HERE, NOW).remove(0);
        assert!(!off.desktop_reachable() && !off.runs_ctl(), "access off: nothing reaches it");
        assert!(list[1].health.ready());
        assert_eq!(list[2].health.headline, "Bro is closed on Build box.");
        assert_eq!(list[2].health.detail, "closed · last open 2 hr ago");
        assert!(computers(&json!({}), HERE, NOW).is_empty());
    }

    #[test]
    fn sessions_that_need_someone_come_first_and_the_orchestrator_is_left_out() {
        let output = json!([
            {"id":"a","title":"Hugh","project":"justgains","agent":"claude","agentActivity":"idle","summary":"waiting for a task","lastOutput":"idle 12m"},
            {"id":"b","title":"Mal","project":"bro-cli-v2","agent":"codex","agentActivity":"working","summary":"running the tests","lastOutput":"active 2s ago"},
            {"id":"c","title":"Orchestrator","agent":"codex","agentActivity":"idle","orchestrator":true},
            {"id":"d","title":"Ada","agent":"claude","agentActivity":"awaiting","summary":"asks: Proceed with the migration?","lastOutput":"idle 40s"},
            {"id":"e","title":"old shell","status":"exited"},
            {"title":"no id"},
        ])
        .to_string();
        let list = remote_sessions(&output);
        assert_eq!(list.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Ada", "Mal", "Hugh"]);
        assert_eq!(list[0].summary, "asks: Proceed with the migration?");
        assert_eq!(list[1].project.as_deref(), Some("bro-cli-v2"));
        assert!(remote_sessions("not json").is_empty());
    }
}
