//! Browser completion resumes only its owning idle agent, through the same
//! confirmed paste/Enter delivery path as a normal conversation message.
use super::App;
use crate::pane::Activity;
use crate::panes::term::{Delivery, DeliveryState};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct State {
    pub(super) selected: usize,
    sent: HashMap<String, (String, Delivery, Instant)>,
}

impl App {
    /// Give each session the browser handoff it asked for (none once it's done): it needs you while one waits.
    pub(super) fn sync_browser_handoffs(&mut self) {
        let pending = self.svc.browser_handoffs();
        if pending.is_empty() && self.panes.values().all(|p| p.as_term_ref().is_none_or(|t| t.browser_handoff.is_none())) {
            return;
        }
        for pane in self.panes.values_mut() {
            let Some(term) = pane.as_term() else { continue };
            let next = pending.iter().find(|h| h.session.as_deref() == Some(term.meta.sid.as_str()));
            if term.browser_handoff.as_ref() != next {
                term.browser_handoff = next.cloned();
            }
        }
    }

    pub(super) fn browser_resume(
        &mut self,
        id: String,
        event: String,
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<bool, String>>,
    ) {
        if reply.is_closed() {
            return;
        }
        if self
            .browser_handoffs
            .sent
            .get(&event)
            .is_some_and(|(_, delivery, _)| matches!(delivery.state(), DeliveryState::Failed(_)))
        {
            self.browser_handoffs.sent.remove(&event);
        }
        let delivery = if let Some((session, delivery, _)) = self.browser_handoffs.sent.get(&event)
        {
            if *session != id {
                let _ = reply.send(Err("Browser handoff identity changed.".into()));
                return;
            }
            delivery.clone()
        } else {
            let Some(pane) = self.term_id(&id) else {
                let _ = reply.send(Err("The browser's owning session is not open.".into()));
                return;
            };
            if self.focus.drafts.get(&pane).is_some_and(|draft| {
                !draft.area.text().trim().is_empty() || !draft.attachments.is_empty()
            }) {
                let _ = reply.send(Ok(false));
                return;
            }
            let Some(term) = self.panes.get_mut(&pane).and_then(|pane| pane.as_term()) else {
                let _ = reply.send(Ok(false));
                return;
            };
            if term.meta.harness.is_none()
                || crate::pane::Pane::activity(term) != Some(Activity::Idle)
                || term.focus_choice().is_some()
                || !term.background_editor_empty()
            {
                let _ = reply.send(Ok(false));
                return;
            }
            let delivery = term.send_message(&text);
            self.browser_handoffs
                .sent
                .retain(|_, (_, _, at)| at.elapsed() < Duration::from_secs(86_400));
            self.browser_handoffs
                .sent
                .insert(event, (id, delivery.clone(), Instant::now()));
            delivery
        };
        let _ = std::thread::Builder::new()
            .name("bro-browser-resume".into())
            .spawn(move || {
                let until = Instant::now() + Duration::from_secs(30);
                while !reply.is_closed() {
                    match delivery.state() {
                        DeliveryState::Submitted => {
                            let _ = reply.send(Ok(true));
                            return;
                        }
                        DeliveryState::Failed(error) => {
                            let _ = reply.send(Err(error));
                            return;
                        }
                        DeliveryState::Sending if Instant::now() < until => {
                            std::thread::sleep(Duration::from_millis(50))
                        }
                        _ => {
                            let _ = reply.send(Err(
                                "The browser wake-up has not confirmed submission yet.".into(),
                            ));
                            return;
                        }
                    }
                }
            });
    }
}
