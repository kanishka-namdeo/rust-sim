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

**End of spec.** Implementation begins at M-T1.
