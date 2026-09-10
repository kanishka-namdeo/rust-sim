# Deployment — running RustSim on your own system

You cloned this repository onto a machine (a lab box, a home server, a cloud
VM — anything Linux x86-64) and want the stack running and reachable from a
browser. This is the guide. It covers the one-host quickstart, then the
networking that matters when the browser is on a *different* machine than
the server, which is the normal way to run it.

For the Z.AI sandbox's constrained environment (no Rust/cmake, trimmed npm
registry) see [SANDBOX_SETUP.md](SANDBOX_SETUP.md) instead — every fix for
that environment lives there and is not needed on a normal machine.

## 0. What ends up running

```
your machine ("the host")
  px4 + sitsim-cli   per vehicle (spawned by the manager, localhost only; on-demand)
  mavfleet           fleet manager + REST/WS control plane   127.0.0.1:8400  (on-demand)
  fleet-supervisor   SITL lifecycle manager (start/stop mavfleet)  127.0.0.1:8500
  fleet-catalog      mission + ULog + preset REST server  127.0.0.1:8300
  console            Next.js standalone server              127.0.0.1:3000
  caddy              the gateway (the ONLY network-facing listener)  :81
```

Everything except the Caddy gateway binds **localhost only** — the Rust
control planes and the console are not directly network-exposed by design.
The gateway is the single entry point, exactly like the preview proxy this
repo was built against.

**SITL is operator-driven (QGC/MP pattern, ADR-0030).** The catalog
(:8300), supervisor (:8500), and console (:3000) start together
(`scripts/stack_up.sh start`); the fleet manager (:8400 + N PX4 SITL pairs)
starts on demand when the operator clicks *Start SITL* in the GCS UI or
runs `scripts/stack_up.sh start-fleet`. Closing the GCS does not have to
mean killing the fleet, and vice versa.

## 1. Prerequisites (once)

| Tool | Version | Notes |
|------|---------|-------|
| git | any | clone + PX4 submodules |
| Rust | stable 1.75+ | `rustup` recommended |
| Node | 20+ (or Bun 1.1+) | console build |
| Python | 3.12+ | harness oracles (`pip install pymavlink pyulog`) |
| Caddy | 2.x | the gateway (`apt install caddy` or the official repo) |
| cmake + ninja | any recent | PX4 build (usually already present) |
| Disk | ~5 GB free | PX4 checkout+build ~3.5 GB, Rust targets ~1.5 GB |

Clone (public read access needs no token; add one only if you push):

```bash
git clone https://github.com/kanishka-namdeo/rust-sim.git
cd rust-sim
```

## 2. Build PX4-Autopilot v1.16.2 (once, ~5-15 min)

```bash
git clone --depth 1 --branch v1.16.2 --recurse-submodules --shallow-submodules \
    https://github.com/PX4/PX4-Autopilot.git ../PX4-Autopilot
cd ../PX4-Autopilot && make px4_sitl_default
# binary: build/px4_sitl_default/bin/px4
```

> Full clones have NuttX tags; the shallow clone above is what the harnesses
> use. If the version-header build step dies with an IndexError, fetch the
> missing tag (see [SANDBOX_SETUP.md](SANDBOX_SETUP.md) §6). Create the
> per-instance dirs the harnesses expect:
> `mkdir -p build/px4_sitl_default/instance_{0,1}`.

## 3. Build the Rust planes + the console

```bash
(cd sim   && cargo build --workspace)    # ~1 min
(cd fleet && cargo build --workspace)    # ~1.5 min
(cd console && npm install --no-audit --no-fund && npm run build)
```

Console output is a standalone server: `console/.next/standalone/server.js`
(~150 MB RSS in production mode; the dev server can exceed 2 GB).

## 4. Run the stack (one host, browser on the same host)

```bash
# terminal 1 — the persistent operator stack (catalog + supervisor + console, NO fleet)
bash scripts/stack_up.sh start
# starts catalog :8300 + supervisor :8500 + console :3000.
# The GCS is cold: browse it, configure it, but no vehicles are live.

# terminal 2 (or the GCS UI) — start SITL on demand
bash scripts/stack_up.sh start-fleet
# hits the supervisor's POST /api/sitl/start → spawns mavfleet on :8400
# + N PX4 SITL pairs. Equivalent: open the GCS in a browser, click
# "Start SITL" in the SITL Manager overlay panel (hold-to-confirm).
```

Open `http://localhost:81` — the Operations Canvas + overlay panels
(Mission, Library, Fleet C2, SITL, Setup, Analyze, PreFlight, Settings,
Cheat) all talk through the gateway. `operator_session.toml` is the
default scenario (a `hold_for_setup` bench that keeps the fleet READY
and disarmed for configuration work); pick a different one from the
SITL Manager's scenario dropdown if you want to fly.

Smoke checks:
```bash
curl http://127.0.0.1:8500/api/sitl/status    # {"ok":true,"data":{"running":true,"vehicle_count":2,...}}
curl http://127.0.0.1:8400/api/fleet          # the live fleet frame (when SITL is running)
curl http://127.0.0.1:8300/api/missions       # the mission catalog
```

To stop SITL (keeps catalog + supervisor + console up):
```bash
bash scripts/stack_up.sh stop-fleet          # hits POST /api/sitl/stop on :8500
```

To bring the whole stack down:
```bash
bash scripts/stack_up.sh stop                 # all four planes, ports verified
```

## 5. Browser on a different machine (the normal setup)

The gateway listens on `:81` on **all** interfaces, so on a trusted LAN
nothing else is needed:

```
http://<host-ip-or-name>:81
```

- The console's default routing mode is `gateway`: every request is a
  relative path + `?XTransformPort=<port>`; Caddy forwards to the
  localhost-bound Rust planes. One origin serves the whole stack — this is
  the topology the browser harness (O-2) verifies end-to-end.
- The Rust planes (:8200+i, :8400) and the console (:3000) stay
  localhost-bound. Do not port-forward them individually; route everything
  through the gateway (or a tunnel, below).
- Check the host firewall allows inbound TCP 81.

**Security posture (read this):** the stack has **no authentication and no
TLS** — it is a simulation bench, not a production service. Keep it on a
trusted LAN or behind a VPN. For anything less trusted, do not expose :81
directly; use the SSH tunnel below.

### Remote access over SSH (no LAN, e.g. a cloud VM)

The console's `direct` mode points at `127.0.0.1`, which tunnels perfectly:

```bash
# on your laptop — forward the console + the catalog + the supervisor
ssh -N -L 3000:127.0.0.1:3000 \
       -L 8300:127.0.0.1:8300 \
       -L 8500:127.0.0.1:8500 \
       -L 8400:127.0.0.1:8400 user@host
```

Then open the console built for direct routing:

```bash
cd console && NEXT_PUBLIC_RSIM_API_STYLE=direct npm run build
node .next/standalone/server.js     # browse http://localhost:3000
```

(`direct` is a **build-time** env — it bakes absolute localhost URLs into
the bundle. The default gateway mode works through the tunnel too, if you
also forward :81 and browse `http://localhost:81`. :8500 is only needed
if you want to drive the SITL supervisor from your laptop; :8400 is only
needed while SITL is running.)

## 6. Port map on the host

| Port | Binds | Purpose |
|------|-------|---------|
| 81 | all interfaces | Caddy gateway — the entry point for browsers |
| 3000 | localhost | console (Next.js standalone) |
| 8500 | localhost | **fleet-supervisor** — SITL lifecycle manager (ADR-0030) |
| 8300 | localhost | fleet-catalog — mission + ULog + preset REST server |
| 8400 | localhost | mavfleet control plane (on-demand, spawned by :8500) — REST + WS |
| 8200 + i | localhost | per-vehicle sitsim-cli control plane (REST + WS; UI no longer consumes) |
| 4560 + i | localhost | HIL TCP (PX4 connects in, lockstep) |
| 14540 + i / 14580 + i | localhost | per-vehicle MAVLink UDP (telemetry / onboard) |

Only 81 needs to be reachable from other machines (or none, with a tunnel).
The supervisor (:8500) is the only SITL lifecycle entry point — the GCS
UI and `start-fleet` both go through it.

## 7. Running scenarios other than the bench

Any scenario TOML works as the `--fleet` argument to `mavfleet run`
(the supervisor passes the same argument when the operator picks the
scenario in the GCS UI's SITL Manager panel). The scenario reference
(fleet, geofence, tasks, env) is
[fleet/docs/SCENARIOS.md](../fleet/docs/SCENARIOS.md). The catalog +
supervisor + console stack stays up while you swap scenarios — stop
SITL (`stack_up.sh stop-fleet` or the GCS UI's Stop button), start SITL
again with a different scenario:

```bash
# pick a scenario from the supervisor's list
curl http://127.0.0.1:8500/api/sitl/scenarios
# stop the current fleet (if running)
bash scripts/stack_up.sh stop-fleet
# start SITL with a different scenario (CLI)
curl -X POST http://127.0.0.1:8500/api/sitl/start \
  -H 'Content-Type: application/json' \
  -d '{"scenario":"demo_live.toml"}'
```

> **Note (2026-09-10 cleanup).** The `mavfleet check` subcommand, the
> scenario DSL `[[event]] kind=fault` timeline, the runtime hot-swap
> (`PUT /api/fleet`), runtime task append (`POST /api/tasks`), and the
> fault proxy (`POST /api/vehicles/{i}/faults`) were removed end-to-end
> in the lean cleanup. The scenario TOML's `[[tasks]]` / `[[event]]` /
> `[success]` blocks are accepted for forward-compat but silently ignored
> by the lean manager. See ADR-0030 + the worklog.

## 8. Keeping it running

- The persistent operator stack (`scripts/stack_up.sh start`) is the
  always-on shape: catalog (:8300) + supervisor (:8500) + console (:3000).
  None of them spawn PX4 SITL pairs; idle CPU/RAM is near-zero. Wrap
  `stack_up.sh start` in a systemd unit if you want it always-on at boot:

  ```ini
  # /etc/systemd/system/rustsim-stack.service (sketch)
  [Service]
  WorkingDirectory=/opt/rust-sim
  ExecStart=/opt/rust-sim/scripts/stack_up.sh start
  ExecStop=/opt/rust-sim/scripts/stack_up.sh stop
  Restart=on-failure
  ```

- The fleet manager is a child of the supervisor; it spawns on demand
  (`POST /api/sitl/start`) and exits when the operator stops SITL or the
  mission ends. Run artifacts (events + per-vehicle logs) land in
  `<fleet>/scratch/fleet-run-<unix_s>/`.
- Teardown is verified automatically: a `stop-fleet` (or
  `POST /api/sitl/stop`) leaves no px4/sim processes and frees :8400 +
  the per-vehicle ports. If a hard kill ever leaves squatters, the
  browser harnesses' cleanup pattern is: `pkill -f run_sitsim_vehicle.sh;
  pkill -x px4`.

## 9. Verification on your machine

The whole claim chain re-runs anywhere PX4 builds (single-invocation
harnesses — they invoke the `mavfleet` CLI directly, no persistent
stack required):

```bash
(cd sim   && bash tests/run_i1.sh)             # PX4 boot gate
(cd sim   && bash tests/run_i2_flight.sh)      # one real flight
(cd fleet && bash tests/run_f1.sh)             # bring-up + e-stop
(cd fleet && bash tests/live_test_setup.sh)    # S-1, vehicle setup plane
(cd fleet && bash tests/live_test_operator.sh) # O-1, the map plane
bash scripts/browser_map_test.sh               # O-2, end-to-end in a browser
```

> **Note (2026-09-10 cleanup).** `fleet/tests/run_f2.sh` (F-2 auctioned
> mission) and `fleet/tests/live_test_runtime.sh` (R-1 runtime plane) were
> deleted — see ADR-0030 + the worklog.

The recorded results live in [VERIFICATION.md](VERIFICATION.md).
