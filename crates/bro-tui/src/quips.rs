//! Quips shared with the phone bridge, plus the desktop picker state.

pub use bro_core::quips::*;
use crate::textarea::TextArea;

#[derive(Default)]
pub struct Picker {
    pub open: bool,
    pub search: String,
    pub selected: usize,
    pub dismissed: Option<(String, usize)>,
    pub editor: Option<Editor>,
    pub error: Option<String>,
    pub delete_armed: bool,
}

pub struct Editor {
    pub original: Option<usize>,
    pub fields: [TextArea; 3], // name, shortcut, instructions
    pub field: usize,
    pub cols: usize,
}

impl Editor {
    pub fn new(original: Option<usize>, q: Option<&Quip>) -> Self {
        let mut fields = [TextArea::new(q.map_or("", |q| &q.name)), TextArea::new(q.map_or("", |q| &q.trigger)), TextArea::new(q.map_or("", |q| &q.instructions))];
        fields[2].newline_on_enter = true;
        Self { original, fields, field: 0, cols: 60 }
    }
    pub fn value(&self) -> Quip {
        Quip { name: self.fields[0].text().trim().into(), trigger: self.fields[1].text().trim().trim_start_matches(':').into(), instructions: self.fields[2].text().trim().into() }
    }
}
