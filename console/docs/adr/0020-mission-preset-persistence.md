# ADR-0020: Mission + preset persistence — filesystem with atomic writes

- Status: Accepted (the mission + preset persistence half is current;
  the **`.replay` symlink + `/api/replays*` routes half is removed in
  the 2026-09-10 cleanup** — see the "Replay file storage" historical
  note below)
- Date: 2026-09-09
- Owning milestone: M1 (Plan View MVP), M4 (Vehicle Setup extensions)
- Supersedes: none
- Related: GCS_SPEC.md §5.1 (mission persistence), §5.3 (param presets), §6.1;
  ADR-0019 (file format — the *what*; this ADR is the *where*)

> **2026-09-10 cleanup note.** The mission + preset persistence half of
> this ADR is still current — the `:8300` catalog server still stores
> missions as `<ulid>.toml` files with atomic writes (temp-file + fsync
> + rename) and per-vehicle param presets in `presets/<vid>.toml`. The
> **`.replay` symlink half is removed**: the GCS UI's Analyze `.replay`
> tab was trimmed (Task 7a/7b), so the catalog's `/api/replays*` routes
> + handlers + replay-list index entries were removed with it. The
> "Replay file storage" section below is kept as the v0.1 design
> record. The console's Analyze overlay now serves only ULog browse +
> plot (G-11 trimmed).

## Context

The GCS catalog server (`:8300`, GCS_SPEC.md §4.2) must persist two
kinds of operator-authored state across process restarts:

1. **Missions** — TOML files (per ADR-0019) containing waypoints,
   geofence, rally points. Estimated volume: 10-1000 missions per
   operator over a project's lifetime; each file is 1-10 KB. The
   operator edits them via the Plan View; the catalog must survive
   crashes, power loss, and process restarts without losing data.

2. **Parameter presets** — named snapshots of a vehicle's parameter
   set (e.g. "aggressive-corners" with 2 changed params). Volume:
   10-100 presets per operator; each is a small TOML (ADR-0025) of
   1-20 KB. The operator saves them from Vehicle Setup; the catalog
   must restore them on demand.

Three candidate persistence layers were considered:

1. **Filesystem (one file per mission/preset)** — TOML files in a
   directory tree. Atomic writes via temp-file + fsync + rename. No
   external dependencies; matches the existing RustSim pattern
   (`.replay` files, scenario TOMLs).

2. **SQLite** — a single `missions.db` file with `missions` and
   `presets` tables, TOML stored as TEXT columns. Transactional,
   queryable, but adds a `rusqlite` dependency (~2 MB compiled) and
   requires bundling SQLite's C library (already present in most
   Linux distros but an extra build step on Windows/macOS).

3. **sled** — a Rust-native embedded KV store (`sled::Db`). No C
   dependency; Rust-native concurrency. But the sled project has
   been in maintenance mode since 2022 (no 1.0 release; the README
   explicitly says "do not use in production"), and the RustSim
   AGENTS.md rule against speculative dependencies applies.

The decision matters because migrating from one persistence layer
to another is a one-way data migration: every saved mission and
preset must be re-written into the new layer. Operators who have
built up a mission library over months will not tolerate a migration
that loses their work. The layer chosen in v1 will likely persist
through v1.x and into v2.

This ADR does **not** decide:
- The file *format* (TOML vs JSON) — that's ADR-0019.
- The server *shape* (new binary vs `fleet-cli serve`) — that's
  ADR-0027.
- The preset *format* — that's ADR-0025 (which will follow ADR-0019's
  TOML-on-disk convention for consistency).

## Decision

**Filesystem with atomic writes.** Missions and presets are stored
as individual TOML files in a versioned directory tree. No SQLite,
no sled, no embedded database. Atomic writes via the standard
temp-file + fsync + rename pattern. Version history is retained as
sibling files (one per version). Soft-deletes are tombstone files.

### Directory layout

```
$RSIM_CATALOG_DIR/                       # default: /var/lib/rustsim/catalog
├── missions/
│   ├── 01J8K2.../                       # ULID per mission
│   │   ├── v1.toml                      # version 1
│   │   ├── v2.toml                      # version 2 (after first edit)
│   │   ├── v3.toml                      # version 3 (current)
│   │   ├── meta.json                    # {name, created_at, updated_at, current_version, deleted: false}
│   │   └── .tombstone                   # present only if soft-deleted
│   └── 01J8K3.../
│       └── ...
├── presets/
│   ├── vehicle_0/
│   │   ├── aggressive-corners.toml
│   │   └── test-1.toml
│   └── vehicle_1/
│       └── ...
└── replays/                             # symlinks to .replay files elsewhere
    ├── i2_flight.replay -> /home/z/my-project/rust-sim/sim/i2_flight.replay
    └── f2_demo.replay -> /home/z/my-project/rust-sim/fleet/vehicle_0.replay
```

### Atomic write protocol

Every write to a mission file follows this exact sequence (the same
pattern SQLite uses internally for its journal):

1. Write the new TOML content to a temp file in the same directory:
   `01J8K2.../v4.toml.tmp.<pid>.<rand>`.
2. `fsync()` the temp file's file descriptor. This flushes the
   content to disk; without it, a crash after `rename()` could leave
   the renamed file with un-flushed content.
3. `rename()` the temp file to the target: `v4.toml.tmp.<pid>.<rand>`
   → `v4.toml`. On POSIX systems, `rename()` is atomic — the
   directory entry either points to the old file or the new file,
   never to a half-written file.
4. `fsync()` the parent directory. This flushes the directory entry
   update to disk; without it, a crash after step 3 could leave the
   directory pointing to the (now-unlinked) old file.

The catalog server implements this in a `write_atomic(path, content)`
helper (`fleet-mission/src/store.rs` or `fleet-cli/src/store.rs`,
per ADR-0027). Every mission save, preset save, and meta.json update
goes through this helper. There is no other write path.

### Version history

Every `PUT /api/missions/{id}` (update) creates a new version file:
- Read the current `meta.json` to get `current_version`.
- Write the new TOML to `v<N+1>.toml` (atomic).
- Update `meta.json` with `current_version = N+1` and `updated_at =
  <now>` (atomic).
- The old `v<N>.toml` is **not** deleted. It remains on disk for
  diff, rollback, and replay referential integrity (a `.replay` file
  recorded at v2 can be diffed against v3 even after v3 is saved).

A `GET /api/missions/{id}?version=N` endpoint fetches a specific
version. A `POST /api/missions/{id}/rollback?to_version=N` creates
a new version (v<N+1>) whose content is a copy of v<N> — it does
not delete intermediate versions.

### Soft-delete

`DELETE /api/missions/{id}` writes a `.tombstone` file (containing
the deletion timestamp) to the mission's directory. The mission
files (v1..vN, meta.json) are **not** deleted. The catalog's
`GET /api/missions` (list) excludes tombstoned missions by default;
a `?include_deleted=true` query includes them with a `deleted: true`
flag. This preserves referential integrity: a `.replay` file or a
fleet-run report that references a deleted mission by id can still
resolve its content.

A future `:8300` admin endpoint `POST /api/admin/gc` will physically
delete tombstoned missions older than a configurable threshold
(default: 90 days). This is a v1.1 candidate; v1 never physically
deletes.

### Concurrency

v1 is single-operator (GCS_SPEC.md §3.2 non-goal: production
hardening). The catalog server is a single process; concurrent
writes from multiple tabs (e.g. two browser tabs editing the same
mission) are undefined behavior — last-write-wins at the file level.
The catalog does not implement locking.

If two `PUT /api/missions/{id}` requests arrive simultaneously:
- Both create temp files (different PIDs / random suffixes, no
  collision).
- Both `fsync()` their temp files.
- Both `rename()` to `v<N+1>.toml`. The second rename overwrites
  the first (POSIX rename is atomic, so the directory entry points
  to either the first or the second write, never a half-write).
- Both update `meta.json`. Same atomic-rename; the second write wins.
- The loser's write is lost. The catalog does not detect this.

This is acceptable for v1. A v1.1 may add optimistic concurrency
control via an `If-Match: <version>` header on `PUT`; the catalog
returns HTTP 409 if the current version differs from the header.
This is a documented v1.1 candidate, not a v1 requirement.

### Preset storage

Presets follow the same atomic-write protocol, but the directory
structure is per-vehicle (`presets/vehicle_<i>/<name>.toml`) rather
than ULID-versioned. A preset is overwritten in place (no version
history) — the operator's mental model is "save presets, not
version presets." If the operator wants to preserve an old preset,
they save it with a new name.

### Replay file storage (HISTORICAL — removed 2026-09-10)

> The `/api/replays*` routes + handlers + replay-list index entries in
> `fleet-mission/src/gcs/server.rs` were removed in the 2026-09-10
> cleanup (Task 7b) when the GCS UI's Analyze `.replay` tab was
> trimmed. The section below is kept as the v0.1 design record.

`.replay` files are not stored in the catalog; they are produced by
`sitsim-cli` in the sim/fleet test directories. The catalog's
`replays/` directory contains symlinks to those files (created by
the catalog's discovery scan on startup + on a 60-second refresh
timer). The catalog never writes to `.replay` files; it only reads
them (for the `/api/replays` list and the WS streaming endpoint).
Symlinks are used (not copies) to avoid duplicating multi-MB files.

If a `.replay` file is deleted from its source location, the
symlink becomes dangling. The catalog's discovery scan prunes
dangling symlinks on each refresh. The `/api/replays` list never
includes dangling symlinks.

## Consequences

### Positive

- **No new dependencies.** The standard library (`std::fs`,
  `std::os::unix::fs::symlink`) plus `toml` (already a workspace
  dep) is sufficient. No `rusqlite`, no `sled`.
- **Human-readable on disk.** An operator can `ls` the catalog
  directory, `cat` a mission TOML, and `cp` a version to recover
  an earlier state — no database client required.
- **Git-friendly.** The catalog directory can be `git init`'d and
  version-controlled by the operator (the catalog does not do this
  itself, but the file layout is git-friendly: no binary blobs, no
  database file). This is a v1.1 candidate (a `git sync` button in
  the UI).
- **Atomic writes are battle-tested.** The temp-file + fsync +
  rename pattern is what SQLite, PostgreSQL, and every mail client
  use. It is correct on every POSIX filesystem (ext4, XFS, APFS,
  btrfs) and on Windows (with `MoveFileEx` + `REPLACE_EXISTING`).
- **Version history is free.** Storing every version as a sibling
  file means diffing is `diff v2.toml v3.toml`. No database query
  needed.
- **Crash recovery is trivial.** On startup, the catalog scans the
  missions directory. A `.tmp.<pid>.<rand>` file indicates a
  crashed write; the catalog deletes it (it was never renamed, so
  it was never the live version). A `meta.json` whose
  `current_version` points to a non-existent `v<N>.toml` indicates
  a crash between the version-file rename and the meta.json update;
  the catalog rolls back `current_version` to the highest existing
  `v<N>.toml` and logs a warning.

### Negative

- **No query language.** "List all missions with a waypoint above
  50 m altitude" requires scanning every TOML file. SQLite would
  answer this in one query. Mitigation: the catalog maintains an
  in-memory index (rebuilt on startup, ~10 ms per 1000 missions)
  for the common queries (list by name, list by mtime, count
  waypoints). Uncommon queries are full scans; acceptable for the
  v1 volume (≤1000 missions).
- **Directory size scales linearly.** With 10,000 missions the
  `missions/` directory has 10,000 subdirectories. `readdir` on a
  flat directory of 10,000 entries is ~50 ms on ext4; acceptable
  but not great. Mitigation: shard into 2-level directories
  (`missions/01/J8K2.../`) if volume exceeds 10,000. This is a
  v1.1 candidate; v1 uses flat `missions/<ULID>/`.
- **No transactions across multiple files.** Saving a mission that
  also updates a fleet-mission binding requires two writes (the
  mission TOML and the binding record); if the second write fails,
  the first is committed. SQLite would wrap both in a transaction.
  Mitigation: the fleet-mission binding lives on `:8400` (not
  `:8300`), so the two-phase-commit problem is between two servers,
  not within one. The catalog's write is atomic; the binding update
  is a separate REST call that can be retried. If the binding fails,
  the mission is still saved (just not bound) — the operator can
  bind it manually.

### Neutral

- **Disk usage is slightly higher than SQLite.** Each mission is a
  separate file (minimum 4 KB on ext4 with default block size); a
  1 KB TOML mission occupies 4 KB on disk. With 1000 missions this
  is 4 MB; negligible. With 10,000 missions it's 40 MB; still
  negligible.
- **Backup is `tar` or `rsync`.** The catalog directory is a
  regular filesystem tree; backup is `tar czf
  catalog_backup.tar.gz /var/lib/rustsim/catalog`. No database
  dump tool needed; no hot-backup vs. cold-backup distinction.
  The catalog does not need to be stopped for backup (the atomic
  write protocol ensures a `tar` running concurrently with writes
  captures a consistent state — each file is either the old or new
  version, never half-written).

## Alternatives considered

### SQLite (rejected for v1)

**Pros:** query language, transactions, mature, well-understood.
**Cons:** adds `rusqlite` (a ~2 MB compiled dependency that bundles
SQLite's C library); requires a `missions.db` file that is not
human-readable; version history requires a separate `versions`
table (more schema); backup requires `sqlite3 .dump` or a hot-
backup API (not a simple `tar`).

**Why rejected:** the v1 volume (≤1000 missions, single operator)
does not need a query language, and the atomic-write protocol gives
us 90% of SQLite's safety at 10% of the complexity. SQLite is the
right answer if v1.1 adds multi-operator support, multi-tenant
isolation, or complex queries (e.g. "find all missions that breach
this airspace polygon") — all of which are v1.1+ scope.

### sled (rejected)

**Pros:** Rust-native, no C dependency, embedded.
**Cons:** the sled project is in maintenance mode (no 1.0 release
as of 2026; the project README explicitly says "do not use in
production"). The RustSim AGENTS.md rule against speculative
dependencies applies: a persistence layer that may be abandoned
is not a foundation for v1.

### PostgreSQL (rejected)

**Pros:** real database, real concurrency, real query language.
**Cons:** requires a running PostgreSQL server — a separate process
to install, configure, back up, and upgrade. The RustSim stack is
currently zero-dependency at runtime (Rust binaries + PX4 SITL
processes); adding PostgreSQL would violate the "single deployable"
goal (GCS_SPEC.md §3.1 G-3). PostgreSQL is the right answer for a
multi-operator production deployment — which is explicitly a v1
non-goal (GCS_SPEC.md §3.2).

### In-memory only (rejected)

**Pros:** simplest possible; no persistence code at all.
**Cons:** every catalog restart loses all missions. Unacceptable
for an operator who has spent hours designing a mission library.
Considered only as a degenerate case; rejected immediately.

## Verification

- **Unit:** `fleet-mission` (or `fleet-cli`) has a `store` test
  module that:
  - Writes a mission, kills the process mid-write (before fsync),
    restarts, asserts the mission is not present (the temp file is
    cleaned up).
  - Writes a mission, kills the process mid-rename (after fsync,
    before rename), restarts, asserts the mission is not present
    (the temp file is renamed to the target only on success).
  - Writes a mission, kills the process after rename but before
    meta.json update, restarts, asserts the catalog rolls back
    `current_version` to the highest existing version file.
  - Writes v1, v2, v3, soft-deletes, lists with `?include_deleted=
    true`, asserts the deleted mission appears with `deleted: true`.
- **Integration:** G-2 (GCS_SPEC.md §9) exercises the full CRUD
  round-trip including: create → fetch → update (v2) → fetch v1 →
  fetch v2 → delete → list (excluded) → list with
  `?include_deleted=true` (included) → rollback to v1 → fetch
  (now v3 with v1's content).
- **Concurrency (v1 known-limitation):** a test that issues two
  simultaneous `PUT` requests and asserts that one wins and the
  other loses (last-write-wins, no corruption). Documented as a
  v1 limitation; optimistic concurrency is a v1.1 candidate.

## References

- GCS_SPEC.md §5.1 (mission persistence), §5.3 (param presets),
  §6.1 (data model), §7 (API contracts), §11 R-4 (persistence
  corruption risk + mitigation)
- ADR-0019 (mission file format — the TOML-on-disk decision this
  ADR depends on)
- ADR-0021 (ULog serving — uses the same `:8300` server but does
  not store ULogs in the catalog; they stay in PX4's build dir)
- ADR-0025 (param preset format — will follow ADR-0019's TOML
  convention; this ADR's `presets/` directory is where they live)
- ADR-0027 (`:8300` server shape — where the `store` module lives)
- SQLite atomic write protocol reference:
  https://www.sqlite.org/atomiccommit.html (the same pattern this
  ADR adopts for the filesystem)
- RustSim AGENTS.md rule against speculative dependencies (grounds
  the sled rejection)
