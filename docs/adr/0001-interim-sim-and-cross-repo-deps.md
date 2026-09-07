# ADR 0001: Interim simulator strategy and deferred cross-repo dependency

**Status**: Accepted (v0.1, sandbox phase)
**Context**: Spec §16 specifies that mavfleet depends on rustsitsim via a **git
dependency** (`sitsim-sdk` for scenario generation and process control,
`sitsim-mavlink` for the codec) plus the `sitsim-cli` binary at runtime via
`FLEET_SITSIM_BIN`/PATH, with both repos pinning the same PX4 version. During
concurrent bring-up, rustsitsim's crates are not published/tagged yet, and the two
repos are built in parallel.

**Decision**:
1. In this phase mavfleet is **path-independent**: `fleet-mavlink` carries its own
   GCS-side MAVLink v2 codec (a different message subset than rustsitsim's HIL codec —
   telemetry, commands, setpoints — with shared framing rules and its own golden
   vectors). No Cargo path/git dependency on the rustsitsim repo.
2. The per-vehicle simulator process is **configurable**: a command template rendered
   per vehicle (see ADR-0008). The default resolution prefers the rustsitsim binary
   (`FLEET_SITSIM_BIN`, then PATH) and falls back to the vendored Python prototype
   `scripts/sim_stream.py`, which boots PX4 fully but has minimal dynamics — enough
   for protocol-level fleet tests (F-1, and F-2/F-3 at the protocol level), while
   physical flight verification is deferred to integration with the real simulator.
3. At publication time (both repos tagged v0.1), switch `Cargo.toml` to the git
   dependency per spec §16 and drop the vendored prototype from the default path.

**Consequences**: Fleet CI does not need rustsitsim to run protocol-level tests; the
`px4-version` pin is identical in both repos and the harness warns on mismatch. The
codec duplication is bounded (framing + ~15 GCS messages) and disappears at
publication.
