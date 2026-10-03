//! Editable omnibox with caret + selection (trusted chrome).

use crate::clipboard::{clipboard_get, clipboard_set};

#[derive(Debug, Clone)]
pub struct Omnibox {
    /// Edited text while focused; when not editing, chrome reads tab URL instead.
    pub text: String,
    pub caret: usize,
    /// Selection as (start, end) in char indices, start <= end. None = no selection.
    pub sel_start: usize,
    pub sel_end: usize,
    pub editing: bool,
    /// URL restored on Escape.
    pub committed_url: String,
}

impl Default for Omnibox {
    fn default() -> Self {
        Self::new()
    }
}

impl Omnibox {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            caret: 0,
            sel_start: 0,
            sel_end: 0,
            editing: false,
            committed_url: String::new(),
        }
    }

    pub fn has_selection(&self) -> bool {
        self.sel_start != self.sel_end
    }

    pub fn selection_range(&self) -> (usize, usize) {
        if self.sel_start <= self.sel_end {
            (self.sel_start, self.sel_end)
        } else {
            (self.sel_end, self.sel_start)
        }
    }

    pub fn sync_from_url(&mut self, url: &str) {
        if !self.editing {
            self.text = url.to_string();
            self.committed_url = url.to_string();
            self.caret = self.text.chars().count();
            self.clear_selection();
        } else {
            self.committed_url = url.to_string();
        }
    }

    pub fn focus_select_all(&mut self, url: &str) {
        self.committed_url = url.to_string();
        self.text = url.to_string();
        self.editing = true;
        self.select_all();
    }

    pub fn select_all(&mut self) {
        let len = self.text.chars().count();
        self.sel_start = 0;
        self.sel_end = len;
        self.caret = len;
    }

    pub fn clear_selection(&mut self) {
        self.sel_start = self.caret;
        self.sel_end = self.caret;
    }

    pub fn blur_commit(&mut self) {
        self.editing = false;
        self.text = self.committed_url.clone();
        self.clear_selection();
    }

    pub fn escape(&mut self) {
        self.text = self.committed_url.clone();
        self.editing = false;
        self.caret = self.text.chars().count();
        self.clear_selection();
    }

    pub fn move_home(&mut self, extend: bool) {
        if extend {
            self.sel_end = 0;
            self.caret = 0;
        } else {
            self.caret = 0;
            self.clear_selection();
        }
    }

    pub fn move_end(&mut self, extend: bool) {
        let len = self.text.chars().count();
        if extend {
            self.sel_end = len;
            self.caret = len;
        } else {
            self.caret = len;
            self.clear_selection();
        }
    }

    pub fn move_left(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            let (a, _) = self.selection_range();
            self.caret = a;
            self.clear_selection();
            return;
        }
        if self.caret > 0 {
            self.caret -= 1;
        }
        if extend {
            self.sel_end = self.caret;
        } else {
            self.clear_selection();
        }
    }

    pub fn move_right(&mut self, extend: bool) {
        let len = self.text.chars().count();
        if !extend && self.has_selection() {
            let (_, b) = self.selection_range();
            self.caret = b;
            self.clear_selection();
            return;
        }
        if self.caret < len {
            self.caret += 1;
        }
        if extend {
            self.sel_end = self.caret;
        } else {
            self.clear_selection();
        }
    }

    pub fn delete_backward(&mut self) {
        if self.has_selection() {
            self.delete_selection();
            return;
        }
        if self.caret == 0 {
            return;
        }
        let before = char_prefix(&self.text, self.caret - 1);
        let after = char_suffix(&self.text, self.caret);
        self.text = format!("{before}{after}");
        self.caret -= 1;
        self.clear_selection();
    }

    pub fn delete_forward(&mut self) {
        if self.has_selection() {
            self.delete_selection();
            return;
        }
        let len = self.text.chars().count();
        if self.caret >= len {
            return;
        }
        let before = char_prefix(&self.text, self.caret);
        let after = char_suffix(&self.text, self.caret + 1);
        self.text = format!("{before}{after}");
        self.clear_selection();
    }

    pub fn insert_text(&mut self, s: &str) {
        if self.has_selection() {
            self.delete_selection();
        }
        let before = char_prefix(&self.text, self.caret);
        let after = char_suffix(&self.text, self.caret);
        self.text = format!("{before}{s}{after}");
        self.caret += s.chars().count();
        self.clear_selection();
    }

    pub fn delete_selection(&mut self) {
        let (a, b) = self.selection_range();
        let before = char_prefix(&self.text, a);
        let after = char_suffix(&self.text, b);
        self.text = format!("{before}{after}");
        self.caret = a;
        self.clear_selection();
    }

    pub fn copy(&self) {
        if !self.has_selection() {
            return;
        }
        let (a, b) = self.selection_range();
        let selected: String = self.text.chars().skip(a).take(b - a).collect();
        clipboard_set(&selected);
    }

    pub fn cut(&mut self) {
        if !self.has_selection() {
            return;
        }
        self.copy();
        self.delete_selection();
    }

    pub fn paste(&mut self) {
        let clip = clipboard_get();
        if !clip.is_empty() {
            self.insert_text(&clip);
        }
    }

    pub fn display_text(&self) -> &str {
        &self.text
    }
}

fn char_prefix(s: &str, char_count: usize) -> String {
    s.chars().take(char_count).collect()
}

fn char_suffix(s: &str, char_start: usize) -> String {
    s.chars().skip(char_start).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_selection_and_escape() {
        let mut o = Omnibox::new();
        o.focus_select_all("https://example.com/");
        assert!(o.editing);
        assert!(o.has_selection());
        o.insert_text("x");
        assert_eq!(o.text, "x");
        o.escape();
        assert_eq!(o.text, "https://example.com/");
        assert!(!o.editing);
    }

    #[test]
    fn paste_and_cut() {
        clipboard_set("hello");
        let mut o = Omnibox::new();
        o.editing = true;
        o.paste();
        assert_eq!(o.text, "hello");
        o.select_all();
        o.cut();
        assert!(o.text.is_empty());
        assert_eq!(clipboard_get(), "hello");
    }
}
