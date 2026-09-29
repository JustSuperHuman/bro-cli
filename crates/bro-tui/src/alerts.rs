//! Toasts and desktop notifications. A toast shows bottom-right for a few seconds; loud kinds also raise an OS
//! notification, but only while the terminal window is unfocused (tracked via FocusGained/FocusLost).

use std::time::{Duration, Instant};

/// What a toast is about (sets its glyph, colour and whether it's worth a desktop notification).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Info,
    AgentDone,
    NeedsYou,
    Bridge,
    Usage,
    Error,
}

impl Kind {
    /// Glyph shown before the text.
    pub fn glyph(self) -> &'static str {
        match self {
            Kind::Info => "·",
            Kind::AgentDone => "●",
            Kind::NeedsYou => "●",
            Kind::Bridge => "⌁",
            Kind::Usage => "◔",
            Kind::Error => "×",
        }
    }
    /// Worth a desktop notification when you're in another window.
    pub fn loud(self) -> bool {
        matches!(self, Kind::AgentDone | Kind::NeedsYou | Kind::Bridge)
    }
}

/// One toast on screen.
#[derive(Clone, Debug)]
pub struct Toast {
    pub kind: Kind,
    pub text: String,
    pub at: Instant,
}

/// How long a toast stays up.
pub const TOAST_FOR: Duration = Duration::from_secs(4);

/// The toast queue: newest last, at most three shown.
#[derive(Default)]
pub struct Toasts {
    pub items: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, kind: Kind, text: impl Into<String>) {
        let text = text.into();
        self.items.retain(|t| t.text != text);
        self.items.push(Toast { kind, text, at: Instant::now() });
        if self.items.len() > 3 {
            self.items.remove(0);
        }
    }
    /// Drop expired toasts; true if any went.
    pub fn expire(&mut self) -> bool {
        let n = self.items.len();
        self.items.retain(|t| t.at.elapsed() < TOAST_FOR + Duration::from_millis(extra_ms(&t.text)));
        n != self.items.len()
    }
    /// Time until the next toast expires.
    pub fn next_expiry(&self) -> Option<Duration> {
        self.items.iter().map(|t| (TOAST_FOR + Duration::from_millis(extra_ms(&t.text))).saturating_sub(t.at.elapsed())).min()
    }
    #[cfg(test)]
    pub fn latest(&self) -> Option<&Toast> {
        self.items.last()
    }
}

/// Long toasts linger a little longer.
fn extra_ms(text: &str) -> u64 {
    (text.chars().count() as u64).saturating_sub(40) * 40
}

/// A notification from the OS: a Windows toast, or notify-send. Never blocks, never in tests.
pub fn desktop(title: &str, body: &str) {
    if cfg!(test) {
        return;
    }
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\'', "&apos;").replace('"', "&quot;");
            let script = format!(
                "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null;\
                 [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null;\
                 $x = New-Object Windows.Data.Xml.Dom.XmlDocument;\
                 $x.LoadXml('<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>');\
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show([Windows.UI.Notifications.ToastNotification]::new($x))",
                esc(&title),
                esc(&body)
            );
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
            let ps = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
            let _ = std::process::Command::new(ps)
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .env("PSModulePath", format!("{root}\\System32\\WindowsPowerShell\\v1.0\\Modules"))
                .creation_flags(0x0800_0000)
                .output();
        }
        #[cfg(not(windows))]
        {
            let _ = std::process::Command::new("notify-send").args(["-a", "bro", &title, &body]).output();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_queue() {
        let mut t = Toasts::default();
        for i in 0..5 {
            t.push(Kind::Info, format!("n{i}"));
        }
        assert_eq!(t.items.len(), 3);
        assert_eq!(t.latest().unwrap().text, "n4");
        t.push(Kind::Info, "n3");
        assert_eq!(t.items.len(), 3, "duplicates replace");
        assert!(t.next_expiry().is_some());
    }
}
