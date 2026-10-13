//! JustWatch's skill: switching it on (finding the skill on a background thread) or off.

use super::{Services, guard, guard_res};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::justwatch;

impl Services {
    fn set_justwatch_skill(&self, on: bool) {
        let mut s = self.settings();
        if s.justwatch.skill != on {
            s.justwatch.skill = on;
            self.save_settings(s);
        }
    }

    /// Give new Claude and Codex sessions the JustWatch skill, or take bro's copies back out.
    pub fn set_justwatch_skill_on(&self, on: bool) {
        if self.is_demo() {
            return self.set_justwatch_skill(on);
        }
        let svc = self.clone();
        if !on {
            self.set_justwatch_skill(false);
            std::thread::spawn(move || {
                let _ = guard("justwatch::remove_skill_everywhere", justwatch::remove_skill_everywhere);
                svc.wake();
            });
            return;
        }
        std::thread::spawn(move || {
            if guard("justwatch::installed", justwatch::installed).ok().flatten().is_none() {
                svc.send(Event::Toast(Kind::Info, "install JustWatch first (Quick Install): its skill drives the app".into()));
                return;
            }
            match guard_res("justwatch::fetch_skill", justwatch::fetch_skill) {
                Ok(()) => {
                    svc.set_justwatch_skill(true);
                    svc.send(Event::Toast(Kind::Info, "JustWatch skill on · new Claude and Codex sessions get it".into()));
                }
                Err(e) => svc.send(Event::Toast(Kind::Error, format!("couldn't get the JustWatch skill: {e}"))),
            }
        });
    }
}
