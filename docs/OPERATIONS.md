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

- Build the Rust binaries:

  ```bash
  (cd sim   && cargo build --workspace)   # target/debug/sitsim-cli
  (cd fleet && cargo build --workspace)   # target/debug/mavfleet
  ```

## 1. Single vehicle, physical flight (I-2)

```bash
cd sim && bash tests/run_i2_flight.sh
```

Starts one rustsitsim + one PX4 + a GCS flight driver; asserts arm ->
OFFBOARD climb -> hover -> land -> disarm from the sim's replay ground
truth. Expect `I-2 PASS: PHYSICAL FLIGHT COMPLETE` (~3 min).

## 2. Fleet mission with real dynamics (F-2)

```bash
cd fleet && bash tests/run_f2.sh
```

Two vehicles, each with its own rustsitsim instance (the scenario's
`[sim]` template renders `scripts/run_sitsim_vehicle.sh`), sequential
auction, offboard missions, RTL + land, physical flight asserted from
replay ground truth. Expect `F-2 PASS: FLEET FLEW WITH REAL rustsitsim
DYNAMICS` (~2.5 min).

## 3. The console, live (browser test)

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

- **Raw sim + REST/WS**: `sim/target/debug/sitsim-cli scenario-run
  sim/docs/examples/i1_boot.toml` then `curl :8200/api/status` and open a
  websocket at `:8200/ws/telemetry` (or `/?XTransformPort=8200` through the
  gateway).
- **Fault injection**: `curl -X POST :8200/api/faults -d
  '{"type":"motor_cut","params":{"motor":1}}'` — or use the Sim Console's
  fault form in the browser.
- **Fleet control plane**: `curl :8400/api/fleet`, `curl :8400/api/events`,
  `curl -X POST :8400/api/estop`.

## Environment knobs

| Var | Default | Effect |
|-----|---------|--------|
| `PX4_ROOT` | `../PX4-Autopilot` | PX4 checkout used by fleet harnesses |
| `FLEET_SITSIM_BIN` | `../../sim/target/debug/sitsim-cli` | rustsitsim binary for per-vehicle sims |
| `FLEET_SIM_CFG_DIR` | `./scratch/vsims` | where per-vehicle scenario TOMLs + replays are written |
| `NEXT_PUBLIC_RSIM_API_STYLE` | `gateway` | console routing: `gateway` or `direct` |

## Notes for constrained machines

- The console dev server (Turbopack compile) can exceed 2 GB RSS; on small
  boxes use the production standalone server (`npm run build && npm start`,
  ~150 MB).
- Every harness is single-invocation by design: background processes do not
  survive between shell calls on some platforms, so each script starts,
  asserts, and tears down everything within one call. Do not split them.
