//! Request scheduler: bounded concurrency, priorities with aging, streaming delivery.
//!
//! * A fixed pool of `max_concurrent` worker threads — never one thread per request.
//! * The highest effective priority runs first; waiting requests gain one priority level
//!   per `aging_interval` so low-priority work cannot starve.
//! * Each request streams [`NetworkEvent`]s into the caller's channel. With a bounded
//!   channel, a slow consumer stalls the worker (backpressure) instead of buffering.
//! * Requests carrying a `context_id` get a child token of the context's token, so
//!   [`RequestScheduler::cancel_context`] cancels queued and in-flight work for a tab.
//! * Worker threads are internal; consumers only see channels and tokens.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{SendTimeoutError, Sender};
use parking_lot::{Condvar, Mutex};

use crate::cancel::CancellationToken;
use crate::error::NetworkError;
use crate::id::NetworkRequestId;
use crate::priority::RequestPriority;
use crate::request::NetworkRequest;
use crate::response::ResponseMeta;
use crate::service::NetworkService;
use crate::timing::ms_since;

#[derive(Debug)]
pub enum NetworkEvent {
    Response {
        id: NetworkRequestId,
        meta: Box<ResponseMeta>,
    },
    Data {
        id: NetworkRequestId,
        bytes: Vec<u8>,
    },
    Complete {
        id: NetworkRequestId,
    },
    Failed {
        id: NetworkRequestId,
        error: NetworkError,
    },
}

impl NetworkEvent {
    pub fn id(&self) -> NetworkRequestId {
        match self {
            Self::Response { id, .. }
            | Self::Data { id, .. }
            | Self::Complete { id }
            | Self::Failed { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    pub max_concurrent: usize,
    pub aging_interval: Duration,
    pub chunk_size: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 12,
            aging_interval: Duration::from_secs(2),
            chunk_size: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SchedulerStats {
    pub queued: usize,
    pub in_flight: usize,
    pub peak_in_flight: usize,
    pub completed: u64,
}

/// Priority after aging: one level better per full `interval` waited.
pub fn effective_priority(
    base: RequestPriority,
    waited: Duration,
    interval: Duration,
) -> RequestPriority {
    if interval.is_zero() {
        return base;
    }
    let boost = (waited.as_millis() / interval.as_millis().max(1)).min(4) as u8;
    match (base as u8).saturating_sub(boost) {
        0 => RequestPriority::VeryHigh,
        1 => RequestPriority::High,
        2 => RequestPriority::Medium,
        3 => RequestPriority::Low,
        _ => RequestPriority::VeryLow,
    }
}

struct Job {
    req: NetworkRequest,
    cancel: CancellationToken,
    sink: Sender<NetworkEvent>,
    enqueued: Instant,
    seq: u64,
}

struct InFlight {
    context: Option<u64>,
    cancel: CancellationToken,
}

#[derive(Default)]
struct State {
    queue: Vec<Job>,
    in_flight: HashMap<NetworkRequestId, InFlight>,
    contexts: HashMap<u64, CancellationToken>,
    peak: usize,
    completed: u64,
    seq: u64,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    shutdown: AtomicBool,
    service: Arc<NetworkService>,
    config: SchedulerConfig,
}

pub struct RequestScheduler {
    shared: Arc<Shared>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl RequestScheduler {
    pub fn new(service: Arc<NetworkService>, config: SchedulerConfig) -> Self {
        let n = config.max_concurrent.max(1);
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
            service,
            config,
        });
        let workers = (0..n)
            .map(|i| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("axiom-net-worker-{i}"))
                    .spawn(move || worker_loop(&shared))
                    .expect("spawn network worker")
            })
            .collect();
        Self {
            shared,
            workers: Mutex::new(workers),
        }
    }

    pub fn service(&self) -> &Arc<NetworkService> {
        &self.shared.service
    }

    pub fn config(&self) -> &SchedulerConfig {
        &self.shared.config
    }

    /// Queue a request; events for it are sent to `sink`. Returns its cancellation token.
    pub fn enqueue(
        &self,
        req: NetworkRequest,
        sink: Sender<NetworkEvent>,
    ) -> Result<CancellationToken, NetworkError> {
        if self.shared.shutdown.load(Ordering::SeqCst) {
            return Err(NetworkError::Cancelled);
        }
        let mut s = self.shared.state.lock();
        let cancel = match req.context_id {
            Some(ctx) => s.contexts.entry(ctx).or_default().child_token(),
            None => CancellationToken::new(),
        };
        s.seq += 1;
        let seq = s.seq;
        s.queue.push(Job {
            req,
            cancel: cancel.clone(),
            sink,
            enqueued: Instant::now(),
            seq,
        });
        drop(s);
        self.shared.wake.notify_one();
        Ok(cancel)
    }

    /// Cancel one request (queued: removed and reported; in flight: I/O interrupted).
    pub fn cancel(&self, id: NetworkRequestId) {
        let mut s = self.shared.state.lock();
        if let Some(pos) = s.queue.iter().position(|j| j.req.id == id) {
            let job = s.queue.swap_remove(pos);
            drop(s);
            self.report_dequeued(job);
            return;
        }
        if let Some(f) = s.in_flight.get(&id) {
            f.cancel.cancel();
        }
    }

    /// Cancel every queued and in-flight request of a browsing context (tab close,
    /// navigation away, stop).
    pub fn cancel_context(&self, context: u64) {
        let mut s = self.shared.state.lock();
        if let Some(token) = s.contexts.remove(&context) {
            token.cancel();
        }
        let (removed, kept): (Vec<Job>, Vec<Job>) = std::mem::take(&mut s.queue)
            .into_iter()
            .partition(|j| j.req.context_id == Some(context));
        s.queue = kept;
        drop(s);
        for job in removed {
            self.report_dequeued(job);
        }
    }

    fn report_dequeued(&self, job: Job) {
        job.cancel.cancel();
        self.shared
            .service
            .metrics()
            .cancelled
            .fetch_add(1, Ordering::Relaxed);
        let _ = job.sink.try_send(NetworkEvent::Failed {
            id: job.req.id,
            error: NetworkError::Cancelled,
        });
    }

    /// Queued + in-flight requests for a context.
    pub fn active_for_context(&self, context: u64) -> usize {
        let s = self.shared.state.lock();
        s.queue
            .iter()
            .filter(|j| j.req.context_id == Some(context))
            .count()
            + s.in_flight
                .values()
                .filter(|f| f.context == Some(context))
                .count()
    }

    pub fn stats(&self) -> SchedulerStats {
        let s = self.shared.state.lock();
        SchedulerStats {
            queued: s.queue.len(),
            in_flight: s.in_flight.len(),
            peak_in_flight: s.peak,
            completed: s.completed,
        }
    }

    /// Cancel everything and join the workers. Idempotent.
    pub fn shutdown(&self) {
        if self.shared.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        let queued = {
            let mut s = self.shared.state.lock();
            for f in s.in_flight.values() {
                f.cancel.cancel();
            }
            for t in s.contexts.values() {
                t.cancel();
            }
            std::mem::take(&mut s.queue)
        };
        for job in queued {
            self.report_dequeued(job);
        }
        self.shared.wake.notify_all();
        for handle in self.workers.lock().drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for RequestScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn next_job(state: &mut State, interval: Duration) -> Option<Job> {
    let now = Instant::now();
    let best = state
        .queue
        .iter()
        .enumerate()
        .min_by_key(|(_, j)| {
            (
                effective_priority(j.req.priority, now - j.enqueued, interval),
                j.seq,
            )
        })
        .map(|(i, _)| i)?;
    Some(state.queue.swap_remove(best))
}

fn worker_loop(shared: &Shared) {
    loop {
        let job = {
            let mut s = shared.state.lock();
            loop {
                if shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(job) = next_job(&mut s, shared.config.aging_interval) {
                    s.in_flight.insert(
                        job.req.id,
                        InFlight {
                            context: job.req.context_id,
                            cancel: job.cancel.clone(),
                        },
                    );
                    s.peak = s.peak.max(s.in_flight.len());
                    break job;
                }
                shared.wake.wait(&mut s);
            }
        };
        let id = job.req.id;
        run_job(shared, job);
        let mut s = shared.state.lock();
        s.in_flight.remove(&id);
        s.completed += 1;
    }
}

fn run_job(shared: &Shared, job: Job) {
    let Job {
        mut req,
        cancel,
        sink,
        enqueued,
        ..
    } = job;
    let id = req.id;
    req.queued_ms = Some(ms_since(enqueued));
    let limit = req.max_body_bytes;
    let flow = req.flow.clone();
    let resp = match shared.service.execute_with_cancel(req, &cancel) {
        Ok(r) => r,
        Err(error) => {
            deliver(
                shared,
                &sink,
                NetworkEvent::Failed { id, error },
                &cancel,
                true,
            );
            return;
        }
    };
    let too_large = |limit| NetworkEvent::Failed {
        id,
        error: NetworkError::TooLarge { limit },
    };
    if let (Some(limit), Some(len)) = (limit, resp.meta.content_length) {
        if resp.meta.content_encoding.is_empty() && len > limit {
            resp.body.abandon();
            deliver(shared, &sink, too_large(limit), &cancel, true);
            return;
        }
    }
    let meta = Box::new(resp.meta.clone());
    if !deliver(
        shared,
        &sink,
        NetworkEvent::Response { id, meta },
        &cancel,
        false,
    ) {
        resp.body.abandon();
        return;
    }
    let mut total: u64 = 0;
    loop {
        if let Some(flow) = &flow {
            if !flow.wait_for_room(&cancel, || shared.shutdown.load(Ordering::SeqCst)) {
                resp.body.abandon();
                deliver(
                    shared,
                    &sink,
                    NetworkEvent::Failed {
                        id,
                        error: NetworkError::Cancelled,
                    },
                    &cancel,
                    true,
                );
                return;
            }
        }
        match resp.body.read_chunk(shared.config.chunk_size) {
            Ok(bytes) if bytes.is_empty() => {
                deliver(shared, &sink, NetworkEvent::Complete { id }, &cancel, true);
                return;
            }
            Ok(bytes) => {
                total += bytes.len() as u64;
                if let Some(flow) = &flow {
                    flow.delivered(bytes.len() as u64);
                }
                if let Some(limit) = limit.filter(|l| total > *l) {
                    resp.body.abandon();
                    deliver(shared, &sink, too_large(limit), &cancel, true);
                    return;
                }
                if !deliver(
                    shared,
                    &sink,
                    NetworkEvent::Data { id, bytes },
                    &cancel,
                    false,
                ) {
                    cancel.cancel();
                    resp.body.abandon();
                    return;
                }
            }
            Err(error) => {
                deliver(
                    shared,
                    &sink,
                    NetworkEvent::Failed { id, error },
                    &cancel,
                    true,
                );
                return;
            }
        }
    }
}

/// Send with backpressure. Gives up when the receiver is gone, on shutdown, or — for
/// non-final events — once the request is cancelled.
fn deliver(
    shared: &Shared,
    sink: &Sender<NetworkEvent>,
    mut ev: NetworkEvent,
    cancel: &CancellationToken,
    is_final: bool,
) -> bool {
    let mut cancelled_waits = 0;
    loop {
        match sink.send_timeout(ev, Duration::from_millis(25)) {
            Ok(()) => return true,
            Err(SendTimeoutError::Disconnected(_)) => return false,
            Err(SendTimeoutError::Timeout(back)) => {
                if shared.shutdown.load(Ordering::SeqCst) {
                    return false;
                }
                if cancel.is_cancelled() {
                    cancelled_waits += 1;
                    if !is_final || cancelled_waits > 40 {
                        return false;
                    }
                }
                ev = back;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aging_boosts_priority() {
        let i = Duration::from_secs(2);
        assert_eq!(
            effective_priority(RequestPriority::VeryLow, Duration::ZERO, i),
            RequestPriority::VeryLow
        );
        assert_eq!(
            effective_priority(RequestPriority::VeryLow, Duration::from_secs(5), i),
            RequestPriority::Medium
        );
        assert_eq!(
            effective_priority(RequestPriority::Low, Duration::from_secs(60), i),
            RequestPriority::VeryHigh
        );
    }
}
