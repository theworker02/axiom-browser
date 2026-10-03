//! Browser event loop queues.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub type Task = Box<dyn FnOnce() + Send>;

#[derive(Default)]
pub struct TaskQueue {
    tasks: VecDeque<Task>,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, task: Task) {
        self.tasks.push_back(task);
    }

    pub fn run_until_empty(&mut self) {
        while let Some(task) = self.tasks.pop_front() {
            task();
        }
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

#[derive(Default)]
pub struct MicrotaskQueue {
    tasks: VecDeque<Task>,
}

impl MicrotaskQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, task: Task) {
        self.tasks.push_back(task);
    }

    pub fn checkpoint(&mut self) {
        while let Some(task) = self.tasks.pop_front() {
            task();
        }
    }
}

struct Timer {
    id: u32,
    fire_at: Instant,
    interval: Option<Duration>,
    /// JS callback id in `__axiom_timeoutFns`.
    callback_id: u32,
}

#[derive(Default)]
pub struct TimerQueue {
    timers: Vec<Timer>,
    next_id: u32,
}

impl TimerQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_timeout(&mut self, delay_ms: u64, callback_id: u32) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        self.timers.push(Timer {
            id,
            fire_at: Instant::now() + Duration::from_millis(delay_ms),
            interval: None,
            callback_id,
        });
        id
    }

    pub fn set_interval(&mut self, delay_ms: u64, callback_id: u32) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        let d = Duration::from_millis(delay_ms);
        self.timers.push(Timer {
            id,
            fire_at: Instant::now() + d,
            interval: Some(d),
            callback_id,
        });
        id
    }

    pub fn clear(&mut self, id: u32) {
        self.timers.retain(|t| t.id != id);
    }

    /// Cancels every timer that runs JS callback `callback_id`.
    pub fn clear_callback(&mut self, callback_id: u32) {
        self.timers.retain(|t| t.callback_id != callback_id);
    }

    pub fn len(&self) -> usize {
        self.timers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.timers.is_empty()
    }

    /// Whether a non-repeating timer is pending (intervals never finish on their own).
    pub fn has_one_shot(&self) -> bool {
        self.timers.iter().any(|t| t.interval.is_none())
    }

    /// Returns JS callback ids that are due, earliest first (ties in scheduling order).
    pub fn poll_due(&mut self) -> Vec<u32> {
        let now = Instant::now();
        let mut due = Vec::new();
        let mut keep = Vec::new();
        for mut timer in self.timers.drain(..) {
            if timer.fire_at <= now {
                due.push((timer.fire_at, timer.id, timer.callback_id));
                if let Some(interval) = timer.interval {
                    timer.fire_at = now + interval;
                    keep.push(timer);
                }
            } else {
                keep.push(timer);
            }
        }
        self.timers = keep;
        due.sort_by_key(|&(at, id, _)| (at, id));
        due.into_iter().map(|(_, _, cb)| cb).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_timers_run_in_fire_time_order_and_intervals_repeat() {
        let mut q = TimerQueue::new();
        q.set_timeout(0, 1);
        q.set_interval(0, 2);
        q.set_timeout(0, 3);
        assert!(q.has_one_shot());
        assert_eq!(q.poll_due(), vec![1, 2, 3]);
        assert!(!q.has_one_shot());
        assert_eq!(q.len(), 1);
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(q.poll_due(), vec![2]);
        q.clear_callback(2);
        assert!(q.is_empty());
    }
}
