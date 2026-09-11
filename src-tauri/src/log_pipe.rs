// src-tauri/src/log_pipe.rs
// M-T4 (Tauri repurpose): reserved for future shared log-rotation helpers.
// Currently the actual piping logic is inline in backends.rs (one
// tokio::spawn task per Child stdout/stderr).
//
// Spec: docs/TAURI_APP_SPEC.md Appendix E.5.

pub const MAX_LOG_LINE_LEN: usize = 4096;
pub const LOG_ROTATION_SIZE_MB: u64 = 50;
