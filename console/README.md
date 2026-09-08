# RustSim Console

Live operator console for the [RustSim](../README.md) stack — one page, four
views (three telemetry engines + the setup plane):

| Console | Backend | What you get |
|---|---|---|
| **Sim Console** | rustsitsim `:8200` | 10 Hz physics telemetry (altitude/attitude/motor strip charts, NED track), tick latency p95, battery, live **fault injection** (10 fault types) and e-stop |
| **Fleet C2** | mavfleet `:8400` | fleet table (mode/FSM/health/heartbeat ages), NED map with geofence + task markers, task board, auction/allocation log, event log, operator e-stop |
| **Operator Map** | mavfleet `:8400` | the QGC Fly/Plan-style **geo map** (Leaflet, ADR-0017): live vehicles from PX4's own GPS fixes with heading + trajectories, the real geofence, home; click the map to **Go To** (OFFBOARD engage) or **Plan** waypoint missions (drag to edit, per-waypoint altitude/hover), Upload → fence-validated → Start mission → the auction flies them; guided action bar: ARM / DISARM / TAKEOFF / LAND / RTL / HOLD, fleet e-stop |
| **Vehicle Setup** | mavfleet `:8400` | the QGC/MP-style configuration workflow (ADR-0016): airframe catalog (UAV/USV/UUV), sensor calibration triggers, power/safety params, flight modes, the live param editor |

All views run **dual-mode**: LIVE when the Rust backend answers,
SIMULATED (client-side mock) when it does not — with automatic 12 s live
retry and a badge that never lies about which mode you are in.

![Sim Console live](../docs/images/rustsim-simconsole-live.png)
![Fleet C2 live](../docs/images/rustsim-fleetc2-live.png)
![Operator Map live](../docs/images/rustsim-operatormap-live.png)

> The Operator Map's geo view anchors to the scenario `[env] origin` the
> sims' HIL_GPS reports from (default: the PX4 test field 47.397770,
> 8.545580) — the map renders PX4's own `GLOBAL_POSITION_INT` estimate, not
> simulator truth. OSM raster tiles when online; an honest graticule
> fallback when tiles are unreachable.

## Run it

```bash
npm install
npm run build && npm start        # production (standalone, ~150 MB RSS)
# or: npm run dev
```

The console itself is a static client — start the Rust backends to see live
data:

```bash
# from the repo root
cd fleet && cargo build --workspace
./target/debug/mavfleet run --fleet tests/demo_live.toml --api-port 8400
```

### Routing modes

| Mode | When | How |
|---|---|---|
| **gateway** (default) | behind the Caddy gateway / a preview proxy | every request is a relative path + `?XTransformPort=<port>`; WS at `/?XTransformPort=<port>` — see `Caddyfile.example` |
| **direct** | local runs without a gateway | `NEXT_PUBLIC_RSIM_API_STYLE=direct npm run build` → absolute `http://127.0.0.1:<port>` REST + `ws://127.0.0.1:<port>/` |

For the gateway mode locally: [install Caddy](https://caddyserver.com/docs/install),
copy `Caddyfile.example` to a `Caddyfile`, `caddy run`, open
`http://localhost:81`.

## Layout

```
src/
  app/                 layout, page (the dual-console shell), globals.css
  components/dashboard SimConsole, FleetC2, OperatorMap + GeoMap (Leaflet),
                       VehicleSetup, charts, fault console
  components/ui/       the shadcn-style primitives actually used
  hooks/               useSimConsole / useFleetC2 / useOperatorMap /
                       useVehicleSetup (the live+mock engines)
  lib/                 conn (routing + protocol normalizers), geo (WGS84
                       NED<->geodetic port), types, mocks
```

The geo math in `src/lib/geo.ts` is a TS port of the Rust `GeoOrigin`
(exact via ECEF + Bowring) — the console converts fence/task NED to
lat/lon with the same numbers the fleet manager uses server-side.

## Browser verification

- `../scripts/browser_live_test.sh` — both consoles LIVE, telemetry moving
- `../scripts/browser_setup_test.sh` — the Vehicle Setup tab end-to-end
- `../scripts/browser_map_test.sh` — the Operator Map end-to-end (map
  clicks place waypoints, upload, start mission, live flight)

See [AGENTS.md](AGENTS.md) for the component's working contracts.
