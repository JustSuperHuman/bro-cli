//! Remote Control: how your phone gets in (phone access, the firewall, pairing), who is connected right now, and
//! remote desktop: its switch with Bro RMM's version and update check right under it, then how the screen is
//! shared. Who's watching or driving the desktop is matched to the connected devices by address.

use super::{Block, DeviceRow, Item, MobileOpt, Row, choice, firewall_item, mac_permissions_item, shown, switch};
use crate::pane::Cx;
use crate::ui::{fg, muted};
use bro_bridge::devices::{Device, Form};
use bro_bridge::rmm::{Phase, Status, Viewer};
use ratatui::style::Modifier;
use ratatui::text::Span;
use std::sync::atomic::{AtomicBool, Ordering};

static JUSTCONNECT_BUSY: AtomicBool = AtomicBool::new(false);
static SESSIONS_BUSY: AtomicBool = AtomicBool::new(false);

pub(super) fn toggle_justconnect(cx: &Cx) {
    if cx.svc.is_demo() || JUSTCONNECT_BUSY.swap(true, Ordering::AcqRel) { return; }
    let tx = cx.tx.clone();
    std::thread::spawn(move || {
        let enabled = bro_core::justconnect::load().managed();
        let result = if enabled { bro_core::justconnect::disable() } else { bro_core::justconnect::enable(None) };
        JUSTCONNECT_BUSY.store(false, Ordering::Release);
        let (kind, message) = match result {
            Ok(_) => (crate::alerts::Kind::Info, if enabled { "Remote access from your account is off for this computer." } else { "This computer is on your account. Find it under JustConnect in your Superhost." }.to_string()),
            Err(e) => (crate::alerts::Kind::Error, e.to_string()),
        };
        let _ = tx.send(crate::pane::Event::Toast(kind, message));
    });
}

/// Your own phone, signed in to the same account, opening the sessions here. Signing in turns it on; off stays off.
pub(super) fn toggle_sessions(cx: &Cx) {
    if cx.svc.is_demo() || SESSIONS_BUSY.swap(true, Ordering::AcqRel) { return; }
    let tx = cx.tx.clone();
    std::thread::spawn(move || {
        let on = !bro_core::justconnect::load().reachable();
        let result = bro_core::justconnect::set_sessions(on);
        SESSIONS_BUSY.store(false, Ordering::Release);
        let (kind, message) = match result {
            Ok(_) if on => (crate::alerts::Kind::Info, "Your sessions here now show on your phone. Open the bro app there and pick this computer.".to_string()),
            Ok(_) => (crate::alerts::Kind::Info, "Your phone can't open the sessions here anymore. Signing in again won't turn it back on.".to_string()),
            Err(e) => (crate::alerts::Kind::Error, e.to_string()),
        };
        let _ = tx.send(crate::pane::Event::Toast(kind, message));
    });
}

/// Whether `option` shows on the page now.
pub(super) fn visible(option: MobileOpt, cx: &Cx) -> bool {
    let s = cx.svc.settings();
    match option {
        MobileOpt::Firewall => cx.svc.firewall_applies(),
        MobileOpt::Pair => s.bridge.enabled,
        MobileOpt::ReadOnly => s.mobile_companion.enabled,
        // a choice only with more than one (Linux shares its whole X11 screen)
        MobileOpt::Display => s.mobile_companion.enabled && bro_core::rmm::display_count() > 1,
        MobileOpt::Permissions => cfg!(target_os = "macos") && s.mobile_companion.enabled,
        MobileOpt::Disconnect => cx.svc.rmm_status().is_some_and(|status| status.active_viewers > 0),
        // remote management already lets the phone in: one switch says it
        MobileOpt::Sessions => bro_core::jsh::signed_in() && !bro_core::justconnect::load().managed(),
        MobileOpt::Access | MobileOpt::Remote | MobileOpt::JustConnect => true,
    }
}

/// The phones and browsers connected to this window's bridge (None: it isn't running here).
fn devices(cx: &Cx) -> Option<Vec<Device>> {
    cx.svc.state().bridge.status.ready().map(|s| s.devices.clone())
}

/// What the link under Remote desktop does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Link {
    /// ask the channel for a newer Bro RMM (a running stream carries on)
    Check,
    /// install the newer one it found, ending the desktop connections that held it back
    Install,
    /// a download that failed or never ran
    Retry,
}

fn link(cx: &Cx) -> Option<(Link, Status)> {
    if !cx.svc.settings().mobile_companion.enabled || !bro_core::rmm::supported() {
        return None;
    }
    let status = cx.svc.rmm_status()?;
    let link = match status.phase {
        _ if status.checking => return None,
        Phase::Error | Phase::NotInstalled => Link::Retry,
        Phase::Ready | Phase::Running if status.update.is_some() => Link::Install,
        Phase::Ready | Phase::Running => Link::Check,
        Phase::Installing | Phase::Starting | Phase::Disabled => return None,
    };
    Some((link, status))
}

/// The link under Remote desktop now (`u` in the terminal), None while a download, a check or the stream is
/// starting.
pub(super) fn update_link(cx: &Cx) -> Option<String> {
    let (link, status) = link(cx)?;
    Some(match link {
        Link::Check => "Check for updates".into(),
        Link::Retry => "Try again".into(),
        Link::Install => match status.active_viewers {
            0 => "Install update".into(),
            1 => "Install now · ends 1 connection".into(),
            n => format!("Install now · ends {n} connections"),
        },
    })
}

/// Follow the link under Remote desktop.
pub(super) fn follow(cx: &Cx) {
    match link(cx).map(|(l, _)| l) {
        Some(Link::Check) => cx.svc.rmm_check_update(),
        Some(Link::Install | Link::Retry) => cx.svc.rmm_retry(),
        None => {}
    }
}

/// A Bro RMM download or check is under way (the terminal UI keeps its line moving).
pub(super) fn busy(cx: &Cx) -> bool {
    JUSTCONNECT_BUSY.load(Ordering::Acquire) || cx.svc.settings().mobile_companion.enabled
        && cx.svc.rmm_status().is_some_and(|s| s.checking || matches!(s.phase, Phase::Installing | Phase::Starting))
}

/// What the section is at, beside its name in the list.
pub(super) fn summary(cx: &Cx) -> Span<'static> {
    let t = cx.theme;
    let s = cx.svc.settings();
    if !s.bridge.enabled {
        return Span::styled("off", muted(t));
    }
    let status = cx.svc.rmm_status();
    if status.as_ref().is_some_and(|s| s.active_viewers > 0) {
        return Span::styled("live", fg(t.good));
    }
    if s.mobile_companion.enabled {
        match status.as_ref().map(|s| s.phase) {
            Some(Phase::Installing) => return Span::styled("downloading…", fg(t.shine)),
            Some(Phase::Error) => return Span::styled("needs attention", fg(t.danger)),
            _ => {}
        }
    }
    match devices(cx).map_or(0, |d| d.len()) {
        0 => Span::styled("on", muted(t)),
        n => Span::styled(format!("{n} online"), fg(t.good)),
    }
}

/// How long something has been connected: "just now", "12 min", "2 h 5 min", "3 days".
fn connected_for(since_ms: i64) -> String {
    let mins = ((crate::util::now_ms() - since_ms).max(0) / 60_000) as u64;
    match mins {
        0 => "just now".into(),
        m if m < 60 => format!("{m} min"),
        m if m < 24 * 60 && m % 60 == 0 => format!("{} h", m / 60),
        m if m < 24 * 60 => format!("{} h {} min", m / 60, m % 60),
        m if m < 48 * 60 => "1 day".into(),
        m => format!("{} days", m / (24 * 60)),
    }
}

/// The connected devices as rows, each desktop connection marked on the device it came from (bro's app before a
/// browser at the same address); a viewer no device matches still counts on the Desktop viewers row.
fn device_rows(devices: &[Device], viewers: &[Viewer], t: &crate::theme::Theme) -> Vec<DeviceRow> {
    // per device: None, Some(false) watching, Some(true) in control
    let mut desktop: Vec<Option<bool>> = vec![None; devices.len()];
    for v in viewers {
        let Some(address) = v.address else { continue };
        let same = |d: &&Device| d.address == Some(address);
        let at = devices.iter().position(|d| same(&d) && d.app).or_else(|| devices.iter().position(|d| same(&d)));
        if let Some(i) = at {
            desktop[i] = Some(desktop[i].unwrap_or(false) || !v.read_only);
        }
    }
    devices
        .iter()
        .zip(desktop)
        .map(|(d, desk)| {
            let local = d.address.is_some_and(|a| a.is_loopback());
            let mut parts = vec![if d.app { "bro app".to_string() } else if d.form == Form::Unknown { "client".into() } else { "web app".into() }];
            parts.extend(d.system.clone());
            match (&d.via, d.address) {
                // through the account relay: say who it is; the address it carries only stands in for one
                (Some(who), _) => parts.push(format!("{who}, through your account")),
                // a phone behind a tunnel arrives from loopback: no address worth showing
                (None, Some(_)) if local && d.app => {}
                (None, Some(_)) if local => parts.push("this computer".into()),
                (None, Some(a)) => parts.push(a.to_string()),
                (None, None) => {}
            }
            let status = desk.map(|control| {
                if control { Span::styled("Controlling desktop", fg(t.shine).add_modifier(Modifier::BOLD)) } else { Span::styled("Watching desktop", fg(t.good)) }
            });
            DeviceRow { form: d.form, app: d.app, name: d.name.clone(), detail: parts.join(" · "), status, since: connected_for(d.since_ms), quiet: false }
        })
        .collect()
}

/// The line under Remote desktop: Bro RMM's version and where it's at.
fn package_line(status: Option<&Status>, t: &crate::theme::Theme) -> Span<'static> {
    let Some(status) = status else {
        return Span::styled("Bro RMM starts with phone access", muted(t));
    };
    let version = status.version.as_deref();
    let named = |what: &str| match version {
        Some(v) => format!("Bro RMM {v} · {what}"),
        None => format!("Bro RMM · {what}"),
    };
    let viewers = |n: usize| if n == 1 { "1 viewer".to_string() } else { format!("{n} viewers") };
    // asked this session and nothing newer: say so
    let idle = if status.checked_ms.is_some() { "up to date" } else { "ready" };
    match status.phase {
        Phase::Ready | Phase::Running if status.checking => Span::styled(named("checking for updates…"), fg(t.shine)),
        Phase::Ready | Phase::Running if status.update.is_some() => {
            let newer = status.update.as_deref().unwrap_or_default();
            Span::styled(named(&format!("{newer} is ready to install")), fg(t.shine).add_modifier(Modifier::BOLD))
        }
        Phase::Running if status.active_viewers > 0 => Span::styled(named(&format!("streaming to {}", viewers(status.active_viewers))), fg(t.good)),
        Phase::Ready | Phase::Running => Span::styled(named(idle), fg(t.good)),
        Phase::Starting => Span::styled(named("starting the stream…"), fg(t.shine)),
        Phase::Installing if version.is_some() => Span::styled(named("checking for updates…"), fg(t.shine)),
        Phase::Installing => Span::styled("Downloading Bro RMM…", fg(t.shine)),
        Phase::NotInstalled => Span::styled("Bro RMM isn't downloaded yet", muted(t)),
        Phase::Error if version.is_some() => Span::styled(named("needs attention"), fg(t.danger)),
        Phase::Error => Span::styled("Bro RMM couldn't download", fg(t.danger)),
        Phase::Disabled => Span::styled(named("off"), muted(t)),
    }
}

/// The section: phone access, who's connected, then remote desktop.
pub(super) fn blocks(cx: &Cx) -> Vec<Block> {
    let t = cx.theme;
    let settings = cx.svc.settings();
    let config = &settings.mobile_companion;
    let on = settings.bridge.enabled;
    let status = cx.svc.rmm_status();
    let supported = bro_core::rmm::supported();
    let enabled = config.enabled;
    let item = |option, label: &str, value, help: &str| Item::new(Row::Mobile(option), label, value, help);

    // ---- the way in
    let access_help = if on { "Your paired phone and browsers can follow your sessions, answer your agents and start new work." }
        else { "Off: no phone or browser can reach bro, and remote desktop stays off with it." };
    let mut blocks = vec![Block::Item(item(MobileOpt::Access, "Phone access", switch(on, true, t), access_help))];
    if cx.svc.firewall_applies() {
        cx.svc.check_firewall(crate::services::firewall::Why::Refresh);
        blocks.push(Block::Item(firewall_item(cx)));
    }
    if on {
        blocks.push(Block::Item(item(MobileOpt::Pair, "Pair a phone", shown("Show QR code ›", fg(t.inline)),
            "Scan it with the bro app, or open its link in a browser on the same network.")));
    }

    // ---- who's here
    if on {
        let rows = match devices(cx) {
            Some(list) if !list.is_empty() => device_rows(&list, status.as_ref().map_or(&[][..], |s| &s.viewers), t),
            Some(_) => vec![DeviceRow::placeholder("Nothing connected right now", "Your phone shows up here the moment it connects.")],
            None => vec![DeviceRow::placeholder("Phone access is starting", "Connections show up here once it's listening.")],
        };
        let live = rows.iter().filter(|r| !r.quiet).count();
        blocks.push(Block::Sub(if live > 0 { format!("Connected now · {live}") } else { "Connected now".into() }));
        blocks.push(Block::Devices(rows));
    }

    // ---- remote desktop
    blocks.push(Block::Sub("Remote desktop".into()));
    let remote_help = if !supported { "Remote desktop isn't available for this computer's system." }
        else if enabled { "See and drive this screen from your paired phone." }
        else { "See and drive this screen from your paired phone. Turning it on downloads Bro RMM." };
    let mut remote = item(MobileOpt::Remote, "Remote desktop", switch(enabled, supported, t), remote_help);
    if enabled && supported {
        remote = remote.note(vec![package_line(status.as_ref(), t)]);
        if let Some(label) = update_link(cx) {
            remote = remote.link(label);
        }
    }
    if let Some(note) = bro_core::rmm::session_note() {
        remote = remote.note(vec![Span::styled(note, fg(t.danger))]);
    }
    if let Some(message) = status.as_ref().filter(|_| enabled).and_then(|s| s.message.clone()) {
        remote = remote.note(vec![Span::styled(message, fg(t.danger))]);
    }
    blocks.push(Block::Item(remote));
    if enabled && cfg!(target_os = "macos") {
        cx.svc.check_mac_permissions(crate::services::firewall::Why::Refresh);
        blocks.push(Block::Item(mac_permissions_item(cx)));
    }
    if enabled {
        blocks.push(Block::Item(item(MobileOpt::ReadOnly, "View only", switch(config.read_only, true, t),
            "Phones can watch, never touch: your mouse, keyboard and clipboard stay yours. Changing this ends current connections.")));
        if bro_core::rmm::display_count() > 1 {
            let display = if config.display == 0 { "Primary display".into() } else { format!("Display {}", config.display + 1) };
            blocks.push(Block::Item(item(MobileOpt::Display, "Shared display", choice(display, fg(t.inline), t),
                "The monitor your phone opens on. Changing it ends current connections.")));
        }
    }
    let viewers = status.as_ref().map_or(0, |s| s.active_viewers);
    if viewers > 0 {
        blocks.push(Block::Item(item(MobileOpt::Disconnect, "Desktop viewers", shown(format!("{viewers} connected"), fg(t.good)),
            "End every desktop connection now. A phone can reconnect while remote desktop stays on.")));
    }
    if enabled {
        let relay = status.as_ref().is_some_and(|s| s.relay_configured);
        blocks.push(Block::Text(vec![Span::styled(if relay { "Connects directly over WebRTC when it can, through your private TURN relay when it can't, and over TCP as a last resort." }
            else { "Connects directly over WebRTC when it can, and over TCP through your paired address when it can't." }, muted(t))]));
        blocks.push(Block::Text(vec![Span::styled(bro_core::rmm::at_the_computer(), muted(t))]));
    }
    blocks.push(Block::Sub("JustConnect".into()));
    let jc = bro_core::justconnect::load();
    let signed_in = bro_core::jsh::signed_in();
    let busy = JUSTCONNECT_BUSY.load(Ordering::Acquire);
    let managed = jc.managed();
    if signed_in && !managed {
        let reachable = jc.reachable();
        let value = if SESSIONS_BUSY.load(Ordering::Acquire) { shown(if reachable { "Turning off…" } else { "Adding…" }, fg(t.shine)) } else { switch(reachable, true, t) };
        let help = if reachable { "The bro app on your phone, signed in to this account, lists this computer and opens its sessions from anywhere. Nobody else on the account sees it, and nothing is installed." }
            else if jc.sessions_off { "Off, and it stays off when you sign in again. Turn it on to see and open the sessions here from the bro app on your phone." }
            else { "Puts this computer on your account for your phone only. Nothing is installed." };
        blocks.push(Block::Item(item(MobileOpt::Sessions, "Your sessions on your phone", value, help)));
    }
    let value = if busy { shown(if managed { "Turning off…" } else { "Setting up…" }, fg(t.shine)) } else { switch(managed, signed_in || managed, t) };
    let help = if busy && !managed { "Installing the monitoring service. Say yes when your computer asks for administrator permission." }
        else if !signed_in && !managed { "Sign in to Just Superhuman first. Then this switch adds the computer to your account, so you can reach it from anywhere." }
        else if managed { "You, and anyone you grant access to on your account, can open this desktop, run commands and work in the sessions here, from JustConnect or from the bro app on a phone. Turning it off stops new work and disconnects those phones right away; the monitoring service stays installed." }
        else { "Adds the desktop and a command line, for you and anyone you grant access to on your account, from JustConnect and the bro app on a phone. Installs the monitoring service; your computer asks for administrator permission once." };
    blocks.push(Block::Item(item(MobileOpt::JustConnect, if managed { "Reach this computer from your account" } else { "Remote desktop and commands" }, value, help)));
    // where its connection stands, once it is on
    if jc.enabled && !busy
        && let Some((text, good)) = link_line(bro_bridge::justconnect_link(), std::time::Instant::now())
    {
        blocks.push(Block::Text(vec![Span::styled(text, if good { fg(t.good) } else { fg(t.shine) })]));
    }
    // someone is here through the account right now: say so where the switch that ends it is
    if jc.enabled && !busy && bro_bridge::justconnect_link().tunnel {
        blocks.push(Block::Text(vec![Span::styled("Open from your account right now. Who it is shows under Connected now.", fg(t.good))]));
    }
    if signed_in {
        blocks.push(Block::Text(vec![Span::styled("Every computer on your account, and the sessions on each, are in the Computers view (search \"computers\").", muted(t))]));
    }
    blocks
}

/// This computer's connection to the account in a sentence, and whether it is the good news. None when this
/// window's Bro isn't the one holding the connection (nothing true to say from here).
pub(crate) fn link_line(link: bro_bridge::JustConnectLink, now: std::time::Instant) -> Option<(String, bool)> {
    use bro_bridge::JustConnectPhase as Phase;
    Some(match link.phase {
        Phase::Off => return None,
        Phase::Connecting => ("Connecting to Just Superhuman.".into(), false),
        Phase::Connected if link.held => ("Connected. Work asked of this computer starts at once.".into(), true),
        Phase::Connected => ("Connected. Work asked of this computer starts within a few seconds.".into(), true),
        Phase::Reconnecting => {
            let again = link.retry_at.map(|at| at.saturating_duration_since(now).as_secs()).filter(|s| *s > 0);
            (
                match again {
                    Some(s) => format!("Can't reach Just Superhuman right now. Trying again in {s} s; sessions here keep running."),
                    None => "Can't reach Just Superhuman right now. Trying again; sessions here keep running.".into(),
                },
                false,
            )
        }
        Phase::Refused => ("Just Superhuman isn't accepting this computer's sign-in. Turn this switch off and on to connect it again.".into(), false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn the_connection_to_the_account_reads_as_what_it_means_for_work() {
        use bro_bridge::{JustConnectLink as Link, JustConnectPhase as Phase};
        let now = std::time::Instant::now();
        let link = |phase, held| Link { phase, held, ..Default::default() };
        assert_eq!(link_line(link(Phase::Off, false), now), None, "this window isn't the one connected: say nothing");
        assert_eq!(link_line(link(Phase::Connected, true), now).unwrap(), ("Connected. Work asked of this computer starts at once.".to_string(), true));
        assert!(link_line(link(Phase::Connected, false), now).unwrap().0.contains("within a few seconds"));
        let retry = Link { phase: Phase::Reconnecting, retry_at: Some(now + std::time::Duration::from_secs(12)), ..Default::default() };
        assert_eq!(link_line(retry, now).unwrap(), ("Can't reach Just Superhuman right now. Trying again in 12 s; sessions here keep running.".to_string(), false));
        assert!(link_line(link(Phase::Reconnecting, false), now).unwrap().0.contains("Trying again;"));
        assert!(!link_line(link(Phase::Refused, false), now).unwrap().1);
    }

    fn device(id: u64, app: bool, name: &str, address: &str) -> Device {
        Device {
            id,
            form: if app { Form::Phone } else { Form::Computer },
            app,
            name: name.into(),
            system: app.then(|| "iOS 18.1".to_string()),
            address: Some(address.parse::<IpAddr>().unwrap()),
            since_ms: crate::util::now_ms() - 12 * 60_000,
            via: None,
        }
    }

    #[test]
    fn desktop_viewers_land_on_the_app_at_their_address() {
        let t = crate::theme::get("graphite");
        let list = [device(1, false, "Safari on iPhone", "192.168.1.40"), device(2, true, "iPhone", "192.168.1.40"), device(3, false, "Chrome on Windows", "127.0.0.1")];
        let viewers = [Viewer { address: Some("192.168.1.40".parse().unwrap()), read_only: false }];
        let rows = device_rows(&list, &viewers, &t);
        assert!(rows[0].status.is_none(), "the browser at the same address isn't the one on the desktop");
        assert_eq!(rows[1].status.as_ref().map(|s| s.content.as_ref()), Some("Controlling desktop"));
        assert_eq!(rows[1].detail, "bro app · iOS 18.1 · 192.168.1.40");
        assert_eq!(rows[1].since, "12 min");
        assert_eq!(rows[2].detail, "web app · this computer");
        let watching = device_rows(&list, &[Viewer { address: Some("192.168.1.40".parse().unwrap()), read_only: true }], &t);
        assert_eq!(watching[1].status.as_ref().map(|s| s.content.as_ref()), Some("Watching desktop"));
    }

    #[test]
    fn a_phone_that_came_through_the_account_is_named_by_its_person() {
        let t = crate::theme::get("graphite");
        // the address a relayed phone carries stands in for one: it marks that phone's desktop connection and is never shown
        let stand_in = "fd62:726f::123:4567:89ab:cdef";
        let mut phone = device(1, true, "Pixel 9", stand_in);
        phone.via = Some("James Greene".into());
        let list = [phone, device(2, true, "iPhone", "192.168.1.40")];
        let rows = device_rows(&list, &[Viewer { address: Some(stand_in.parse().unwrap()), read_only: false }], &t);
        assert_eq!(rows[0].detail, "bro app · iOS 18.1 · James Greene, through your account");
        assert_eq!(rows[0].status.as_ref().map(|s| s.content.as_ref()), Some("Controlling desktop"));
        assert!(rows[1].status.is_none());
        assert_eq!(rows[1].detail, "bro app · iOS 18.1 · 192.168.1.40");
    }

    #[test]
    fn the_page_reads_well_in_the_terminal() {
        use super::super::{Section, SettingsView, lay_out};
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut settings = bro_core::config::Settings::default();
        settings.bridge.enabled = true;
        settings.mobile_companion.enabled = true;
        let svc = crate::services::Services::demo(settings, tx.clone());
        let t = crate::theme::get("graphite");
        let mut v = SettingsView::default();
        v.go(Section::Mobile);
        let mut actions = vec![];
        let cx = Cx { id: 1, theme: &t, svc: &svc, tx: &tx, actions: &mut actions, focused: true };
        let lines: Vec<String> = lay_out(Section::Mobile, v.blocks(Section::Mobile, &cx), v.sel, 96, &t)
            .into_iter()
            .map(|(_, l)| l.iter().map(|s| s.content.as_ref()).collect::<String>().trim_end().to_string())
            .collect();
        let page = lines.join("
");
        println!("{page}");
        for want in [
            "Remote Control",
            "Phone access",
            "Pair a phone",
            "Show QR code",
            "CONNECTED NOW · 3",
            "iPhone",
            "Controlling desktop",
            "bro app · iOS 18.1 · 192.168.1.40",
            "14 min",
            "1 h 3 min",
            "web app · this computer",
            "REMOTE DESKTOP",
            "Bro RMM 0.1.0 · streaming to 1 viewer",
            "View only",
            "Desktop viewers",
        ] {
            assert!(page.contains(want), "missing {want:?} in
{page}");
        }
        // the check leaves a running stream alone, so it's there while the iPhone is on the desktop
        assert!(page.contains("u: check for updates"), "{page}");
        assert!(lines.iter().all(|l| crate::ui::width(l) <= 96), "nothing runs past the width");
        assert_eq!(summary(&cx).content, "live");
    }

    #[test]
    fn connection_times_read_plainly() {
        let now = crate::util::now_ms();
        assert_eq!(connected_for(now - 20_000), "just now");
        assert_eq!(connected_for(now - 125 * 60_000), "2 h 5 min");
        assert_eq!(connected_for(now - 120 * 60_000), "2 h");
        assert_eq!(connected_for(now - 3 * 24 * 60 * 60_000), "3 days");
    }
}
