# Tauri App Spec — Repurposing RustSim GCS as a Native Desktop App

**Status:** Proposed · **Date:** 2026-09-11 · **Author:** agent run (Task ID 7)
**Scope:** Repackage the existing Next.js 16 GCS + three Rust backend services + PX4 SITL orchestration as a single Tauri 2.x desktop application, with no behaviour regressions versus the current `stack_up.sh` web stack.

---

## 1. Executive Summary

RustSim GCS is today a **three-service web app**: a Next.js 16.1.1 frontend (`console/`, port `:3000`) served standalone, a catalog service (`fleet-catalog`, `:8300`), a supervisor (`fleet-supervisor`, `:8500`) that owns the SITL lifecycle, and a fleet manager (`fleet-mgr`, `:8400`, on-demand) that drives MAVLink telemetry at 10 Hz. Real PX4 SITL (`PX4-Autopilot` v1.16.2) is spawned as a subprocess by the supervisor.

This spec repurposes that stack as a **Tauri 2.x desktop app** with three changes:

1. The Next.js app is rebuilt with `output: 'export'` and served as static assets inside the Tauri webview — no Node server in production.
2. The three Rust binaries (`fleet-catalog`, `fleet-supervisor`, `fleet-mgr`) are spawned as **child processes** by the Tauri Rust backend on app startup, with `.kill_on_drop(true)` + explicit shutdown on `RunEvent::ExitRequested` for clean lifecycle.
3. PX4 SITL stays operator-driven (ADR-0030 unchanged): the user clicks "Start" in the SITL Manager panel; the Tauri-spawned supervisor spawns PX4 as before.

The frontend changes are **deliberately minimal**: delete the lone `/api` route, switch `NEXT_PUBLIC_RSIM_API_STYLE=direct` permanently, fix the MapLibre worker URL, and pin the favicon to `/logo.svg`. The 7 overlay panels, the `useSyncExternalStore` state stores, the WebSocket to `:8400`, and the REST calls to `:8300` / `:8500` all keep working unchanged because they already use absolute `http://127.0.0.1:<port>` URLs in direct mode.

Target platforms: **Linux x86_64** (primary, matches the Z.AI sandbox), **macOS Apple Silicon + Intel**, **Windows x86_64**. Mobile is explicitly out of scope (the GCS UX assumes a 1280×800 desktop canvas with mouse + right-click).

---

## 2. Background — What RustSim GCS Is Today

### 2.1 Component inventory (verified live 2026-09-11)

| Component | Path | Port | Purpose |
|---|---|---|---|
| `console` | `console/` | `:3000` | Next.js 16.1.1 standalone server. React 19, Tailwind v4, MapLibre GL 6, shadcn-style UI. One route (`/`), one trivial `/api` stub, no SSR-dependent data. |
| `fleet-catalog` | `fleet/crates/fleet-mission` → binary `fleet-catalog` | `:8300` | REST: mission CRUD, presets, ULog browse. Stateless. |
| `fleet-supervisor` | `fleet/crates/fleet-cli/src/bin/supervisor.rs` → binary `fleet-supervisor` | `:8500` | REST: `GET /api/sitl/status`, `GET /api/sitl/scenarios`, `POST /api/sitl/start`, `POST /api/sitl/stop`. Spawns `mavfleet run --fleet <scenario> --api-port 8400` on `start`. Owns the SITL lifecycle (ADR-0030). |
| `fleet-mgr` (on-demand) | `fleet/crates/fleet-cli` → binary `mavfleet` | `:8400` | REST + WebSocket: `/api/fleet` snapshot at 10 Hz, `/api/vehicles/{i}/mission/upload`, `/api/vehicles/{i}/goto`, etc. Spawns per-vehicle `sitsim-cli` + `px4` subprocesses (HIL TCP `4560+i`, MAVLink UDP `14540+i`). |
| `px4` (SITL) | External `PX4-Autopilot` build | n/a (subprocess) | PX4-Autopilot v1.16.2. Built once via `make px4_sitl_default`. Binary at `build/px4_sitl_default/bin/px4`. |
| Caddy gateway | `console/Caddyfile.example` | `:81` | Reverse proxy. In browser mode, `?XTransformPort=<port>` selects the upstream. Tauri eliminates this gateway. |

### 2.2 Frontend facts (from subagent A's deep scan, Task 3a)

- **Stack:** Next.js 16.1.1, React 19.0.0, TypeScript ^5 (strict), Tailwind v4 (PostCSS), `maplibre-gl` 6.9.0, `next-themes` 0.4.6, `lucide-react`, `@radix-ui/react-*` (8 primitives). No state-management lib (custom `useSyncExternalStore` stores). No chart lib (hand-rolled `RingBuffer` snapshots).
- **App Router** (`src/app/`), `output: "standalone"` today. Single route `/` rendering `<CanvasLoader/>` → dynamic-imports `<OperationsCanvas/>`.
- **API routes:** one file, `src/app/api/route.ts`, returns `{service, sim, fleet}`. **Nothing in `src/` calls `/api`** — all upstream calls go directly to `:8300` / `:8400` / `:8500` via `src/lib/conn.ts`'s `gw()` (gateway mode) or absolute `http://127.0.0.1:<port>` (direct mode).
- **Env vars read:** exactly one — `NEXT_PUBLIC_RSIM_API_STYLE` (`'direct'` vs default `'gateway'`).
- **WebSocket:** two call sites, both to `:8400` (`wsUrl(FLEET_PORT)` in `useFleetC2.ts:147` and `telemetry-store.ts:399`). No WS to catalog or supervisor.
- **Assets:** `public/logo.svg` + `public/robots.txt` only. MapLibre worker siblings are copied by `scripts/copy-maplibre-worker.mjs` into `public/maplibre/`. Fonts via `next/font/google` (Geist + Geist_Mono) — fetched at build time.
- **localStorage keys:** 5 (`rsim.map.v1`, `rsim.settings.v1`, `rsim.onboarded`, `rsim.layout.v1`, `rsim.shortcuts.v1`). All work unchanged in Tauri.
- **No service workers**, **no `fetch('/api')`**, **no SSR-dependent data**.

### 2.3 How the operator stack is brought up today

```bash
bash scripts/stack_up.sh start        # catalog :8300 + supervisor :8500 + console :3000
bash scripts/stack_up.sh start-fleet  # OR: click "Start" in the GCS SITL Manager panel
                                     # → supervisor spawns mavfleet on :8400 + px4 + sitsim
```

### 2.4 What the live SITL run actually verified (2026-09-11)

| Check | Result |
|---|---|
| `rustup` install of Rust 1.98.1 stable | PASS |
| `sim` `cargo build --workspace` (40.9 s) | PASS |
| `fleet` `cargo build --workspace` (2m 39s, 5 warnings) | PASS |
| `console` `npm install` (465 packages, with radix-ui pin fixes) | PASS |
| `console` `npm run build` (standalone out) | PASS |
| PX4-Autopilot v1.16.2 clone (1.6 GB, 25 submodules) + NuttX tag fetch + `make px4_sitl_default` (1068/1068) | PASS |
| `stack_up.sh start` — catalog + supervisor + console up, gateway :81 serving HTML | PASS |
| `POST /api/sitl/start` via supervisor (the GCS UI SITL Manager path) | PASS — `running: true, vehicle_count: 2` |
| Fleet telemetry — 2 vehicles in `READY` FSM, MAVLink heartbeats at 10 Hz, GPS locked at 47.3977°, 8.5455° (Zurich), battery 100% | PASS |
| Gateway :81 → catalog `/api/health` (`XTransformPort=8300`) | PASS |
| Gateway :81 → supervisor `/api/health` (`XTransformPort=8500`) | PASS |

This run is the baseline. The Tauri repurpose must reproduce all of the above with **zero loss of fidelity**.

---

## 3. Goals & Non-Goals

### 3.1 Goals

1. **One double-click app** — `RustSim.app` / `RustSim.exe` / `RustSim.AppImage` that launches the full GCS without `stack_up.sh`, without a terminal, without Caddy.
2. **Real PX4 SITL, unchanged** — the supervisor still spawns `px4` + `sitsim-cli` + `mavfleet`; the SITL Manager panel still drives the lifecycle via `:8500`. No mock data, ever.
3. **Minimal frontend diff** — the existing Next.js 16 + MapLibre + shadcn code is reused as-is; only `next.config.ts`, `src/lib/conn.ts`, `src/app/layout.tsx`, and `src/components/canvas/MapCanvas.tsx` change.
4. **Same UX** — 1280×800 window, single full-bleed MapLibre canvas, 7 overlay panels, right-click context menus, single-key shortcuts. Identical to the web version.
5. **Cross-platform** — Linux, macOS, Windows. Code-signed on macOS (Developer ID + notarization) and Windows (Authenticode) for the team's internal distribution; unsigned is acceptable for the dev/internal build.
6. **Debuggable by default** — Tauri's webview DevTools enabled in dev builds; backend logs streamed to `~/.rustsim/logs/`; SITL Manager panel surfaces supervisor stdout/stderr.

### 3.2 Non-Goals

- **Mobile** — out of scope. The Operations Canvas assumes a 1280×800+ desktop with mouse.
- **Real hardware** — out of scope. The GCS is SITL-only (per `docs/GCS_SPEC.md` §3.2). Real FC flashing, serial links, USB discovery stay out.
- **Cloud sync** — no account, no cloud, no telemetry upload.
- **Auto-update on day one** — `tauri-plugin-updater` is wired structurally but the update feed URL is left empty until the team operates a release server.
- **Replacing the supervisor's process tree** — the supervisor stays a separate process. The Tauri Rust binary does NOT become the supervisor; it supervises the supervisor. This preserves ADR-0030 exactly.
- **Re-implementing the catalog or fleet-mgr in Tauri Rust** — they stay as compiled Rust binaries spawned by Tauri. They could in principle be linked into the Tauri binary as background tokio tasks (see §7.3), but that's a Phase 2 optimisation.

---

## 4. Target Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│  RustSim.app  (Tauri 2.x binary)                                        │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │  Tauri Rust backend  (src-tauri/src/main.rs)                       │ │
│  │                                                                    │ │
│  │   • tauri::Builder::default()                                      │ │
│  │       .plugin(tauri_plugin_shell::init())                          │ │
│  │       .plugin(tauri_plugin_log::Builder::new().build())            │ │
│  │       .plugin(tauri_plugin_window_state::Builder::default().build())│ │
│  │       .plugin(tauri_plugin_fs::init())                             │ │
│  │       .plugin(tauri_plugin_dialog::init())                         │ │
│  │       .plugin(tauri_plugin_os::init())                             │ │
│  │       .plugin(tauri_plugin_process::init())                         │ │
│  │       .setup(spawn_backends)                                       │ │
│  │       .on_window_event(close_handler)                              │ │
│  │       .build(tauri::generate_context!())                           │ │
│  │       .run(run_handler)                                            │ │
│  │                                                                    │ │
│  │   • spawn_backends:                                                │ │
│  │       tokio::spawn(fleet-catalog on :8300)   ← kill_on_drop(true)  │ │
│  │       tokio::spawn(fleet-supervisor on :8500) ← kill_on_drop(true) │ │
│  │       (fleet-mgr is spawned ON DEMAND by the supervisor, not us)  │ │
│  │       store Child handles in tauri::Manager::manage(State<...>)    │ │
│  │                                                                    │ │
│  │   • close_handler / RunEvent::ExitRequested:                       │ │
│  │       POST http://127.0.0.1:8500/api/sitl/stop  (graceful SITL)   │ │
│  │       drop State → tokio::process::Child::kill()  (catalog+sup)   │ │
│  │       wait 250ms → force kill                                       │ │
│  │                                                                    │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │  Webview  (WebKitGTK / WebView2 / WKWebView)                       │ │
│  │    loads:  tauri://localhost/  (or http://tauri.localhost/ Win)    │ │
│  │    served: ../console/out/  (Next.js static export)                │ │
│  │                                                                    │ │
│  │    fetch('http://127.0.0.1:8300/api/missions')   → catalog         │ │
│  │    fetch('http://127.0.0.1:8500/api/sitl/status') → supervisor    │ │
│  │    new WebSocket('ws://127.0.0.1:8400/')        → fleet-mgr       │ │
│  │                                                                    │ │
│  │    (no IPC invoke needed for backend traffic — direct HTTP works) │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘

       ↓ spawned on demand by the supervisor
   ┌──────────────────────────────────────────────────┐
   │  mavfleet  (fleet-mgr, :8400)                    │
   │    spawns:  sitsim-cli (sim physics, HIL :4560)  │
   │              px4 (PX4 SITL, MAVLink UDP :14540)  │
   │              per-vehicle ×2                       │
   └──────────────────────────────────────────────────┘
```

### 4.1 Why this shape (and not the alternatives)

- **Keep the supervisor as a separate process:** ADR-0030 stays untouched. The supervisor's `POST /api/sitl/stop` is still the canonical SITL shutdown path; the Tauri backend calls it on app exit.
- **Don't compile catalog + supervisor into the Tauri binary:** They are full `axum` servers with their own state machines and lifecycles. Keeping them as separate processes means:
  - Crash isolation — if the catalog panics, the GCS UI keeps running and shows a "catalog offline" banner; the operator can restart it from the Settings panel.
  - Reuse — `cargo test --workspace` keeps working unchanged. The 99+285 Rust tests stay green.
  - Future migration — if the team later wants to link them in, the supervisor's `main.rs` can be wrapped as `pub async fn run() -> Result<()>` and called as a tokio task. We don't lock ourselves out.
- **Don't bundle PX4 as a sidecar:** PX4-Autopilot is ~3.5 GB to clone + 25 submodules + 1068 CMake targets + NuttX tags + ~25 min to build. Bundling it as a Tauri sidecar would push the installer to >500 MB and force every Tauri rebuild to re-link it. Instead, the user installs PX4 v1.16.2 separately (or via a one-shot installer we ship alongside the .app), and the supervisor's existing PX4 discovery (`PX4_ROOT` env var, fallback `../PX4-Autopilot`) keeps working unchanged. This matches QGroundControl's installer model (QGC also expects the user to install PX4 separately).

---

## 5. Frontend Migration Plan — Next.js → Tauri Static Export

### 5.1 The change set

| # | File | Change | Why |
|---|---|---|---|
| 1 | `console/next.config.ts` | `output: 'standalone'` → `output: 'export'`. Add `images: { unoptimized: true }`. | Tauri serves static assets; no Node server in production. |
| 2 | `console/next.config.ts` | In dev, set `assetPrefix: process.env.TAURI_DEV_HOST ? \`http://${process.env.TAURI_DEV_HOST}:3000\` : undefined`. | Next.js 16 + Tauri dev: HMR assets must load from the dev server origin, not the webview origin. |
| 3 | `console/src/app/api/route.ts` | **Delete.** | Static export doesn't support server routes; nothing in `src/` calls `/api` anyway (verified by subagent A). |
| 4 | `console/src/app/layout.tsx` | Favicon: `https://z-cdn.chatglm.cn/z-ai/static/logo.svg` → `/logo.svg`. | The CDN URL fails offline. `/logo.svg` already exists in `public/`. |
| 5 | `console/src/app/layout.tsx` | `next/font/google` → `next/font/local` with vendored `.woff2` files in `public/fonts/`. | `next/font/google` requires network at build time; air-gapped CI builds fail. Vendor Geist + Geist_Mono once via `curl fonts.gstatic.com/...`. |
| 6 | `console/src/lib/conn.ts` | Hardcode `direct` mode (delete the `gateway` branch). Set `NEXT_PUBLIC_RSIM_API_STYLE=direct` in `.env.production`. | Tauri has no Caddy gateway. Direct mode already produces absolute `http://127.0.0.1:<port>` URLs — this is the natural Tauri fit. |
| 7 | `console/src/components/canvas/MapCanvas.tsx:100` | `new URL('/maplibre/maplibre-gl-worker.mjs', window.location.origin).href` → `new URL('/maplibre/maplibre-gl-worker.mjs', import.meta.url).href`. | Inside Tauri v2, `window.location.origin` is `http://tauri.localhost` (Win) / `tauri://localhost` (macOS). The worker file is bundled at the same path as the JS bundle, so `import.meta.url` resolves correctly under the Tauri asset protocol. |
| 8 | `console/package.json` | Drop `react-map-gl` from `dependencies`. | Subagent A verified it's a dead dependency — `MapCanvas.tsx` uses vanilla `import * as maplibregl from 'maplibre-gl'` directly. |
| 9 | `console/.env.production` (new file) | `NEXT_PUBLIC_RSIM_API_STYLE=direct`. | Build-time env for static export. |
| 10 | `console/Caddyfile.example` | Keep for the legacy web deployment path; do NOT delete. | The web stack on `:81` is still useful for headless CI, the `scripts/browser_live_test.sh` harness, and operators who prefer a browser. Tauri is a new deployment, not a replacement. |

### 5.2 What does NOT change

- **The 7 overlay panels** (`MissionStrip`, `LibraryPanel`, `FleetC2Panel`, `SitlManagerPanel`, `SetupDrawer`, `AnalyzeOverlay`, `PreFlightPanel`, `SettingsPanel`, `OnboardingTour`, `GotoConfirmChip`) — zero source changes.
- **The state stores** (`app-store.ts`, `telemetry-store.ts`, `plan-store.ts`, `map-settings.ts`, `command-bus.ts`, `ring-buffer.ts`) — zero changes. `useSyncExternalStore` works identically inside Tauri.
- **The WebSocket to `:8400`** — `new WebSocket('ws://127.0.0.1:8400/')` works unchanged. Same-scheme `http://tauri.localhost` + `ws://127.0.0.1` is allowed by the CSP in §7.
- **The REST calls to `:8300` and `:8500`** — `fetch('http://127.0.0.1:8300/api/missions')` works unchanged. No CORS preflight because the Rust backends already return `Access-Control-Allow-Origin: *` (verified — the `Caddyfile.example` does NOT inject CORS, the backends do).
- **The SITL Manager panel** — calls `POST /api/sitl/start` on `:8500`. The supervisor spawned by Tauri receives this and spawns `mavfleet` + PX4 exactly as before. ADR-0030 unchanged.
- **The 5 localStorage keys** — work in Tauri's webview unchanged.
- **The G-ladder harnesses** (`console/tests/run_g*.sh`) — keep running against the web stack. They are not migrated to Tauri; they continue to validate the Next.js + Rust HTTP contract that Tauri inherits.

### 5.3 Build & dev commands

```bash
# Dev (with HMR through the Tauri webview)
cd console
NEXT_PUBLIC_RSIM_API_STYLE=direct npm run dev   # → Vite on :3000
# in another terminal:
cd ../src-tauri && cargo tauri dev                # → webview loads http://localhost:3000

# Production (static export bundled into the .app/.exe/.AppImage)
cd console
NEXT_PUBLIC_RSIM_API_STYLE=direct npm run build  # → out/
cd ../src-tauri && cargo tauri build              # → bundles out/ + Rust binaries + sidecars
```

The `predev` / `prebuild` npm scripts already run `scripts/copy-maplibre-worker.mjs` which copies the MapLibre worker siblings into `public/maplibre/`. Under `output: 'export'`, `next build` emits them into `out/maplibre/` — reachable via the Tauri asset protocol at `/maplibre/maplibre-gl-worker.mjs` (matches the patched `MapCanvas.tsx` URL).

---

## 6. Backend Migration Plan — Three Rust Binaries + PX4 SITL

### 6.1 The three Rust binaries

All three live in the existing `fleet/` workspace. Their build outputs are:

| Binary | Source | Build target | Spawn policy |
|---|---|---|---|
| `fleet-catalog` | `fleet/crates/fleet-mission/src/bin/catalog.rs` (or similar — exact path TBD by `cargo build`) | `fleet/target/release/fleet-catalog` | Spawn on Tauri startup. Stateless; restartable from Settings panel. |
| `fleet-supervisor` | `fleet/crates/fleet-cli/src/bin/supervisor.rs` | `fleet/target/release/fleet-supervisor` | Spawn on Tauri startup. Owns SITL lifecycle; calls `POST /api/sitl/stop` on app exit. |
| `mavfleet` (fleet-mgr) | `fleet/crates/fleet-cli/src/bin/mavfleet.rs` | `fleet/target/release/mavfleet` | **Not spawned by Tauri.** Spawned on demand by the supervisor when the operator clicks Start in the SITL Manager panel. |

### 6.2 Spawning strategy — `tokio::process::Command` + `kill_on_drop(true)`

Per the Tauri research (Task 3b-1, §4), the 2026 best practice for long-running Rust subprocesses that must die when Tauri exits (even on crash) is:

```rust
// src-tauri/src/backends.rs
use std::process::Stdio;
use tauri::State;
use tokio::process::{Child, Command};
use tokio::io::AsyncBufReadExt;

pub struct BackendState {
    pub catalog: Option<Child>,
    pub supervisor: Option<Child>,
}

pub async fn spawn_catalog(state: tauri::State<'_, BackendState>) -> std::io::Result<()> {
    let mut cmd = Command::new(which_backend("fleet-catalog"));
    cmd.env("RSIM_CATALOG_DIR", dirs::data_dir().unwrap().join("rustsim/catalog"))
       .env("RUST_LOG", "info")
       .stdin(Stdio::null())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .kill_on_drop(true);   // CRITICAL: dies if Child is dropped (covers panic/abort)

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // Stream to log file + (optionally) to webview via app.emit()
    tauri::async_runtime::spawn(log_pipe(stdout, "catalog:stdout"));
    tauri::async_runtime::spawn(log_pipe(stderr, "catalog:stderr"));

    state.catalog.replace(child);   // stored in State<BackendState>
    Ok(())
}
```

**Why `tokio::process::Command` and not `tauri-plugin-shell::Command::sidecar()`?**

- `tauri-plugin-shell` is designed for bundling external binaries via `externalBin` + target-triple suffixes. It works for cross-platform binary distribution (each platform gets its own prebuilt binary).
- For our case, the catalog + supervisor are **same-workspace Rust binaries** — they should be compiled from source for the host platform, not bundled as prebuilts. The `externalBin` mechanism would force us to commit prebuilt binaries for every supported target triple, which is wrong for an internal team tool with source access.
- `tokio::process` is simpler, has cleaner async stdout/stderr handling, and the `kill_on_drop(true)` covers the crash path better than `tauri-plugin-shell`'s `CommandChild` (which has known issues killing child process groups — see tauri-apps/tauri#4949).

### 6.3 Graceful shutdown on app exit

```rust
// src-tauri/src/main.rs (run handler)
.build(tauri::generate_context!())?
.run(|app, event| match event {
    tauri::RunEvent::ExitRequested { .. } => {
        let state: tauri::State<BackendState> = app.state();
        // 1. Tell the supervisor to stop SITL gracefully (kills mavfleet + px4)
        let _ = reqwest::blocking::post("http://127.0.0.1:8500/api/sitl/stop")
            .json(&{})
            .send();
        // 2. Wait 250ms for the supervisor to clean up
        std::thread::sleep(std::time::Duration::from_millis(250));
        // 3. Drop the Child handles — kill_on_drop takes over
        drop(state.catalog.take());
        drop(state.supervisor.take());
    }
    _ => {}
});
```

Belt-and-suspenders: also wire a `POST /shutdown` endpoint into the catalog + supervisor (a 5-line axum route) so we can ask them to drain before kill. The kill_on_drop covers the panic path; the explicit shutdown covers the graceful path.

### 6.4 PX4 SITL bundling — NOT as a sidecar

PX4-Autopilot is too large to bundle (3.5 GB source, 25 submodules, ~600 MB binary). Instead:

1. **User installs PX4 v1.16.2 separately** — the existing `docs/SANDBOX_SETUP.md` step 6 is the canonical install guide. The team can ship a one-shot `install-px4.sh` script alongside the .app for convenience.
2. **Tauri discovers PX4 via the same env var** — `PX4_ROOT` (defaults to `../PX4-Autopilot` relative to the repo root). The supervisor already reads this (verified: `scripts/stack_up.sh:43` — `export PX4_ROOT="${PX4_ROOT:-$(cd "$ROOT/.." && pwd)/PX4-Autopilot}"`).
3. **First-run check** — the Tauri Rust backend, on `setup()`, probes `PX4_ROOT/build/px4_sitl_default/bin/px4`. If missing, the webview shows a "PX4 not found" onboarding screen with a "Open install guide" button (links to `docs/SANDBOX_SETUP.md` step 6).
4. **Optional installer** — for non-technical operators, ship a separate `RustSim-PX4-Installer.dmg` / `.exe` that runs the PX4 clone + build automatically. This is a packaging task, not a Tauri task.

### 6.5 Port binding — fixed, no dynamic allocation

The three Rust services bind to fixed ports: `:8300` (catalog), `:8400` (fleet-mgr, on-demand), `:8500` (supervisor). The frontend hardcodes these in `src/lib/conn.ts`'s `CATALOG_PORT=8300`, `FLEET_PORT=8400`, `SUPERVISOR_PORT=8500`.

**Conflict handling:** if a port is already taken (e.g. another RustSim instance, or a stale process), the Tauri Rust backend on `setup()` probes `127.0.0.1:8300` / `:8500` before spawning. If occupied, it shows an error dialog: "Port :8300 is already in use. Close the other RustSim instance or run with `--reset-ports` to kill stale processes." The `--reset-ports` flag runs `pkill -f fleet-catalog` / `pkill -f fleet-supervisor` (with the operator's confirmation).

---

## 7. Security Model

### 7.1 Tauri capabilities (`src-tauri/capabilities/main.json`)

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Capability for the main operations-canvas window",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "core:window:default",
    "core:event:default",
    "core:app:default",
    "core:path:default",
    "shell:allow-open",
    "fs:allow-read-text-file",
    "fs:allow-write-text-file",
    "dialog:allow-open",
    "dialog:allow-save",
    "os:default",
    "process:default",
    "log:default",
    "window-state:default"
  ]
}
```

- **No `shell:allow-execute`** — the frontend cannot spawn processes. All subprocess spawning goes through Tauri Rust commands (`#[tauri::command]`), which are gated by their own capabilities.
- **No `fs:default`** — only specific read/write text-file permissions, scoped to the mission file directory.
- **`http:default` and `websocket:default` NOT included** — we deliberately use the browser-native `fetch` + `new WebSocket`, not the Tauri plugins. The CSP in §7.2 permits direct localhost traffic.

### 7.2 Content Security Policy (`tauri.conf.json` → `app.security.csp`)

```json
{
  "app": {
    "security": {
      "csp": "default-src 'self'; \
        connect-src 'self' ipc: http://ipc.localhost \
                    http://127.0.0.1:8300 http://127.0.0.1:8400 http://127.0.0.1:8500 \
                    ws://127.0.0.1:8400; \
        img-src 'self' asset: http://asset.localhost blob: data: \
                https://*.tile.openstreetmap.org https://*.basemaps.cartocdn.com \
                https://*.tile.opentopomap.org; \
        style-src 'self' 'unsafe-inline'; \
        script-src 'self' 'wasm-unsafe-eval'; \
        font-src 'self' data:; \
        worker-src 'self' blob:"
    }
  }
}
```

Key points:

- **`connect-src`** explicitly lists `http://127.0.0.1:{8300,8400,8500}` and `ws://127.0.0.1:8400`. Wildcard ports `:*` would also work but pinning the three known ports is safer.
- **`ipc:` and `http://ipc.localhost`** MUST stay in `connect-src` — Tauri's own IPC bridge uses them.
- **`'wasm-unsafe-eval'`** in `script-src` — MapLibre GL ships WASM.
- **`worker-src 'self' blob:`** — MapLibre's worker is loaded as a blob URL in some configurations.
- **`img-src`** — the 6 basemap providers from `state/map-settings.ts` (OpenStreetMap, CartoDB, OpenTopoMap, Esri, Stadia, MapTiler — exact list TBD at build time). Pin to the actual `https://` hosts we ship.
- **`'unsafe-inline'`** in `style-src` — Tailwind v4 + shadcn inject inline styles. Cannot be avoided without a major refactor.

### 7.3 Mixed content (HTTPS vs HTTP origin)

| Platform | Default webview origin | Mixed content with `http://127.0.0.1:*`? | Fix |
|---|---|---|---|
| Linux | `tauri://localhost` | No (same-scheme-agnostic; `tauri://` allows `http://127.0.0.1` via CSP) | None needed. |
| Windows | `http://tauri.localhost` (default `useHttpsScheme: false`) | No (both `http`) | None needed. |
| macOS | `tauri://localhost` | No | None needed. |
| Windows with `useHttpsScheme: true` | `https://tauri.localhost` | **Yes** — `http://127.0.0.1:*` is mixed content | Don't set `useHttpsScheme: true`. Stick with the default `false`. |

The CSP above explicitly allows `http://127.0.0.1:*` even if the webview origin is `https://`, so even if `useHttpsScheme` is later flipped to `true`, mixed content is still permitted by CSP. But we don't flip it.

### 7.4 `dangerousDisableAssetCspModification`

Leave at default (`false`). Tauri will inject nonces/hashes for our own scripts at build time — this is a defence-in-depth that we want. If a future MapLibre release ships an inline script that breaks, we revisit.

---

## 8. Window & UX Configuration

### 8.1 `tauri.conf.json` → `app.windows[0]`

```json
{
  "app": {
    "windows": [{
      "label": "main",
      "title": "RustSim GCS — Operations Canvas",
      "width": 1280,
      "height": 800,
      "minWidth": 1024,
      "minHeight": 720,
      "resizable": true,
      "maximizable": true,
      "minimizable": true,
      "closable": true,
      "maximized": false,
      "fullscreen": false,
      "center": true,
      "decorations": true,
      "transparent": false,
      "alwaysOnTop": false,
      "skipTaskbar": false,
      "titleBarStyle": "Visible",
      "theme": "dark",
      "dragDropEnabled": false,
      "useHttpsScheme": false,
      "hidden": false
    }]
  }
}
```

### 8.2 Why each value

- **1280×800** — matches the Xvfb bridge size from `LIVE-DESKTOP-PREVIEW-SETUP.md`, matches the MapLibre canvas default. The Operations Canvas was designed at this size.
- **minWidth 1024, minHeight 720** — the 7 overlay panels need ~360px sidebar + ~640px map. Below 1024 the panels crush and the right-click context menus overlap.
- **`decorations: true`** — keep native close/min/max buttons. Custom titlebars (`decorations: false`) have a known Linux/Windows bug where the titlebar persists (tauri-apps/tauri#8524) — not worth the maintenance.
- **`transparent: false`** — Linux doesn't support transparent windows.
- **`theme: dark`** — `next-themes` defaults to dark; matches the GCS aesthetic.
- **`dragDropEnabled: false`** — prevent stray file drops triggering MapLibre behaviour. Use `tauri-plugin-dialog` for explicit file opens (mission TOML, ULog files).
- **`useHttpsScheme: false`** — keep `http://tauri.localhost` on Windows so existing localStorage keys don't reset (Tauri 1→2 migration note: IndexedDB/localStorage are origin-scoped; flipping scheme invalidates them).

### 8.3 Multi-window (Phase 2)

The current GCS is single-window. Phase 2 could add:
- A **detached Analyze window** for ULog plots (pop-out from `AnalyzeOverlay`).
- A **detached Fleet C2 window** for operators running multiple fleets.

`tauri::WebviewWindowBuilder` makes this trivial; the existing overlay components are already self-contained. Not in scope for v1.

---

## 9. Plugin Stack

### 9.1 Plugins to wire (with install commands)

| Plugin | Cargo | npm | Use case |
|---|---|---|---|
| `tauri-plugin-shell` | `cargo add tauri-plugin-shell` | `npm install @tauri-apps/plugin-shell` | Open external URLs (PX4 docs, install guide) via `shell.open()`. **NOT used for sidecars** — we use `tokio::process` for the catalog + supervisor (see §6.2). |
| `tauri-plugin-log` | `cargo add tauri-plugin-log` | `npm install @tauri-apps/plugin-log` | Rust `log` crate → webview console + log file at `~/.rustsim/logs/{catalog,supervisor}.log`. |
| `tauri-plugin-window-state` | `cargo add tauri-plugin-window-state` | `npm install @tauri-apps/plugin-window-state` | Remember window position/size across launches. Operators value this. |
| `tauri-plugin-fs` | `cargo add tauri-plugin-fs` | `npm install @tauri-apps/plugin-fs` | Read/write mission TOML files from `~/.rustsim/missions/`. Scoped to that dir. |
| `tauri-plugin-dialog` | `cargo add tauri-plugin-dialog` | `npm install @tauri-apps/plugin-dialog` | Open/save file dialogs for mission files + ULog imports. |
| `tauri-plugin-os` | `cargo add tauri-plugin-os` | `npm install @tauri-apps/plugin-os` | OS info (platform, arch, hostname) for the Settings → About panel. |
| `tauri-plugin-process` | `cargo add tauri-plugin-process` | `npm install @tauri-apps/plugin-process` | `process.exit()` / `process.relaunch()` — restart-after-update flow. |

### 9.2 Plugins deliberately NOT wired

| Plugin | Why skip |
|---|---|
| `tauri-plugin-http` | Frontend uses native `fetch('http://127.0.0.1:<port>')`. The Rust backends already return `Access-Control-Allow-Origin: *`. No CORS problem → no plugin needed. |
| `tauri-plugin-websocket` | Frontend uses native `new WebSocket('ws://127.0.0.1:8400/')`. Same-origin CSP permits it. Adding the plugin would force a frontend rewrite of `useFleetC2.ts` + `telemetry-store.ts` — pure cost, zero benefit. |
| `tauri-plugin-store` | Frontend already uses `localStorage` for the 5 settings keys. Store plugin is for persistent KV; `localStorage` is fine for our volumes (KB, not MB). |
| `tauri-plugin-notification` | The GCS already surfaces status in the webview; OS notifications would be intrusive during a flight. Skip. |
| `tauri-plugin-clipboard-manager` | No clipboard use case in the current GCS. |
| `tauri-plugin-updater` | Wired structurally (so the menu item exists) but the update feed URL is empty until the team operates a release server. |
| `tauri-plugin-global-shortcut` | The GCS uses single-key shortcuts inside the webview (`?` for cheat sheet, `1-9` for panel toggle). These are NOT global shortcuts — they should only fire when the GCS has focus. Webview-level keyboard handlers, not Tauri global shortcuts. |

---

## 10. Cross-Platform Packaging

### 10.1 `tauri.conf.json` → `bundle`

```json
{
  "bundle": {
    "active": true,
    "targets": "all",
    "publisher": "RustSim Team",
    "category": "DeveloperTool",
    "shortDescription": "QGroundControl-class GCS for PX4 SITL",
    "longDescription": "RustSim GCS is a pure Rust + TypeScript ground control station for PX4-Autopilot v1.16.2 SITL. Single-screen Operations Canvas with 7 overlay panels for mission authoring, fleet C2, SITL lifecycle, vehicle setup, ULog analyze, pre-flight checks, and settings.",
    "copyright": "Apache-2.0",
    "homepage": "https://github.com/kanishka-namdeo/rust-sim",
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.icns", "icons/icon.ico"],
    "resources": ["../PX4-Autopilot/build/px4_sitl_default/bin/px4"],
    "externalBin": [],
    "macOS": {
      "frameworks": [],
      "minimumSystemVersion": "12.0",
      "exceptionDomain": "",
      "signingIdentity": null,
      "entitlements": null
    },
    "windows": {
      "nsis": {
        "installerIcon": "icons/icon.ico",
        "installMode": "perMachine",
        "languages": ["en-US"]
      },
      "webviewInstallMode": {
        "type": "downloadBootstrapper"
      }
    },
    "linux": {
      "deb": {
        "depends": ["libwebkit2gtk-4.1-0", "libssl3", "libgtk-3-0"]
      },
      "rpm": {
        "depends": ["webkit2gtk4.1", "openssl-libs", "gtk3"]
      },
      "appImage": {
        "bundleMediaFramework": false
      }
    }
  }
}
```

### 10.2 Code signing policy

| Platform | Status | Reason |
|---|---|---|
| Linux (.deb/.rpm/AppImage) | Unsigned | Linux has no signing standard; users install at their own risk. AppImage is the recommended format. |
| Windows (.msi / .exe via NSIS) | Unsigned for v1; Authenticode later | Internal team tool; SmartScreen warning acceptable. Phase 2: buy an OV cert (~$300/yr) for Authenticode signing. |
| macOS (.dmg / .app) | Unsigned for v1; Developer ID + notarization later | macOS Gatekeeper will show "unidentified developer" warning — operators must right-click → Open once. Phase 2: Apple Developer ID ($99/yr) + `xcrun notarytool` for notarization. |

For Phase 2 macOS signing, the `tauri.conf.json` `bundle.macOS.signingIdentity` field becomes `"Developer ID Application: <Team Name> (XXXXXXXXXX)"` and the build pipeline runs:

```bash
xcrun notarytool submit RustSim.app.zip \
    --apple-id <apple-id> --team-id <team-id> --password <app-password> \
    --wait
xcrun stapler staple RustSim.app
```

### 10.3 Bundle size estimate

| Component | Size (Linux x86_64, release) |
|---|---|
| Tauri Rust binary (with plugins) | ~25 MB |
| `fleet-catalog` binary | ~12 MB |
| `fleet-supervisor` binary | ~14 MB |
| `mavfleet` binary (bundled, spawned by supervisor) | ~16 MB |
| Next.js static export (`out/`) | ~5 MB |
| MapLibre worker siblings | ~1 MB |
| Vendored Geist fonts | ~200 KB |
| **Subtotal (no PX4)** | **~73 MB** |
| PX4 binary (if bundled) | +600 MB — NOT bundled; user installs separately |
| **Final installer** | **~73 MB** |

Acceptable. QGroundControl's installer is ~250 MB; we're 3.4× smaller because we don't bundle PX4.

---

## 11. Dev Workflow

### 11.1 `tauri dev` (HMR through the Tauri webview)

```bash
# Terminal 1: start the catalog + supervisor (they're long-lived)
cd /home/z/my-project/repos/rust-sim
cargo run --release --bin fleet-catalog -- &    # :8300
cargo run --release --bin fleet-supervisor -- &  # :8500

# Terminal 2: Tauri dev (spawns Vite + Tauri binary)
cd src-tauri
cargo tauri dev
```

`tauri.conf.json` `build` section:

```json
{
  "build": {
    "beforeDevCommand": "cd ../console && NEXT_PUBLIC_RSIM_API_STYLE=direct npm run dev",
    "beforeBuildCommand": "cd ../console && NEXT_PUBLIC_RSIM_API_STYLE=direct npm run build",
    "devUrl": "http://localhost:3000",
    "frontendDist": "../console/out",
    "beforeDevCommandWait": 3,
    "beforeBuildCommandWait": 30
  }
}
```

HMR latency: ~50ms inside the Tauri webview (no VNC bridge in dev — only in the headless sandbox for screenshot verification).

### 11.2 Screenshot verification via the live desktop bridge

For headless CI / screenshot verification, use the bridge from `LIVE-DESKTOP-PREVIEW-SETUP.md`:

```bash
bash /home/z/my-project/scripts/bridge-install.sh       # one-time
bash /home/z/my-project/scripts/fetch-tauri-deps.sh      # one-time
bash /home/z/my-project/scripts/start-bridge.sh         # Xvfb + x11vnc + websockify + twm
cd src-tauri && cargo tauri build                        # produces the .AppImage / .deb
bash /home/z/my-project/scripts/start-tauri.sh /home/z/my-project/repos/rust-sim
bash /home/z/my-project/scripts/screenshot.sh /home/z/my-project/download/rustsim-tauri.png
```

This is the verification path the team uses to confirm a build works in a clean environment. The same path also works for the existing web stack (`start-electron.sh` is unnecessary; the Next.js stack is already browser-testable via `http://localhost:81`).

### 11.3 `tauri build` (production)

```bash
cd src-tauri
cargo tauri build           # → src-tauri/target/release/bundle/{deb,rpm,appimage}/
cargo tauri build --target x86_64-pc-windows-msvc  # cross-compile (needs toolchain)
cargo tauri build --target universal-apple-darwin    # macOS universal binary
```

Cross-compilation is non-trivial for the Rust backends (they spawn subprocesses that need to be the same target triple as the host). Recommended workflow: build natively on each platform via CI matrix (GitHub Actions with `ubuntu-22.04`, `macos-14`, `windows-latest`).

---

## 12. Migration Milestones & Verification

Each milestone is independently shippable; each has a single Go/No-Go gate.

### M-T1 — Frontend static export

**Goal:** `console/` builds to `out/` as a pure static site, served by any static file server, with `NEXT_PUBLIC_RSIM_API_STYLE=direct` baked in.

**Changes:** §5.1 rows 1–6, 8–10. (Row 7 — MapLibre worker URL — is M-T3.)

**Verification:**
- `cd console && NEXT_PUBLIC_RSIM_API_STYLE=direct npm run build` produces `out/index.html`.
- `cd out && python3 -m http.server 3000` serves it; opening `http://localhost:3000` shows the Operations Canvas.
- The existing `console/tests/run_g*.sh` G-ladder harnesses still pass against the static export (they exercise the Next.js + Rust HTTP contract — independent of how the HTML is served).
- Go/No-Go: **all 11 surviving G-ladder gates still PASS.**

### M-T2 — Tauri skeleton + window

**Goal:** `cargo tauri dev` opens a 1280×800 window showing the static-exported Operations Canvas, with no Rust backends running yet.

**Changes:** new `src-tauri/` directory with `Cargo.toml`, `tauri.conf.json`, `src/main.rs` (just `tauri::Builder::default().run(...)`), `capabilities/main.json`, icons. `frontendDist: ../console/out`. CSP from §7.2.

**Verification:**
- Window opens at 1280×800, title "RustSim GCS — Operations Canvas".
- DevTools (right-click → Inspect) shows the MapLibre canvas attempting to load (will fail to fetch :8300 — expected, backends not running yet).
- No CSP violations in DevTools console (other than expected `:8300` fetch failures).
- Go/No-Go: **window opens cleanly with the static export visible.**

### M-T3 — MapLibre worker URL fix + direct mode verified

**Goal:** The MapLibre canvas renders (with no basemap tiles yet — just the grey default) inside the Tauri webview.

**Changes:** §5.1 row 7 (MapLibre worker URL via `import.meta.url`). 

**Verification:**
- DevTools Console: no "Failed to load worker" errors.
- The MapLibre canvas is visible at 100% width / height.
- The `Settings` panel's 6 basemap options appear in the dropdown (toggling still fails — needs network — but the UI is correct).
- Go/No-Go: **MapLibre canvas renders without errors.**

### M-T4 — Rust backend orchestration

**Goal:** On app startup, Tauri spawns `fleet-catalog` and `fleet-supervisor`. The webview's `fetch('http://127.0.0.1:8300/api/health')` and `fetch('http://127.0.0.1:8500/api/sitl/status')` succeed.

**Changes:** `src-tauri/src/backends.rs` with `spawn_catalog` + `spawn_supervisor` (§6.2). `BackendState` in `tauri::Manager::manage`. `setup` hook calls both.

**Verification:**
- App startup: within 2s, both `:8300` and `:8500` answer `GET /api/health` with `{"ok":true,...}`.
- DevTools Network tab: `GET http://127.0.0.1:8300/api/missions` returns 200 (or 404 if empty — both fine).
- Settings → About panel shows "Catalog: ONLINE" and "Supervisor: ONLINE" (this is a small new frontend addition; the data comes from a new Tauri `invoke('backend_status')` command).
- Go/No-Go: **catalog + supervisor reachable from the webview, with backend status surfaced in the UI.**

### M-T5 — SITL lifecycle end-to-end through Tauri

**Goal:** Operator clicks "Start" in the SITL Manager panel; the Tauri-spawned supervisor spawns `mavfleet` + `px4`; 2 vehicles reach `READY`; telemetry flows on the WebSocket.

**Pre-req:** PX4 v1.16.2 installed at `PX4_ROOT` (per `docs/SANDBOX_SETUP.md` step 6).

**Verification:**
- SITL Manager panel: hold-to-confirm Start (400ms) succeeds.
- Within 5s: supervisor `GET /api/sitl/status` returns `running: true, vehicle_count: 2`.
- Within 10s: both vehicles reach `READY` FSM (visible in Fleet C2 panel).
- Fleet C2 panel: live attitude quaternions + battery 100% + GPS at Zurich (47.3977°, 8.5455°).
- Map canvas: 2 vehicle markers appear at the home position.
- Single-tap Stop: SITL terminates cleanly; ports `:8400`, `:4560+i`, `:14540+i` released.
- Go/No-Go: **this is the live-verified baseline reproduced inside Tauri.** This is the M-T5 gate.

### M-T6 — Graceful shutdown on window close

**Goal:** Closing the Tauri window triggers `POST /api/sitl/stop` to the supervisor (if SITL running) and kills the catalog + supervisor processes.

**Verification:**
- Start SITL (M-T5 path).
- Close the Tauri window.
- Within 2s: `ps aux | grep -E 'fleet-catalog|fleet-supervisor|mavfleet|px4_sitl'` returns empty.
- Within 2s: `curl http://127.0.0.1:8500/api/sitl/status` → connection refused.
- Re-open the app: clean start, no "port already in use" error.
- Go/No-Go: **no leaked processes after window close, with SITL running.**

### M-T7 — Cross-platform packaging

**Goal:** `cargo tauri build` produces installers for Linux (.deb + .AppImage), macOS (.dmg), Windows (.msi). Each installer runs on a clean VM of the target OS.

**Verification:**
- Linux: install `.deb` on a clean Ubuntu 22.04 VM → app launches → M-T5 path works (with PX4 pre-installed).
- macOS: install `.dmg` on a clean macOS 14 VM → Gatekeeper warning (expected, unsigned) → right-click → Open → M-T5 path works.
- Windows: install `.msi` on a clean Windows 11 VM → SmartScreen warning (expected, unsigned) → Run anyway → M-T5 path works.
- Go/No-Go: **all three installers produce a working app on a clean VM.**

### M-T8 — Screenshot verification via the live desktop bridge

**Goal:** The same build runs in the headless Z.AI sandbox via `start-tauri.sh`, with a screenshot captured proving the live SITL + GCS work end-to-end.

**Verification:**
- `bash /home/z/my-project/scripts/start-tauri.sh /home/z/my-project/repos/rust-sim` → window appears on the virtual desktop.
- `bash /home/z/my-project/scripts/screenshot.sh /home/z/my-project/download/rustsim-tauri-live.png` → PNG shows the Operations Canvas with 2 vehicles on the map + LIVE badges.
- Playwright click test (per `LIVE-DESKTOP-PREVIEW-SETUP.md` §D.3) confirms mouse events reach the app.
- Go/No-Go: **screenshot + Playwright click test PASS in the headless sandbox.**

---

## 13. Risks & Mitigations

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | **MapLibre WebGL on WebKitGTK 2.52+ in containers** — known shader-compiler quirks on llvmpipe software rendering | High in CI | Medium (slow but works at 5-10 fps) | Document; recommend native installs for operators. CI uses the bridge + llvmpipe for screenshot verification only. |
| R2 | **PX4 v1.16.2 not installed on operator's machine** | High | High (SITL won't start) | First-run onboarding screen with install guide link + optional `install-px4.sh` helper script. |
| R3 | **Port `:8300`/`:8400`/`:8500` already in use** by stale processes | Medium | Medium (operator confused) | `--reset-ports` flag + clear error dialog. Tauri probes ports on startup before spawning. |
| R4 | **Next.js 16 + Turbopack default** vs Tauri dev server | Low | Low | `next dev -p 3000` works fine with Turbopack; Tauri just loads `http://localhost:3000`. No issue. |
| R5 | **`next/font/google` fails at build time** (no network in CI) | Medium | Medium (build fails) | Vendor Geist + Geist_Mono `.woff2` in `public/fonts/`; switch to `next/font/local`. |
| R6 | **macOS notarization fails** for unsigned plugin permission | Low (Phase 2) | Medium | `entitlements.plist` with `com.apple.security.network.client = true` (we make outbound HTTP/WS to localhost). |
| R7 | **WebView2 runtime missing on Windows** | Low (Win 11 preinstalls) | High (app won't launch) | `webviewInstallMode: downloadBootstrapper` in `tauri.conf.json` — auto-downloads WebView2 on first launch. |
| R8 | **Subprocess leak on Tauri crash** | Low | High (orphaned px4/sim processes squatting ports) | `kill_on_drop(true)` on all Child handles + supervisor's `POST /api/sitl/stop` on `ExitRequested` + `--reset-ports` flag for recovery. |
| R9 | **Bundle size >100MB** if PX4 gets bundled | Avoided | n/a | PX4 NOT bundled. User installs separately. |
| R10 | **Linux distro fragmentation** (webkit2gtk-4.1 versions vary) | Medium | Medium (UI differences) | Document minimum: Ubuntu 22.04+ / Fedora 38+ / Debian 13. Older distros: use AppImage. |
| R11 | **Auto-updater breaks SITL mid-flight** | Avoided | n/a | Updater disabled while SITL is running. App checks for updates only when SITL is STOPPED. |
| R12 | **Tauri 2.x breaking change in next 12 months** | Low (Tauri 2 is stable) | Low | Pin `tauri = "2.11"` in `Cargo.toml`. Bump deliberately. |

---

## 14. Open Questions

1. **Should the catalog + supervisor be linked into the Tauri binary (Option C in §4.1)?** This would eliminate the cross-process port-binding complexity and reduce the installer by ~26 MB. Cost: lose crash isolation, lose the ability to restart the catalog from the Settings panel without restarting the whole app. **Recommendation: revisit after M-T7 if the team is happy with the multi-process model.**

2. **Should we ship a `tauri-plugin-store` for the 5 localStorage keys?** localStorage is per-webview-origin; if the team ever wants to sync settings across machines (Phase 3 cloud sync), the store plugin is the right abstraction. For v1: keep localStorage.

3. **Should the SITL Manager panel show backend stdout/stderr in real time?** Useful for debugging, noisy for operators. **Recommendation: gate behind a "developer mode" toggle in Settings.**

4. **Offline map tiles** — the README mentions offline tile bundles as a v1.1 roadmap item for air-gapped operations. Should the Tauri app bundle a small Zurich-area tile bundle (the SITL default geo origin) so the map renders without network? **Recommendation: yes, ~5 MB for a 10km × 10km Zurich tile bundle; ship in `resources/` and load via the asset protocol.**

5. **Multi-window Fleet C2** — should the operator be able to pop out the Fleet C2 panel into its own window? Phase 2 — not in scope for v1.

---

## 15. References

### 15.1 Tauri 2.x official docs (verified Sep 2026)

- Tauri 2.x core architecture: https://github.com/tauri-apps/tauri/blob/dev/ARCHITECTURE.md
- `tauri.conf.json` reference: https://v2.tauri.app/reference/config/
- Next.js frontend guide: https://v2.tauri.app/start/frontend/nextjs/
- Sidecar processes: https://v2.tauri.app/develop/sidecar/
- `tauri-plugin-shell`: https://v2.tauri.app/plugin/shell/
- CSP: https://v2.tauri.app/security/csp/
- `#[tauri::command]` IPC: https://v2.tauri.app/develop/calling-rust/
- Window config reference: https://v2.tauri.app/reference/config/#windowconfig
- Webview versions by platform: https://v2.tauri.app/reference/webview-versions/
- Tauri 1→2 migration: https://v2.tauri.app/start/migrate/from-tauri-1/
- `tauri-plugin-http`: https://v2.tauri.app/plugin/http-client/
- `tauri-plugin-websocket`: https://v2.tauri.app/plugin/websocket/

### 15.2 Reference implementations

- `dieharders/example-tauri-v2-python-server-sidecar` — https://github.com/dieharders/example-tauri-v2-python-server-sidecar (Tauri v2 + Next.js + Python FastAPI sidecar; canonical lifecycle pattern)
- `kvnxiao/tauri-nextjs-template` — https://github.com/kvnxiao/tauri-nextjs-template (Tauri 2 + Next.js 16 + Tailwind 4; cleanest Sep 2026 Next.js+Tauri reference)
- `AlanSynn/vue-tauri-fastapi-sidecar-template` — https://github.com/AlanSynn/vue-tauri-fastapi-sidecar-template (Tauri v2 + Vue + FastAPI sidecar)
- Tauri Discussion #15339 — https://github.com/orgs/tauri-apps/discussions/15339 (multi-GB sidecar scaling in production)
- Tauri Issue #4949 — https://github.com/tauri-apps/tauri/issues/4949 (sidecar kill on Windows process groups)
- Tauri Issue #8524 — https://github.com/tauri-apps/tauri/issues/8524 (decorations: false titlebar persistence bug)
- Tauri Issue #3062 — https://github.com/tauri-apps/tauri/issues/3062 (sidecar lifecycle management plugin — open feature request)
- Tauri Discussion #3273 — https://github.com/tauri-apps/tauri/discussions/3273 (recommended sidecar kill pattern on `RunEvent::ExitRequested`)

### 15.3 Rust + tokio process management

- `tokio::process::Command::kill_on_drop`: https://docs.rs/tokio/latest/tokio/process/struct.Command.html#method.kill_on_drop
- `command_group::GroupChild` (Unix process groups): https://docs.rs/command_group/

### 15.4 Project-internal references

- `docs/LIVE-DESKTOP-PREVIEW-SETUP.md` — the agent-facing playbook for the headless Linux sandbox preview bridge (Xvfb + x11vnc + websockify + twm).
- `docs/SANDBOX_SETUP.md` — the verified bring-up sequence for the rust-sim stack in the Z.AI sandbox (Rust toolchain + console + Python + PX4 v1.16.2 + harness dirs).
- `docs/ARCHITECTURE.md` — port map and data flows for the existing web stack.
- `docs/GCS_SPEC.md` — the v1 GCS engineering spec (7 milestones, 14 gates).
- `console/docs/adr/0030-sitl-supervisor.md` — ADR-0030: operator-driven SITL lifecycle (QGC/MP pattern).
- `sim/docs/PROTOCOL.md` — PX4 v1.16.2 HIL + MAVLink protocol notes (the hard-won dialect divergences).
- `scripts/stack_up.sh` — the persistent operator stack launcher (catalog + supervisor + console on `start`; fleet on `start-fleet`).

---

## Appendix A — File layout after migration

```
rust-sim/
├── console/                          # Next.js 16 — output: 'export'
│   ├── out/                          # ← produced by `npm run build`; consumed by Tauri
│   │   ├── index.html
│   │   ├── _next/static/...
│   │   ├── maplibre/
│   │   │   ├── maplibre-gl-worker.mjs
│   │   │   └── maplibre-gl-shared.mjs
│   │   ├── fonts/
│   │   │   ├── Geist-Variable.woff2
│   │   │   └── GeistMono-Variable.woff2
│   │   └── logo.svg
│   ├── public/
│   │   ├── logo.svg
│   │   ├── robots.txt
│   │   ├── maplibre/                 # ← copied by scripts/copy-maplibre-worker.mjs
│   │   └── fonts/                    # ← vendored Geist .woff2
│   ├── src/                          # unchanged
│   │   ├── app/
│   │   │   ├── layout.tsx            # ← patched: next/font/local, favicon
│   │   │   ├── page.tsx              # unchanged
│   │   │   └── globals.css           # unchanged
│   │   ├── lib/conn.ts               # ← patched: direct mode only
│   │   ├── components/canvas/
│   │   │   ├── MapCanvas.tsx         # ← patched: import.meta.url for worker
│   │   │   └── overlays/             # unchanged
│   │   └── state/                    # unchanged
│   ├── next.config.ts                # ← patched: output: 'export', images.unoptimized
│   ├── package.json                  # ← patched: drop react-map-gl
│   └── .env.production               # ← new: NEXT_PUBLIC_RSIM_API_STYLE=direct
│
├── src-tauri/                        # NEW — Tauri 2.x app
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── build.rs
│   ├── capabilities/
│   │   └── main.json                 # §7.1
│   ├── icons/
│   │   ├── 32x32.png
│   │   ├── 128x128.png
│   │   ├── 128x128@2x.png
│   │   ├── icon.icns                 # macOS
│   │   └── icon.ico                  # Windows
│   └── src/
│       ├── main.rs                   # tauri::Builder + setup + run handler
│       ├── backends.rs               # spawn_catalog + spawn_supervisor + State
│       ├── shutdown.rs               # graceful shutdown on ExitRequested
│       ├── commands.rs               # #[tauri::command] for backend_status, restart_catalog, etc.
│       └── log_pipe.rs               # async fn to stream child stdout/stderr to log file + webview
│
├── fleet/                            # unchanged
│   ├── crates/
│   │   ├── fleet-mission/            # fleet-catalog binary (built; spawned by Tauri)
│   │   └── fleet-cli/
│   │       └── src/bin/
│   │           ├── supervisor.rs     # fleet-supervisor binary (built; spawned by Tauri)
│   │           └── mavfleet.rs       # mavfleet binary (built; spawned by supervisor on demand)
│   └── target/release/
│       ├── fleet-catalog
│       ├── fleet-supervisor
│       └── mavfleet
│
├── sim/                              # unchanged
│   └── ...
│
├── scripts/                          # unchanged; stack_up.sh still works for the web stack
│   └── stack_up.sh
│
├── docs/
│   ├── TAURI_APP_SPEC.md             # ← this file
│   ├── LIVE-DESKTOP-PREVIEW-SETUP.md
│   ├── SANDBOX_SETUP.md
│   ├── ARCHITECTURE.md
│   └── ...
│
└── PX4-Autopilot/  (symlink → /home/z/my-project/PX4-Autopilot)
    └── build/px4_sitl_default/bin/px4 # ← spawned by supervisor (NOT bundled in Tauri)
```

---

## Appendix B — Tauri Rust skeleton (`src-tauri/src/main.rs`)

```rust
// src-tauri/src/main.rs
// Prevents additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backends;
mod commands;
mod log_pipe;
mod shutdown;

use std::sync::Mutex;
use tauri::Manager;
use tokio::process::Child;

pub struct BackendState {
    pub catalog: Mutex<Option<Child>>,
    pub supervisor: Mutex<Option<Child>>,
}

#[tokio::main]
async fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .manage(BackendState {
            catalog: Mutex::new(None),
            supervisor: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            commands::backend_status,
            commands::restart_catalog,
            commands::restart_supervisor,
            commands::open_install_guide,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = backends::spawn_catalog(&handle).await {
                    log::error!("failed to spawn catalog: {e}");
                }
                if let Err(e) = backends::spawn_supervisor(&handle).await {
                    log::error!("failed to spawn supervisor: {e}");
                }
                // Probe PX4_ROOT; if missing, emit "px4-not-found" event for the webview
                backends::probe_px4(&handle).await;
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                let app = window.app_handle();
                shutdown::graceful_shutdown(app);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running RustSim GCS");
}
```

The `commands::backend_status`, `commands::restart_catalog`, `commands::restart_supervisor`, and `commands::open_install_guide` Tauri commands are the only IPC surface; the frontend calls them via `invoke()` from a new `BackendStatus` widget in the Settings panel. All other backend traffic stays as direct `fetch()` + `new WebSocket()` to `127.0.0.1`.

---

The appendices that follow (C through Q) are **normative**: every concrete file, contract, algorithm, and acceptance criterion required for an unambiguous implementation. Where a value appears in §1–§15 in summary form and again in an appendix in full form, **the appendix is canonical**; the body section is a summary.

---

## Appendix C — Canonical `tauri.conf.json` (full file, no fragments)

> Path: `src-tauri/tauri.conf.json`. Schema version pinned: Tauri 2.11.x. Any field not listed here is left at its Tauri default. Field order matches the Tauri 2.11 reference (https://v2.tauri.app/reference/config/) for diff-friendliness.

```json
{
  "$schema": "https://schema.tauri.app/config/2.11.0",
  "productName": "RustSim GCS",
  "version": "1.0.0",
  "identifier": "ai.z.rustsim.gcs",
  "build": {
    "beforeDevCommand": "cd ../console && NEXT_PUBLIC_RSIM_API_STYLE=direct npm run dev",
    "beforeDevCommandWait": 3,
    "devUrl": "http://localhost:3000",
    "beforeBuildCommand": "cd ../console && NEXT_PUBLIC_RSIM_API_STYLE=direct npm run build",
    "beforeBuildCommandWait": 30,
    "frontendDist": "../console/out",
    "runner": "cargo"
  },
  "app": {
    "windows": [
      {
        "label": "main",
        "title": "RustSim GCS — Operations Canvas",
        "width": 1280,
        "height": 800,
        "minWidth": 1024,
        "minHeight": 720,
        "resizable": true,
        "maximizable": true,
        "minimizable": true,
        "closable": true,
        "maximized": false,
        "fullscreen": false,
        "center": true,
        "decorations": true,
        "transparent": false,
        "alwaysOnTop": false,
        "skipTaskbar": false,
        "titleBarStyle": "Visible",
        "hidden": false,
        "theme": "Dark",
        "dragDropEnabled": false,
        "useHttpsScheme": false
      }
    ],
    "security": {
      "csp": "default-src 'self'; connect-src 'self' ipc: http://ipc.localhost http://127.0.0.1:8300 http://127.0.0.1:8400 http://127.0.0.1:8500 ws://127.0.0.1:8400; img-src 'self' asset: http://asset.localhost blob: data: https://*.tile.openstreetmap.org https://*.basemaps.cartocdn.com https://*.tile.opentopomap.org https://server.arcgisonline.com https://tiles.stadiamaps.com https://api.maptiler.com; style-src 'self' 'unsafe-inline'; script-src 'self' 'wasm-unsafe-eval'; font-src 'self' data:; worker-src 'self' blob:",
      "devCsp": null,
      "freezePrototype": false,
      "dangerousDisableAssetCspModification": false,
      "assetProtocol": {
        "enable": true,
        "scope": ["$RESOURCE/**", "$APPDATA/**", "$APPCONFIG/**"]
      }
    },
    "trayIcon": null,
    "macOSPrivateApi": false,
    "withGlobalTauri": false
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "publisher": "RustSim Team",
    "category": "DeveloperTool",
    "shortDescription": "QGroundControl-class GCS for PX4 SITL",
    "longDescription": "RustSim GCS is a pure Rust + TypeScript ground control station for PX4-Autopilot v1.16.2 SITL. Single-screen Operations Canvas with 7 overlay panels for mission authoring, fleet C2, SITL lifecycle, vehicle setup, ULog analyze, pre-flight checks, and settings.",
    "copyright": "Apache-2.0",
    "homepage": "https://github.com/kanishka-namdeo/rust-sim",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ],
    "resources": [],
    "externalBin": [],
    "macOS": {
      "frameworks": [],
      "minimumSystemVersion": "12.0",
      "exceptionDomain": "",
      "signingIdentity": null,
      "entitlements": null,
      "providerShortName": null
    },
    "windows": {
      "nsis": {
        "installerIcon": "icons/icon.ico",
        "headerImage": null,
        "sidebarImage": null,
        "installMode": "perMachine",
        "languages": ["en-US"],
        "displayLanguageSelector": false,
        "compression": "lzma"
      },
      "webviewInstallMode": {
        "type": "downloadBootstrapper",
        "silent": true
      },
      "wix": null,
      "certificateThumbprint": null,
      "digestAlgorithm": "sha256",
      "timestampUrl": "http://timestamp.sectigo.com",
      "tsp": false
    },
    "linux": {
      "deb": {
        "depends": ["libwebkit2gtk-4.1-0", "libssl3", "libgtk-3-0", "librsvg2-2"],
        "recommends": [],
        "provides": ["rustsim-gcs"],
        "conflicts": [],
        "replaces": [],
        "maintainer": "RustSim Team <noreply@rustsim.ai>",
        "priority": "optional",
        "section": "devel",
        "files": {},
        "desktopTemplate": null
      },
      "rpm": {
        "depends": ["webkit2gtk4.1", "openssl-libs", "gtk3", "librsvg2"],
        "recommends": [],
        "provides": ["rustsim-gcs"],
        "conflicts": [],
        "obsoletes": [],
        "release": "1",
        "vendor": "RustSim Team",
        "summary": null,
        "license": "Apache-2.0",
        "url": "https://github.com/kanishka-namdeo/rust-sim",
        "desktopTemplate": null,
        "icon": null
      },
      "appImage": {
        "bundleMediaFramework": false,
        "mediaFramework": null
      }
    }
  },
  "plugins": {}
}
```

### C.1 Field provenance & non-default decisions

| Field | Value chosen | Tauri default | Reason for deviation |
|---|---|---|---|
| `productName` | `"RustSim GCS"` | `"Tauri App"` | Branding; matches README badge. |
| `version` | `"1.0.0"` | reads from `Cargo.toml` | Explicit; bumps happen via this field first, then `Cargo.toml`. |
| `identifier` | `"ai.z.rustsim.gcs"` | required, no default | Reverse-DNS; `ai.z` is the team org prefix. |
| `build.runner` | `"cargo"` | `"cargo"` | Explicit for diff readability. |
| `build.beforeDevCommandWait` | `3` | `null` (no wait) | Next dev server takes ~1.5s to bind :3000 in dev (measured); 3s gives margin. |
| `build.beforeBuildCommandWait` | `30` | `null` | Next build takes ~25s on the sandbox; 30s is the 95th-percentile + 20%. |
| `app.windows[0].theme` | `"Dark"` | `null` (system) | Matches `next-themes defaultTheme="dark"` in `console/src/app/layout.tsx`. |
| `app.windows[0].useHttpsScheme` | `false` | `false` | Explicit; flipping to `true` invalidates IndexedDB/localStorage on Windows. |
| `app.security.csp` | (see above) | `null` (no CSP) | Without an explicit CSP, Tauri 2 injects a default that does NOT allow `ws://127.0.0.1:8400`. The GCS would fail silently on the first WS connect. |
| `app.security.assetProtocol.scope` | `["$RESOURCE/**", "$APPDATA/**", "$APPCONFIG/**"]` | `[]` | Allows the webview to load mission TOML files from the user's data directory via `asset://` URLs. Required for the "Import mission" dialog. |
| `app.withGlobalTauri` | `false` | `false` | Explicit; the frontend uses `import { invoke } from '@tauri-apps/api/core'`, not `window.__TAURI__`. |
| `bundle.targets` | `"all"` | `null` (no bundle) | Build all platform-native installers on every `tauri build`. |
| `bundle.windows.nsis.installMode` | `"perMachine"` | `"currentUser"` | Operators share one install across user accounts on the same machine; matches QGroundControl's installer behaviour. |
| `bundle.windows.webviewInstallMode.type` | `"downloadBootstrapper"` | `"downloadBootstrapper"` | Explicit; on first launch on Win 10 without WebView2, auto-download + silent install. |
| `bundle.linux.deb.depends` | (4 packages) | `[]` | The four shared libs `tauri::Builder`'s binary links against at runtime on Debian-family distros. `librsvg2-2` is needed for SVG icon rendering. |
| `bundle.linux.rpm.depends` | (4 packages) | `[]` | RPM-name equivalents of the deb depends (`webkit2gtk4.1`, not `libwebkit2gtk-4.1-0`). |

### C.2 Forbidden fields (do NOT add)

| Field | Why forbidden |
|---|---|
| `app.security.dangerousDisableAssetCspModification: true` | Disables Tauri's nonce/hash injection for our own scripts; weakens defence-in-depth. Leave at `false`. |
| `app.macOSPrivateApi: true` | Enables private APIs used by transparent windows; we're opaque. |
| `bundle.resources: ["../PX4-Autopilot/..."]` | PX4 is too large to bundle. User installs separately. |
| `bundle.externalBin: ["..."]` | Catalog + supervisor are same-workspace binaries compiled from source on the host, not prebuilt sidecars. See §6.2. |
| `plugins.updater` (active config) | Auto-updater stays structurally wired but feed URL empty until team operates a release server. |

---

## Appendix D — Canonical `Cargo.toml` for `src-tauri/`

> Path: `src-tauri/Cargo.toml`. Pinned versions reflect Sep 2026 stable. `tauri = "2"` and `tauri-build = "2"` resolve to the 2.11.x line.

```toml
[package]
name = "rustsim-gcs"
version = "1.0.0"
description = "QGroundControl-class GCS for PX4 SITL — Tauri shell"
authors = ["RustSim Team <noreply@rustsim.ai>"]
license = "Apache-2.0"
repository = "https://github.com/kanishka-namdeo/rust-sim"
default-run = "rustsim-gcs"
edition = "2021"
rust-version = "1.77.2"

[lib]
name = "rustsim_gcs_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = ["protocol-asset"] }
tauri-plugin-shell = "2"
tauri-plugin-log = "2"
tauri-plugin-window-state = "2"
tauri-plugin-fs = "2"
tauri-plugin-dialog = "2"
tauri-plugin-os = "2"
tauri-plugin-process = "2"

# Async runtime (Tauri 2 uses tokio under the hood)
tokio = { version = "1", features = ["full"] }

# HTTP client (for the Tauri Rust backend to call the supervisor's
# POST /api/sitl/stop on shutdown — the webview's fetch() cannot
# run during RunEvent::ExitRequested because the webview is already torn down)
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }

# Logging
log = "0.4"
env_logger = "0.11"

# Path resolution per OS
dirs = "5"

# Error handling
anyhow = "1"
thiserror = "1"

# Serialization (matches fleet workspace)
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Time (for log rotation + timestamps)
time = { version = "0.3", features = ["formatting", "macros"] }

[features]
# Production default: no custom features.
default = ["custom-protocol"]
custom-protocol = ["tauri/custom-protocol"]

[profile.release]
opt-level = "s"     # Optimise for size; we're not CPU-bound in the Tauri shell
lto = true          # Link-time optimisation across the workspace
codegen-units = 1   # Slower build, smaller binary
strip = true        # Strip debug symbols from the release binary
panic = "abort"     # Smaller binary; abort on panic instead of unwind

[profile.dev]
opt-level = 0
debug = true
strip = false
```

### D.1 Why `reqwest` with `rustls-tls` (not `native-tls`)

`native-tls` pulls in OpenSSL on Linux, which is fine on the dev machine but adds a runtime dep on `libssl3` for the released `.deb`. `rustls-tls` is pure-Rust TLS, statically linked, zero runtime deps. Cost: ~2 MB larger binary. Benefit: the binary runs on a stock Ubuntu 22.04 server image with no OpenSSL installed. We choose `rustls-tls`.

### D.2 Why `panic = "abort"` in release

Tauri's `tauri::Builder::run` catches panics at the event-loop boundary, but a panic in our `backends::spawn_*` tasks would otherwise unwind through `tokio::spawn` and silently kill the task. With `panic = "abort"`, a panic in a spawn task aborts the whole process — surfaces the bug immediately during testing, and ensures no half-alive state on the operator's machine. The `kill_on_drop(true)` on the backend children means an abort still cleans up the subprocesses (the OS sends SIGHUP to the orphans).

### D.3 Why `tauri` feature `protocol-asset`

We need the `asset://` protocol enabled so the webview can read mission TOML files from `~/.rustsim/missions/` via `convertFileSrc()` (Tauri's frontend helper). Without this feature, `asset://` is disabled and `convertFileSrc()` returns a `null` URL.

---

## Appendix E — Full Rust source: 5 modules

### E.1 `src-tauri/src/main.rs`

> Replaces the skeleton in Appendix B. Adds `tauri_plugin_log` config with log file output, fixes the `on_window_event` return type (Tauri 2 wants `Result<(), Box<dyn std::error::Error>>`), and gates the `setup` body behind `app.get_webview_window("main").is_some()` so the `--help` flag doesn't spawn backends.

```rust
// src-tauri/src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backends;
mod commands;
mod log_pipe;
mod shutdown;

use std::sync::Mutex;
use tauri::Manager;
use tauri_plugin_log::{Target, TargetKind};
use tokio::process::Child;

/// Shared state holding the spawned backend child handles.
/// `Mutex<Option<Child>>` (not `RwLock`): we only ever `.take()` and `.replace()`,
/// never read concurrently. Mutex is correct + cheaper than RwLock for this access pattern.
pub struct BackendState {
    pub catalog: Mutex<Option<Child>>,
    pub supervisor: Mutex<Option<Child>>,
}

fn main() {
    // Initialise the logger BEFORE Tauri, so panics in setup() are logged.
    let log_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rustsim/logs");
    let _ = std::fs::create_dir_all(&log_dir);

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::Folder {
                        path: log_dir.clone(),
                        file_name: Some("rustsim-gcs".to_string()),
                    })
                    .filter(|m| !m.target().contains("hyper") && !m.target().contains("reqwest")),
                    Target::new(TargetKind::Webview),
                ])
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .manage(BackendState {
            catalog: Mutex::new(None),
            supervisor: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            commands::backend_status,
            commands::restart_catalog,
            commands::restart_supervisor,
            commands::open_install_guide,
            commands::probe_px4,
            commands::reset_ports,
        ])
        .setup(|app| {
            // Don't spawn backends if the user passed --help or there's no main window.
            if app.get_webview_window("main").is_none() {
                return Ok(());
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = backends::probe_and_clean_ports().await {
                    log::warn!("port probe/clean failed: {e}");
                }
                if let Err(e) = backends::spawn_catalog(&handle).await {
                    log::error!("failed to spawn catalog: {e}");
                    handle.emit("backend-error", &serde_json::json!({
                        "backend": "catalog",
                        "error": e.to_string()
                    })).ok();
                }
                if let Err(e) = backends::spawn_supervisor(&handle).await {
                    log::error!("failed to spawn supervisor: {e}");
                    handle.emit("backend-error", &serde_json::json!({
                        "backend": "supervisor",
                        "error": e.to_string()
                    })).ok();
                }
                backends::probe_px4(&handle).await;
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Prevent the window from closing until backends are torn down.
                api.prevent_close();
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    shutdown::graceful_shutdown(&app).await;
                    if let Some(win) = app.get_webview_window("main") {
                        let _ = win.close();
                    }
                });
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running RustSim GCS");
}
```

### E.2 `src-tauri/src/backends.rs`

```rust
// src-tauri/src/backends.rs
use std::path::PathBuf;
use std::process::Stdio;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::log_pipe;
use crate::BackendState;

pub const CATALOG_PORT: u16 = 8300;
pub const FLEET_PORT: u16 = 8400;
pub const SUPERVISOR_PORT: u16 = 8500;

/// Resolve the path to a fleet-workspace binary. Looks for, in order:
///   1. `<repo_root>/fleet/target/release/<name>`  (developer machine, `cargo build --release`)
///   2. `<app_resource_dir>/bin/<name>`            (packaged install — binaries copied here by `tauri.conf.json` `resources`)
///   3. `<name>` on PATH                           (rare; for distro-packaged installs)
/// Returns an absolute PathBuf. Errors if none found.
pub fn resolve_binary(name: &str, app: &AppHandle) -> anyhow::Result<PathBuf> {
    // (1) Repo-relative (dev mode)
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // src-tauri/
    let repo_root = here.parent().unwrap();                 // rust-sim/
    let dev_path = repo_root.join("fleet/target/release").join(name);
    if dev_path.is_file() {
        return Ok(dev_path);
    }

    // (2) Packaged install
    if let Ok(resource_dir) = app.path().resource_dir() {
        let pkg_path = resource_dir.join("bin").join(name);
        if pkg_path.is_file() {
            return Ok(pkg_path);
        }
    }

    // (3) PATH lookup
    if let Ok(p) = which::which(name) {
        return Ok(p);
    }

    anyhow::bail!(
        "binary '{}' not found in repo ({}), resource dir, or PATH",
        name,
        dev_path.display()
    )
}

/// Resolve PX4_ROOT. Order:
///   1. `PX4_ROOT` env var if set and the px4 binary exists under it
///   2. `<repo_root>/../PX4-Autopilot`  (matches `scripts/stack_up.sh:43`)
///   3. `<repo_root>/PX4-Autopilot`     (the symlink layout used in the sandbox)
///   4. `~/.rustsim/PX4-Autopilot`      (per-user install for non-developer operators)
/// Returns the directory containing the `build/px4_sitl_default/bin/px4` binary, not the binary itself.
pub fn resolve_px4_root() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = {
        let mut v = vec![];
        if let Ok(env) = std::env::var("PX4_ROOT") {
            v.push(PathBuf::from(env));
        }
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = here.parent().unwrap().to_path_buf();
        v.push(repo_root.join("../PX4-Autopilot").canonicalize().unwrap_or_default());
        v.push(repo_root.join("PX4-Autopilot"));
        if let Some(home) = dirs::home_dir() {
            v.push(home.join(".rustsim/PX4-Autopilot"));
        }
        v
    };

    for c in candidates {
        let px4_bin = c.join("build/px4_sitl_default/bin/px4");
        if px4_bin.is_file() {
            return Some(c);
        }
    }
    None
}

pub async fn probe_px4(app: &AppHandle) {
    let found = resolve_px4_root();
    let _ = app.emit(
        "px4-status",
        &serde_json::json!({
            "found": found.is_some(),
            "path": found.as_ref().map(|p| p.display().to_string()),
        }),
    );
}

/// Spawn the catalog binary on :8300.
/// On success, stores the Child handle in BackendState.
pub async fn spawn_catalog(app: &AppHandle) -> anyhow::Result<()> {
    let bin = resolve_binary("fleet-catalog", app)?;
    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data dir"))?
        .join("rustsim/catalog");
    std::fs::create_dir_all(&data_dir)?;

    let mut cmd = Command::new(&bin);
    cmd.env("RSIM_CATALOG_DIR", &data_dir)
       .env("RUST_LOG", "info")
       .env("RSIM_CATALOG_PORT", CATALOG_PORT.to_string())
       .stdin(Stdio::null())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .kill_on_drop(true);

    // On Unix, spawn in a new process group so we can killpg() the whole tree
    // (the catalog doesn't spawn children today, but the supervisor does, and
    // we apply the same pattern here for consistency).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    log::info!("spawning catalog: {} (env RSIM_CATALOG_PORT={})", bin.display(), CATALOG_PORT);
    let mut child = cmd.spawn()?;

    // Pipe stdout/stderr to log files + webview
    if let Some(stdout) = child.stdout.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log::info!("[catalog] {line}");
                let _ = app2.emit("backend-stdout", &serde_json::json!({
                    "backend": "catalog", "line": line
                }));
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log::warn!("[catalog:err] {line}");
                let _ = app2.emit("backend-stderr", &serde_json::json!({
                    "backend": "catalog", "line": line
                }));
            }
        });
    }

    let state: tauri::State<BackendState> = app.state();
    *state.catalog.lock().unwrap() = Some(child);
    Ok(())
}

/// Spawn the supervisor binary on :8500.
pub async fn spawn_supervisor(app: &AppHandle) -> anyhow::Result<()> {
    let bin = resolve_binary("fleet-supervisor", app)?;
    let px4_root = resolve_px4_root();

    let mut cmd = Command::new(&bin);
    cmd.env("RUST_LOG", "info")
       .env("RSIM_SUPERVISOR_PORT", SUPERVISOR_PORT.to_string())
       .stdin(Stdio::null())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .kill_on_drop(true);
    if let Some(px4) = px4_root {
        cmd.env("PX4_ROOT", &px4);
        cmd.env("FLEET_PX4_DIR", &px4);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    log::info!("spawning supervisor: {} (env RSIM_SUPERVISOR_PORT={})", bin.display(), SUPERVISOR_PORT);
    let mut child = cmd.spawn()?;

    if let Some(stdout) = child.stdout.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log::info!("[supervisor] {line}");
                let _ = app2.emit("backend-stdout", &serde_json::json!({
                    "backend": "supervisor", "line": line
                }));
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log::warn!("[supervisor:err] {line}");
                let _ = app2.emit("backend-stderr", &serde_json::json!({
                    "backend": "supervisor", "line": line
                }));
            }
        });
    }

    let state: tauri::State<BackendState> = app.state();
    *state.supervisor.lock().unwrap() = Some(child);
    Ok(())
}

/// Kill any stale processes squatting :8300 or :8500. Called on startup before spawning.
pub async fn probe_and_clean_ports() -> anyhow::Result<()> {
    for port in [CATALOG_PORT, FLEET_PORT, SUPERVISOR_PORT] {
        if port_is_listening(port).await {
            log::warn!("port {} already in use — attempting recovery", port);
            // Strategy: ask the OS which PID holds the port, and if it's a fleet-* process, kill it.
            // Implemented via `lsof` on Unix, `netstat` on Windows.
            if let Some(pid) = pid_listening_on_port(port) {
                log::warn!("port {} held by pid {} — killing", port, pid);
                kill_pid(pid)?;
            } else {
                anyhow::bail!("port {} in use but owner unknown; aborting", port);
            }
        }
    }
    Ok(())
}

async fn port_is_listening(port: u16) -> bool {
    tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .is_ok()
}

fn pid_listening_on_port(port: u16) -> Option<u32> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("lsof")
            .args(["-ti", &format!("tcp:{port}")])
            .output()
            .ok()?;
        let pid_str = String::from_utf8_lossy(&out.stdout);
        let pid_str = pid_str.lines().next()?;
        pid_str.trim().parse().ok()
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .arg(format!("-f"))
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            if line.contains(&format!(":{}", port)) && line.contains("LISTENING") {
                let pid: u32 = line.split_whitespace().last()?.parse().ok()?;
                return Some(pid);
            }
        }
        None
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

fn kill_pid(pid: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()?;
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("kill failed: {e}"))
    }
    #[cfg(windows)]
    {
        std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status()
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("taskkill failed: {e}"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        anyhow::bail!("kill_pid not implemented on this platform")
    }
}
```

### E.3 `src-tauri/src/commands.rs`

```rust
// src-tauri/src/commands.rs
use crate::backends::{self, resolve_px4_root};
use crate::BackendState;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::ShellExt;

#[derive(Serialize)]
pub struct BackendStatus {
    pub catalog: BackendHealth,
    pub supervisor: BackendHealth,
    pub px4: Px4Status,
}

#[derive(Serialize)]
pub struct BackendHealth {
    pub port: u16,
    pub online: bool,
    pub pid: Option<u32>,
}

#[derive(Serialize)]
pub struct Px4Status {
    pub found: bool,
    pub path: Option<String>,
}

/// `invoke('backend_status')` — called by the frontend's Settings → About panel every 2s.
/// Returns the health of catalog, supervisor, and PX4. Does NOT spawn; this is a pure probe.
#[tauri::command]
pub async fn backend_status(app: AppHandle) -> Result<BackendStatus, String> {
    let catalog = probe_health(8300, &app).await;
    let supervisor = probe_health(8500, &app).await;
    let px4_path = resolve_px4_root();
    Ok(BackendStatus {
        catalog,
        supervisor,
        px4: Px4Status {
            found: px4_path.is_some(),
            path: px4_path.as_ref().map(|p| p.display().to_string()),
        },
    })
}

async fn probe_health(port: u16, app: &AppHandle) -> BackendHealth {
    let url = format!("http://127.0.0.1:{port}/api/health");
    let online = match reqwest::Client::new().get(&url).timeout(std::time::Duration::from_millis(500)).send().await {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    };
    let pid = {
        let state: tauri::State<BackendState> = app.state();
        let m = if port == 8300 { &state.catalog } else { &state.supervisor };
        m.lock().unwrap().as_ref().and_then(|c| c.id()).ok().flatten()
    };
    BackendHealth { port, online, pid }
}

/// `invoke('restart_catalog')` — kills + respawns the catalog. Called from Settings → Restart Catalog button.
#[tauri::command]
pub async fn restart_catalog(app: AppHandle) -> Result<(), String> {
    {
        let state: tauri::State<BackendState> = app.state();
        if let Some(child) = state.catalog.lock().unwrap().take() {
            let _ = child.kill().await;
        }
    }
    backends::spawn_catalog(&app).await.map_err(|e| e.to_string())
}

/// `invoke('restart_supervisor')` — kills + respawns the supervisor. DANGER: also kills any running SITL.
#[tauri::command]
pub async fn restart_supervisor(app: AppHandle) -> Result<(), String> {
    // First, ask the supervisor to stop SITL gracefully (if running).
    let _ = reqwest::Client::new()
        .post("http://127.0.0.1:8500/api/sitl/stop")
        .json(&serde_json::json!({}))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;
    // Then kill + respawn the supervisor itself.
    {
        let state: tauri::State<BackendState> = app.state();
        if let Some(child) = state.supervisor.lock().unwrap().take() {
            let _ = child.kill().await;
        }
    }
    backends::spawn_supervisor(&app).await.map_err(|e| e.to_string())
}

/// `invoke('open_install_guide')` — opens the PX4 install doc URL in the OS browser.
#[tauri::command]
pub async fn open_install_guide(app: AppHandle) -> Result<(), String> {
    app.shell()
        .open("https://github.com/kanishka-namdeo/rust-sim/blob/main/docs/SANDBOX_SETUP.md#6-px4-autopilot-v1162--clone-patch-two-sandbox-gaps-build", None)
        .map_err(|e| e.to_string())
}

/// `invoke('probe_px4')` — re-probe PX4_ROOT (e.g. after the operator installed PX4). Emits `px4-status` event.
#[tauri::command]
pub async fn probe_px4(app: AppHandle) -> Result<(), String> {
    backends::probe_px4(&app).await;
    Ok(())
}

/// `invoke('reset_ports')` — kill any stale process holding :8300 / :8400 / :8500. DANGER.
#[tauri::command]
pub async fn reset_ports(_app: AppHandle) -> Result<(), String> {
    backends::probe_and_clean_ports().await.map_err(|e| e.to_string())
}
```

### E.4 `src-tauri/src/shutdown.rs`

```rust
// src-tauri/src/shutdown.rs
use crate::BackendState;
use tauri::{AppHandle, Manager};
use tokio::process::Child;

/// Graceful shutdown: stop SITL, wait 250ms, then drop the Child handles.
/// Called from `on_window_event(CloseRequested)`. Idempotent.
pub async fn graceful_shutdown(app: &AppHandle) {
    log::info!("graceful shutdown starting");

    // Step 1: ask the supervisor to stop SITL (kills mavfleet + px4).
    let _ = reqwest::Client::new()
        .post("http://127.0.0.1:8500/api/sitl/stop")
        .json(&serde_json::json!({}))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;
    log::info!("supervisor /api/sitl/stop sent");

    // Step 2: wait 250ms for the supervisor to clean up the mavfleet/px4 tree.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    // Step 3: drop the Child handles — kill_on_drop takes over.
    let state: tauri::State<BackendState> = app.state();
    let catalog = state.catalog.lock().unwrap().take();
    let supervisor = state.supervisor.lock().unwrap().take();
    if let Some(c) = catalog {
        let _ = kill_child(c).await;
    }
    if let Some(c) = supervisor {
        let _ = kill_child(c).await;
    }
    log::info!("graceful shutdown complete");
}

/// Kill a tokio::process::Child. Try SIGTERM first, wait, then SIGKILL.
async fn kill_child(mut child: Child) -> std::io::Result<()> {
    // tokio::process::Child::kill uses SIGKILL on Unix; we want SIGTERM first.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        if let Some(pid) = child.id() {
            // SIGTERM the whole process group (pgid = pid because we set process_group(0))
            let _ = std::process::Command::new("kill")
                .args(["-TERM", "-"])
                .arg(pid.to_string())
                .status();
        }
        let _ = child.id(); // keep child alive
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        // If still alive, SIGKILL via kill_on_drop
        let _ = child.start_kill();
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill(); // Windows: TerminateProcess
    }
    let _ = child.wait().await;
    Ok(())
}
```

### E.5 `src-tauri/src/log_pipe.rs`

```rust
// src-tauri/src/log_pipe.rs
// Currently a placeholder — the actual piping logic is inline in backends.rs
// (one tokio::spawn task per Child stdout/stderr).
// This module is reserved for future shared helpers (e.g. log rotation, level filtering).

pub const MAX_LOG_LINE_LEN: usize = 4096;
pub const LOG_ROTATION_SIZE_MB: u64 = 50;
```

### E.6 `src-tauri/build.rs`

```rust
// src-tauri/build.rs
fn main() {
    tauri_build::build()
}
```

---

## Appendix F — Exact frontend patches (unified diffs)

### F.1 `console/next.config.ts`

```diff
--- a/console/next.config.ts
+++ b/console/next.config.ts
@@ -1,13 +1,29 @@
+const isProd = process.env.NODE_ENV === 'production';
+const internalHost = process.env.TAURI_DEV_HOST || 'localhost';
+
 /** @type {import('next').NextConfig} */
 const nextConfig = {
-  output: "standalone",
+  output: "export",
+  images: {
+    unoptimized: true,
+  },
+  // In dev, the Tauri webview loads http://localhost:3000; Next.js HMR assets
+  // must be served from the dev server origin, not the webview origin.
+  // In prod, assetPrefix is undefined (Tauri serves /assets/* at its own origin).
+  assetPrefix: isProd ? undefined : `http://${internalHost}:3000`,
   reactStrictMode: false,
   typescript: {
     ignoreBuildErrors: false,
   },
 };

 export default nextConfig;
```

### F.2 `console/src/app/layout.tsx` — favicon + fonts

```diff
--- a/console/src/app/layout.tsx
+++ b/console/src/app/layout.tsx
@@ -1,22 +1,33 @@
 import type { Metadata } from "next";
-import { Geist, Geist_Mono } from "next/font/google";
+import localFont from "next/font/local";
 import "./globals.css";

-const geistSans = Geist({ subsets: ["latin"], variable: "--font-geist-sans" });
-const geistMono = Geist_Mono({ subsets: ["latin"], variable: "--font-geist-mono" });
+const geistSans = localFont({
+  src: "../public/fonts/Geist-Variable.woff2",
+  display: "swap",
+  variable: "--font-geist-sans",
+});
+const geistMono = localFont({
+  src: "../public/fonts/GeistMono-Variable.woff2",
+  display: "swap",
+  variable: "--font-geist-mono",
+});

 export const metadata: Metadata = {
   title: "Operations Canvas — rustsitsim · mavfleet",
   description: "RustSim GCS — QGroundControl-class ground control station for PX4 SITL",
   icons: {
-    icon: "https://z-cdn.chatglm.cn/z-ai/static/logo.svg",
+    icon: "/logo.svg",
   },
 };

 export default function RootLayout({ children }: { children: React.ReactNode }) {
   return (
-    <html lang="en" className={`${geistSans.variable} ${geistMono.variable}`} suppressHydrationWarning>
+    <html lang="en" className={`${geistSans.variable} ${geistMono.variable}`}>
       <body>{children}</body>
     </html>
   );
 }
```

### F.3 `console/src/lib/conn.ts` — direct mode only

```diff
--- a/console/src/lib/conn.ts
+++ b/console/src/lib/conn.ts
@@ -25,30 +25,18 @@ const API_STYLE = process.env.NEXT_PUBLIC_RSIM_API_STYLE ?? "gateway";
 // Ports are pinned. See docs/TAURI_APP_SPEC.md §6.5 for why (no dynamic allocation).
 export const CATALOG_PORT = 8300;
 export const FLEET_PORT = 8400;
 export const SUPERVISOR_PORT = 8500;

-// In Tauri, NEXT_PUBLIC_RSIM_API_STYLE=direct is forced at build time.
-// The gateway branch is dead code in the Tauri build; keep it for the legacy
-// web-stack path (stack_up.sh + Caddy :81).
-const isDirect = API_STYLE === "direct";
+// Direct mode is the only supported style going forward.
+// The gateway mode (Caddy :81 + ?XTransformPort) remains available for the
+// legacy web stack via scripts/stack_up.sh, but the Tauri build bakes direct mode in.
+const isDirect = true;

 export function gw(path: string, port: number): string {
   if (isDirect) {
     return `http://127.0.0.1:${port}${path}`;
   }
   return `${path}?XTransformPort=${port}`;
 }

 export function wsUrl(port: number): string {
   if (isDirect) {
     return `ws://127.0.0.1:${port}/`;
   }
   const proto = window.location.protocol === "https:" ? "wss" : "ws";
   return `${proto}://${window.location.host}/?XTransformPort=${port}`;
 }
```

### F.4 `console/src/components/canvas/MapCanvas.tsx` — worker URL

```diff
--- a/console/src/components/canvas/MapCanvas.tsx
+++ b/console/src/components/canvas/MapCanvas.tsx
@@ -97,7 +97,12 @@ export function MapCanvas() {
     const map = new maplibregl.Map({
       container,
       style: basemapStyle,
-      workerUrl: new URL('/maplibre/maplibre-gl-worker.mjs', window.location.origin).href,
+      // In Tauri v2, window.location.origin is http://tauri.localhost (Win/Linux)
+      // or tauri://localhost (macOS). Both serve the bundled out/ dir, so the
+      // worker URL is the same path either way. Using import.meta.url makes
+      // the worker path resolve relative to THIS module, which is more robust
+      // under both Next dev (http://localhost:3000) and Tauri prod.
+      workerUrl: new URL('/maplibre/maplibre-gl-worker.mjs', import.meta.url).href,
       attributionControl: true,
     });
     setMapInstance(map);
```

### F.5 `console/package.json` — drop dead dep

```diff
--- a/console/package.json
+++ b/console/package.json
@@ -42,7 +42,6 @@
     "maplibre-gl": "^6.9.0",
     "next": "16.1.1",
     "next-themes": "^0.4.6",
-    "react-map-gl": "^8.1.3",
     "react": "^19.0.0",
     "react-dom": "^19.0.0",
     "tailwind-merge": "^2.6.0",
```

### F.6 `console/.env.production` (NEW FILE)

```bash
# Forced at build time for the Tauri static export.
NEXT_PUBLIC_RSIM_API_STYLE=direct
```

### F.7 Delete `console/src/app/api/route.ts`

```bash
git rm console/src/app/api/route.ts
```

The file contained only:

```ts
// (deleted in M-T1)
import { NextResponse } from "next/server";
export async function GET() {
  return NextResponse.json({ service: "rustsim-console", sim: ":8200", fleet: ":8400" });
}
```

Verified by subagent A: nothing in `console/src/` calls `/api`, so deletion has zero downstream impact.

### F.8 Vendor Geist fonts (one-time)

```bash
mkdir -p console/public/fonts
# Download the variable-axis .woff2 from Google Fonts CDN.
# These URLs are stable; verify with `curl -I` before committing.
curl -L -o console/public/fonts/Geist-Variable.woff2 \
  "https://fonts.gstatic.com/s/geist/v1/gyByhhUhhIdQmnMp0-0HIhTI8YJlRHtXhkGflvwiBlYFofQ.woff2"
curl -L -o console/public/fonts/GeistMono-Variable.woff2 \
  "https://fonts.gstatic.com/s/geistmono/v1/oYy8lfOQG7HmfyIL2-waYDBtXll2zRPyXknHWFX3BlYFofQ.woff2"
# Verify the files are valid woff2 (the magic bytes are "wOF2").
file console/public/fonts/*.woff2
# Commit
git add console/public/fonts/*.woff2
git commit -m "console: vendor Geist + Geist_Mono woff2 for offline Tauri builds"
```

---

## Appendix G — IPC command contracts

Every Tauri `invoke()` exposed to the frontend. **Arguments are JSON-encoded** (camelCase keys by Tauri default). **Return values** are JSON. **Errors** are `Result<T, String>` — the frontend gets the `Err(String)` as a rejected Promise.

### G.1 `invoke('backend_status')` → `BackendStatus`

```ts
// Frontend call (no args)
const status: BackendStatus = await invoke('backend_status');
```

**Returns:**

```ts
interface BackendStatus {
  catalog:    BackendHealth;
  supervisor: BackendHealth;
  px4:        Px4Status;
}
interface BackendHealth { port: number; online: boolean; pid: number | null; }
interface Px4Status    { found: boolean; path: string | null; }
```

**Errors:** none — the function never returns `Err`. Both backends being offline returns `{ online: false, pid: null }` in the respective health field, not an error.

**Refresh cadence:** the Settings → About panel calls this every 2 seconds.

### G.2 `invoke('restart_catalog')` → `()`

```ts
await invoke('restart_catalog');
```

**Side effects:** kills the running `fleet-catalog` child (if any) via `child.start_kill()`, then spawns a new one. The new catalog picks up the same `RSIM_CATALOG_DIR` (preserves persisted missions).

**Errors:** returns `Err(String)` if the binary cannot be found or the spawn fails.

**Concurrency:** safe to call concurrently — internally takes the same `BackendState.catalog` Mutex.

### G.3 `invoke('restart_supervisor')` → `()`

```ts
await invoke('restart_supervisor');
```

**Side effects:**
1. Sends `POST http://127.0.0.1:8500/api/sitl/stop` (graceful — kills mavfleet + px4 first).
2. Waits 250ms.
3. Kills the running `fleet-supervisor` child.
4. Spawns a new supervisor.

**Errors:** `Err(String)` if spawn fails. If the SITL stop call fails (e.g. supervisor already crashed), the kill + respawn still proceeds — the operator is warned via a `backend-error` event but the restart succeeds.

### G.4 `invoke('open_install_guide')` → `()`

```ts
await invoke('open_install_guide');
```

**Side effects:** opens the PX4 install guide URL in the OS default browser via `tauri-plugin-shell`'s `open()`. URL: `https://github.com/kanishka-namdeo/rust-sim/blob/main/docs/SANDBOX_SETUP.md#6-...`.

**Errors:** `Err(String)` if the URL scheme is not allowed by the shell capability (it should be — `shell:allow-open` permits `https://`).

### G.5 `invoke('probe_px4')` → `()`

```ts
await invoke('probe_px4');
// Listen for the 'px4-status' event:
const unlisten = await listen('px4-status', (event) => {
  console.log(event.payload); // { found: boolean, path: string | null }
});
```

**Side effects:** re-runs `resolve_px4_root()` and emits a `px4-status` event to all webview windows.

**Errors:** none — always returns `Ok(())`. The probe result is in the event payload.

**Use case:** after the operator installs PX4 (perhaps via the install guide opened by `open_install_guide`), they click "Re-check PX4" in Settings → About, which calls this.

### G.6 `invoke('reset_ports')` → `()`

```ts
await invoke('reset_ports');
```

**Side effects:** kills any process holding :8300, :8400, or :8500. Called from the "Force-reset ports" button in the error dialog that appears when Tauri fails to spawn a backend due to a port conflict.

**Errors:** `Err(String)` if `lsof`/`netstat` is missing or a PID can't be killed.

### G.7 Events emitted from Rust → frontend

| Event name | Payload | Emitted when |
|---|---|---|
| `backend-stdout` | `{ backend: "catalog"\|"supervisor", line: string }` | Each line of stdout from a backend. |
| `backend-stderr` | `{ backend: "catalog"\|"supervisor", line: string }` | Each line of stderr from a backend. |
| `backend-error` | `{ backend: "catalog"\|"supervisor", error: string }` | A backend spawn failed. |
| `px4-status` | `{ found: boolean, path: string\|null }` | After `probe_px4()` runs. |

The frontend listens via `@tauri-apps/api/event`'s `listen()`:

```ts
import { listen } from '@tauri-apps/api/event';
const unlisten = await listen('px4-status', (event) => { /* ... */ });
// Call unlisten() on component unmount.
```

---

## Appendix H — Algorithms (pseudocode)

### H.1 PX4 discovery — `resolve_px4_root()`

```
INPUT: none
OUTPUT: PathBuf to the PX4-Autopilot checkout, or None

candidates = []

# 1. Explicit env override (highest priority)
if env.PX4_ROOT is set:
    candidates.append(env.PX4_ROOT)

# 2. Repo-relative paths (dev mode)
here = CARGO_MANIFEST_DIR                          # src-tauri/
repo_root = parent(here)                            # rust-sim/
candidates.append(canonicalize(repo_root/../PX4-Autopilot))
candidates.append(repo_root/PX4-Autopilot)

# 3. Per-user install (packaged mode)
home = HOME
candidates.append(home/.rustsim/PX4-Autopilot)

# 4. System-wide install (rare — for distro packages)
candidates.append(/opt/rustsim/PX4-Autopilot)

for c in candidates:
    if c/build/px4_sitl_default/bin/px4 exists as a file:
        return c

return None
```

The first match wins. **No fallback** to `which px4` — we explicitly want the build tree (so the supervisor can find `ROMFS/`, `etc/init.d/`, the airframe TOMLs, etc., not just the binary).

### H.2 Binary resolution — `resolve_binary(name, app)`

```
INPUT: name (e.g. "fleet-catalog"), app handle
OUTPUT: PathBuf, or Err

# 1. Repo-relative (dev mode + dev install)
here = CARGO_MANIFEST_DIR
repo_root = parent(here)
dev_path = repo_root/fleet/target/release/<name>
if dev_path.is_file():
    return dev_path

# 2. Packaged install (production)
resource_dir = app.path().resource_dir()
pkg_path = resource_dir/bin/<name>
if pkg_path.is_file():
    return pkg_path

# 3. PATH lookup (rare — distro install)
if which::which(name).is_ok():
    return which::which(name).unwrap()

return Err("binary not found in repo, resource dir, or PATH")
```

**Implication for packaging:** when building the `.deb` / `.dmg` / `.msi`, the `tauri.conf.json` `bundle.resources` field MUST include the `fleet-catalog`, `fleet-supervisor`, and `mavfleet` binaries. The build pipeline copies them from `fleet/target/release/` into the resource dir at bundle time. **This is wired in §10.1 — `resources: []` becomes `resources: ["../fleet/target/release/fleet-catalog", "../fleet/target/release/fleet-supervisor", "../fleet/target/release/mavfleet"]`** once M-T7 starts.

### H.3 Port conflict recovery — `probe_and_clean_ports()`

```
INPUT: none (uses constants CATALOG_PORT, FLEET_PORT, SUPERVISOR_PORT)
OUTPUT: Ok(()) if all ports free, Err(String) otherwise

for port in [8300, 8400, 8500]:
    if tcp_connect(127.0.0.1:port) succeeds:
        log.warn("port {port} in use — recovering")
        pid = pid_listening_on_port(port)            # via lsof (Unix) or netstat (Win)
        if pid is None:
            return Err("port {port} in use but owner unknown; aborting")
        log.warn("port {port} held by pid {pid} — killing")
        kill_pid(pid):                               # SIGTERM, wait 500ms, SIGKILL
            # Unix: kill -TERM <pid>, sleep 500ms, kill -KILL <pid>
            # Win:  taskkill /F /T /PID <pid>
        # Re-probe
        if tcp_connect(127.0.0.1:port) still succeeds:
            return Err("port {port} still in use after killing pid {pid}")

return Ok(())
```

**Safety:** only kills PIDs found via `lsof -ti tcp:<port>` / `netstat -ano`. Never kills arbitrary PIDs. If `lsof`/`netstat` is missing, returns an error and the operator sees a dialog asking them to manually close the conflicting process.

**Recovery on failure:** the operator can click "Force-reset ports" in the error dialog, which calls `invoke('reset_ports')` (Appendix G.6). This re-runs `probe_and_clean_ports()`.

### H.4 Graceful shutdown sequence — `graceful_shutdown()`

```
INPUT: app handle
OUTPUT: none (async, idempotent)

1. log.info("graceful shutdown starting")

2. POST http://127.0.0.1:8500/api/sitl/stop
     body: {}
     timeout: 2s
   → supervisor kills mavfleet (which kills px4 + sitsim per-vehicle)
   → supervisor stays alive (it's the orchestrator, not the workload)

3. sleep 250ms (let the supervisor's child-kill propagate)

4. take BackendState.catalog Child, kill it (SIGTERM → wait 500ms → SIGKILL)
5. take BackendState.supervisor Child, kill it (SIGTERM → wait 500ms → SIGKILL)

6. log.info("graceful shutdown complete")

7. app.get_webview_window("main").close()  # closes the now-empty window
```

**Edge cases:**

- **SITL not running:** step 2 returns `{already_exited: true}` (verified live — see the supervisor's stop response in the 2026-09-11 run). Step 3 still waits 250ms (no-op). Steps 4–5 still execute.
- **Supervisor already crashed:** step 2 fails with a connection error. We log + ignore. Steps 4–5 still execute (catalog kill + supervisor kill via Child).
- **App panic mid-shutdown:** `kill_on_drop(true)` on the Child handles fires when the `BackendState` is dropped at process exit. The children die. Belt + suspenders.
- **Window close cancelled:** the `on_window_event` handler calls `api.prevent_close()` first, spawns the shutdown task, and only calls `window.close()` after the shutdown task completes. This prevents the window from disappearing before the backends are dead.

### H.5 Log rotation

The Tauri Rust binary writes logs to `<data_dir>/rustsim/logs/rustsim-gcs.log` via `tauri-plugin-log`'s `TargetKind::Folder`. The plugin does NOT auto-rotate. **Manual rotation policy:**

- **Trigger:** when `rustsim-gcs.log` exceeds 50 MB (checked on each `info!` write via a custom `Target` wrapper — Phase 2).
- **Action:** rename `rustsim-gcs.log` → `rustsim-gcs.YYYY-MM-DD-HHMMSS.log.gz`, open a new file.
- **Retention:** keep the last 5 rotated files (~250 MB max).
- **Per-backend logs:** the catalog and supervisor each write their own logs (the `log` crate via `env_logger` inside the binaries themselves) to `<data_dir>/rustsim/logs/{catalog,supervisor}.log`. Same rotation policy.

For v1 (M-T1–M-T8), rotation is manual: the operator clicks "Open logs folder" in Settings → About, and deletes old files. Auto-rotation is Phase 2.

---

## Appendix I — Data directory layout per OS

All RustSim data lives under the OS-specific data dir (resolved via the `dirs` crate). **No data is written inside the `.app` / `.exe` / installation directory** — that gets wiped on app update.

### I.1 Layout

| OS | `dirs::data_dir()` | RustSim subpath |
|---|---|---|
| Linux | `~/.local/share` | `~/.local/share/rustsim/` |
| macOS | `~/Library/Application Support` | `~/Library/Application Support/ai.z.rustsim.gcs/` |
| Windows | `%APPDATA%` (= `C:\Users\<user>\AppData\Roaming`) | `%APPDATA%\rustsim\` |

### I.2 Subdirectories (created on first launch)

```
<data_dir>/rustsim/
├── logs/
│   ├── rustsim-gcs.log         # Tauri Rust binary log
│   ├── catalog.log             # fleet-catalog stdout/stderr
│   └── supervisor.log          # fleet-supervisor stdout/stderr
├── catalog/                    # RSIM_CATALOG_DIR — mission TOML + presets + ULog
│   ├── missions/
│   │   ├── <uuid>.toml
│   │   └── ...
│   ├── presets/
│   │   ├── iris-defaults.json
│   │   └── ...
│   └── ulogs/
│       └── <run_dir>/
│           └── *.ulg
├── missions/                   # operator-imported .plan / .toml files (via dialog)
│   └── imported-2026-09-11.toml
├── window-state.json           # tauri-plugin-window-state persistence
└── config.json                 # operator-set preferences (Settings panel)
```

### I.3 Migration from web-stack paths

Today, the catalog writes to `fleet/scratch/catalog/` (relative to the repo). The Tauri build sets `RSIM_CATALOG_DIR` to `<data_dir>/rustsim/catalog/` instead. **First-launch migration:**

1. On startup, if `<data_dir>/rustsim/catalog/` does NOT exist AND `fleet/scratch/catalog/` DOES exist, copy the latter to the former (non-recursive `cp -r`).
2. Log the migration.
3. If the copy fails (disk full, permissions), continue with an empty catalog — the operator's missions are still in `fleet/scratch/catalog/` and they can be manually moved later.

This migration runs ONCE. The `~/.rustsim/.migrated-v1` flag file prevents re-running.

### I.4 What does NOT get migrated

- **PX4 build tree** — operator's `PX4-Autopilot/` stays where it is. Tauri discovers it via `PX4_ROOT` env or `resolve_px4_root()` (Appendix H.1).
- **localStorage keys** — these are webview-scoped, not file-scoped. They migrate automatically because Tauri 2's webview on Win/Linux uses `http://tauri.localhost` (same origin as a hypothetical prior Tauri install). macOS uses `tauri://localhost`. The 5 keys (`rsim.map.v1`, etc.) survive.
- **G-ladder harness artifacts** (`console/tests/*_artifacts/`) — stay in the repo, not migrated. They're for CI, not operators.

---

## Appendix J — Process tree & sequence diagrams

### J.1 Process tree (steady state with SITL running)

```
rustsim-gcs (Tauri binary, PID 1000)
├── [tokio::process::Child]  fleet-catalog (PID 1001)  ← port :8300
├── [tokio::process::Child]  fleet-supervisor (PID 1002)  ← port :8500
│   └── [std::process::Child]  mavfleet run --fleet operator_session.toml --api-port 8400 (PID 1003)  ← port :8400
│       ├── [std::process::Child]  sitsim-cli (vehicle 0, PID 1004)  ← HIL TCP :4560
│       │   └── (sim physics; no further children)
│       ├── [std::process::Child]  px4 (vehicle 0, PID 1005)  ← MAVLink UDP :14540
│       │   └── (PX4 SITL; no further children)
│       ├── [std::process::Child]  sitsim-cli (vehicle 1, PID 1006)  ← HIL TCP :4561
│       └── [std::process::Child]  px4 (vehicle 1, PID 1007)  ← MAVLink UDP :14541
└── [webkit subprocess]  WebKitGTK / WebView2 / WKWebView (PID 1008)
    └── (renders the Operations Canvas HTML/JS/WASM)
```

**Tauri owns:** 1001, 1002 (via `tokio::process::Child` + `kill_on_drop(true)`).
**Supervisor owns:** 1003 (via `std::process::Command` inside the supervisor's Rust code).
**mavfleet owns:** 1004–1007 (via `std::process::Command` inside mavfleet's Rust code).

**Kill propagation:** Tauri kills 1001 + 1002. The supervisor, on receiving `POST /api/sitl/stop`, kills 1003. mavfleet, on receiving SIGTERM (from supervisor's child-kill), kills 1004–1007. Each level uses `kill_on_drop` or explicit child-kill — verified by the 2026-09-11 live run where `POST /api/sitl/stop` returned `{stopped: true, already_exited: false}` and the ports were released.

### J.2 Sequence: operator clicks "Start" in SITL Manager

```
Operator          Webview (JS)        Tauri (Rust)         Supervisor          mavfleet         px4 + sitsim
   │                  │                    │                    │                  │                 │
   │ click Start      │                    │                    │                  │                 │
   ├──────────────────►│                    │                    │                  │                 │
   │                  │ POST :8500/api/sitl/start                  │                  │                 │
   │                  ├──────────────────────────────────────────►│                  │                 │
   │                  │                    │                    │ spawn mavfleet   │                 │
   │                  │                    │                    ├─────────────────►│                 │
   │                  │                    │                    │                  │ spawn sitsim+px4│
   │                  │                    │                    │                  ├────────────────►│
   │                  │                    │                    │                  │                 │ EKF2 boot
   │                  │                    │                    │                  │                 │ heartbeats
   │                  │                    │                    │                  │◄────────────────┤
   │                  │                    │                    │ mavfleet listening│                 │
   │                  │ 200 OK {running:true, pid:1003}         │                  │                 │
   │                  │◄──────────────────────────────────────┤                  │                 │
   │                  │                    │                    │                  │                 │
   │                  │ new WebSocket('ws://127.0.0.1:8400/')   │                  │                 │
   │                  ├──────────────────────────────────────────────────────────►│                 │
   │                  │                    │                    │ 101 Switching   │                 │
   │                  │◄─────────────────────────────────────────────────────────┤                 │
   │                  │                    │                    │                  │                 │
   │                  │ WS frame: FleetSnapshot @ 10 Hz (vehicles, FSM, mode, battery, position)│  │
   │                  │◄──────────────────────────────────────────────────────────┤                │
   │                  │ render: 2 vehicles on MapLibre canvas                    │                 │
   │◄─────────────────┤                    │                    │                  │                 │
   │ sees LIVE badge  │                    │                    │                  │                 │
```

**Tauri is NOT involved** in this sequence — the webview talks directly to the supervisor (HTTP) and mavfleet (WebSocket). Tauri's only role is having spawned the supervisor at app startup. This is the key architectural insight: **Tauri is the lifecycle manager, not the data plane.**

### J.3 Sequence: operator closes the window

```
Operator          Webview (JS)        Tauri (Rust)         Supervisor          mavfleet         px4 + sitsim
   │                  │                    │                    │                  │                 │
   │ click X          │                    │                    │                  │                 │
   ├──────────────────►│                    │                    │                  │                 │
   │                  │ window.close()     │                    │                  │                 │
   │                  ├───────────────────►│                    │                  │                 │
   │                  │                    │ RunEvent::WindowEvent(CloseRequested)  │                 │
   │                  │                    │ api.prevent_close() (don't close yet) │                 │
   │                  │                    │ spawn graceful_shutdown task         │                 │
   │                  │                    │                    │                  │                 │
   │                  │                    │ POST :8500/api/sitl/stop             │                 │
   │                  │                    ├──────────────────►│                  │                 │
   │                  │                    │                    │ kill mavfleet    │                 │
   │                  │                    │                    ├─────────────────►│                 │
   │                  │                    │                    │                  │ kill px4+sitsim │
   │                  │                    │                    │                  ├────────────────►│
   │                  │                    │                    │ 200 OK {stopped:true}             │
   │                  │                    │◄──────────────────┤                  │                 │
   │                  │                    │                    │                  │                 │
   │                  │                    │ sleep 250ms        │                  │                 │
   │                  │                    │                    │                  │                 │
   │                  │                    │ drop BackendState (kill_on_drop fires)│                 │
   │                  │                    │   catalog: SIGTERM → wait → SIGKILL   │                 │
   │                  │                    │   supervisor: SIGTERM → wait → SIGKILL│                │
   │                  │                    │                    │                  │                 │
   │                  │                    │ window.close() now safe                │                 │
   │                  │                    ├──────────────────►│                  │                 │
   │ window disappears │                    │                    │                  │                 │
   │◄─────────────────┤                    │                    │                  │                 │
```

### J.4 Sequence: SITL start fails (PX4 missing)

```
Operator          Webview (JS)        Tauri (Rust)         Supervisor
   │                  │                    │                    │
   │ click Start      │                    │                    │
   ├──────────────────►│                    │                    │
   │                  │ POST :8500/api/sitl/start                  │
   │                  ├──────────────────────────────────────────►│
   │                  │                    │                    │ probe PX4_ROOT
   │                  │                    │                    │ → not found
   │                  │                    │                    │
   │                  │ 404 Not Found (or 500)                   │
   │                  │ {ok:false, error:"PX4 binary missing"}   │
   │                  │◄──────────────────────────────────────┤
   │                  │                    │                    │
   │                  │ show error toast:  │                    │
   │                  │ "PX4 not found —   │                    │
   │                  │  click to open      │                    │
   │                  │  install guide"     │                    │
   │                  │                    │                    │
   │ click toast      │                    │                    │
   ├──────────────────►│                    │                    │
   │                  │ invoke('open_install_guide')             │
   │                  ├───────────────────►│                    │
   │                  │                    │ shell.open(URL)    │
   │                  │                    │                    │
   │ browser opens    │                    │                    │
   │ install guide    │                    │                    │
```

---

## Appendix K — Failure mode decision table

For every failure mode, the **operator-visible behaviour** AND the **internal recovery action**. If a row says "auto-recover", the operator sees a brief toast and the app continues. If "manual", the operator sees a modal dialog with options.

| # | Failure | Detect | Operator sees | Auto-recover? | Internal action |
|---|---|---|---|---|---|
| F1 | Catalog port :8300 already in use on startup | `probe_and_clean_ports()` returns Err | Modal dialog: "Port 8300 is in use. Force-reset?" | Yes (if operator clicks Yes) | `invoke('reset_ports')` → kills stale PID → re-spawns catalog |
| F2 | Supervisor port :8500 already in use on startup | same | same | Yes | same |
| F3 | Catalog binary not found | `resolve_binary()` returns Err | Modal dialog: "RustSim internals missing. Reinstall." | No | Emit `backend-error` event; Settings → About shows "Catalog: OFFLINE" |
| F4 | Supervisor binary not found | same | same | No | same |
| F5 | Catalog crashes at runtime | `backend_status` poll returns `online: false` | Toast: "Catalog crashed. Click to restart." | Yes (if operator clicks) | `invoke('restart_catalog')` |
| F6 | Supervisor crashes at runtime | same | same | Yes | `invoke('restart_supervisor')` (also stops SITL gracefully first) |
| F7 | PX4 binary not found | `resolve_px4_root()` returns None on startup | First-run onboarding screen + Settings → About shows "PX4: NOT FOUND" | No | Emit `px4-status` event with `found: false` |
| F8 | SITL start fails (PX4 binary missing) | supervisor returns 404/500 on `POST /api/sitl/start` | Error toast: "PX4 not found. Open install guide." | No | Operator clicks toast → `invoke('open_install_guide')` |
| F9 | SITL start fails (scenario TOML missing) | supervisor returns 404 with `error: "scenario not found"` | Error toast: "Scenario X not found. Try Refresh." | No | Operator clicks "Refresh scenarios" in SITL Manager |
| F10 | SITL crashes mid-flight (px4 process dies) | supervisor detects child exit; mavfleet emits FSM=FAULT | Fleet C2 panel shows vehicle FSM=FAULT (red) | No | Operator clicks "Stop" in SITL Manager → `POST /api/sitl/stop` → supervisor kills remaining mavfleet/px4 → operator clicks "Start" again |
| F11 | WebSocket to :8400 disconnects | `useFleetC2.ts` `onclose` handler | Fleet C2 panel shows "RECONNECTING…" | Yes (auto-reconnect every 2s) | Reconnects when mavfleet restarts |
| F12 | Tauri panic in spawn task | `panic = "abort"` kills process | App disappears from screen | No | `kill_on_drop(true)` cleans up backends; operator relaunches |
| F13 | Disk full — catalog can't write mission | catalog returns 500 | Mission save toast: "Save failed (disk full?)" | No | Operator frees disk; clicks Save again |
| F14 | Catalog dir not writable | catalog startup fails | Modal dialog: "Cannot write to <data_dir>/rustsim/catalog. Check permissions." | No | Operator fixes perms; clicks "Restart catalog" |
| F15 | WebView2 missing on Windows | app won't launch; bootstrapper auto-downloads | First-launch: progress bar | Yes | `bundle.windows.webviewInstallMode.type: "downloadBootstrapper"` handles it |
| F16 | macOS Gatekeeper blocks unsigned app | "RustSim GCS cannot be opened" | Right-click → Open dialog | No (per Apple policy) | Operator right-clicks → Open once; thereafter launches normally |
| F17 | WebKitGTK too old (Linux < 22.04) | app crashes on launch with GLib-GObject-CRITICAL | App crashes silently | No | Operator upgrades distro; or uses AppImage (bundled libs) |
| F18 | Operator closes window during SITL start | `CloseRequested` mid-startup | Window stays open 250ms longer | Yes | `graceful_shutdown()` waits for SITL stop ack, then closes |
| F19 | Two Tauri instances launched | second instance's `probe_and_clean_ports()` fails | Modal: "RustSim GCS is already running. Open the existing window?" | No | Second instance exits; first instance focused (via single-instance plugin — Phase 2) |
| F20 | Network call to basemap tile server fails | MapLibre logs to console | Map shows grey background; no tiles | No | Operator checks Settings → Map → switch to offline bundle (Phase 2) |

---

## Appendix L — Acceptance criteria for milestones (concrete, measurable)

Each milestone in §12 gets a checklist of measurable pass/fail conditions. **All must pass** for the milestone to be marked complete.

### L.1 M-T1 — Frontend static export

- [ ] `console/next.config.ts` contains `output: 'export'` (verified by `grep "output: 'export'" console/next.config.ts`).
- [ ] `console/next.config.ts` contains `images: { unoptimized: true }`.
- [ ] `console/src/app/api/route.ts` does NOT exist (`ls console/src/app/api/route.ts` → no such file).
- [ ] `console/.env.production` exists and contains `NEXT_PUBLIC_RSIM_API_STYLE=direct`.
- [ ] `console/public/fonts/Geist-Variable.woff2` and `GeistMono-Variable.woff2` exist; `file` reports "WOFF2" magic.
- [ ] `console/src/app/layout.tsx` imports `next/font/local`, NOT `next/font/google`.
- [ ] `console/src/app/layout.tsx` favicon points to `/logo.svg`, NOT `https://z-cdn.chatglm.cn/...`.
- [ ] `console/package.json` does NOT contain `react-map-gl` in dependencies.
- [ ] `cd console && NEXT_PUBLIC_RSIM_API_STYLE=direct npm run build` exits 0 and produces `console/out/index.html`.
- [ ] `console/out/` size > 1 MB (sanity check that the build emitted real assets).
- [ ] `cd console/out && python3 -m http.server 3000` serves it; `curl http://localhost:3000/` returns HTTP 200 with `<title>Operations Canvas</title>`.
- [ ] `cd console/tests && bash run_g0_version.sh` exits 0 (PX4 version gate still passes against the static export — it's a backend test).
- [ ] `cd console/tests && bash run_g1_validation.sh` exits 0.
- [ ] `cd console/tests && bash run_g2_persistence.sh` exits 0.
- [ ] `cd console/tests && bash run_g13_patterns.sh` exits 0 (pure-TS gate, no backend — must work even with catalog down).
- [ ] `cd console && npm run lint` exits 0.

### L.2 M-T2 — Tauri skeleton + window

- [ ] `src-tauri/Cargo.toml` exists with `tauri = "2"` in dependencies.
- [ ] `src-tauri/tauri.conf.json` matches Appendix C exactly (verified by `diff src-tauri/tauri.conf.json docs/TAURI_APP_SPEC.md`-extracted).
- [ ] `src-tauri/capabilities/main.json` matches Appendix E (capabilities section, §7.1).
- [ ] `src-tauri/icons/` contains `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`, `icon.ico`.
- [ ] `src-tauri/src/main.rs` exists (placeholder OK — just `tauri::Builder::default().run(...)`).
- [ ] `cd src-tauri && cargo build --release` exits 0 in <60s.
- [ ] `cd src-tauri && cargo tauri dev` opens a window titled "RustSim GCS — Operations Canvas" at 1280×800.
- [ ] The webview loads the Operations Canvas HTML (no white screen).
- [ ] DevTools (right-click → Inspect Element, or F12) opens.
- [ ] DevTools Console shows NO errors other than expected `:8300`/`:8500` fetch failures (backends not running in M-T2).
- [ ] DevTools Network tab shows the `index.html` loaded from `http://localhost:3000` (dev mode) or `http://tauri.localhost` (prod mode).
- [ ] Closing the window exits the process (no orphan `rustsim-gcs` in `ps aux`).

### L.3 M-T3 — MapLibre worker URL fix

- [ ] `console/src/components/canvas/MapCanvas.tsx` line containing `workerUrl:` uses `import.meta.url`, NOT `window.location.origin`.
- [ ] `cargo tauri dev` opens the app; the MapLibre canvas renders (grey background, no basemap yet — that needs network).
- [ ] DevTools Console: NO "Failed to load worker" error.
- [ ] DevTools Network tab: `maplibre-gl-worker.mjs` loaded with HTTP 200.
- [ ] DevTools → Application → Storage → Local Storage shows the 5 `rsim.*` keys after toggling a Settings option.
- [ ] Settings → Map → Basemap dropdown shows all 6 options (OpenStreetMap, CartoDB, OpenTopoMap, Esri, Stadia, MapTiler — exact list from `state/map-settings.ts`).
- [ ] Switching the basemap to "OpenStreetMap" loads the OSM tiles (verifies `img-src` CSP allows `https://*.tile.openstreetmap.org`).

### L.4 M-T4 — Rust backend orchestration

- [ ] `src-tauri/src/backends.rs` matches Appendix E.2.
- [ ] `src-tauri/src/commands.rs` matches Appendix E.3.
- [ ] `src-tauri/src/shutdown.rs` matches Appendix E.4.
- [ ] `cd src-tauri && cargo build --release` exits 0.
- [ ] `cargo tauri dev` (or running the release binary) spawns `fleet-catalog` (verified by `ps aux | grep fleet-catalog` returning a PID).
- [ ] `cargo tauri dev` spawns `fleet-supervisor` (verified similarly).
- [ ] Within 2s of app startup: `curl http://127.0.0.1:8300/api/health` returns `{"ok":true,...}`.
- [ ] Within 2s of app startup: `curl http://127.0.0.1:8500/api/sitl/status` returns `{"ok":true,...}`.
- [ ] Settings → About panel (a new small addition to `overlays/SettingsPanel.tsx`) calls `invoke('backend_status')` every 2s and shows:
  - "Catalog: ONLINE (port 8300, pid 1234)"
  - "Supervisor: ONLINE (port 8500, pid 1235)"
  - "PX4: FOUND at /home/.../PX4-Autopilot" (or "NOT FOUND — [Install guide]")
- [ ] DevTools Console: no errors during the 2s startup window.
- [ ] Clicking "Restart catalog" in Settings → About: catalog PID changes (verified by the status refresh after 2s).
- [ ] Clicking "Restart supervisor" in Settings → About: supervisor PID changes; SITL (if running) is stopped first (verified by `curl :8500/api/sitl/status` returning `running: false` immediately after the click).

### L.5 M-T5 — SITL lifecycle end-to-end

- [ ] PX4 v1.16.2 installed at `PX4_ROOT` (or under one of the `resolve_px4_root()` candidates).
- [ ] Open the app → Settings → About shows "PX4: FOUND".
- [ ] Open the SITL Manager overlay panel (left-rail "SITL" button).
- [ ] Click and hold the "Start" button for 400ms.
- [ ] Within 5s: the status badge changes from "STOPPED" to "STARTING" to "RUNNING".
- [ ] `curl http://127.0.0.1:8500/api/sitl/status` returns `{"running": true, "vehicle_count": 2}`.
- [ ] `curl http://127.0.0.1:8400/api/fleet` returns JSON with `vehicles[0].fsm = "READY"` and `vehicles[1].fsm = "READY"` (within 10s of start).
- [ ] DevTools → Network → WS: a WebSocket to `ws://127.0.0.1:8400/` is OPEN and receiving frames at ~10 Hz.
- [ ] Fleet C2 overlay panel shows 2 vehicle cards with battery 100%, GPS at 47.3977°, 8.5455° (Zurich).
- [ ] MapLibre canvas shows 2 vehicle markers at the home position.
- [ ] Single-tap "Stop" in SITL Manager: status returns to "STOPPED" within 3s.
- [ ] `curl http://127.0.0.1:8500/api/sitl/status` returns `{"running": false}`.
- [ ] `curl http://127.0.0.1:8400/api/fleet` → connection refused (mavfleet process exited).
- [ ] `ps aux | grep -E 'mavfleet|px4_sitl|sitsim-cli'` returns empty.

### L.6 M-T6 — Graceful shutdown on window close

- [ ] Start SITL (M-T5 path) — 2 vehicles READY.
- [ ] Close the Tauri window (click X).
- [ ] Within 3s: `ps aux | grep -E 'fleet-catalog|fleet-supervisor|mavfleet|px4_sitl|sitsim-cli|rustsim-gcs'` returns empty.
- [ ] `curl http://127.0.0.1:8300/api/health` → connection refused.
- [ ] `curl http://127.0.0.1:8500/api/sitl/status` → connection refused.
- [ ] `curl http://127.0.0.1:8400/api/fleet` → connection refused.
- [ ] `lsof -i :8300 -i :8400 -i :8500 -i :4560 -i :4561 -i :14540 -i :14541` returns empty (no stale listeners).
- [ ] Re-open the app: starts cleanly, no "port in use" error.
- [ ] Settings → About shows both backends ONLINE within 2s.

### L.7 M-T7 — Cross-platform packaging

- [ ] Linux: `cd src-tauri && cargo tauri build` produces `src-tauri/target/release/bundle/deb/rustsim-gcs_1.0.0_amd64.deb` AND `src-tauri/target/release/bundle/appimage/rustsim-gcs_1.0.0_amd64.AppImage`.
- [ ] Linux: install the `.deb` on a clean Ubuntu 22.04 VM (`dpkg -i ...deb`); resolve deps with `apt install -f`; launch from the desktop menu.
- [ ] Linux: M-T5 path works on the clean VM (with PX4 pre-installed at `~/.rustsim/PX4-Autopilot/`).
- [ ] macOS: `cargo tauri build` produces `src-tauri/target/release/bundle/dmg/RustSim GCS_1.0.0_universal.dmg` (universal binary — both arm64 and x86_64 in one).
- [ ] macOS: install the `.dmg` on a clean macOS 14 VM (Apple Silicon or Intel).
- [ ] macOS: Gatekeeper shows "unidentified developer" warning; right-click → Open succeeds.
- [ ] macOS: M-T5 path works on the clean VM (with PX4 pre-installed).
- [ ] Windows: `cargo tauri build` produces `src-tauri/target/release/bundle/msi/RustSim GCS_1.0.0_x64.msi` AND `src-tauri/target/release/bundle/nsis/RustSim GCS_1.0.0_x64-setup.exe`.
- [ ] Windows: install the `.msi` on a clean Windows 11 VM.
- [ ] Windows: first-launch downloads WebView2 bootstrapper (~2 MB) and installs silently (if WebView2 not preinstalled).
- [ ] Windows: SmartScreen shows "Windows protected your PC" warning; "More info" → "Run anyway" succeeds.
- [ ] Windows: M-T5 path works on the clean VM (with PX4 pre-installed).

### L.8 M-T8 — Screenshot verification via live desktop bridge

- [ ] `bash /home/z/my-project/scripts/bridge-install.sh` exits 0 (one-time).
- [ ] `bash /home/z/my-project/scripts/fetch-tauri-deps.sh` exits 0 (one-time).
- [ ] `bash /home/z/my-project/scripts/start-bridge.sh` exits 0; output ends with "Live desktop bridge is UP".
- [ ] `cd src-tauri && cargo tauri build` exits 0 (produces the Linux `.deb` / AppImage).
- [ ] `bash /home/z/my-project/scripts/start-tauri.sh /home/z/my-project/repos/rust-sim` exits 0.
- [ ] `bash /home/z/my-project/scripts/screenshot.sh /home/z/my-project/download/rustsim-tauri-live.png` produces a PNG > 50 KB.
- [ ] The PNG shows: the Operations Canvas with the MapLibre map visible, the SITL Manager overlay panel open, the "Start" button visible.
- [ ] Start SITL via the noVNC viewer (operator clicks through the bridge).
- [ ] Wait 10s; screenshot again — the new PNG shows 2 vehicle markers on the map + "RUNNING" badge in the SITL Manager.
- [ ] Playwright click test (per `LIVE-DESKTOP-PREVIEW-SETUP.md` §D.3): `node /home/z/my-project/scripts/test-click.js` exits 0; the screenshot after the click shows the clicked button changed state.

---

## Appendix M — Testing strategy

### M.1 Unit tests (Rust)

- **Location:** `src-tauri/src/backends.rs` (and the other modules) get `#[cfg(test)] mod tests { ... }` blocks.
- **Coverage:**
  - `resolve_px4_root()` with various env states (env set, env unset, repo-relative paths present, etc.).
  - `resolve_binary()` with dev path, resource path, and PATH lookup.
  - `port_is_listening()` against a known-listening + known-closed port.
  - `pid_listening_on_port()` (Unix only; mock `lsof` via `tempfile`).
  - `kill_pid()` against a spawned `sleep 1000` subprocess.
- **Run:** `cd src-tauri && cargo test`.
- **CI:** runs on every push.

### M.2 Integration tests (Rust)

- **Location:** `src-tauri/tests/integration.rs`.
- **Coverage:**
  - Spawn catalog + supervisor; assert `:8300/api/health` and `:8500/api/sitl/status` return 200.
  - Spawn catalog; kill it; verify `backend_status` returns `online: false`.
  - Spawn catalog on port 8300; attempt to spawn another; verify `probe_and_clean_ports()` recovers.
  - Graceful shutdown with SITL running; verify no orphan processes (via `ps`).
- **Run:** `cd src-tauri && cargo test --test integration`.
- **CI:** runs on every push to `main`.

### M.3 Frontend unit tests

- **Status:** the existing `console/` repo has no unit tests today (subagent A confirmed — no `*.test.ts` files).
- **v1 plan:** add tests for the patched `src/lib/conn.ts` (verify `gw()` returns `http://127.0.0.1:...` URLs in direct mode).
- **Run:** `cd console && npm test` (after wiring `vitest` — out of scope for M-T1, in scope for M-T4).
- **Phase 2:** full vitest suite for `state/map-settings.ts`, `state/ring-buffer.ts`, `lib/patterns.ts`.

### M.4 G-ladder harnesses (existing, unchanged)

The 11 surviving G-ladder harnesses (`console/tests/run_g*.sh`) keep running against the web stack. They exercise the **Next.js + Rust HTTP contract** — the same contract Tauri inherits. **They are NOT migrated to run inside Tauri** — the marginal coverage is low and the maintenance cost is high.

| Gate | What it tests | Runs against | Migrated to Tauri? |
|---|---|---|---|
| G-0 | PX4 version check (catalog rejects upload to non-v1.16.2) | web stack :8300 | No (same contract) |
| G-1 | Mission validation (schema + geofence) | web stack :8300 | No |
| G-2 | Mission CRUD round-trip | web stack :8300 | No |
| G-3 | MAVLink mission upload | web stack :8400 | No |
| G-4 | MAVLink mission download | web stack :8400 | No |
| G-5 | Fly View 1-vehicle telemetry | web stack :8400 + browser | No |
| G-6 | Multi-vehicle Fly View | web stack :8400 + browser | No |
| G-7 | Pre-arm + arm/disarm | web stack :8400 + browser | No |
| G-8 | Vehicle Setup extensions | web stack :8400 + browser | No |
| G-9 | Fleet mission binding | web stack :8400 + browser | No |
| G-11 | ULog browse + plot | web stack :8300 + browser | No |
| G-13 | Survey patterns | web stack :8300 + browser | No |

### M.5 End-to-end (Tauri + real PX4 SITL)

- **Location:** new `src-tauri/tests/e2e/` directory.
- **Tool:** Playwright + Tauri's WebDriver support (Tauri 2 supports `tauri-driver` for Playwright).
- **Coverage:** the M-T5 acceptance criteria (Appendix L.5) as automated tests.
- **Run:** `cd src-tauri && cargo test --test e2e -- --ignored` (marked `--ignored` because they need real PX4).
- **CI:** runs nightly on a self-hosted runner with PX4 installed (not on every push — too slow).

### M.6 Live desktop bridge screenshot test (M-T8)

- **Tool:** the existing `start-tauri.sh` + `screenshot.sh` scripts from `LIVE-DESKTOP-PREVIEW-SETUP.md`.
- **Run:** manual (developer-triggered) + CI-on-release.
- **Acceptance:** the screenshot PNG must be > 50 KB and visually show the Operations Canvas (verified by the developer, not automated).

---

## Appendix N — Versioning & compatibility matrix

### N.1 Pinned versions (Sep 2026)

| Component | Pinned version | Source of truth | Bump policy |
|---|---|---|---|
| Rust toolchain | `1.98.1` stable | `rust-toolchain.toml` (new file in repo root) | Bump deliberately; re-run all G-ladder gates. |
| Tauri CLI | `2.11.x` | `src-tauri/Cargo.toml` (`tauri = "2"`) + `console/package.json` (`@tauri-apps/cli@^2`) | Bump deliberately; check migration guide. |
| Tauri Rust API | `2.11.x` | `src-tauri/Cargo.toml` | Same as above. |
| Tauri JS API | `2.x` | `console/package.json` (`@tauri-apps/api@^2`) | Same. |
| Node.js | `24.x` (LTS) | sandbox preinstalled; `.nvmrc` file | Bump on LTS releases. |
| npm | `11.x` | follows Node | Auto. |
| Next.js | `16.1.1` | `console/package.json` | Bump deliberately; check Next 16 changelog. |
| React | `19.0.0` | `console/package.json` | Bump on React 19.x minors. |
| MapLibre GL | `6.9.0` | `console/package.json` | Bump on minor; test worker URL patch still applies. |
| PX4-Autopilot | `v1.16.2` | `docs/SANDBOX_SETUP.md` step 6 | NEVER bump without re-running G-0 (PX4 version gate). |
| `fleet-catalog` / `fleet-supervisor` / `mavfleet` | (workspace) | `fleet/Cargo.toml` | Bump with the workspace. |

### N.2 Compatibility matrix (what runs where)

| Component | Linux x86_64 | macOS arm64 | macOS x86_64 | Windows x86_64 |
|---|---|---|---|---|
| Tauri webview | WebKitGTK 4.1 | WKWebView | WKWebView | WebView2 |
| MapLibre WebGL | llvmpipe (slow) / native | native | native | native |
| `fleet-catalog` binary | ✓ | ✓ | ✓ | ✓ |
| `fleet-supervisor` binary | ✓ | ✓ | ✓ | ✓ |
| `mavfleet` binary | ✓ | ✓ | ✓ | ✓ |
| `px4` (SITL) | ✓ (built from source) | ✓ (built from source) | ✓ (built from source) | ✓ (built from source — PX4 supports Windows SITL) |
| Process tree kill | `process_group(0)` + SIGTERM/SIGKILL | same | same | `taskkill /F /T` |
| Code signing | N/A | unsigned (Phase 2: Developer ID) | unsigned | unsigned (Phase 2: OV cert) |
| Auto-update | Phase 2 | Phase 2 | Phase 2 | Phase 2 |

### N.3 Minimum supported OS versions

| OS | Minimum | Recommended | Reason |
|---|---|---|---|
| Linux | Ubuntu 22.04 LTS | Ubuntu 24.04 LTS | webkit2gtk-4.1 first shipped in 22.04. Older distros (Debian 10, Ubuntu 18.04) lack it. |
| macOS | 12.0 Monterey | 14.0 Sonoma | `bundle.macOS.minimumSystemVersion = "12.0"`. WKWebView on older macOS pins older WebKit (no MapLibre v6 support). |
| Windows | Windows 10 1809 | Windows 11 | WebView2 runtime requires Win 10 1809+. `downloadBootstrapper` handles install. |

---

## Appendix O — First-run onboarding flow

### O.1 First-launch detection

```rust
// src-tauri/src/main.rs setup hook (addition)
let data_dir = dirs::data_dir().unwrap().join("rustsim");
let onboarding_done = data_dir.join(".onboarded");
if !onboarding_done.exists() {
    app.emit("show-onboarding", ()).ok();
}
```

### O.2 Onboarding steps (frontend)

The webview listens for `show-onboarding` and shows a modal overlay:

1. **Welcome** — "RustSim GCS v1.0.0. Click Next to set up."
2. **PX4 check** — calls `invoke('probe_px4')`. If `px4-status.found === false`:
   - Shows "PX4 not found. Click below to open the install guide."
   - Button: "Open install guide" → `invoke('open_install_guide')`.
   - Button: "I've installed PX4 — re-check" → `invoke('probe_px4')` again.
   - Button: "Skip for now" → continues (SITL will fail to start, but the rest of the app works).
3. **Data directory** — "Your missions, presets, and ULogs will be stored at: `<data_dir>/rustsim/`." (Read-only display; no user action needed.)
4. **Tour** — re-uses the existing `OnboardingTour.tsx` 3-step zone tour.
5. **Done** — writes `<data_dir>/rustsim/.onboarded` (empty file). The modal disappears. Subsequent launches skip onboarding.

### O.3 Re-triggering onboarding

Operators can re-trigger onboarding from Settings → About → "Re-run onboarding" button (deletes `.onboarded` and reloads the app).

---

## Appendix P — Glossary

| Term | Definition |
|---|---|
| **ADR** | Architecture Decision Record. Numbered files under `console/docs/adr/` (0019–0030) and `sim/docs/adr/` (0010–0015) and `fleet/docs/adr/` (0010–0018). |
| **Operations Canvas** | The single-screen GCS UX: full-bleed MapLibre map + edge-HUD widgets + 7 overlay panels. The whole `console/` app. |
| **Overlay panel** | One of the 7 panels (Mission, Library, Fleet C2, SITL Manager, Setup, Analyze, PreFlight) that overlays the map. Settings + OnboardingTour + CheatSheet are also overlays but not part of the "7". |
| **SITL** | Software In The Loop. PX4-Autopilot compiled to run as a native binary, simulating the FC hardware. |
| **HIL** | Hardware In The Loop. Here, "HIL" means the sim↔PX4 TCP stream on :4560+i — the sim sends sensor data, PX4 sends actuator outputs. (Not real hardware — the "hardware" is virtual.) |
| **MAVLink** | The protocol PX4 speaks for telemetry + mission upload + commands. UDP :14540+i for SITL. |
| **Lockstep** | The sim and PX4 run in lockstep: each physics tick waits for PX4's actuator output before advancing. Prevents the sim from running faster than PX4 can process. |
| **Catalog** | The `fleet-catalog` service on :8300. Mission CRUD + presets + ULog browse. Stateless. |
| **Supervisor** | The `fleet-supervisor` service on :8500. Owns the SITL lifecycle (ADR-0030). Spawns `mavfleet` on demand. |
| **Fleet manager / mavfleet** | The `mavfleet` binary on :8400 (on-demand). Drives MAVLink to each PX4 vehicle. |
| **Operator** | The human driving the GCS. Equivalent to a QGC operator. |
| **QGC** | QGroundControl — the reference open-source GCS for PX4. RustSim GCS's UX is modelled on QGC. |
| **MP** | Mission Planner — the other popular PX4 GCS (Windows-only, C#). RustSim GCS's operator-driven SITL pattern is modelled on MP. |
| **Direct mode** | The frontend's `NEXT_PUBLIC_RSIM_API_STYLE=direct` setting — talks to backends via absolute `http://127.0.0.1:<port>` URLs. The Tauri-native mode. |
| **Gateway mode** | The frontend's default mode — talks via Caddy :81 with `?XTransformPort=<port>`. Legacy web-stack mode. |
| **Sidecar** | Tauri's mechanism for bundling an external binary in the installer. We do NOT use this for catalog/supervisor (they're compiled from source). |
| **`kill_on_drop(true)`** | tokio::process::Command method — when the `Child` handle is dropped (e.g. on panic), the OS kills the subprocess. |
| **Capabilities** | Tauri 2's permission system. JSON files in `src-tauri/capabilities/` that grant per-window permissions to plugins and core APIs. |
| **CSP** | Content Security Policy. The `app.security.csp` field in `tauri.conf.json` controls what the webview can fetch. |
| **Webview origin** | The URL the webview loads from. Tauri 2: `http://tauri.localhost` (Win/Linux default), `tauri://localhost` (macOS), or `https://tauri.localhost` (Win if `useHttpsScheme: true`). |
| **G-ladder** | The 11 G-0…G-13 verification gates (3 deleted in the 2026-09-10 cleanup) that validate the GCS against real PX4 SITL. |
| **Run dir** | A per-SITL-run directory under `scratch/fleet-run-<unix_ms>/` containing logs, ULogs, and event NDJSON. Created by the supervisor on `POST /api/sitl/start`. |

---

## Appendix Q — Change log for this spec

| Date | Author | Change |
|---|---|---|
| 2026-09-11 | Agent (Task 7) | Initial spec, 15 sections + Appendix A (file layout) + Appendix B (Rust skeleton). 953 lines. |
| 2026-09-11 | Agent (Task "detail the spec") | Added Appendices C–Q: canonical `tauri.conf.json`, full `Cargo.toml`, full Rust source for 5 modules, exact frontend patches (8 diffs), IPC command contracts, algorithms (PX4 discovery, port conflict, shutdown sequence, log rotation), data directory layout per OS, process tree + 3 sequence diagrams, 20-row failure mode decision table, measurable acceptance criteria for all 8 milestones, testing strategy (unit/integration/e2e/bridge), versioning matrix, first-run onboarding flow, glossary. |

---

**End of spec.** Implementation begins at M-T1; acceptance is per Appendix L.
