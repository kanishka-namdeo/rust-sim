# Screenshots

Live captures of the RustSim GCS v1 operator console. Each screenshot
shows one of the 7 tabs in SIMULATED mode (backends offline, mock
telemetry engines active) — the same UI renders against live PX4 SITL
when the full stack is running.

## GCS v1 tabs (captured 2026-09-09)

| Screenshot | Tab | GCS_SPEC § | Description |
|---|---|---|---|
| `rustsim-planview-live.png` | Plan | §5.1 | The QGC Plan View analog: Leaflet map + waypoint table + geofence drawing + Patterns dropdown + Save/Upload buttons. Shows a 4-waypoint demo-square mission with a green inclusion fence. |
| `rustsim-flyview-live.png` | Fly | §5.2 | The QGC Fly View analog: SVG attitude HUD + instrument widgets (battery, signal, mode, GPS, EKF) + pre-arm checklist + action bar (ARM/TAKEOFF/LAND/RTL/HOLD) + vehicle selector + strip charts. |
| `rustsim-simconsole-live.png` | Sim Console | §4 | The rustsitsim physics telemetry view: 10 Hz strip charts (altitude, battery, velocity), NED trajectory map, fault injection console, E-STOP. |
| `rustsim-fleetc2-live.png` | Fleet C2 | §5.4 | The fleet manager view: fleet table + task board + event log + mission bindings panel (M5) + Start Fleet button + Patterns dropdown (swarming). |
| `rustsim-operatormap-live.png` | Operator Map | §5.2 | The ADR-0017 geo map: live GPS-marked vehicles, trajectories, geofence; click-to-fly Go To, mission upload, arm/takeoff/land/RTL/hold action bar. |
| `rustsim-vehiclesetup-live.png` | Vehicle Setup | §5.3 | The QGC Vehicle Setup view: airframe/sensors/power/modes/params tabs. Shows the M4 extensions: param search, group dropdown, diff-against-defaults toggle, save/load presets. |
| `rustsim-analyzeview-live.png` | Analyze | §5.5 | The QGC Analyze View analog: file list (replays + ULogs) + trajectory map + timeline scrubber + canvas-based strip charts with "now" line + overlay-live button. |

## Pre-GCS screenshots (I/F/S/O/R era)

These screenshots were captured during the pre-GCS verification runs
(I-1/I-2/F-1/F-2/S-1/S-2/O-1/O-2/R-1) and show the original 4-tab
console. They are preserved for historical reference.

| Screenshot | Description |
|---|---|
| `rustsim-fleetc2-live.png` | Fleet C2 during a live F-2 run (2 vehicles, OFFBOARD mid-mission). |
| `rustsim-operatormap-flight-live.png` | Operator Map during O-1 (vehicle armed, flying a go-to). |
| `rustsim-operatormap-plan-live.png` | Operator Map in Plan mode (waypoint placement). |
| `rustsim-operatormap-live.png` | Operator Map with live GPS markers + fence. |
| `rustsim-vehiclesetup-airframe-live.png` | Vehicle Setup airframe tab (Iris quad). |
| `rustsim-vehiclesetup-boat-live.png` | Vehicle Setup airframe tab (Boat/USV). |
| `rustsim-vehiclesetup-params-live.png` | Vehicle Setup params tab (pre-M4, no search/diff). |
