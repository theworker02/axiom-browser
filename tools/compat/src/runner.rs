//! Parallel execution with panic and timeout isolation.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Once;
use std::thread;
use std::time::Duration;

const WORKER_PREFIX: &str = "compat-";

/// Silence the panic message of isolated test threads (the panic is reported as a CRASH
/// outcome instead); panics elsewhere still print normally.
pub fn install_quiet_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let isolated = thread::current()
                .name()
                .is_some_and(|n| n.starts_with(WORKER_PREFIX));
            if !isolated {
                default(info);
            }
        }));
    });
}

pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

/// How an isolated run ended when it did not return normally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Abnormal {
    Crash(String),
    Timeout,
}

/// Run `f` on its own thread; a panic becomes [`Abnormal::Crash`], and a run exceeding
/// `timeout` becomes [`Abnormal::Timeout`] (the thread is abandoned, not killed).
pub fn isolated<R, F>(name: &str, timeout: Duration, f: F) -> Result<R, Abnormal>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    install_quiet_panic_hook();
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name(format!("{WORKER_PREFIX}{name}"))
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let r = catch_unwind(AssertUnwindSafe(f)).map_err(|p| panic_message(&*p));
            let _ = tx.send(r);
        });
    if let Err(e) = spawned {
        return Err(Abnormal::Crash(format!("spawn: {e}")));
    }
    match rx.recv_timeout(timeout) {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(msg)) => Err(Abnormal::Crash(msg)),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(Abnormal::Timeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(Abnormal::Crash("worker vanished".into())),
    }
}

/// Map `items` through `f` on `jobs` threads, preserving order.
pub fn parallel_map<T, R, F>(items: &[T], jobs: usize, f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let next = AtomicUsize::new(0);
    let jobs = jobs.clamp(1, items.len().max(1));
    let mut results: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    let slots: Vec<std::sync::Mutex<&mut Option<R>>> =
        results.iter_mut().map(std::sync::Mutex::new).collect();
    thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= items.len() {
                    break;
                }
                let r = f(&items[i]);
                **slots[i].lock().unwrap() = Some(r);
            });
        }
    });
    drop(slots);
    results
        .into_iter()
        .map(|r| r.expect("every item ran"))
        .collect()
}

pub fn default_jobs() -> usize {
    thread::available_parallelism().map_or(4, |n| n.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolation_reports_panics_and_timeouts() {
        assert_eq!(isolated("ok", Duration::from_secs(5), || 7), Ok(7));
        assert_eq!(
            isolated("boom", Duration::from_secs(5), || -> u8 {
                panic!("kaboom")
            }),
            Err(Abnormal::Crash("kaboom".into()))
        );
        let (tx, rx) = mpsc::channel::<()>();
        let r = isolated("hang", Duration::from_millis(50), move || {
            let _ = rx.recv();
        });
        assert_eq!(r, Err(Abnormal::Timeout));
        drop(tx);
    }

    #[test]
    fn parallel_map_preserves_order() {
        let items: Vec<u32> = (0..100).collect();
        assert_eq!(
            parallel_map(&items, 8, |x| x * 2),
            items.iter().map(|x| x * 2).collect::<Vec<_>>()
        );
    }
}
