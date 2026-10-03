//! Tab manager — independent browsing contexts, not cosmetic UI state.

use axiom_engine::DetachedKeepalive;

use crate::profile::ProfileId;
use crate::tab::{Tab, TabId, TabLifecycle};

#[derive(Debug, Clone)]
pub struct ClosedTab {
    pub url: String,
    pub title: String,
    pub profile_id: ProfileId,
}

pub struct TabManager {
    tabs: Vec<Tab>,
    active: usize,
    closed: Vec<ClosedTab>,
    viewport_w: u32,
    viewport_h: u32,
    profile_id: ProfileId,
    /// Keepalive fetches of closed tabs, polled until they finish.
    keepalive: DetachedKeepalive,
}

impl TabManager {
    pub fn new(profile_id: ProfileId, viewport_w: u32, viewport_h: u32) -> Self {
        let tabs = vec![Tab::new(profile_id, viewport_w, viewport_h)];
        Self {
            tabs,
            active: 0,
            closed: Vec::new(),
            viewport_w,
            viewport_h,
            profile_id,
            keepalive: DetachedKeepalive::default(),
        }
    }

    /// Keepalive fetches of closed tabs that are still in flight.
    pub fn detached_keepalive_count(&self) -> usize {
        self.keepalive.len()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active_index(&self) -> usize {
        self.active.min(self.tabs.len().saturating_sub(1))
    }

    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_index()]
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        let i = self.active_index();
        &mut self.tabs[i]
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn tabs_mut(&mut self) -> &mut [Tab] {
        &mut self.tabs
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn new_tab(&mut self) -> TabId {
        for t in &mut self.tabs {
            if t.lifecycle == TabLifecycle::Active {
                t.lifecycle = TabLifecycle::Background;
            }
        }
        let tab = Tab::new(self.profile_id, self.viewport_w, self.viewport_h);
        let id = tab.id;
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        id
    }

    pub fn close_active(&mut self) -> bool {
        self.close_at(self.active_index())
    }

    pub fn close_at(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            // Keep at least one tab; clear it instead.
            if self.tabs.len() == 1 && index == 0 {
                let tab = &mut self.tabs[0];
                self.closed.push(ClosedTab {
                    url: tab.url(),
                    title: tab.title(),
                    profile_id: tab.profile_id,
                });
                self.keepalive.adopt(tab.context.take_keepalive());
                *tab = Tab::new(self.profile_id, self.viewport_w, self.viewport_h);
                return true;
            }
            return false;
        }
        let mut tab = self.tabs.remove(index);
        self.keepalive.adopt(tab.context.take_keepalive());
        self.closed.push(ClosedTab {
            url: tab.last_url.clone(),
            title: tab.title(),
            profile_id: tab.profile_id,
        });
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
        self.tabs[self.active].lifecycle = TabLifecycle::Active;
        true
    }

    pub fn restore_closed(&mut self) -> Option<ClosedTab> {
        self.closed.pop()
    }

    /// Restore a previously closed tab shell (caller navigates).
    pub fn reopen_shell(&mut self) -> TabId {
        self.new_tab()
    }

    pub fn duplicate_active(&mut self) -> TabId {
        let url = self.active_tab().url();
        let id = self.new_tab();
        // Caller (Browser) should navigate; keep URL stash for convenience.
        self.active_tab_mut().last_url = url;
        id
    }

    pub fn select(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        for t in &mut self.tabs {
            if t.lifecycle == TabLifecycle::Active {
                t.lifecycle = TabLifecycle::Background;
            }
        }
        self.active = index;
        self.tabs[index].lifecycle = TabLifecycle::Active;
    }

    pub fn select_next(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let next = (self.active_index() + 1) % self.tabs.len();
        self.select(next);
    }

    pub fn select_prev(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let i = self.active_index();
        let prev = if i == 0 { self.tabs.len() - 1 } else { i - 1 };
        self.select(prev);
    }

    pub fn reorder(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if self.active == from {
            self.active = to;
        } else if from < self.active && to >= self.active {
            self.active -= 1;
        } else if from > self.active && to <= self.active {
            self.active += 1;
        }
    }

    pub fn pin_active(&mut self, pinned: bool) {
        self.active_tab_mut().pinned = pinned;
    }

    pub fn mute_active(&mut self, muted: bool) {
        self.active_tab_mut().muted = muted;
    }

    pub fn set_viewport(&mut self, w: u32, h: u32) {
        self.viewport_w = w;
        self.viewport_h = h;
        for tab in &mut self.tabs {
            tab.context.set_viewport(w, h);
        }
    }

    pub fn tick_active(&mut self) -> axiom_engine::HudStats {
        // Background tabs share the profile scheduler; draining their network events keeps
        // undelivered events from pinning shared workers (and applies their images).
        let active = self.active;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if i != active {
                tab.context.poll_loader();
            }
        }
        self.keepalive.poll();
        self.active_tab_mut().tick()
    }
}
