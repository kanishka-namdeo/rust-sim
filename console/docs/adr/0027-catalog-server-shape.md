# ADR-0027: `:8300` server shape — new `fleet-catalog` binary in fleet-mission crate

- Status: Accepted
- Date: 2026-09-09
- Owning milestone: M1 (non-blocking for design, blocking for first commit)
- Supersedes: none
- Related: GCS_SPEC.md §4.2 (topology — the `:8300` box), §7 (API contracts
  on `:8300`); ADR-0019 (MissionFile struct), ADR-0020 (store module),
  ADR-0026 (validation module), ADR-0029 (version check)

## Context

GCS_SPEC.md §4.2 introduces a new `:8300` mission-catalog + replay server.
The spec listed three candidate shapes (GCS_SPEC.md §12 Q-9 / ADR-0027):

1. **New `fleet-mission` binary** — a standalone binary that owns the
   catalog, validation, and replay-server code.
2. **`fleet-cli serve` extension** — add a `serve` subcommand to the
   existing `mavfleet` binary that starts the `:8300` server.
3. **Next.js API route with Rust sidecar** — the console's Next.js app
   proxies to a Rust sidecar.

The decision matters because it determines where the `MissionFile`
struct, the `store` module, the validation module, and the version-check
code live — and whether the catalog can stay up while the fleet manager
(`:8400`) is torn down between runs (the existing harnesses tear down
`:8400` frequently; the catalog must survive).

## Decision

**New `fleet-catalog` binary in the existing `fleet-mission` crate.**
The `fleet-mission` crate already exists and owns mission-related types
(scenario DSL, runner, report). The GCS catalog code (`MissionFile`,
`store`, `validation`, `catalog` REST server) joins it as new modules.
The binary `fleet-catalog` is a new `[[bin]]` target in the same crate,
parallel to how `fleet-cli`'s `mavfleet` binary sits alongside library
code.

### Module layout

```
fleet/crates/fleet-mission/
├── Cargo.toml              # adds axum, tokio, ulid, chrono deps + new bin target
├── src/
│   ├── lib.rs              # re-exports new modules
│   ├── scenario.rs         # existing (unchanged)
│   ├── compile.rs         # existing (unchanged)
│   ├── runner/             # existing (unchanged)
│   ├── report.rs           # existing (unchanged)
│   ├── gcs/                # NEW — GCS catalog code
│   │   ├── mod.rs          # pub mod mission_file; pub mod store; pub mod validation; pub mod server; pub mod version_check;
│   │   ├── mission_file.rs # ADR-0019: MissionFile struct + serde
│   │   ├── store.rs        # ADR-0020: atomic writes, versioning, soft-delete
│   │   ├── validation.rs   # ADR-0026: 13 validation rules
│   │   ├── version_check.rs# ADR-0029: PX4 version gate
│   │   └── server.rs       # axum REST server on :8300
│   └── main.rs             # NEW — the fleet-catalog binary entry point
└── tests/                  # integration tests
```

The binary is named `fleet-catalog` (not `fleet-mission` to avoid
confusion with the crate name; not `mavfleet` to avoid confusion with
the fleet manager binary). It is invoked as `fleet-catalog --port 8300
--catalog-dir /var/lib/rustsim/catalog`.

### Why a new binary (not `fleet-cli serve`)

- **Lifecycle independence.** The catalog must stay up while the fleet
  manager is torn down between runs (the existing harnesses tear down
  `:8400` frequently; a mission saved during one run must be available
  for the next). A separate binary means the catalog has its own
  process, its own PID, its own restart policy. If `mavfleet` crashes,
  the catalog survives.
- **Dependency isolation.** The catalog needs `axum` (already a
  workspace dep), `ulid` (new), `chrono` (new). Adding these to
  `fleet-cli`'s dependency closure would slow the `mavfleet` build
  (already 88 s) for code that `mavfleet` doesn't use.
- **Single responsibility.** `mavfleet` is the fleet manager (spawn,
  supervise, fly, report). `fleet-catalog` is the mission catalog
  (store, validate, serve). Mixing them in one binary blurs the
  boundary.

### Why in the existing `fleet-mission` crate (not a new crate)

- **Code reuse.** The catalog's `MissionFile` struct is a GCS-specific
  type, but the validation rules (ADR-0026 V-3, V-4) reuse the
  existing `fleet-core` polygon-containment code. Keeping the catalog
  in `fleet-mission` means it can depend on `fleet-core` (already a
  dep) without a new crate boundary.
- **The crate name fits.** `fleet-mission` already owns "mission" types
  (scenario, runner, report). The GCS catalog is mission-related; the
  name is accurate.
- **No new crate overhead.** A new crate means a new `Cargo.toml`, a
  new entry in the workspace `members` list, a new `lib.rs`. Adding
  modules to an existing crate is simpler.

### Why not Next.js API route + Rust sidecar

- **Two moving parts.** A Next.js API route that proxies to a Rust
  sidecar adds a hop (browser → Next.js → Rust sidecar) for no
  benefit. The catalog's API is REST + WS, same as `:8400`; the
  console already talks to `:8400` directly via the Caddy gateway.
  Adding Next.js in the middle would complicate the routing.
- **The console is a pure client.** `console/AGENTS.md` Local Contracts:
  "No database, no auth, no server state: the console is a pure client
  of the two Rust control planes." Adding API routes to the console
  would violate this contract. The catalog is a third control plane
  (`:8300`), not a console feature.

## Consequences

### Positive

- **Lifecycle isolation.** `fleet-catalog` can be started once and left
  running; `mavfleet` can be started/stopped per run without affecting
  the catalog.
- **Existing deps reused.** `axum`, `tokio`, `serde`, `serde_json`,
  `toml` are already workspace deps; only `ulid` and `chrono` are new
  (both small, both pure Rust).
- **Clean binary name.** `fleet-catalog` is unambiguous; `--help` makes
  its purpose clear.
- **Test isolation.** The catalog's unit tests live in `fleet-mission`'s
  test suite; the `mavfleet` binary's integration tests are unaffected.

### Negative

- **Two binaries to run.** An operator running the full GCS stack must
  start both `mavfleet` (fleet manager) and `fleet-catalog` (mission
  catalog). Mitigation: a `scripts/start_gcs.sh` wrapper that starts
  both (plus the console dev server + Caddy) is a v1 deliverable.
- **The `fleet-mission` crate grows.** It now contains both the
  existing scenario/runner/report code and the new GCS catalog code.
  Mitigation: the GCS code is namespaced under `src/gcs/`; the existing
  code is unchanged. The crate's public API (`lib.rs` re-exports) keeps
  the two concerns separable.

## Alternatives considered

### `fleet-cli serve` extension (rejected)

Add a `serve` subcommand to `mavfleet` that starts the `:8300` server
in a thread alongside the fleet manager. **Rejected** because: the
fleet manager's lifecycle (start → run → teardown → exit) does not
match the catalog's lifecycle (start → run forever). Forcing them
into one process means either the catalog dies when `mavfleet` exits
(bad for mission persistence) or `mavfleet` must be refactored to
support a "serve-only" mode (unnecessary complexity).

### New `fleet-catalog` crate (rejected)

A brand-new crate `fleet/crates/fleet-catalog/` that depends on
`fleet-core`. **Rejected** because: the crate name would collide with
the binary name, and the code is mission-related (fits `fleet-mission`).
A new crate is justified only when the code is reusable across
multiple consumers or has a different release cadence; neither applies
here.

### Next.js API route (rejected)

Implement the catalog as Next.js API routes in `console/src/app/api/`.
**Rejected** because: it violates `console/AGENTS.md`'s "no server
state" contract, and the catalog's persistence (filesystem with atomic
writes) and validation (polygon geometry) are Rust-native code that
would need reimplementation in TypeScript.

## Verification

- **Unit:** `fleet-mission`'s test suite grows with the new `gcs/`
  modules' tests. `cargo test -p fleet-mission` runs both the existing
  scenario/runner/report tests and the new catalog tests.
- **Integration:** G-0, G-1, G-2 (GCS_SPEC.md §9) all start
  `fleet-catalog` as a subprocess, exercise the REST API, and tear it
  down. The harness scripts live under `console/tests/` (per the spec)
  and invoke `fleet/crates/fleet-mission/target/debug/fleet-catalog`.
- **Regression:** the existing `cargo test --workspace` in `fleet/`
  (169 tests) continues to pass — the new modules are additive; the
  existing modules are unchanged.

## References

- GCS_SPEC.md §4.2 (topology), §7 (API contracts on `:8300`), §12 Q-9
- ADR-0019 (MissionFile struct — lives in `src/gcs/mission_file.rs`)
- ADR-0020 (store module — lives in `src/gcs/store.rs`)
- ADR-0026 (validation module — lives in `src/gcs/validation.rs`)
- ADR-0029 (version check — lives in `src/gcs/version_check.rs`)
- `console/AGENTS.md` Local Contracts (no server state in console)
- Existing crate pattern: `fleet-cli`'s `mavfleet` binary alongside
  library code in the same crate
