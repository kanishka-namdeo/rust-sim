# Operations — running RustSim

## 0. Prerequisites (once)

- Rust (stable, 1.75+), Node 20+ (or Bun 1.1+), Python 3.12+ with
  `pymavlink` and `pyulog` for the harness oracles.
- PX4-Autopilot v1.16.2 built from source (the harnesses expect it at
  `../PX4-Autopilot` relative to the repo root, overridable with
  `PX4_ROOT`):

  ```bash
  git clone --depth 1 --branch v1.16.2 --recurse-submodules --shallow-submodules \
      https://github.com/PX4/PX4-Autopilot.git
  cd PX4-Autopilot && make px4_sitl_default
  # binary: build/px4_sitl_default/bin/px4
  ```

- Build the Rust binaries (the supervisor + the manager + the catalog
  all come from the fleet workspace):

  ```bash
  (cd sim   && cargo build --workspace)   # target/debug/sitsim-cli
  (cd fleet && cargo build --workspace)   # target/debug/mavfleet + fleet-catalog + fleet-supervisor
  ```

## 1. The persistent operator stack (QGC/MP pattern, ADR-0030)

SITL is operator-driven. `scripts/stack_up.sh start` brings up the
catalog (:8300) + supervisor (:8500) + console (:3000) — NO fleet.
The operator starts SITL on demand:

```bash
bash scripts/stack_up.sh start               # catalog + supervisor + console (cold)
bash scripts/stack_up.sh start-fleet         # CLI: spawn the fleet (:8400 + PX4 SITL)
bash scripts/stack_up.sh status              # 4-plane state + SITL lifecycle
bash scripts/stack_up.sh stop-fleet          # tear the fleet down (via :8500)
bash scripts/stack_up.sh stop                # everything down, ports verified
```

Or, in the GCS UI (browse http://localhost:81 via the Caddy gateway):
open the **SITL Manager** overlay panel, pick a scenario from the
dropdown, and hold the *Start SITL* button to confirm. The panel
polls `GET :8500/api/sitl/status` at 1 Hz and shows the live vehicle
count once the manager is up.

## 2. Single vehicle, physical flight (I-2)

```bash
cd sim && bash tests/run_i2_flight.sh
```

Starts one rustsitsim + one PX4 + a GCS flight driver; asserts arm ->
OFFBOARD climb -> hover -> land -> disarm from the sim's replay ground
truth. Expect `I-2 PASS: PHYSICAL FLIGHT COMPLETE` (~3 min).

## 3. Fleet mission with real dynamics (F-1)

```bash
cd fleet && bash tests/run_f1.sh
```

Two vehicles, each with its own rustsitsim instance (the scenario's
`[sim]` template renders `scripts/run_sitsim_vehicle.sh`), offboard
missions, RTL + land, physical flight asserted from replay ground
truth. Expect `[F-1] PASS: FLEET BRING-UP + E-STOP COMPLETE` (~2 min).

> **Note (2026-09-10 cleanup).** The F-2 auctioned-mission harness
> (`run_f2.sh`) was deleted end-to-end — its assertions depended on
> the auction allocator, runner, and run-report, all removed. The
> surviving fleet-coverage harness is `run_f1.sh` (bring-up + e-stop)
> plus the operator-driven fleet start path. See ADR-0030.

## 4. The console, live (browser test)

```bash
(cd console && npm install && npm run build)
node console/.next/standalone/server.js &      # :3000
caddy run --config console/Caddyfile.example & # :81 (gateway mode)
bash scripts/browser_live_test.sh
```

The script starts the demo fleet (`fleet/tests/demo_live.toml`), drives a
headless browser through the gateway-served console, asserts LIVE badges +
moving telemetry on both consoles, and saves screenshots to
`docs/images/`.

## Quick manual tours

- **Raw sim + REST/WS** (the sim stays as PX4's HIL physics engine — the
  GCS UI no longer consumes its internal data plane post-2026-09-10):
  `sim/target/debug/sitsim-cli scenario-run
  sim/docs/examples/i1_boot.toml` then `curl :8200/api/status` and open a
  websocket at `:8200/ws/telemetry` (or `/?XTransformPort=8200` through the
  gateway).
- **SITL lifecycle (ADR-0030)** — the supervisor's REST API on :8500:

  ```bash
  curl http://127.0.0.1:8500/api/sitl/status
  curl http://127.0.0.1:8500/api/sitl/scenarios
  curl -X POST http://127.0.0.1:8500/api/sitl/start \
    -H 'Content-Type: application/json' \
    -d '{"scenario":"operator_session.toml"}'
  curl -X POST http://127.0.0.1:8500/api/sitl/stop
  ```

  In the browser this is the **SITL Manager** overlay panel (QGC's Mock
  Link / Mission Planner's Simulation tab equivalent): hold-to-confirm
  Start, single-tap safety-positive Stop.
- **Fleet control plane**: `curl :8400/api/fleet`, `curl :8400/api/events`,
  `curl -X POST :8400/api/estop`.
- **Operator map control (ADR-0017, from the fly/plan map or curl)**:
  upload waypoints (fence-validated, `lat_deg`/`lon_deg`/`alt_m` AGL), start
  the mission, fly it under guided commands, then land:

  ```bash
  curl -X POST :8400/api/mission \
    -d '{"items":[{"lat_deg":47.397889,"lon_deg":8.545734,"alt_m":15,"hover_s":5}]}'
  curl -X POST :8400/api/mission/start
  curl -X POST :8400/api/vehicles/0/takeoff -d '{"alt_m":10}'
  curl -X POST :8400/api/vehicles/0/goto \
    -d '{"lat_deg":47.397889,"lon_deg":8.545734,"alt_m":15}'
  curl -X POST :8400/api/vehicles/0/hold
  curl -X POST :8400/api/vehicles/0/land
  ```

  In the browser this is the **Operator Map** overlay (QGC Fly/Plan-style):
  click the map to Go To, place waypoints in Plan mode, Upload + Start, and
  drive the guided action bar (arm/takeoff/land/RTL/hold, e-stop).

> **Note (2026-09-10 cleanup).** The runtime control plane (ADR-0018) —
> `PUT /api/fleet` (hot-swap), `POST /api/tasks` (runtime append),
> `POST /api/vehicles/{i}/faults` (fault proxy), the auction allocator,
> the 8-policy safety ladder, and the `mavfleet check` subcommand — were
> removed end-to-end. The Sim Console overlay (physics telemetry + fault
> UI) and the Fleet C2 swarming-patterns sub-panel were removed from the
> console. See ADR-0030 + the worklog.

## Environment knobs

| Var | Default | Effect |
|-----|---------|--------|
| `PX4_ROOT` | `../PX4-Autopilot` | PX4 checkout used by fleet harnesses |
| `FLEET_SITSIM_BIN` | `../../sim/target/debug/sitsim-cli` | rustsitsim binary for per-vehicle sims |
| `FLEET_SIM_CFG_DIR` | `./scratch/vsims` | where per-vehicle scenario TOMLs + replays are written |
| `RSIM_ORIGIN_LAT` / `_LON` / `_ALT` | 47.397770 / 8.545580 / 500.0 | geo anchor a per-vehicle sim's HIL_GPS reports from — exported by the manager from the scenario `[env] origin` (ADR-0017); override only for manual `run_sitsim_vehicle.sh` runs |
| `NEXT_PUBLIC_RSIM_API_STYLE` | `gateway` | console routing: `gateway` or `direct` |

## Notes for constrained machines

- The console dev server (Turbopack compile) can exceed 2 GB RSS; on small
  boxes use the production standalone server (`npm run build && npm start`,
  ~150 MB).
- Every harness is single-invocation by design: background processes do not
  survive between shell calls on some platforms, so each script starts,
  asserts, and tears down everything within one call. Do not split them.
- The persistent operator stack (`scripts/stack_up.sh start`) is the
  exception — it daemonizes via a classic double-fork (setsid) so it
  survives the agent-shell process reaper. The catalog + supervisor +
  console idle at near-zero CPU/RAM; the fleet spawns on demand.
