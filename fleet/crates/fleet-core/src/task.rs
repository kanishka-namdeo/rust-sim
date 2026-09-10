//! Task board entry (the wire shape behind `GET /api/fleet`'s `tasks`
//! array and `GET /api/tasks`). Lives in fleet-core since it's the
//! fleet-wide task-table vocabulary, not the (removed) scenario DSL's.
//!
//! Originally part of `fleet-mission::report` (run-report side); moved
//! here when the scenario DSL / run-report / auction autonomy was
//! removed (Task 7b). The GCS still surfaces a task table for operator
//! mission uploads — `TaskStatus` is the wire shape of one row.

#![forbid(unsafe_code)]

use serde::Serialize;

/// One row of the operator task board.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TaskStatus {
    pub id: String,
    pub pos_ned_m: [f32; 3],
    /// None = unassigned; Some(v) = vehicle index.
    pub assigned: Option<u8>,
    /// pending | queued | active | complete | rejected | cleared
    pub state: String,
    pub hover_observed: Option<bool>,
}
