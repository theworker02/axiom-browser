//! Web API / DOM bindings for Axiom (Phase 2).
//!
//! Full bindings live primarily through `axiom-js` host callbacks.
//! This crate re-exports the runtime facade for future expansion.

use axiom_js::JsRuntime;

pub struct WebApis {
    pub js: JsRuntime,
}

impl WebApis {
    pub fn new() -> Self {
        Self {
            js: JsRuntime::new(),
        }
    }
}

impl Default for WebApis {
    fn default() -> Self {
        Self::new()
    }
}
