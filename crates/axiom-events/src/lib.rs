//! EventTarget / Event dispatch (capture → target → bubble).

use std::collections::HashMap;

use axiom_dom::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPhase {
    None,
    Capturing,
    AtTarget,
    Bubbling,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub type_: String,
    pub target: Option<NodeId>,
    pub current_target: Option<NodeId>,
    pub phase: EventPhase,
    pub bubbles: bool,
    pub cancelable: bool,
    pub default_prevented: bool,
    pub propagation_stopped: bool,
    pub client_x: f32,
    pub client_y: f32,
    pub button: i16,
    pub key: String,
    pub scroll_delta_y: f32,
}

impl Event {
    pub fn new(type_: impl Into<String>) -> Self {
        Self {
            type_: type_.into(),
            target: None,
            current_target: None,
            phase: EventPhase::None,
            bubbles: true,
            cancelable: true,
            default_prevented: false,
            propagation_stopped: false,
            client_x: 0.0,
            client_y: 0.0,
            button: 0,
            key: String::new(),
            scroll_delta_y: 0.0,
        }
    }

    pub fn prevent_default(&mut self) {
        if self.cancelable {
            self.default_prevented = true;
        }
    }

    pub fn stop_propagation(&mut self) {
        self.propagation_stopped = true;
    }
}

pub type EventCallback = Box<dyn FnMut(&mut Event) + Send>;

struct Listener {
    callback: EventCallback,
    capture: bool,
}

#[derive(Default)]
pub struct EventTargetMap {
    listeners: HashMap<(NodeId, String), Vec<Listener>>,
}

impl EventTargetMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_event_listener(
        &mut self,
        target: NodeId,
        type_: &str,
        callback: EventCallback,
        capture: bool,
    ) {
        self.listeners
            .entry((target, type_.to_string()))
            .or_default()
            .push(Listener { callback, capture });
    }

    pub fn remove_event_listeners(&mut self, target: NodeId, type_: &str) {
        self.listeners.remove(&(target, type_.to_string()));
    }

    /// `path` is root → … → target (document to element).
    pub fn dispatch(&mut self, path: &[NodeId], event: &mut Event) {
        if path.is_empty() {
            return;
        }
        event.target = path.last().copied();

        // Capture: root → target exclusive
        event.phase = EventPhase::Capturing;
        for &node in &path[..path.len().saturating_sub(1)] {
            if event.propagation_stopped {
                return;
            }
            event.current_target = Some(node);
            self.fire(node, event, true);
        }

        // Target
        if event.propagation_stopped {
            return;
        }
        event.phase = EventPhase::AtTarget;
        if let Some(&target) = path.last() {
            event.current_target = Some(target);
            self.fire(target, event, true);
            self.fire(target, event, false);
        }

        // Bubble
        if !event.bubbles {
            event.phase = EventPhase::None;
            return;
        }
        event.phase = EventPhase::Bubbling;
        for &node in path.iter().rev().skip(1) {
            if event.propagation_stopped {
                break;
            }
            event.current_target = Some(node);
            self.fire(node, event, false);
        }
        event.phase = EventPhase::None;
    }

    fn fire(&mut self, node: NodeId, event: &mut Event, capture: bool) {
        let key = (node, event.type_.clone());
        // Take listeners temporarily to avoid borrow issues
        let Some(mut list) = self.listeners.remove(&key) else {
            return;
        };
        for listener in list.iter_mut() {
            if listener.capture == capture {
                (listener.callback)(event);
                if event.propagation_stopped {
                    break;
                }
            }
        }
        self.listeners.insert(key, list);
    }
}
