//! Process-local clipboard for omnibox (OS clipboard deferred).

use std::sync::{Mutex, OnceLock};

static CLIPBOARD: OnceLock<Mutex<String>> = OnceLock::new();

fn store() -> &'static Mutex<String> {
    CLIPBOARD.get_or_init(|| Mutex::new(String::new()))
}

pub fn clipboard_get() -> String {
    store().lock().map(|g| g.clone()).unwrap_or_default()
}

pub fn clipboard_set(text: &str) {
    if let Ok(mut g) = store().lock() {
        *g = text.to_string();
    }
}
