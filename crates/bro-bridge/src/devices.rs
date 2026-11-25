//! Who is connected: every `/ws` client as bro's Remote Control page lists it (bro's phone app, a browser, or
//! something else), from what it says about itself as it connects. bro's app names itself in its query
//! (`client=app&device=iPhone&os=iOS 18.1`); a browser is read from its user agent.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};

/// What a client runs on, for its icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Form {
    Phone,
    Tablet,
    Computer,
    #[default]
    Unknown,
}

/// A connected client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: u64,
    pub form: Form,
    /// bro's phone app (else a web page or another client)
    pub app: bool,
    /// what it is: "iPhone", "Pixel 9", "Chrome on Windows"
    pub name: String,
    /// its system, when known: "iOS 18.1", "Android 15"
    pub system: Option<String>,
    /// where it connected from
    pub address: Option<IpAddr>,
    /// when it connected (unix ms)
    pub since_ms: i64,
    /// Who it is, when it came through the person's Just Superhuman account instead of the local network: their
    /// name (or address), as the account server gave it.
    #[serde(default)]
    pub via: Option<String>,
}

/// The connected clients, by the order they came in.
#[derive(Default)]
pub struct Devices {
    next: AtomicU64,
    list: Mutex<BTreeMap<u64, Device>>,
}

impl Devices {
    /// A client connected; its id is its key for [`Devices::remove`].
    pub(crate) fn add(&self, about: About, address: Option<IpAddr>, via: Option<String>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let since_ms = chrono::Utc::now().timestamp_millis();
        let device = Device { id, form: about.form, app: about.app, name: about.name, system: about.system, address, since_ms, via };
        self.list.lock().insert(id, device);
        id
    }

    pub(crate) fn remove(&self, id: u64) {
        self.list.lock().remove(&id);
    }

    pub(crate) fn list(&self) -> Vec<Device> {
        self.list.lock().values().cloned().collect()
    }
}

/// What a client says it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct About {
    pub form: Form,
    pub app: bool,
    pub name: String,
    pub system: Option<String>,
}

/// A short, printable piece of a query: control characters dropped, 48 characters at most.
fn clean(value: &str) -> Option<String> {
    let text: String = value.chars().filter(|c| !c.is_control()).take(48).collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// What a client connecting with `query` (the `/ws` URL's) and `user_agent` is.
pub(crate) fn describe(query: Option<&str>, user_agent: Option<&str>) -> About {
    let mut app = false;
    let (mut device, mut system) = (None, None);
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        match key.as_ref() {
            "client" => app = value == "app",
            "device" => device = clean(&value),
            "os" => system = clean(&value),
            _ => {}
        }
    }
    if app {
        let name = device.unwrap_or_else(|| "Phone".into());
        let lower = name.to_ascii_lowercase();
        let form = if lower.contains("ipad") || lower.contains("tablet") { Form::Tablet } else { Form::Phone };
        return About { form, app, name, system };
    }
    from_user_agent(user_agent.unwrap_or("").trim())
}

/// A client from its user agent alone: a browser ("Safari on iPhone"), bro's app from before it named itself (no
/// user agent on iOS, OkHttp's on Android), or a tool.
fn from_user_agent(ua: &str) -> About {
    if ua.is_empty() {
        // iOS sends no user agent on an app's WebSocket; browsers and tools always send one
        return About { form: Form::Phone, app: true, name: "Phone".into(), system: None };
    }
    if ua.starts_with("okhttp/") {
        return About { form: Form::Phone, app: true, name: "Android phone".into(), system: None };
    }
    if !ua.contains("Mozilla/") {
        let tool = ua.split(['/', ' ']).next().filter(|s| !s.is_empty()).unwrap_or("Client");
        return About { form: Form::Unknown, app: false, name: clean(tool).unwrap_or_else(|| "Client".into()), system: None };
    }
    let version = |marker: &str, until: &[char]| {
        ua.split_once(marker).map(|(_, rest)| rest.split(until).next().unwrap_or("").replace('_', ".")).filter(|v| !v.is_empty())
    };
    let (form, place, system) = if ua.contains("iPhone") {
        (Form::Phone, "iPhone", version("iPhone OS ", &[' ', ';', ')']).map(|v| format!("iOS {v}")))
    } else if ua.contains("iPad") {
        (Form::Tablet, "iPad", version("CPU OS ", &[' ', ';', ')']).map(|v| format!("iPadOS {v}")))
    } else if ua.contains("Android") {
        let form = if ua.contains("Mobile") { Form::Phone } else { Form::Tablet };
        (form, if form == Form::Phone { "Android" } else { "Android tablet" }, version("Android ", &[';', ')', ' ']).map(|v| format!("Android {v}")))
    } else if ua.contains("Windows") {
        (Form::Computer, "Windows", None)
    } else if ua.contains("CrOS") {
        (Form::Computer, "ChromeOS", None)
    } else if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        (Form::Computer, "Mac", None)
    } else if ua.contains("Linux") {
        (Form::Computer, "Linux", None)
    } else {
        (Form::Unknown, "", None)
    };
    let browser = if ua.contains("Edg/") || ua.contains("EdgA/") || ua.contains("EdgiOS/") {
        "Edge"
    } else if ua.contains("OPR/") {
        "Opera"
    } else if ua.contains("Firefox/") || ua.contains("FxiOS/") {
        "Firefox"
    } else if ua.contains("CriOS/") || ua.contains("Chrome/") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "Browser"
    };
    let name = if place.is_empty() { browser.to_string() } else { format!("{browser} on {place}") };
    About { form, app: false, name, system }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_names_itself() {
        let a = describe(Some("token=abc&client=app&device=Pixel%209&os=Android%2015"), Some("okhttp/4.12.0"));
        assert_eq!(a, About { form: Form::Phone, app: true, name: "Pixel 9".into(), system: Some("Android 15".into()) });
        let ipad = describe(Some("client=app&device=iPad&os=iPadOS%2018.1"), None);
        assert_eq!(ipad.form, Form::Tablet);
        // nothing printable, nothing long
        let odd = describe(Some(&format!("client=app&device=%0A%0D&os={}", "x".repeat(200))), None);
        assert_eq!(odd.name, "Phone");
        assert_eq!(odd.system.map(|s| s.len()), Some(48));
    }

    #[test]
    fn older_app_builds_still_read_as_phones() {
        assert_eq!(describe(Some("token=abc"), None), About { form: Form::Phone, app: true, name: "Phone".into(), system: None });
        assert_eq!(describe(None, Some("okhttp/4.9.2")).name, "Android phone");
    }

    #[test]
    fn browsers_are_read_from_their_user_agent() {
        let safari = describe(None, Some("Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1"));
        assert_eq!(safari, About { form: Form::Phone, app: false, name: "Safari on iPhone".into(), system: Some("iOS 18.1".into()) });
        let chrome = describe(None, Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"));
        assert_eq!((chrome.form, chrome.name.as_str()), (Form::Computer, "Chrome on Windows"));
        let edge = describe(None, Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0"));
        assert_eq!(edge.name, "Edge on Windows");
        let android = describe(None, Some("Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36"));
        assert_eq!((android.form, android.name.as_str(), android.system.as_deref()), (Form::Phone, "Chrome on Android", Some("Android 15")));
        let mac = describe(None, Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15"));
        assert_eq!(mac.name, "Safari on Mac");
        assert_eq!(describe(None, Some("curl/8.9.1")).name, "curl");
    }

    #[test]
    fn devices_come_and_go_in_order() {
        let d = Devices::default();
        let a = d.add(describe(None, None), None, None);
        let b = d.add(describe(None, Some("curl/8")), None, None);
        assert_eq!(d.list().iter().map(|x| x.id).collect::<Vec<_>>(), vec![a, b]);
        d.remove(a);
        assert_eq!(d.list().len(), 1);
    }
}
