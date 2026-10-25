//! Agent teams in the app: the team board overlay (prefix M, or a click on the team under a session in the
//! sidebar), kept live from the lead's transcript worker, and the running teams the settings view lists.

use super::{App, Overlay};
use crate::alerts::Kind;
use crate::layout::PaneId;
use crate::team_board::{Outcome, TeamView};
use bro_core::Harness;

impl App {
    /// The team board for `pane` (None: the focused session).
    pub(crate) fn show_team(&mut self, pane: Option<PaneId>) {
        let Some(pane) = pane.or_else(|| self.focused()) else { return };
        let Some(t) = self.panes.get(&pane).and_then(|p| p.as_term_ref()) else {
            self.toast(Kind::Info, "agent team: focus a Claude session first");
            return;
        };
        let why = match t.meta.harness {
            Some(Harness::Claude) => None,
            Some(h) => Some(format!("Agent teams are a Claude Code feature: {} sessions don't have one.", h.label())),
            None => Some("A shell has no agent team. Agent teams are a Claude Code feature.".into()),
        };
        let enabled = self.svc.settings().claude_env.agent_teams();
        let v = TeamView::new(pane, t.display_name(), t.team(), enabled, why);
        self.sel = None;
        self.overlay = Overlay::Team(Box::new(v));
    }

    /// Before each draw: the board shows the team as its lead's worker last read it.
    pub(crate) fn refresh_team_view(&mut self) {
        let Overlay::Team(v) = &self.overlay else { return };
        let term = self.panes.get(&v.pane).and_then(|p| p.as_term_ref());
        let (team, gone) = (term.and_then(|t| t.team()), term.is_none());
        if let Overlay::Team(v) = &mut self.overlay {
            v.team = team;
            if gone {
                v.why = Some("The session ended, and its team with it.".into());
            }
        }
    }

    pub(crate) fn team_outcome(&mut self, o: Outcome) {
        let Overlay::Team(v) = &self.overlay else { return };
        let pane = v.pane;
        match o {
            Outcome::None => {}
            Outcome::Close => self.overlay = Overlay::None,
            Outcome::Go => {
                self.overlay = Overlay::None;
                if self.panes.contains_key(&pane) {
                    self.focus_pane(pane);
                    self.leave_sidebar();
                }
            }
            Outcome::Settings => {
                self.overlay = Overlay::None;
                self.open_team_settings();
            }
        }
    }

    /// The settings view, on agent teams.
    pub(crate) fn open_team_settings(&mut self) {
        let open = self.panes.iter().find(|(_, p)| p.view() == Some("settings")).map(|(id, _)| *id);
        match open {
            Some(id) => {
                self.focus_pane(id);
                self.leave_sidebar();
            }
            None => self.open_view("settings"),
        }
        if let Some(v) = self.panes.values_mut().find(|p| p.view() == Some("settings")).and_then(|p| p.as_any_mut().downcast_mut::<crate::views::settings::SettingsView>()) {
            v.show_agent_teams();
        }
    }

    /// Every team running in a session here, for the settings view (after each batch of events).
    pub(crate) fn publish_teams(&self) {
        let mut teams: Vec<crate::services::LiveTeam> = self
            .panes
            .iter()
            .filter_map(|(id, p)| {
                let t = p.as_term_ref()?;
                let team = t.team().filter(|team| !team.is_empty())?;
                Some(crate::services::LiveTeam { lead: t.display_name(), project: t.meta.project.name.clone(), seq: self.seqs.get(id).copied().unwrap_or(*id), team })
            })
            .collect();
        teams.sort_by_key(|l| l.seq);
        self.svc.set_live_teams(teams);
    }
}
