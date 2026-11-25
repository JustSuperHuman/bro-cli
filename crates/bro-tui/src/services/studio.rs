//! Create Slop: who can be cast and what this machine has for making media ([`bro_core::studio`]). Nothing is
//! looked at until the launcher's Create Slop tab opens; then a background thread shows the roster kept from last
//! time, looks at the machine, and fetches the roster again when the kept one is old. It all lands in
//! [`StudioState`], which the launcher reads.

use super::{Services, guard, guard_res};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::studio::{self, Character, Facts, Kit, ToolStatus};
use std::path::PathBuf;

/// What bro knows for the Create Slop tab.
#[derive(Clone, Debug, Default)]
pub struct StudioState {
    /// a look is under way
    pub loading: bool,
    /// the machine has been looked at: the toolkit is known
    pub checked: bool,
    /// Claudia, then SLOPCANNON's roster
    pub cast: Vec<Character>,
    /// when the roster was fetched (unix seconds)
    pub fetched_at: Option<i64>,
    /// why the roster couldn't be fetched (the one kept from before still shows)
    pub error: Option<String>,
    pub facts: Facts,
    pub tools: Vec<ToolStatus>,
    /// the kit each project last cooked with
    pub remembered: Vec<(PathBuf, Kit)>,
}

/// Claudia first, then the roster.
fn cast_of(roster: Vec<Character>) -> Vec<Character> {
    std::iter::once(Character::claudia()).chain(roster).collect()
}

/// The demo's roster: a few of SLOPCANNON's characters by name, without pictures to fetch.
fn demo_cast() -> Vec<Character> {
    let one = |slug: &str, name: &str, tagline: &str, creator: &str, refs: u32| Character {
        source: studio::Source::Slop,
        slug: slug.into(),
        version: 1,
        name: name.into(),
        tagline: tagline.into(),
        creator: creator.into(),
        tags: vec![],
        refs,
        motion: true,
        portrait: None,
        focus: None,
    };
    cast_of(vec![
        one("chinese-bone-b6b8d48b5a2f", "Chinese Bone", "Owner of Slop Cannon. Runs it out of a secret Florida data center.", "AirBudd", 6),
        one("becky-77c85f6bce0f", "Becky", "Blond haired girl", "Marty", 4),
        one("hildegarde-c605375ed1b0", "Hildegarde", "Auburn haired girl", "Marty", 4),
        one("deep-singer-b9ba0e563649", "Deep Singer", "A burly bass background singer", "Marty", 6),
        one("astra-3961fd2727cc", "Astra", "A graceful young woman who is secretly an advanced humanoid robot", "JedPowers", 6),
        one("tv-boy-969601704e0c", "TV Boy", "A TV-headed host with a reruns-and-mute gimmick", "Jobi", 6),
        one("gustav-24fd8a74742d", "Gustav", "A retired Viennese conductor whose hair never settles", "ryan", 6),
        one("hollis-7bd99a01b250", "Hollis", "A Maine lobster boat captain who has seen worse weather", "ryan", 6),
        one("rook-ca5afdbaef3f", "Rook", "A 1997 PlayStation-era bounty hunter who never got the sequel", "Ryan", 6),
        one("armand-66d6b080bd1c", "Armand", "A four-armed watch salesman who doesn't mention it", "Ryan", 6),
        one("hal-denning-4a2d06beda63", "Hal Denning", "A salesman who has never sold the product and never stopped trying", "ryan", 6),
    ])
}

/// What the demo's machine has. `BRO_DEMO_STUDIO=bare` is a machine with none of it and `=partial` one that is
/// halfway there (for captures of what the toolkit offers to do).
fn demo_facts() -> Facts {
    let running = Facts { justimagine_url: Some("http://127.0.0.1:8793".into()), justimagine_installed: true, ffmpeg: true, node: true, ..Facts::default() };
    match std::env::var("BRO_DEMO_STUDIO").as_deref() {
        Ok("bare") => Facts::default(),
        Ok("partial") => Facts { justimagine_url: None, ffmpeg: false, slopcannon_token_env: Some(studio::SLOPCANNON_TOKEN_ENVS[0].into()), ..running },
        _ => running,
    }
}

impl Services {
    /// Look at the toolkit and the roster for the Create Slop tab. `force`: fetch the roster again even when the
    /// kept one is fresh.
    pub fn refresh_studio(&self, force: bool) {
        // the demo has a roster of its own; `BRO_DEMO_STUDIO=live` shows the real one (for captures)
        if self.is_demo() && std::env::var("BRO_DEMO_STUDIO").as_deref() != Ok("live") {
            // a look again keeps what the demo was given since (a key typed into the toolkit)
            let facts = Some(self.state().studio.facts.clone()).filter(|_| self.state().studio.checked).unwrap_or_else(demo_facts);
            return self.update(|st| {
                st.studio = StudioState { checked: true, cast: demo_cast(), tools: studio::statuses(&facts), facts, ..StudioState::default() };
            });
        }
        {
            let mut st = self.inner.state.write();
            if st.studio.loading {
                return;
            }
            st.studio.loading = true;
        }
        self.wake();
        let svc = self.clone();
        std::thread::spawn(move || {
            // what was kept shows at once
            let kept = guard("studio::load_roster", studio::load_roster).ok().flatten();
            let remembered = guard("studio::remembered_all", studio::remembered_all).unwrap_or_default();
            let fetched = kept.as_ref().map(|k| k.0);
            svc.update(|st| {
                if let Some((at, roster)) = kept {
                    (st.studio.cast, st.studio.fetched_at) = (cast_of(roster), Some(at));
                } else if st.studio.cast.is_empty() {
                    st.studio.cast = cast_of(vec![]);
                }
                st.studio.remembered = remembered;
            });
            let facts = guard("studio::facts", Facts::gather).unwrap_or_default();
            svc.update(|st| {
                st.studio.tools = studio::statuses(&facts);
                st.studio.facts = facts;
                st.studio.checked = true;
            });
            let stale = fetched.is_none_or(|at| bro_core::util::now_secs() - at > studio::ROSTER_FRESH_SECS);
            if force || stale {
                match guard_res("studio::fetch_roster", studio::fetch_roster) {
                    Ok(roster) => {
                        let _ = guard("studio::save_roster", || studio::save_roster(&roster));
                        svc.update(|st| (st.studio.cast, st.studio.fetched_at, st.studio.error) = (cast_of(roster), Some(bro_core::util::now_secs()), None));
                    }
                    Err(e) => svc.update(|st| st.studio.error = Some(e)),
                }
            }
            svc.update(|st| st.studio.loading = false);
        });
    }

    /// Keep `key` for media tool `tool` (one of [`studio::KEYS`]) and look at the toolkit again with it.
    pub fn save_studio_key(&self, tool: &'static str, key: String) {
        let name = studio::tool(tool).map_or(tool, |t| t.name);
        if self.is_demo() {
            // the demo keeps nothing: its toolkit just has the key from now on
            self.update(|st| {
                match tool {
                    studio::ELEVENLABS => st.studio.facts.elevenlabs_key = true,
                    studio::HIGGSFIELD => st.studio.facts.higgsfield_key = true,
                    studio::SLOPCANNON => st.studio.facts.slopcannon_token_env = Some(studio::SLOPCANNON_TOKEN_ENVS[0].into()),
                    _ => {}
                }
                st.studio.tools = studio::statuses(&st.studio.facts);
            });
            return self.send(Event::Toast(Kind::Info, format!("{name}'s key is kept (the demo forgets it when it closes)")));
        }
        let svc = self.clone();
        std::thread::spawn(move || match guard_res("studio::save_key", || studio::save_key(tool, &key)) {
            Ok(()) => {
                let facts = guard("studio::facts", Facts::gather).unwrap_or_default();
                svc.update(|st| {
                    st.studio.tools = studio::statuses(&facts);
                    st.studio.facts = facts;
                    st.studio.checked = true;
                });
                svc.send(Event::Toast(Kind::Info, format!("{name}'s key is kept with your other keys. Sessions get it from now on.")));
            }
            Err(e) => svc.send(Event::Toast(Kind::Error, format!("{name}'s key wasn't saved: {e}"))),
        });
    }

    /// Keep `kit` as what `project` cooks with, here and on disk (off the UI thread).
    pub fn remember_studio(&self, project: PathBuf, kit: Kit) {
        self.update(|st| {
            st.studio.remembered.retain(|(dir, _)| !bro_core::util::same_path(dir, &project));
            st.studio.remembered.insert(0, (project.clone(), kit.clone()));
        });
        if self.is_demo() {
            return;
        }
        std::thread::spawn(move || {
            let _ = guard("studio::remember", || studio::remember(&project, &kit));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::fallback_settings;

    #[test]
    fn the_demo_has_a_cast_and_a_toolkit_without_the_network() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx);
        assert!(svc.state().studio.cast.is_empty(), "nothing is looked at until the tab opens");
        svc.refresh_studio(false);
        let st = svc.state();
        assert!(st.studio.checked && !st.studio.loading);
        assert_eq!(st.studio.cast[0].name, "Claudia", "Claudia leads the roster");
        assert!(st.studio.cast.len() > 8);
        assert_eq!(st.studio.tools.len(), studio::TOOLS.len());
    }

    #[test]
    fn a_key_typed_in_the_demo_shows_as_found() {
        let (tx, rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx);
        svc.refresh_studio(false);
        let slop = |svc: &Services| svc.state().studio.tools.iter().find(|t| t.id == studio::SLOPCANNON).cloned().unwrap();
        assert_eq!((slop(&svc).ready, slop(&svc).keyless), (studio::Ready::SignIn, true));
        svc.save_studio_key(studio::SLOPCANNON, "tok".into());
        assert_eq!((slop(&svc).ready, slop(&svc).keyless, slop(&svc).note.as_str()), (studio::Ready::Yes, false, "token found"));
        assert!(rx.try_iter().any(|e| matches!(e, Event::Toast(Kind::Info, m) if m.starts_with("SLOPCANNON's key is kept"))));
        // looking again keeps it
        svc.refresh_studio(true);
        assert_eq!(slop(&svc).ready, studio::Ready::Yes);
    }

    #[test]
    fn a_kit_is_remembered_for_its_project() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx);
        let project = std::env::temp_dir().join("bro-studio-demo-project");
        let kit = Kit { cast: vec![Character::claudia()], tools: vec![studio::FFMPEG.into()] };
        svc.remember_studio(project.clone(), kit.clone());
        svc.remember_studio(project.clone(), Kit::default());
        svc.remember_studio(project.clone(), kit.clone());
        assert_eq!(svc.state().studio.remembered, [(project, kit)], "once per project, the newest");
    }
}
