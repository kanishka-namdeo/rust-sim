//! Append-only event log (spec §5.2): a 10,000-entry in-memory ring plus a
//! newline-delimited JSON file per run. The log is the run report's spine.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;
use tokio::sync::mpsc;

pub const RING_CAPACITY: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    FsmTransition,
    FsmRejected,
    SupervisorAction,
    FaultInjection,
    TaskAward,
    LinkEscalation,
    RunBoundary,
    HealthFlag,
    TaskEvent,
    SimEvent,
}

impl EventKind {
    pub fn name(&self) -> &'static str {
        match self {
            EventKind::FsmTransition => "fsm_transition",
            EventKind::FsmRejected => "fsm_rejected",
            EventKind::SupervisorAction => "supervisor_action",
            EventKind::FaultInjection => "fault_injection",
            EventKind::TaskAward => "task_award",
            EventKind::LinkEscalation => "link_escalation",
            EventKind::RunBoundary => "run_boundary",
            EventKind::HealthFlag => "health_flag",
            EventKind::TaskEvent => "task_event",
            EventKind::SimEvent => "sim_event",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    /// Fleet-epoch milliseconds.
    pub t_ms: u64,
    pub kind: EventKind,
    pub vehicle: Option<u8>,
    pub detail: String,
}

/// Shared handle: callers log without awaiting; a writer task drains the
/// channel into the NDJSON file and mirrors into the ring.
pub struct EventLog {
    ring: Mutex<VecDeque<Event>>,
    tx: mpsc::UnboundedSender<Event>,
    total: Mutex<u64>,
}

impl EventLog {
    /// Create the log and its writer task writing to `path` (created
    /// alongside the run report). Returns the shared handle. Works both
    /// inside and outside a tokio runtime (the std-thread fallback keeps
    /// unit tests runtime-free).
    pub fn spawn_writer(path: std::path::PathBuf) -> std::sync::Arc<EventLog> {
        let (tx, rx) = mpsc::unbounded_channel::<Event>();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move { writer_task(rx, path).await });
            }
            Err(_) => {
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_time()
                        .build()
                        .expect("event-writer fallback runtime");
                    rt.block_on(async move { writer_task(rx, path).await });
                });
            }
        }
        std::sync::Arc::new(EventLog {
            ring: Mutex::new(VecDeque::with_capacity(RING_CAPACITY)),
            tx,
            total: Mutex::new(0),
        })
    }

    /// Log without a file writer (unit tests, dry runs).
    pub fn in_memory() -> std::sync::Arc<EventLog> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    while rx.recv().await.is_some() {}
                });
            }
            Err(_) => {
                std::thread::spawn(move || {
                    while rx.blocking_recv().is_some() {}
                });
            }
        }
        std::sync::Arc::new(EventLog {
            ring: Mutex::new(VecDeque::with_capacity(RING_CAPACITY)),
            tx,
            total: Mutex::new(0),
        })
    }

    pub fn log(&self, kind: EventKind, vehicle: Option<u8>, detail: impl Into<String>) {
        let ev = Event {
            t_ms: fleet_epoch_ms(),
            kind,
            vehicle,
            detail: detail.into(),
        };
        *self.total.lock().unwrap() += 1;
        {
            let mut ring = self.ring.lock().unwrap();
            if ring.len() == RING_CAPACITY {
                ring.pop_front();
            }
            ring.push_back(ev.clone());
        }
        let _ = self.tx.send(ev);
    }

    pub fn tail(&self, n: usize) -> Vec<Event> {
        let ring = self.ring.lock().unwrap();
        let skip = ring.len().saturating_sub(n);
        ring.iter().skip(skip).cloned().collect()
    }

    pub fn all(&self) -> Vec<Event> {
        self.ring.lock().unwrap().iter().cloned().collect()
    }

    pub fn total(&self) -> u64 {
        *self.total.lock().unwrap()
    }
}

/// Drain the event channel into the NDJSON file (LineWriter: append-only,
/// no fsync in the hot path, spec §10.1).
async fn writer_task(mut rx: mpsc::UnboundedReceiver<Event>, path: std::path::PathBuf) {
    let out = std::fs::File::create(&path)
        .ok()
        .map(std::io::LineWriter::new);
    let mut out = out;
    while let Some(ev) = rx.recv().await {
        if let Some(w) = out.as_mut() {
            use std::io::Write;
            let mut buf = Vec::with_capacity(128);
            if let Ok(s) = serde_json::to_string(&ev) {
                buf.extend_from_slice(s.as_bytes());
                buf.push(b'\n');
                let _ = w.write_all(&buf);
            }
        }
    }
    if let Some(mut w) = out {
        use std::io::Write;
        let _ = w.flush();
    }
}

/// Fleet-epoch clock. Set once by the manager at startup via
/// [`set_fleet_epoch`]; events and states share it so CI assertions can
/// order them.
static FLEET_EPOCH: std::sync::OnceLock<Option<std::time::Instant>> = std::sync::OnceLock::new();

pub fn set_fleet_epoch() {
    let _ = FLEET_EPOCH.set(Some(std::time::Instant::now()));
}

pub fn fleet_epoch_ms() -> u64 {
    match FLEET_EPOCH.get() {
        Some(Some(t0)) => t0.elapsed().as_millis() as u64,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_capacity_bounded() {
        let log = EventLog::in_memory();
        for i in 0..(RING_CAPACITY + 500) {
            log.log(EventKind::TaskEvent, Some(i as u8), format!("e{i}"));
        }
        assert_eq!(log.tail(1)[0].detail, format!("e{}", RING_CAPACITY + 499));
        assert_eq!(log.ring_len_for_test(), RING_CAPACITY);
        assert_eq!(log.total(), (RING_CAPACITY + 500) as u64);
    }

    #[test]
    fn tail_returns_last_n_in_order() {
        let log = EventLog::in_memory();
        for i in 0..10 {
            log.log(EventKind::RunBoundary, None, format!("e{i}"));
        }
        let t = log.tail(3);
        assert_eq!(t.iter().map(|e| e.detail.as_str()).collect::<Vec<_>>(), ["e7", "e8", "e9"]);
    }
}

impl EventLog {
    #[cfg(test)]
    pub fn ring_len_for_test(&self) -> usize {
        self.ring.lock().unwrap().len()
    }
}
