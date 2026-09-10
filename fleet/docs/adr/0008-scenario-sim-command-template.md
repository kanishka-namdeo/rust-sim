# ADR 0008: Per-vehicle simulator command template in the scenario DSL

**Status**: Accepted (v0.1). **Note (2026-09-10 cleanup)**: the
scenario DSL was removed end-to-end in the 2026-09-10 cleanup, but the
`[sim]` section's `command` + `duration_s` keys are still parsed by
the lean `fleet-cli/src/config.rs` parser (kept for the persistent
`tests/*.toml` fixtures + the live harness scripts). The placeholder
substitution (HIL port, instance, sysid, duration, sitsim binary
path) is unchanged. The `[[tasks]]` / `[[event]]` / `[success]` blocks
in the same file are silently ignored post-cleanup.
**Context**: fleet-simctl must spawn a simulator process per vehicle. Which simulator
(rustsitsim binary, the Python prototype, a future custom sim) and with which arguments
is a deployment decision, not a code decision. Spec §9.1's schema table lists no `[sim]`
section.

**Decision**: The scenario TOML gains a `[sim]` section (documented addition to the
spec, schema-frozen like every other key):

```toml
[sim]
command = "python3 /abs/path/sim_stream.py {hil_port} {duration_s}"
duration_s = 300
```

`command` is a whitespace-split argv template (no shell — quoting hazards are avoided
by construction). Placeholders substituted per vehicle: `{hil_port}` (4560+i),
`{instance}` (i), `{sysid}` (i+1), `{duration_s}`, `{sitsim}` (from `FLEET_SITSIM_BIN`
or PATH), `{sim_script}` (vendored prototype). Resolution order: scenario `[sim]
command` → `FLEET_SIM_COMMAND` env → the interim default
(`python3 {sim_script} {hil_port} {duration_s}`). `duration_s` defaults to 300 and is
floored at 30.

**Consequences**: F-tests can run with any simulator without code changes; the default
remains the interim prototype until rustsitsim's CLI is pinned (ADR-0001). The key is
rejected-by-default like all others (`deny_unknown_fields`), so typos fail loudly.
