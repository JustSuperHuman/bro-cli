//! Why an account computer can't be reached, said by where the connection stops.
//!
//! A request to run work on another computer crosses four links in order: this computer (its sign-in, its way
//! out), Just Superhuman (the account server and the remote management behind it), the other computer (on the
//! account, allowed, switched on) and Bro running there. "It didn't work" names none of them; a [`Diagnosis`]
//! names the first one that fails, says so in one line and lists what fixes it. Every surface that shows a
//! computer (the Computers view, `bro computers`, the orchestrator's tools) reads the same one.

use serde_json::Value;

/// The links between this computer and work running on another, in the order a request crosses them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hop {
    /// This computer: its sign-in and its way out to the internet.
    Here,
    /// Just Superhuman's account server, and the remote management it speaks to.
    Account,
    /// The other computer: on the account, access allowed, switched on.
    Computer,
    /// Bro on the other computer, which runs the sessions.
    Bro,
}

impl Hop {
    pub const ALL: [Hop; 4] = [Hop::Here, Hop::Account, Hop::Computer, Hop::Bro];

    /// The link's name in a list of checks (`name`: the other computer's, when known).
    pub fn label(self, name: Option<&str>) -> String {
        match self {
            Hop::Here => "This computer".into(),
            Hop::Account => "Just Superhuman".into(),
            Hop::Computer => name.unwrap_or("That computer").to_string(),
            Hop::Bro => "Bro there".into(),
        }
    }
}

/// How one link came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// It answered.
    Passed,
    /// The connection stops here.
    Failed,
    /// Not reached: an earlier link failed, so nothing is known about this one.
    Unchecked,
}

/// What stands between this computer and work on another one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diagnosis {
    /// The first link that fails (None: every link answers).
    pub stopped_at: Option<Hop>,
    /// The other computer's name, when known.
    pub name: Option<String>,
    /// One line naming what is wrong, or that the computer is ready.
    pub headline: String,
    /// A few words for the failing link in a list of checks ("closed · last open 3 hr ago").
    pub detail: String,
    /// What to do about it, the most useful first. Empty when nothing is wrong.
    pub steps: Vec<String>,
    /// Worth knowing, not in the way (an older Bro there).
    pub notes: Vec<String>,
    /// `bro computers` commands that do what the steps and notes say from this computer. A terminal prints
    /// them; a window has buttons for the same things and leaves them out.
    pub commands: Vec<String>,
}

impl Diagnosis {
    pub fn ready(&self) -> bool {
        self.stopped_at.is_none()
    }

    /// How `hop` came out: the links before the failing one passed, the ones after it weren't reached.
    pub fn check(&self, hop: Hop) -> Check {
        let at = |h: Hop| Hop::ALL.iter().position(|x| *x == h).unwrap_or(0);
        match self.stopped_at {
            None => Check::Passed,
            Some(stop) if at(hop) < at(stop) => Check::Passed,
            Some(stop) if hop == stop => Check::Failed,
            Some(_) => Check::Unchecked,
        }
    }

    /// The diagnosis as lines for a terminal: the headline, each link with its result, then the steps.
    pub fn render(&self) -> String {
        let name = self.name.as_deref();
        let width = Hop::ALL.iter().map(|h| h.label(name).chars().count()).max().unwrap_or(0);
        let mut lines = vec![self.headline.clone(), String::new()];
        for hop in Hop::ALL {
            let (mark, said) = match self.check(hop) {
                Check::Passed => ('✓', passed(hop)),
                Check::Failed => ('✗', self.detail.as_str()),
                Check::Unchecked => ('·', "not reached"),
            };
            lines.push(format!("  {mark} {:<width$}  {said}", hop.label(name)).trim_end().to_string());
        }
        if !self.steps.is_empty() {
            lines.push(String::new());
            lines.push("What to do".into());
            for (i, step) in self.steps.iter().enumerate() {
                lines.push(format!("  {}. {step}", i + 1));
            }
        }
        for note in &self.notes {
            lines.push(String::new());
            lines.push(note.clone());
        }
        if !self.commands.is_empty() {
            lines.push(String::new());
            lines.push("From a terminal here".into());
            for command in &self.commands {
                lines.push(format!("  {command}"));
            }
        }
        lines.join("\n")
    }
}

/// What a link that answered is said to have done.
pub fn passed(hop: Hop) -> &'static str {
    match hop {
        Hop::Here => "signed in and online",
        Hop::Account => "answering",
        Hop::Computer => "on your account, access on",
        Hop::Bro => "open and taking work",
    }
}

/// "4 min ago" for anything recent, "3 days ago" up to a week, then nothing (too long ago to be worth a clause).
pub fn ago(now_ms: i64, at_ms: i64) -> Option<String> {
    let seconds = (now_ms - at_ms).max(0) / 1000;
    Some(match seconds {
        0..=44 => "just now".into(),
        45..=3599 => format!("{} min ago", (seconds / 60).max(1)),
        3600..=86_399 => format!("{} hr ago", seconds / 3600),
        86_400..=172_799 => "yesterday".into(),
        172_800..=604_799 => format!("{} days ago", seconds / 86_400),
        _ => return None,
    })
}

/// `major.minor.patch` as numbers (anything after them ignored); None when it isn't a version.
fn version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split(['.', '-', '+']);
    let mut next = || parts.next()?.parse::<u64>().ok();
    Some((next()?, next()?, next().unwrap_or(0)))
}

/// Whether Bro `theirs` is older than Bro `ours`.
pub fn older(theirs: &str, ours: &str) -> bool {
    matches!((version(theirs), version(ours)), (Some(t), Some(o)) if t < o)
}

/// What a computer's own record says about reaching it. `computer` is one entry of the account's list (or the
/// `connection` of its status): `enabled`, `provisioned`, `broOnline`, `broLastSeen`, `broVersion`. `service`
/// is the monitoring service's word for the machine itself when it was asked (`online`, `offline`, `overdue`,
/// `awaiting_install`); None when it wasn't.
pub fn of_computer(computer: &Value, service: Option<&str>, now_ms: i64, own_version: &str) -> Diagnosis {
    let name = computer["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or("That computer").to_string();
    let id = computer["id"].as_str().unwrap_or("<id>");
    let mut d = Diagnosis { name: Some(name.clone()), ..Default::default() };
    let stop = |d: &mut Diagnosis, hop: Hop, headline: String, detail: &str, steps: Vec<String>| {
        d.stopped_at = Some(hop);
        d.headline = headline;
        d.detail = detail.to_string();
        d.steps = steps;
    };
    if computer["enabled"] != true {
        stop(
            &mut d,
            Hop::Computer,
            format!("Remote access is turned off on {name}."),
            "access turned off",
            vec![
                format!("On {name}, open Bro's Settings, go to Remote Control and turn on \"Reach this computer from your account\"."),
                "Or run `bro computers enable` in a terminal there. Nothing is reinstalled: access comes back at once.".into(),
            ],
        );
        return d;
    }
    if computer["provisioned"] == false || service == Some("awaiting_install") {
        stop(
            &mut d,
            Hop::Computer,
            format!("{name} hasn't finished setting up."),
            "set-up not finished",
            vec![
                format!("On {name}, run `bro computers enable` and approve the administrator prompt. The install takes about a minute."),
                "If the prompt was declined, running it again asks once more.".into(),
            ],
        );
        return d;
    }
    if matches!(service, Some("offline" | "overdue")) {
        let gone = service == Some("offline");
        stop(
            &mut d,
            Hop::Computer,
            if gone { format!("{name} is switched off or offline.") } else { format!("{name} has stopped checking in.") },
            if gone { "switched off or offline" } else { "not checking in" },
            vec![
                format!("Turn {name} on, or wake it. It reconnects by itself within a minute of getting online."),
                "If it is on, check its internet connection: it reaches Just Superhuman over ordinary HTTPS, nothing needs opening on its network.".into(),
            ],
        );
        return d;
    }
    if computer["broOnline"] != true {
        let seen = computer["broLastSeen"].as_i64().filter(|at| *at > 0);
        let last = seen.and_then(|at| ago(now_ms, at));
        match seen {
            None => stop(
                &mut d,
                Hop::Bro,
                format!("Bro hasn't run on {name} yet."),
                "never opened",
                vec![
                    format!("Open Bro on {name} and sign in there with `bro jsh login`."),
                    "Not installed there yet? On a Windows computer Bro can be installed from here.".into(),
                    "Running a command and opening the desktop don't need Bro, so both already work.".into(),
                ],
            ),
            Some(_) => stop(
                &mut d,
                Hop::Bro,
                format!("Bro is closed on {name}."),
                &last.map_or_else(|| "closed".to_string(), |when| format!("closed · last open {when}")),
                vec![
                    format!("Open Bro on {name}. Sessions run inside it, so it has to be running to start or follow one."),
                    "Can't get to it? Open its desktop from here: its screen comes up in your browser, and you can start Bro there.".into(),
                    "To keep it reachable with its window closed, turn on \"Keep working while bro is closed\" under Settings, Session server, on that computer.".into(),
                ],
            ),
        }
        d.commands = match seen {
            None => vec![format!("bro computers install-bro {id}"), format!("bro computers desktop {id}")],
            Some(_) => vec![format!("bro computers desktop {id}")],
        };
        return d;
    }
    d.headline = format!("{name} is ready.");
    if let Some(theirs) = computer["broVersion"].as_str().filter(|v| older(v, own_version)) {
        d.notes.push(format!("{name} runs Bro {theirs}; this computer runs {own_version}. Newer features may be missing there until it updates."));
        d.commands.push(format!("bro computers install-bro {id}"));
    }
    d
}

/// Whether a failed call is about the connection (no answer, signed out, not allowed, not there, closed, busy,
/// the service down), which [`of_failure`] explains link by link. False for the caller's own mistake (a bad
/// argument, an unknown tool): the server's sentence already says what to change.
pub fn explains(error: &anyhow::Error) -> bool {
    match super::refused(error) {
        Some(r) => matches!(r.status, 401 | 403 | 404 | 409 | 429 | 500..=599),
        None => {
            let said = error.to_string();
            error.chain().any(|cause| cause.downcast_ref::<ureq::Error>().is_some()) || said.contains("bro jsh login") || said.contains("invalid response")
        }
    }
}

/// What a failed call says about reaching `name` (None: the call wasn't about one computer).
pub fn of_failure(error: &anyhow::Error, name: Option<&str>) -> Diagnosis {
    let who = name.unwrap_or("That computer");
    let mut d = Diagnosis { name: name.map(str::to_string), ..Default::default() };
    let mut stop = |hop: Hop, headline: String, detail: &str, steps: Vec<String>| {
        d.stopped_at = Some(hop);
        d.headline = headline;
        d.detail = detail.to_string();
        d.steps = steps;
    };
    let said = error.to_string();
    match super::refused(error) {
        // no answer at all: signed out before asking, or the request never got there
        None if said.contains("bro jsh login") => stop(
            Hop::Here,
            "This computer isn't signed in to Just Superhuman.".into(),
            "signed out",
            vec!["Sign in with `bro jsh login`, or from the Profiles view. Your computers appear as soon as you do.".into()],
        ),
        None if said.contains("invalid response") => stop(
            Hop::Account,
            "Just Superhuman answered with something Bro couldn't read.".into(),
            "unreadable answer",
            vec![
                "Try again in a minute: this is what a restart or a busy moment looks like from here.".into(),
                "Your computers and the sessions on them keep running meanwhile.".into(),
            ],
        ),
        None => stop(
            Hop::Here,
            "This computer couldn't reach Just Superhuman.".into(),
            "no connection out",
            vec![
                "Check this computer's internet connection, then try again.".into(),
                "If other sites load, Just Superhuman may be restarting. It is usually back within a minute.".into(),
            ],
        ),
        Some(r) => match r.status {
            401 => stop(
                Hop::Here,
                "Your Just Superhuman sign-in has ended.".into(),
                "sign-in ended",
                vec!["Sign in again with `bro jsh login`. Computers you added stay on your account.".into()],
            ),
            403 => stop(
                Hop::Account,
                r.message.clone(),
                "not allowed for this sign-in",
                vec![
                    "Sign in to the account that owns the computer, or ask one of its admins to add you.".into(),
                    "Admins can also make you an IT Admin, which reaches every computer on the account.".into(),
                ],
            ),
            404 => stop(
                Hop::Computer,
                format!("{who} isn't on your account."),
                "not on your account",
                vec![
                    "Run `bro computers list` to see the computers you can reach.".into(),
                    "If it should be there, turn on \"Reach this computer from your account\" in its Bro settings.".into(),
                ],
            ),
            409 if r.message.to_ascii_lowercase().contains("offline") => stop(
                Hop::Bro,
                format!("Bro is closed on {who}."),
                "closed",
                vec![
                    format!("Open Bro on {who}. Sessions run inside it, so it has to be running to start or follow one."),
                    "Commands and the desktop don't need Bro: `bro computers run` and `bro computers desktop` still work.".into(),
                ],
            ),
            409 => stop(Hop::Computer, r.message.clone(), "refused", vec![format!("Check `bro computers status` for {who}, then try again.")]),
            429 => stop(
                Hop::Bro,
                format!("{who} has too much work waiting."),
                "queue full",
                vec![
                    "Give it a moment: work is handed over in order, and the queue clears as each piece starts.".into(),
                    "`bro computers history` shows what is waiting there.".into(),
                ],
            ),
            500..=599 => stop(
                Hop::Account,
                "Just Superhuman's remote management isn't answering.".into(),
                "not answering",
                vec![
                    "Try again in a minute.".into(),
                    "Your computers and the sessions on them keep running meanwhile; only reaching them from here is paused.".into(),
                ],
            ),
            _ => stop(Hop::Account, r.message.clone(), "refused", vec![]),
        },
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_800_000_000_000;

    fn computer(extra: Value) -> Value {
        let mut c = json!({"id":"dbb88f69-0da5-444f-a10c-9c17afcd48e0","name":"Studio workstation","platform":"windows","enabled":true,"provisioned":true,"broOnline":true,"broLastSeen":NOW - 4_000,"broVersion":"2.1.48"});
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    }

    #[test]
    fn a_ready_computer_passes_every_link() {
        let d = of_computer(&computer(json!({})), Some("online"), NOW, "2.1.48");
        assert!(d.ready());
        assert_eq!(d.headline, "Studio workstation is ready.");
        assert!(d.steps.is_empty() && d.notes.is_empty());
        assert!(Hop::ALL.iter().all(|h| d.check(*h) == Check::Passed));
    }

    #[test]
    fn the_first_failing_link_is_named_and_later_ones_are_not_judged() {
        let d = of_computer(&computer(json!({"enabled":false,"broOnline":false})), None, NOW, "2.1.48");
        assert_eq!(d.stopped_at, Some(Hop::Computer));
        assert_eq!(d.check(Hop::Here), Check::Passed);
        assert_eq!(d.check(Hop::Account), Check::Passed);
        assert_eq!(d.check(Hop::Computer), Check::Failed);
        assert_eq!(d.check(Hop::Bro), Check::Unchecked, "access is off: nothing is known about Bro there");
        assert!(d.headline.contains("turned off on Studio workstation"));
    }

    #[test]
    fn a_closed_bro_says_when_it_was_last_open_and_what_still_works() {
        let d = of_computer(&computer(json!({"broOnline":false,"broLastSeen":NOW - 3 * 3_600_000})), Some("online"), NOW, "2.1.48");
        assert_eq!(d.stopped_at, Some(Hop::Bro));
        assert_eq!(d.headline, "Bro is closed on Studio workstation.");
        assert_eq!(d.detail, "closed · last open 3 hr ago");
        assert!(d.steps[1].contains("Open its desktop from here"), "{:?}", d.steps);
        assert_eq!(d.commands, ["bro computers desktop dbb88f69-0da5-444f-a10c-9c17afcd48e0"], "the command is beside the steps, not in them");
        // never opened reads differently, and offers the install
        let d = of_computer(&computer(json!({"broOnline":false,"broLastSeen":null})), None, NOW, "2.1.48");
        assert_eq!(d.headline, "Bro hasn't run on Studio workstation yet.");
        assert!(d.commands[0].contains("install-bro"), "{:?}", d.commands);
        assert!(d.steps.iter().all(|s| !s.contains("dbb88f69")), "no ID to read out in a sentence");
    }

    #[test]
    fn a_machine_that_is_off_is_not_blamed_on_bro() {
        let d = of_computer(&computer(json!({"broOnline":false})), Some("offline"), NOW, "2.1.48");
        assert_eq!(d.stopped_at, Some(Hop::Computer));
        assert_eq!(d.headline, "Studio workstation is switched off or offline.");
        let d = of_computer(&computer(json!({"provisioned":false})), None, NOW, "2.1.48");
        assert_eq!(d.headline, "Studio workstation hasn't finished setting up.");
    }

    #[test]
    fn an_older_bro_is_a_note_not_a_failure() {
        let d = of_computer(&computer(json!({"broVersion":"2.1.30"})), Some("online"), NOW, "2.1.48");
        assert!(d.ready());
        assert!(d.notes[0].contains("runs Bro 2.1.30; this computer runs 2.1.48"), "{:?}", d.notes);
        assert_eq!(d.commands, ["bro computers install-bro dbb88f69-0da5-444f-a10c-9c17afcd48e0"]);
        assert!(of_computer(&computer(json!({"broVersion":"2.2.0"})), None, NOW, "2.1.48").notes.is_empty());
        assert!(older("2.1.9", "2.1.10") && !older("2.1.10", "2.1.10") && !older("", "2.1.10") && older("v2.0.0-beta", "2.0.1"));
    }

    #[test]
    fn failures_are_placed_on_the_link_that_failed() {
        let refused = |status, message: &str| anyhow::Error::new(super::super::Refused { status, message: message.into() });
        let at = |e: anyhow::Error| of_failure(&e, Some("Studio workstation"));
        assert_eq!(at(anyhow::anyhow!("Sign in with `bro jsh login` first.")).stopped_at, Some(Hop::Here));
        assert_eq!(at(anyhow::anyhow!("io: connection refused")).headline, "This computer couldn't reach Just Superhuman.");
        assert_eq!(at(refused(401, "Sign in again.")).headline, "Your Just Superhuman sign-in has ended.");
        assert_eq!(at(refused(403, "You are not a member of this account.")).stopped_at, Some(Hop::Account));
        assert_eq!(at(refused(404, "Computer not found.")).headline, "Studio workstation isn't on your account.");
        let closed = at(refused(409, "Bro is offline on this computer. Open Bro there and try again."));
        assert_eq!((closed.stopped_at, closed.headline.as_str()), (Some(Hop::Bro), "Bro is closed on Studio workstation."));
        assert_eq!(at(refused(409, "Remote management is disabled for this computer.")).stopped_at, Some(Hop::Computer));
        assert_eq!(at(refused(429, "This computer already has 20 pending jobs.")).stopped_at, Some(Hop::Bro));
        assert_eq!(at(refused(503, "Remote management did not answer.")).stopped_at, Some(Hop::Account));
        assert_eq!(at(anyhow::anyhow!("JustConnect returned an invalid response.")).stopped_at, Some(Hop::Account));
    }

    #[test]
    fn a_callers_own_mistake_is_not_explained_as_a_connection_problem() {
        let refused = |status, message: &str| anyhow::Error::new(super::super::Refused { status, message: message.into() });
        assert!(!explains(&refused(400, "Choose a supported Bro tool, arguments and a UUID requestId.")));
        assert!(!explains(&anyhow::anyhow!("That isn't a computer ID. Copy one from `bro computers list`.")));
        for status in [401, 403, 404, 409, 429, 503] {
            assert!(explains(&refused(status, "no")), "{status}");
        }
        assert!(explains(&anyhow::anyhow!("Sign in with `bro jsh login` first.")));
    }

    #[test]
    fn the_terminal_rendering_lists_each_link_with_its_result() {
        let d = of_computer(&computer(json!({"broOnline":false,"broLastSeen":NOW - 240_000})), Some("online"), NOW, "2.1.48");
        let text = d.render();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Bro is closed on Studio workstation.");
        assert_eq!(lines[2], "  ✓ This computer       signed in and online");
        assert_eq!(lines[3], "  ✓ Just Superhuman     answering");
        assert_eq!(lines[4], "  ✓ Studio workstation  on your account, access on");
        assert_eq!(lines[5], "  ✗ Bro there           closed · last open 4 min ago");
        assert_eq!(lines[7], "What to do");
        assert!(lines[8].starts_with("  1. Open Bro on Studio workstation."));
        assert!(text.ends_with("From a terminal here\n  bro computers desktop dbb88f69-0da5-444f-a10c-9c17afcd48e0"), "{text}");
        // an earlier failure leaves the later links unjudged
        let text = of_failure(&anyhow::anyhow!("timed out"), Some("Studio workstation")).render();
        assert!(text.contains("✗ This computer       no connection out") && text.contains("· Bro there           not reached"), "{text}");
    }

    #[test]
    fn times_read_like_someone_would_say_them() {
        assert_eq!(ago(NOW, NOW - 10_000).as_deref(), Some("just now"));
        assert_eq!(ago(NOW, NOW - 50_000).as_deref(), Some("1 min ago"));
        assert_eq!(ago(NOW, NOW - 26 * 3_600_000).as_deref(), Some("yesterday"));
        assert_eq!(ago(NOW, NOW - 4 * 86_400_000).as_deref(), Some("4 days ago"));
        assert_eq!(ago(NOW, NOW - 30 * 86_400_000), None);
    }
}
