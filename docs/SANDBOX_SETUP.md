# Z.AI sandbox setup — the exact verified sequence

This is the **exact, live-verified** bring-up sequence for the full RustSim
stack (sim + fleet + console + real-PX4 harnesses) in a Z.AI sandbox (the
`zai-web` agent environment). Every command below was executed as written and
every gate it enables PASSED in the verification run recorded at the bottom.

It exists because the sandbox differs from a dev laptop in five ways that
each break the stock quickstart: no Rust toolchain, no cmake/ninja, an npm
registry snapshot that lacks some pinned radix-ui versions, shallow-submodule
PX4 clones that carry no git tags, and a 2-vCPU / 4 GB / ~6 GB-free-disk
budget. All five fixes are below.

Follow it top to bottom; total wall time is ~25 minutes (PX4 build dominates).

## 0. Sandbox environment facts (measured 2026-09-07)

- 2 vCPU, 4.1 GB RAM, ~5.6 GB free disk (a full PX4 clone+build takes
  ~3.5 GB; the two Rust workspaces ~1.5 GB — it fits, but not twice over).
- Preinstalled: git, curl, gcc/g++, make, Node 24 + npm 11, caddy,
  agent-browser, Python 3.12 (venv, `/home/z/.venv/bin/python3`) and
  `/usr/bin/python3.13` (no pip packages initially).
- **Missing**: `rustc`/`cargo`, `cmake`, `ninja` — install in steps 2 and 6.
- The npm registry snapshot does **not** contain `@radix-ui/*` 1.2.x/1.3.x
  versions for alert-dialog, progress, or tabs (step 4 pins what exists).
- `python3` on PATH resolves to the venv Python 3.12; PX4's build scripts
  use it, the live harnesses default to `/usr/bin/python3.13`. Both need
  packages (steps 5 and 6).

## 1. Clone rust-sim (PAT-authenticated)

```bash
cd /home/z/my-project
git clone https://<user>:<PAT>@github.com/kanishka-namdeo/rust-sim.git
cd rust-sim
```

> Do not leave the PAT in shell history or docs; the clone URL with the
> embedded token stays only in `.git/config`.

## 2. Rust toolchain (sandbox ships none)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh
sh /tmp/rustup.sh -y --default-toolchain stable --profile minimal
source "$HOME/.cargo/env"      # needed in EVERY later shell call that runs cargo
```

Verified: rustc/cargo 1.98.1 stable. gcc/g++ are already present so C
dependencies (ring etc.) compile cleanly.

## 3. Build the two Rust workspaces

```bash
source "$HOME/.cargo/env"
(cd sim   && cargo build --workspace)    # ~42 s
(cd fleet && cargo build --workspace)    # ~87 s (one harmless unused-var warning)
```

## 4. Console — pin radix deps to the sandbox registry, then build

`console/package.json` pins three radix-ui versions the sandbox npm registry
does not have. Check availability first, then pin to the newest versions that
exist (all API-compatible for the component subsets `src/` uses):

```bash
cd console
# required pins (verified present in the sandbox registry):
npm pkg set 'dependencies.@radix-ui/react-alert-dialog=^1.1.23' \
             'dependencies.@radix-ui/react-label=^2.1.15' \
             'dependencies.@radix-ui/react-progress=^1.1.16' \
             'dependencies.@radix-ui/react-tabs=^1.1.21'
npm install --no-audit --no-fund          # 411 packages
npm run build                             # standalone out + finish-standalone.mjs
npm run lint                              # clean
```

If `npm install` throws `ETARGET`/`notarget` for a different radix package,
`npm view <pkg> versions --json` and pin the highest stable below the
requested one — the registry snapshot is the constraint, not the API.

## 5. Python harness dependencies

The live harnesses default their oracle to `/usr/bin/python3.13`:

```bash
/usr/bin/python3.13 -m pip install --user --break-system-packages \
    pymavlink pyulog
```

## 6. PX4-Autopilot v1.16.2 — clone, patch two sandbox gaps, build

```bash
cd /home/z/my-project
git clone --depth 1 --branch v1.16.2 --recurse-submodules --shallow-submodules \
    https://github.com/PX4/PX4-Autopilot.git PX4-Autopilot     # ~1.7 GB, minutes
```

The build needs cmake+ninja+Python build deps that the sandbox lacks. cmake
and ninja install cleanly via the venv pip (and land on PATH):

```bash
/home/z/.venv/bin/python3 -m pip install cmake ninja \
    "empy==3.3.4" jinja2 pyyaml pyserial kconfiglib jsonschema pyros-genmsg
/usr/bin/python3.13 -m pip install --user --break-system-packages pyros-genmsg
```

**Gap 1 — `genmsg` module**: without `pyros-genmsg` the build dies at
`[2/1068] Generating uORB topic headers` (`Failed to import genmsg`).
Installed above.

**Gap 2 — NuttX tags missing in a shallow clone**: the version-header
generator (`src/lib/version/px_update_git_header.py`) reads NuttX git tags;
a `--shallow-submodules` clone has none, and the build dies at
`[34/1068] Generating git version header` (IndexError). PX4's NuttX fork
tops out at `nuttx-12.12.0`, which is exactly what a full clone would see:

```bash
cd PX4-Autopilot/platforms/nuttx/NuttX/nuttx
git fetch --depth 1 origin tag nuttx-12.12.0
cd /home/z/my-project/PX4-Autopilot
make px4_sitl_default                    # ~2 min on 2 vCPU after warmup
```

Then create the harness working dirs (PX4's own
`Tools/simulation/sitl_multiple_run.sh` pattern — the repo does not create
them):

```bash
mkdir -p build/px4_sitl_default/instance_0 build/px4_sitl_default/instance_1
```

## 7. Author-layout compatibility for the F-2 harness

`fleet/tests/run_f2.sh` still carries two absolute paths from the author's
pre-unification standalone layout (`/home/z/my-project/rustsitsim`, the sim
repo root, and `/home/z/my-project/mavfleet/scratch/vsims`). Reproduce the
layout with one symlink and one env var instead of patching the harness:

```bash
ln -sfn /home/z/my-project/rust-sim/sim /home/z/my-project/rustsitsim
export FLEET_SIM_CFG_DIR=/home/z/my-project/rust-sim/fleet/scratch/vsims
```

(`run_sitsim_vehicle.sh` and `run_f2.sh` both honor `FLEET_SIM_CFG_DIR`;
without it the two default to *different* directories and the replay
assertions fail.)

## 8. Run the verification ladder

Single-invocation harnesses, one shell call each (do not background them
across calls — processes do not survive between agent tool invocations):

```bash
(cd sim   && bash tests/run_i1.sh)        # I-1 boot gate,   ~1 min, expects "[I-1] PASS"
(cd sim   && bash tests/run_i2_flight.sh) # I-2 real flight, ~3 min, expects "I-2 PASS"
(cd fleet && bash tests/run_f1.sh)        # F-1 bring-up+estop, ~2 min, expects "[F-1] PASS"
(cd fleet && FLEET_SIM_CFG_DIR=... bash tests/run_f2.sh)  # F-2, ~5 min, "F-2 PASS"
```

Browser end-to-end through the Caddy gateway (:81 must answer 502 before
the script runs — start Caddy first, in the same tool call as the test is
NOT required, but the script tears the fleet down itself):

```bash
(cd console && caddy run --config Caddyfile.example &)   # :81 gateway
sleep 3
(cd /home/z/my-project/rust-sim && FLEET_SIM_CFG_DIR=... \
    bash scripts/browser_live_test.sh)                   # "BROWSER LIVE TEST PASS"
```

Vehicle-setup plane (ADR-0016 — the QGC/Mission-Planner-style
configuration workflow, real PX4, single-invocation like everything else):

```bash
(cd fleet && bash scripts/live_test_setup.sh)
# S-1: "LIVE SETUP TEST: 44 passed, 0 failed" — full param download, typed
# writes, calibration, mode switch, airframe apply Iris->Boat->Iris with
# controlled restarts, persistence, error gates, clean teardown.

bash scripts/browser_setup_test.sh
# S-2: "VEHICLE SETUP BROWSER LIVE TEST PASS (13 checks)" — the console's
# Vehicle Setup tab driven end-to-end through the gateway (:81).
# NOTE: caddy :81 must be answering 502 first; the script also kills any
# leaked px4/sim pairs from earlier runs (they squat the per-instance
# ports and abort the fresh fleet with process-death FAULT).
```

Operator map control plane (ADR-0017 — the geo map where the user flies
SITL themselves, single-invocation like everything else):

```bash
(cd fleet && bash tests/live_test_operator.sh)
# O-1: "[O-1] PASS — 15 checks" — geo blocks on the frame, go-to flight,
# hold+land, fence-validated mission upload, auction-flown op* tasks,
# estop teardown.

bash scripts/browser_map_test.sh
# O-2: "OPERATOR MAP BROWSER LIVE TEST PASS (16 checks)" — the Operator Map
# tab through the gateway: Leaflet LIVE, map clicks place waypoints,
# upload + start mission, live flight, screenshots.
# Same caddy/leak preconditions as S-2; also kills stale next-server
# instances (a leaked console server serves an old bundle — the map-fit
# bug class documented in console/AGENTS.md).
```

## 9. Known flakiness and its fix (applied in-repo)

**F-1 READY poll race.** On a fast machine both vehicles flip
`READY -> ACTIVE` together ~200 ms after the second one reaches READY, and
`run_f1.sh`'s 1 Hz strict `fsm == "READY"` poll misses the window — the
manager log and `events.ndjson` prove both vehicles DID reach READY.
`run_f2.sh` already gates on "READY or any post-READY state" for exactly
this race (its inline comment says the window "can be <200 ms wide");
`run_f1.sh` now uses the same predicate. If a future harness regresses this
pattern, apply the f2-style gate, do not widen time budgets.

## 10. Verification record (this sequence, executed 2026-09-07)

| Gate | Result |
|---|---|
| `sim` unit tests | 99/99 PASS |
| `fleet` unit tests | 126/126 PASS |
| I-1 boot gate (real PX4 rcS + EKF2 + loop closed + ULog) | PASS |
| I-2 physical flight (arm → offboard → z −1.72 m → land → disarm) | PASS |
| F-1 bring-up + estop → ABORTED(2) + clean teardown | PASS |
| F-2 two-vehicle auctioned mission, real dynamics, from replay truth | PASS |
| Browser live test (gateway :81, both consoles LIVE, telemetry moving) | PASS |
| S-1 vehicle-setup REST live test (param download, typed writes, calibration, modes, airframe apply + restart + persistence) | PASS (44/44) |
| S-2 Vehicle Setup browser test (QGC-style tab end-to-end, Boat apply via dialog) | PASS (13/13) |
| O-1 operator-map REST live test (geo frame, go-to flight, hold+land, fence-validated upload, auction-flown mission, estop) | PASS (15 checks) |
| O-2 Operator Map browser test (Leaflet LIVE, map-click waypoints, upload + start mission, live flight) | PASS (16/16) |
| Console `npm run lint` + `npm run build` | clean |

Artifacts from this run live under each harness's `tests/*_artifacts/`;
screenshots under `docs/images/`.
