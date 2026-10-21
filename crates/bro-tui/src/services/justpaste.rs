//! JustPaste's Smart Paste for File Explorer (Windows, comes with JustTools): switching it on or off on a
//! background thread and publishing where it ended up into [`super::justtools::JustToolsState`].

use super::{Services, guard_res};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::justpaste;
use std::time::Instant;

impl Services {
    /// Smart Paste on (now and at sign-in) or off.
    pub fn set_smart_paste(&self, on: bool) {
        if self.is_demo() {
            return self.send(Event::Toast(Kind::Info, "the demo doesn't change Explorer".into()));
        }
        let Some(jt) = self.state().justtools.installed.clone() else {
            return self.send(Event::Toast(Kind::Info, "install JustTools first: JustPaste comes with it".into()));
        };
        {
            let mut st = self.inner.state.write();
            if st.justtools.paste_busy.is_some() {
                return;
            }
            st.justtools.paste_busy = Some(Instant::now());
        }
        self.wake();
        let svc = self.clone();
        std::thread::spawn(move || {
            let result = guard_res("justpaste::set", || justpaste::set(&jt, on));
            let toast = match &result {
                Ok(s) if s.running != on => (Kind::Error, format!("Smart Paste is still {} · `justpaste --status` says why", justpaste::describe(s))),
                Ok(s) if on => (Kind::Info, format!("Smart Paste on · Ctrl+V in Explorer saves copied images and downloads copied links · {}", if s.filters { "copy filters are on" } else { "file copies stay Windows'" })),
                Ok(_) => (Kind::Info, "Smart Paste off · Ctrl+V in Explorer is all Windows again".into()),
                Err(e) => (Kind::Error, format!("couldn't switch Smart Paste: {e}")),
            };
            svc.update(|st| {
                st.justtools.paste_busy = None;
                if let Ok(s) = result {
                    st.justtools.paste = Some(s);
                }
            });
            svc.send(Event::Toast(toast.0, toast.1));
        });
    }
}
