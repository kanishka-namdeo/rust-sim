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
  px4 + sitsim-cli   per vehicle (spawned by the manager, localhost only)
  mavfleet           fleet manager + REST/WS control plane  127.0.0.1:8400
  console            Next.js standalone server              127.0.0.1:3000
  caddy              the gateway (the ONLY network-facing listener)  :81
```

Everything except the Caddy gateway binds **localhost only** — the Rust
control planes and the console are not directly network-exposed by design.
The gateway is the single entry point, exactly like the preview proxy this
repo was built against.

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
# terminal 1 — the fleet manager (spawns sims + PX4, serves :8400)
cd fleet && ./target/debug/mavfleet run --fleet tests/operator_bench.toml --api-port 8400

# terminal 2 — the console
cd console && node .next/standalone/server.js          # :3000

# terminal 3 — the gateway
caddy run --config console/Caddyfile.example           # :81
```

Open `http://localhost:81` — the Operator Map, Fleet C2, Sim Console and
Vehicle Setup tabs all talk through the gateway. `operator_bench.toml` is
the recommended first scenario: it holds the fleet READY and disarmed for
configuration work, and the Operator Map flies it on demand.

Smoke check: `curl http://127.0.0.1:8400/api/fleet` returns the fleet frame
(a JSON index of every endpoint on `GET /`).

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
# on your laptop — forward the console + both control planes
ssh -N -L 3000:127.0.0.1:3000 \
       -L 8200:127.0.0.1:8200 \
       -L 8400:127.0.0.1:8400 user@host
```

Then open the console built for direct routing:

```bash
cd console && NEXT_PUBLIC_RSIM_API_STYLE=direct npm run build
node .next/standalone/server.js     # browse http://localhost:3000
```

(`direct` is a **build-time** env — it bakes absolute localhost URLs into
the bundle. The default gateway mode works through the tunnel too, if you
also forward :81 and browse `http://localhost:81`.)

## 6. Port map on the host

| Port | Binds | Purpose |
|------|-------|---------|
| 81 | all interfaces | Caddy gateway — the entry point for browsers |
| 3000 | localhost | console (Next.js standalone) |
| 8400 | localhost | mavfleet control plane (REST + WS) |
| 8200 + i | localhost | per-vehicle sitsim-cli control plane (REST + WS) |
| 4560 + i | localhost | HIL TCP (PX4 connects in, lockstep) |
| 14540 + i / 14580 + i | localhost | per-vehicle MAVLink UDP (telemetry / onboard) |

Only 81 needs to be reachable from other machines (or none, with a tunnel).

## 7. Running scenarios other than the bench

Any scenario TOML works as the `--fleet` argument; `mavfleet check <file>`
validates one without running. The scenario reference (fleet, geofence,
tasks, timeline events incl. live-injected faults, success criteria) is
[fleet/docs/SCENARIOS.md](../fleet/docs/SCENARIOS.md). You can also swap
scenarios **without restarting the process**:

```bash
curl -X PUT http://127.0.0.1:8400/api/fleet \
  -d '{"scenario_toml": "<contents of the TOML>"}'
```

(ADR-0018: validated first — 422 on a bad file; 409 while a mission is
flying; the fleet restarts on the same port within seconds and the frame's
`scenario` field names the new source.)

## 8. Keeping it running

- The manager is one process per fleet run: it exits with a CI-classifiable
  code (0 COMPLETE, 2 ABORTED, 3 infrastructure) when the run ends — wrap it
  in a loop or a systemd unit if you want it always-on:

  ```ini
  # /etc/systemd/system/rustsim.service (sketch)
  [Service]
  WorkingDirectory=/opt/rust-sim/fleet
  ExecStart=/opt/rust-sim/fleet/target/debug/mavfleet run --fleet tests/operator_bench.toml
  Restart=on-failure
  ```

- Run artifacts (report + events + per-vehicle logs) land in
  `fleet-runs/fleet-run-<unix_s>/` (or `--run-dir`); each hot-swap run gets
  a `-hot-N` sibling.
- Teardown is verified automatically: a run's exit leaves no px4/sim
  processes and all ports free. If a hard kill ever leaves squatters, the
  browser harnesses' cleanup pattern is: `pkill -f run_sitsim_vehicle.sh;
  pkill -x px4`.

## 9. Verification on your machine

The whole claim chain re-runs anywhere PX4 builds:

```bash
(cd sim   && bash tests/run_i1.sh)             # PX4 boot gate
(cd sim   && bash tests/run_i2_flight.sh)      # one real flight
(cd fleet && bash tests/run_f1.sh)             # bring-up + e-stop
(cd fleet && FLEET_SIM_CFG_DIR=$PWD/scratch/vsims bash tests/run_f2.sh)
(cd fleet && bash tests/live_test_operator.sh)   # O-1, the map plane
(cd fleet && bash tests/live_test_runtime.sh)    # R-1, the runtime plane
bash scripts/browser_map_test.sh                  # O-2, end-to-end in a browser
```

The recorded results live in [VERIFICATION.md](VERIFICATION.md).
