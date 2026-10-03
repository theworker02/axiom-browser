//! Document lifecycle and navigation history.

use axiom_url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentReadyState {
    Created,
    Loading,
    Parsing,
    Interactive,
    Complete,
    Unloading,
    Destroyed,
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub url: Url,
    pub title: String,
    pub scroll_y: f32,
    /// The document came from a POST form submission; revisiting it would resend the form.
    pub form_post: bool,
    /// The document that created the entry ([`crate::page::DocumentShared::document_key`]);
    /// traversing between entries of the live document does not reload it.
    pub document_key: u64,
    /// Script state id (`pushState` / `replaceState`), kept by that document's realm.
    pub state: Option<u64>,
}

#[derive(Debug, Default)]
pub struct History {
    entries: Vec<HistoryEntry>,
    index: usize,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> Option<&HistoryEntry> {
        self.entries.get(self.index)
    }

    pub fn push(&mut self, entry: HistoryEntry) {
        if !self.entries.is_empty() {
            self.entries.truncate(self.index + 1);
        }
        self.entries.push(entry);
        self.index = self.entries.len().saturating_sub(1);
    }

    pub fn replace(&mut self, entry: HistoryEntry) {
        if self.entries.is_empty() {
            self.push(entry);
        } else {
            self.entries[self.index] = entry;
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.index + 1 < self.entries.len()
    }

    pub fn back(&mut self) -> Option<&HistoryEntry> {
        if self.can_go_back() {
            self.index -= 1;
            self.current()
        } else {
            None
        }
    }

    pub fn forward(&mut self) -> Option<&HistoryEntry> {
        if self.can_go_forward() {
            self.index += 1;
            self.current()
        } else {
            None
        }
    }

    /// Move `delta` entries (negative is back); `None` and no move when out of range.
    pub fn go(&mut self, delta: i32) -> Option<&HistoryEntry> {
        let target = self.index as i64 + delta as i64;
        if target < 0 || target >= self.entries.len() as i64 {
            return None;
        }
        self.index = target as usize;
        self.current()
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn update_scroll(&mut self, y: f32) {
        if let Some(e) = self.entries.get_mut(self.index) {
            e.scroll_y = y;
        }
    }

    pub fn set_form_post(&mut self, form_post: bool) {
        if let Some(e) = self.entries.get_mut(self.index) {
            e.form_post = form_post;
        }
    }

    pub fn update_title(&mut self, title: &str) {
        if let Some(e) = self.entries.get_mut(self.index) {
            e.title = title.to_string();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
