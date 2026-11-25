//! A portable draft and revision-bound background palette work for the native theme editor.
use super::{Theme, import};
use crate::pane::Event;
use crate::textarea::TextArea;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Map, Value};
use std::sync::mpsc::Sender;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Field {
    None,
    Name,
    Color,
    Search,
    Css,
}

pub enum Reply {
    Draft {
        owner: Uuid,
        revision: u64,
        result: Result<import::Imported, String>,
    },
    Saved {
        owner: Uuid,
        result: Result<String, String>,
    },
}

pub struct Editor {
    pub owner: Uuid,
    pub before: Theme,
    pub file: Value,
    pub light: bool,
    pub advanced: bool,
    pub inspect: bool,
    pub minimized: bool,
    pub field: Field,
    pub name: TextArea,
    pub color: TextArea,
    pub search: TextArea,
    pub css: TextArea,
    pub role: &'static str,
    pub error: Option<String>,
    pub save_error: Option<String>,
    pub revision: u64,
    pub pending: bool,
    pub saving: bool,
    pub scroll: usize,
    pub checkpoint: Option<Value>,
    pub hsv: (f32, f32, f32),
    pub picker_drag: Option<bool>,
    pub sv_bounds: (f32, f32, f32, f32),
    pub hue_bounds: (f32, f32, f32, f32),
    /// GUI geometry is kept as scalars so the draft also builds without the desktop feature.
    pub position: Option<(f32, f32)>,
    pub size: (f32, f32),
    pub drag: Option<(f32, f32, f32, f32, bool)>,
    pub bounds: (f32, f32, f32, f32),
    tx: Sender<Event>,
}

impl Editor {
    pub fn new(theme: &Theme, tx: Sender<Event>) -> Self {
        let file = super::document(theme);
        let light = theme.is_light();
        let label = if theme.imported.is_some() {
            format!(
                "{} copy",
                super::label(theme)
                    .chars()
                    .scan(0, |units, c| {
                        *units += c.len_utf16();
                        (*units <= 43).then_some(c)
                    })
                    .collect::<String>()
            )
        } else {
            "My theme".into()
        };
        let css = file["css"].as_str().unwrap_or("");
        let mut css = TextArea::new(css);
        css.selectable = true;
        css.newline_on_enter = true;
        let mut name = TextArea::new(&label);
        name.selectable = true;
        let mut color = TextArea::new("");
        color.selectable = true;
        let mut search = TextArea::new("");
        search.selectable = true;
        let mut editor = Self {
            owner: Uuid::new_v4(),
            before: theme.clone(),
            advanced: theme.imported.is_some() && file["managed"] != true,
            file,
            light,
            inspect: false,
            minimized: false,
            field: Field::None,
            name,
            color,
            search,
            css,
            role: "canvas",
            error: None,
            save_error: None,
            revision: 0,
            pending: false,
            saving: false,
            scroll: 0,
            checkpoint: None,
            hsv: (0.0, 0.0, 0.0),
            picker_drag: None,
            sv_bounds: (0.0, 0.0, 0.0, 0.0),
            hue_bounds: (0.0, 0.0, 0.0, 0.0),
            position: None,
            size: (440.0, 690.0),
            drag: None,
            bounds: (0.0, 0.0, 0.0, 0.0),
            tx,
        };
        editor.select("canvas");
        editor.field = Field::None;
        editor
    }
    pub fn map(&self) -> &Map<String, Value> {
        palette(&self.file, self.light).expect("editor has a complete active map")
    }
    pub fn value(&self, role: &str) -> &str {
        self.map()
            .get(role)
            .and_then(Value::as_str)
            .unwrap_or("#000000")
    }
    pub fn select(&mut self, role: &'static str) {
        if self.pending || self.saving {
            return;
        }
        self.error = None;
        self.checkpoint = Some(self.file.clone());
        self.role = role;
        self.color.set_text(
            &import::display_hex(self.value(role)).unwrap_or_else(|| self.value(role).into()),
        );
        self.color.select_all();
        self.sync_picker();
        self.field = Field::Color;
    }
    pub fn sync_picker(&mut self) {
        if let Some(c) = import::css(self.color.text()) {
            let max = c.r.max(c.g).max(c.b);
            let min = c.r.min(c.g).min(c.b);
            let delta = max - min;
            let hue = if delta < 0.0001 {
                self.hsv.0
            } else if max == c.r {
                ((c.g - c.b) / delta).rem_euclid(6.0) / 6.0
            } else if max == c.g {
                ((c.b - c.r) / delta + 2.0) / 6.0
            } else {
                ((c.r - c.g) / delta + 4.0) / 6.0
            };
            self.hsv = (hue, if max <= 0.0 { 0.0 } else { delta / max }, max);
        }
    }
    pub fn filtered(&self) -> Vec<(&'static str, &'static str, &'static [&'static str])> {
        let query = self.search.text().trim().to_lowercase();
        import::families()
            .iter()
            .copied()
            .filter(|(role, label, _)| {
                query.is_empty()
                    || role.to_lowercase().contains(&query)
                    || label.to_lowercase().contains(&query)
            })
            .collect()
    }
    pub fn valid_name(&self) -> bool {
        !self.name.text().trim().is_empty() && self.name.text().trim().encode_utf16().count() <= 48
    }
    pub fn can_save(&self) -> bool {
        self.valid_name() && self.error.is_none() && !self.pending && !self.saving
    }
    fn request(&mut self, guided: bool) {
        self.revision += 1;
        let revision = self.revision;
        let owner = self.owner;
        let tx = self.tx.clone();
        let mut file = self.file.clone();
        let light = self.light;
        let role = self.role;
        let value = self.color.text().to_string();
        let canvas = if role == "canvas" {
            value.clone()
        } else {
            self.value("canvas").into()
        };
        let accent = if role == "accent" {
            value.clone()
        } else {
            self.value("accent").into()
        };
        self.pending = true;
        self.error = None;
        std::thread::spawn(move || {
            let result = (|| {
                let colors = if guided {
                    import::generate(light, &canvas, &accent)?
                } else {
                    import::update_family(
                        light,
                        palette(&file, light).ok_or("Missing appearance palette.")?,
                        role,
                        &value,
                    )?
                };
                set_palette(&mut file, light, colors);
                if guided {
                    file["managed"] = true.into();
                }
                import::parse(&file.to_string(), "draft", selected_variant(&file, light))
            })();
            let _ = tx.send(Event::ThemeEditor(Box::new(Reply::Draft {
                owner,
                revision,
                result,
            })));
        });
    }
    pub fn cancel_color(&mut self) {
        if let Some(snapshot) = self.checkpoint.take() {
            self.file = snapshot;
            self.preview();
        }
        self.color
            .set_text(&import::display_hex(self.value(self.role)).unwrap_or_default());
        self.sync_picker();
        self.picker_drag = None;
        self.field = Field::None;
    }
    pub fn change_color(&mut self, value: &str) {
        if self.checkpoint.is_none() {
            self.checkpoint = Some(self.file.clone());
        }
        if self.saving {
            return;
        }
        self.color.set_text(value);
        self.request(!self.advanced);
    }
    pub fn mode(&mut self, light: bool) {
        if self.saving || self.pending || self.light == light {
            return;
        }
        self.light = light;
        if palette(&self.file, light).is_none() {
            set_palette(&mut self.file, light, import::defaults(light));
        }
        self.select(self.role);
        self.preview();
    }
    pub fn preview(&mut self) {
        self.revision += 1;
        self.pending = true;
        self.error = None;
        let owner = self.owner;
        let revision = self.revision;
        let file = self.file.clone();
        let light = self.light;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = import::parse(&file.to_string(), "draft", selected_variant(&file, light));
            let _ = tx.send(Event::ThemeEditor(Box::new(Reply::Draft {
                owner,
                revision,
                result,
            })));
        });
    }
    pub fn guided(&mut self) {
        if self.saving || self.pending {
            return;
        }
        self.advanced = !self.advanced;
        self.scroll = 0;
        if self.advanced {
            self.file.as_object_mut().unwrap().remove("managed");
        } else {
            self.file["managed"] = true.into();
            self.select("canvas");
            self.request(true);
        }
    }
    pub fn save(&mut self) {
        if !self.can_save() {
            return;
        }
        let mut file = self.file.clone();
        file["name"] = self.name.text().trim().into();
        let id = format!(
            "{}-custom",
            import::id_from_name(self.name.text())
                .chars()
                .take(41)
                .collect::<String>()
        );
        file["id"] = id.into();
        file.as_object_mut().unwrap().remove("collection");
        if self.css.text().is_empty() {
            file.as_object_mut().unwrap().remove("css");
        } else {
            file["css"] = self.css.text().into();
        }
        let light = self.light;
        let owner = self.owner;
        let tx = self.tx.clone();
        self.saving = true;
        self.save_error = None;
        std::thread::spawn(move || {
            let variant = selected_variant(&file, light).map(str::to_string);
            let result = super::install_document(&file)
                .map(|(name, _)| variant.map_or(name.clone(), |v| format!("{name}@{v}")));
            let _ = tx.send(Event::ThemeEditor(Box::new(Reply::Saved { owner, result })));
        });
    }
    pub fn input(&mut self, key: KeyEvent) -> bool {
        if self.saving {
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.save();
            return true;
        }
        if matches!(key.code, KeyCode::Up | KeyCode::Down)
            && (matches!(self.field, Field::None | Field::Search)
                || key.modifiers.contains(KeyModifiers::ALT))
        {
            let list = if self.advanced {
                self.filtered()
            } else {
                import::families()
                    .iter()
                    .copied()
                    .filter(|(r, _, _)| matches!(*r, "canvas" | "accent"))
                    .collect()
            };
            if !list.is_empty() {
                let at = list
                    .iter()
                    .position(|(r, _, _)| *r == self.role)
                    .unwrap_or(0) as isize;
                let dir = if key.code == KeyCode::Down { 1 } else { -1 };
                let next = (at + dir).rem_euclid(list.len() as isize) as usize;
                let searching = self.field == Field::Search;
                self.select(list[next].0);
                self.scroll = next.saturating_sub(1);
                if searching {
                    self.field = Field::Search;
                }
            }
            return true;
        }
        if key.code == KeyCode::Tab || key.code == KeyCode::BackTab {
            let fields = if self.advanced {
                &[Field::Name, Field::Search, Field::Color, Field::Css][..]
            } else {
                &[Field::Name, Field::Color, Field::Css][..]
            };
            let at = fields
                .iter()
                .position(|f| *f == self.field)
                .map(|i| i as isize)
                .unwrap_or(-1);
            let direction =
                if key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT) {
                    -1
                } else {
                    1
                };
            self.field = fields[(at + direction).rem_euclid(fields.len() as isize) as usize];
            return true;
        }
        let field = match self.field {
            Field::Name => &mut self.name,
            Field::Color => &mut self.color,
            Field::Search => &mut self.search,
            Field::Css => &mut self.css,
            Field::None => return false,
        };
        if key.code == KeyCode::Enter && self.field != Field::Css {
            self.field = Field::None;
            self.checkpoint = None;
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'x'))
        {
            if let Some(text) = field.selected() {
                crate::clip::copy(text);
            }
            let changed = key.code == KeyCode::Char('x') && field.delete_selection();
            if changed && self.field == Field::Color {
                self.sync_picker();
                self.request(!self.advanced);
            }
            return true;
        }
        let before = field.text().to_string();
        if field.key(key, 55) {
            if field.text() != before {
                match self.field {
                    Field::Color => {
                        self.sync_picker();
                        self.request(!self.advanced);
                    }
                    Field::Search => self.scroll = 0,
                    _ => {}
                }
            }
            return true;
        }
        !key.modifiers.contains(KeyModifiers::ALT)
    }
    pub fn paste(&mut self, text: &str) {
        if self.saving {
            return;
        }
        let field = match self.field {
            Field::Name => &mut self.name,
            Field::Color => &mut self.color,
            Field::Search => &mut self.search,
            Field::Css => &mut self.css,
            Field::None => return,
        };
        field.insert(
            &text
                .chars()
                .filter(|c| self.field == Field::Css || !c.is_control())
                .collect::<String>(),
        );
        if self.field == Field::Color {
            self.sync_picker();
            self.request(!self.advanced);
        }
    }
}

pub fn selected_variant(file: &Value, light: bool) -> Option<&'static str> {
    let appearance = if light { "light" } else { "dark" };
    (file["appearance"].as_str() != Some(appearance)).then_some(appearance)
}
fn palette(file: &Value, light: bool) -> Option<&Map<String, Value>> {
    match selected_variant(file, light) {
        None => file["colors"].as_object(),
        Some(mode) => file["variants"][mode].as_object(),
    }
}
fn set_palette(file: &mut Value, light: bool, colors: Map<String, Value>) {
    match selected_variant(file, light) {
        None => file["colors"] = Value::Object(colors),
        Some(mode) => {
            if !file["variants"].is_object() {
                file["variants"] = serde_json::json!({});
            }
            file["variants"][mode] = Value::Object(colors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;
    fn receive(rx: &Receiver<Event>) -> import::Imported {
        let Event::ThemeEditor(reply) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!("theme reply")
        };
        let Reply::Draft { result, .. } = *reply else {
            panic!("draft reply")
        };
        result.unwrap()
    }
    #[test]
    fn two_seed_edits_change_the_palette_and_keep_the_other_half_and_css() {
        let (tx, rx) = channel();
        let source = super::super::get("aurora-studio");
        let mut editor = Editor::new(&source, tx);
        assert!(!editor.advanced);
        let light = editor.file["variants"]["light"].clone();
        let css = editor.file["css"].clone();
        editor.select("accent");
        editor.change_color("#1ad7c0");
        let result = receive(&rx);
        assert_eq!(result.file["variants"]["light"], light);
        assert_eq!(result.file["css"], css);
        assert_ne!(
            result.file["colors"]["messageAction"],
            source.imported.as_ref().unwrap().file["colors"]["messageAction"]
        );
        assert_eq!(result.colors.len(), 61);
    }
    #[test]
    fn appearance_switch_preserves_both_existing_halves() {
        let (tx, rx) = channel();
        let mut editor = Editor::new(&super::super::get("aurora-studio"), tx);
        let original = editor.file.clone();
        editor.mode(true);
        let result = receive(&rx);
        assert!(result.light);
        assert_eq!(result.file["colors"], original["colors"]);
        for role in import::ROLES {
            let before = import::css(original["variants"]["light"][role].as_str().unwrap())
                .unwrap()
                .to_rgba8();
            assert_eq!(result.role(role).to_rgba8(), before, "{role}");
        }
    }
    #[test]
    fn escape_reverts_the_complete_color_edit_and_invalid_values_do_not_save() {
        let (tx, rx) = channel();
        let mut e = Editor::new(&super::super::get("aurora-studio"), tx);
        let original = e.file.clone();
        e.select("accent");
        e.change_color("#ff9900");
        let changed = receive(&rx);
        e.file = changed.file;
        e.pending = false;
        e.cancel_color();
        let restored = receive(&rx);
        assert_eq!(
            restored.role("accent").to_rgba8(),
            import::css(original["colors"]["accent"].as_str().unwrap())
                .unwrap()
                .to_rgba8()
        );
        assert_eq!(e.field, Field::None);
        e.pending = false;
        e.name.set_text("");
        assert!(!e.can_save());
        e.name.set_text("A name");
        e.change_color("not a color");
        let Event::ThemeEditor(reply) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!()
        };
        assert!(matches!(*reply, Reply::Draft { result: Err(_), .. }));
    }
    #[test]
    fn bundled_demo_is_portable_in_both_modes_with_its_exact_css_recipe() {
        for name in ["aurora-studio", "aurora-studio@light"] {
            let theme = super::super::get(name);
            assert!(theme.aurora());
            let text = super::super::document(&theme).to_string();
            let imported =
                import::parse(&text, "aurora", name.ends_with("@light").then_some("light"))
                    .unwrap();
            assert_eq!(imported.colors.len(), 61);
            assert!(imported.has_css());
            let mut family = std::collections::HashSet::new();
            for (_, _, roles) in import::families() {
                for role in *roles {
                    assert!(family.insert(*role));
                }
            }
            assert_eq!(family.len(), 61);
        }
    }
}
