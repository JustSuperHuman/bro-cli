//! The phone uses the desktop's choices without opening or changing its menus.

use super::*;
use bro_bridge::FocusControlChange;
use serde_json::{Value, json};

impl App {
    pub(crate) fn focus_remote_controls(&mut self, id: PaneId, change: Option<FocusControlChange>) -> Result<Value, String> {
        let term = self.panes.get(&id).and_then(|p| p.as_term_ref()).filter(|t| focusable(t))
            .ok_or("Model and thinking controls are available in Claude and Codex sessions.")?;
        if let Some(change) = change {
            if self.focus_choice(id).is_some() { return Err("Answer the current question before changing the model or thinking level.".into()); }
            let version = term.insights().and_then(|s| s.conversation).and_then(|c| c.version.clone());
            if term.meta.harness == Some(Harness::Codex) && version.is_some() && !codex_takes_arguments(version.as_deref()) {
                return Err("This Codex version changes models and thinking in its terminal. Update Codex to use these controls.".into());
            }
            let (kind, value, label) = match change {
                FocusControlChange::Model { value } => {
                    let models = self.focus_models(id).ok_or("The models are still loading. Try again.")?;
                    let model = models.iter().find(|m| m.id == value).ok_or("That model is no longer available. Refresh the list.")?;
                    (MenuKind::Model, value, model.name.clone())
                }
                FocusControlChange::Thinking { value } => {
                    let (levels, _) = self.focus_levels(id).ok_or("The thinking levels are still loading. Try again.")?;
                    if !levels.iter().any(|l| l.id == value) { return Err("That thinking level is no longer available. Refresh the list.".into()); }
                    let label = level_name(&value);
                    (MenuKind::Thinking, value, label)
                }
            };
            self.focus_pick_row(id, kind, MenuRow { id: value, label, detail: None, current: false, default: false });
        }
        let (model, effort) = self.focus_running_on(id);
        let models = self.focus_models(id).map(|ms| ms.iter().map(|m| json!({ "id": m.id, "name": m.name, "detail": m.detail })).collect::<Vec<_>>()).unwrap_or_default();
        let (levels, default) = self.focus_levels(id).unwrap_or_default();
        Ok(json!({
            "model": model,
            "effort": effort,
            "models": models,
            "levels": levels.iter().map(|l| json!({ "id": l.id, "detail": l.detail })).collect::<Vec<_>>(),
            "defaultLevel": default,
            "pending": self.focus_change_waits(id),
        }))
    }
}
