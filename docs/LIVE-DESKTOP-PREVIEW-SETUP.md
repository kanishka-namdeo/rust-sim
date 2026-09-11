# Live Interactive Desktop Preview in the Sandbox

This document is the **agent-facing playbook** for bringing up a live, interactive preview of an arbitrary Electron or Tauri app inside this headless Linux sandbox, streamed to the user's browser through the preview panel.

The sandbox is a Debian 13 (trixie) container, no GPU, no sudo, no display. The user's only window into it is a Caddy reverse proxy on port 81 that forwards `/` to `localhost:3000`. Everything below works within those constraints.

---

## TL;DR for agents

```bash
# One-time install (idempotent, ~2 min)
bash /home/z/my-project/scripts/bridge-install.sh
# If using Tauri, also:
bash /home/z/my-project/scripts/fetch-tauri-deps.sh

# Per-session: bring up the bridge (Xvfb + x11vnc + websockify + twm)
bash /home/z/my-project/scripts/start-bridge.sh

# Launch any Electron app on the bridge
bash /home/z/my-project/scripts/start-electron.sh /path/to/electron-app

# Or any Tauri app (after `cargo tauri build`)
bash /home/z/my-project/scripts/start-tauri.sh /path/to/tauri-app

# Screenshot to verify what's visible
bash /home/z/my-project/scripts/screenshot.sh /home/z/my-project/download/preview.png

# Status / health check
bash /home/z/my-project/scripts/status-bridge.sh

# Tear down
bash /home/z/my-project/scripts/stop-bridge.sh
```

Tell the user to open the preview panel (or click "Open in New Tab"). They'll see the live app and can interact with it through the noVNC viewer.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│  User's browser (preview panel / "Open in New Tab")                       │
│    │                                                                       │
│    │  HTTP + WebSocket                                                     │
│    ▼                                                                       │
│  Caddy gateway  (port 81, root process, config at /app/Caddyfile)         │
│    │  reverse proxy → localhost:3000                                       │
│    ▼                                                                       │
│  websockify + noVNC  (port 3000, static files + WS bridge)                 │
│    │  WebSocket → TCP                                                      │
│    ▼                                                                       │
│  x11vnc            (port 5900, VNC server)                                │
│    │  VNC over TCP                                                         │
│    ▼                                                                       │
│  Xvfb             (display :99, virtual 1280×800×24 framebuffer)          │
│    │  X11 protocol                                                         │
│    ▼                                                                       │
│  twm (window manager — required for click routing to app windows)        │
│    │                                                                       │
│    ▼                                                                       │
│  Electron / Tauri app  (renders into Xvfb)                                │
└──────────────────────────────────────────────────────────────────────────┘
```

The user's mouse and keyboard travel the opposite direction: browser → noVNC JS → WebSocket → websockify → x11vnc → Xvfb → twm → app. End-to-end latency is typically under 300ms.

---

## Decision tree: what does the user have?

```
Is the user's app an Electron app (has package.json with "electron" in deps)?
├── YES → Use start-electron.sh after `npm install` + electron postinstall
│
Is it a Tauri app (has src-tauri/tauri.conf.json)?
├── YES → Use start-tauri.sh after `tauri build`
│
Is it a plain HTML/web app that just needs a browser preview?
├── YES → Use the fullstack-dev skill (Next.js on port 3000) — this doc is NOT needed
│
Is it some other X11 / GTK / Qt / SDL app?
├── YES → Use start-bridge.sh, then launch the app with DISPLAY=:99
```

For Tauri/Electron apps that use a JS dev server (Vite, webpack-dev-server, Next.js dev), the user usually wants the production-built binary, not the dev server. Production builds render through the system WebView / Chromium, which is what end users see. If the user specifically wants dev hot-reload, see "Dev mode with HMR" below.

---

## Conventions

- All deliverables and persisted files live under `/home/z/my-project/`.
- All locally-extracted system packages live under `~/local/` (i.e. `/home/z/local/`).
- All orchestration scripts live under `/home/z/my-project/scripts/`.
- All scripts accept their target app dir as the first argument and are idempotent.
- All logs go to `/tmp/bridge-logs/<service>.log`.

---

## Part A — One-time install (run once per sandbox)

### A.1 — Bridge install (required for both Electron and Tauri)

```bash
bash /home/z/my-project/scripts/bridge-install.sh
```

Installs (all via `apt-get download` + `dpkg-deb -x` to `~/local/`, no sudo):
- **x11vnc** + its ~17 runtime libs (libvncserver, libvncclient, libxtst, libxdamage, …)
- **websockify** (via pip into the user venv)
- **noVNC v1.5.0** (static HTML/JS at `~/local/novnc/`)
- **`~/local/novnc/index.html`** — redirect page so the preview container's iframe at `/` loads the noVNC viewer instead of a directory listing
- **Screenshot tooling**: `xwd` (x11-apps), `xwdtopnm` + `pnmtopng` (netpbm + libnetpbm11t64 + libpng16-16t64)
- **Window management**: `twm` (required for click routing to app windows)
- **Debugging**: `xdotool`, `xprop`, `xwininfo`
- **LD_PRELOAD interceptor** at `~/local/interceptors/exec_redirect.so` (needed only for Tauri — redirects WebKitGTK's hardcoded `/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/` helper path to `~/local/...`)

Idempotent — safe to re-run.

### A.2 — Tauri system deps (only if you'll use Tauri)

```bash
bash /home/z/my-project/scripts/fetch-tauri-deps.sh
```

Downloads and extracts ~40 packages: `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libsoup-3.0-dev`, `librsvg2-dev`, `libcairo2-dev`, `libpango1.0-dev`, `libatk1.0-dev`, `libatk-bridge2.0-dev`, `libglib2.0-dev`, `libharfbuzz-dev`, `libfreetype-dev`, `libfontconfig1-dev`, `libwayland-dev`, `libxkbcommon-dev`, `libegl-dev`, `libgles-dev`, `libgl-dev`, `libepoxy-dev`, `libgbm-dev`, `libdrm-dev`, `libhyphen-dev`, `libmanette-0.2-dev`, `libnotify-dev`, `libseccomp-dev`, `libwebp-dev`, `liblcms2-dev`, `libtiff-dev`, `libpng-dev`, `libjpeg-dev`, `libxml2-dev`, `libsqlite3-dev`, `pkg-config`, `libgcc-14-dev`, `libstdc++-14-dev`, plus runtime deps (`libwebkit2gtk-4.1-0`, `at-spi2-core`, `libatspi2.0-0t64`, `libatspi2.0-dev`, `libmanette-0.2-0`, `libenchant-2-2`, `libsecret-1-0`, `libgudev-1.0-0`, `libhidapi-hidraw0`, `libevdev2`, `libegl1`, `libegl-mesa0`, `libgl1`, `libglx-mesa0`, `mesa-libgallium`, `libtalloc2`).

Also creates a `rsvg-2.0.pc` → `librsvg-2.0.pc` symlink in `~/local/usr/lib/x86_64-linux-gnu/pkgconfig/` (Tauri asks for `rsvg-2.0` but the package provides `librsvg-2.0`).

### A.3 — Rust toolchain (only if you'll use Tauri)

```bash
curl -fsSL --proto '=https' --tlsv1.2 -o /tmp/rustup-init.sh https://sh.rustup.rs
bash /tmp/rustup-init.sh -y --default-toolchain stable --profile minimal
source ~/.cargo/env
```

Installs into `~/.cargo/` and `~/.rustup/`. No sudo.

### A.4 — Tauri CLI (do NOT use `cargo install tauri-cli`)

`cargo install tauri-cli` will repeatedly fail in this sandbox — the `rustc` process gets killed after compiling only a few crates (likely a kernel watchdog or memory cgroup limit). Use the prebuilt binary from npm instead:

```bash
# In your Tauri app's directory:
npm install --no-audit --no-fund @tauri-apps/cli@^2 @tauri-apps/api@^2
./node_modules/.bin/tauri --version   # → tauri-cli 2.11.4
```

The native binary lives at `node_modules/@tauri-apps/cli-linux-x64-gnu/cli.linux-x64-gnu.node` (~18 MB, prebuilt, no compilation).

---

## Part B — Per-session usage

### B.1 — Start the bridge

```bash
bash /home/z/my-project/scripts/start-bridge.sh
```

Brings up Xvfb :99 + x11vnc :5900 + websockify :3000 + twm. Output:

```
Live desktop bridge is UP.
Display:    :99  (1280x800x24)
VNC port:    5900
HTTP port:   3000  (noVNC viewer + WS bridge)
Gateway URL: http://localhost:81/   (auto-redirects to noVNC viewer)
```

### B.2 — Launch the app

#### Electron (any repo)

```bash
bash /home/z/my-project/scripts/start-electron.sh /path/to/electron-app
```

Prerequisites (the script checks and tells you if missing):
- `node_modules/electron` installed (`npm install --no-audit --no-fund`)
- Electron's postinstall ran (`node node_modules/electron/install.js`)
- Bridge is running

The script:
- Kills any previously-launched Electron + Tauri on the bridge
- Sets `DISPLAY=:99`, `LD_LIBRARY_PATH`, `PATH`
- Launches with `--no-sandbox --disable-gpu --disable-dev-shm-usage --disable-software-rasterizer` (required for headless container)
- Optionally accepts extra Electron args after the app dir, e.g. `--inspect=9229`

#### Tauri (any repo, after `cargo tauri build`)

```bash
bash /home/z/my-project/scripts/start-tauri.sh /path/to/tauri-app
```

Or if the binary has a non-default name:

```bash
bash /home/z/my-project/scripts/start-tauri.sh /path/to/tauri-app src-tauri/target/release/custom-name
```

Prerequisites (the script checks):
- `src-tauri/tauri.conf.json` exists (confirms it's a Tauri app)
- Binary at `src-tauri/target/release/<productName>` exists (from `tauri build`)
- LD_PRELOAD interceptor at `~/local/interceptors/exec_redirect.so` (from `bridge-install.sh`)
- Bridge is running

The script:
- Reads `productName` from `tauri.conf.json` (uses it verbatim, NOT converted to underscores)
- Kills any previously-launched Tauri + Electron on the bridge
- Ensures `twm` is running (without it, clicks don't reach Tauri windows)
- Sets all the env vars Tauri+WebKitGTK need:
  - `LD_PRELOAD=~/local/interceptors/exec_redirect.so` — so WebKit can find its helper processes
  - `WEBKIT_HELPER_DIR=~/local/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1` — where the interceptor redirects to
  - `LIBGL_DRIVERS_PATH`, `LIBGL_ALWAYS_SOFTWARE=1`, `GALLIUM_DRIVER=llvmpipe` — software rendering via llvmpipe
  - `__EGL_VENDOR_LIBRARY_FILENAMES=~/local/usr/share/glvnd/egl_vendor.d/50_mesa.json` — point libEGL at Mesa
  - `WEBKIT_DISABLE_COMPOSITING_MODE=1`, `WEBKIT_DISABLE_DMABUF_RENDERER=1` — disable GPU/DMABuf paths that don't work in a container
  - `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` — newer WebKitGTK 2.52 rejects the deprecated `WEBKIT_FORCE_SANDBOX=0`
  - `GDK_BACKEND=x11` — force X11 (not Wayland)
- Launches via `setsid`

### B.3 — Verify

```bash
# Take a screenshot to see what's on the virtual desktop
bash /home/z/my-project/scripts/screenshot.sh /home/z/my-project/download/preview.png

# Health check (processes + endpoints)
bash /home/z/my-project/scripts/status-bridge.sh

# Tell the user to open the preview panel
```

The user can interact with the live app through the noVNC viewer. Mouse and keyboard events travel: browser → noVNC JS → WebSocket → websockify → x11vnc → Xvfb → twm → app.

### B.4 — Tear down

```bash
bash /home/z/my-project/scripts/stop-bridge.sh
```

Kills Xvfb, x11vnc, websockify, twm, and any app launched on the bridge.

---

## Part C — Adapting existing apps

### C.1 — Existing Electron app

Most existing Electron apps work as-is. The only hard requirement is that the app's `node_modules/electron` is installed (the `electron` binary). Common patterns:

```bash
# Clone or use existing repo
cd /path/to/existing-electron-app

# If package.json has "electron" in devDependencies:
npm install --no-audit --no-fund
node node_modules/electron/install.js   # force the postinstall

# Launch on the bridge
bash /home/z/my-project/scripts/start-electron.sh /path/to/existing-electron-app
```

**Common pitfalls with existing Electron apps:**

- **Native modules** (e.g. `better-sqlite3`, `node-pty`, `sharp`): may need `npm rebuild` against the Electron version, not Node. If the app crashes on startup with "Module did not self-register", rebuild: `npx electron-rebuild` or `npm rebuild --runtime=electron --target=33.0.0`.
- **App that opens multiple windows**: the script doesn't restrict this; all windows appear on the virtual desktop. The preview is the whole desktop, not a single window.
- **Apps with `--inspect` / DevTools**: pass `--inspect=9229` as an extra arg. DevTools will appear as another window on the same desktop.
- **Apps that require specific runtime flags** (`--disable-dev-shm-usage` etc.): the script already adds the container-required flags. To add more, pass them after the app dir.
- **Apps that exit when their main window closes** (`app.on('window-all-closed', () => app.quit())`): the app will exit if the user closes the window in the preview. Use `start-electron.sh` again to relaunch.
- **Apps with auto-update**: the auto-updater will fail to write to its install dir (no sudo). Disable auto-update in the user's code or via env var before launch.
- **Apps that need GPU/WebGL**: software rendering via llvmpipe works but is slow. WebGL apps will render at ~5-10 fps.

### C.2 — Existing Tauri app

Tauri apps need to be built first (`tauri build`), then launched on the bridge.

```bash
cd /path/to/existing-tauri-app

# Install Tauri CLI + API if not already there
npm install --no-audit --no-fund @tauri-apps/cli@^2 @tauri-apps/api@^2

# Set build env
export PATH=~/local/usr/bin:~/.cargo/bin:$PATH
export LD_LIBRARY_PATH=~/local/usr/lib/x86_64-linux-gnu:~/local/usr/lib:$LD_LIBRARY_PATH
export PKG_CONFIG_PATH=~/local/usr/lib/x86_64-linux-gnu/pkgconfig:~/local/usr/share/pkgconfig
export CARGO_TARGET_DIR=/path/to/existing-tauri-app/src-tauri/target
export RUSTFLAGS="-C link-arg=-Wl,-rpath,$HOME/local/usr/lib/x86_64-linux-gnu \
                  -Clink-arg=-L$HOME/local/usr/lib/x86_64-linux-gnu \
                  -Clink-arg=-L$HOME/local/usr/lib"
export CXXFLAGS="-L$HOME/local/usr/lib/x86_64-linux-gnu"
export CFLAGS="-L$HOME/local/usr/lib/x86_64-linux-gnu"
export LDFLAGS="-L$HOME/local/usr/lib/x86_64-linux-gnu"

# Build
./node_modules/.bin/tauri build

# Launch on the bridge
bash /home/z/my-project/scripts/start-tauri.sh /path/to/existing-tauri-app
```

**Common pitfalls with existing Tauri apps:**

- **`tauri.conf.json` with `frontendDist` pointing at a Vite/webpack dev server URL**: change it to a static path for production builds. The bridge doesn't run a JS dev server.
- **App uses `withGlobalTauri: false`** (the Tauri 2 default): the frontend must `import { invoke } from '@tauri-apps/api/core'`. If you want `window.__TAURI__` available instead (for apps that use it), set `"withGlobalTauri": true` in `tauri.conf.json` and rebuild.
- **No `capabilities/main.json`**: IPC commands are denied without a capability. Create `src-tauri/capabilities/main.json` with `core:default` permission for the main window.
- **Custom plugins**: each plugin needs its own capability entry. See the plugin's docs.
- **App expects a specific window size**: edit the `windows[0]` config in `tauri.conf.json` to match `1280x800` (the Xvfb size), or change the `start-bridge.sh` args to a different size.
- **App uses `tauri-plugin-*` plugins that need system services** (e.g. `tauri-plugin-notification` needs `libnotify`, `tauri-plugin-store` is fine): the deps may need additional `apt-get download` extractions. Check `ldd src-tauri/target/release/<binary>` for "not found" libraries.
- **App expects GPU**: same as Electron — llvmpipe software rendering works but is slow.
- **App needs `webkit2gtk-4.0`** (Tauri 1.x): not supported here. Stick to Tauri 2.x with `webkit2gtk-4.1`.
- **Build fails with "Package webkit2gtk-4.1 was not found"**: re-run `fetch-tauri-deps.sh` and verify `pkg-config --exists webkit2gtk-4.1`.
- **Build fails with "rust-lld: error: unable to find library -lgdk-3"**: pass the `RUSTFLAGS` with `-L$HOME/local/usr/lib/x86_64-linux-gnu` (see above).

### C.3 — Dev mode with HMR

If the user wants dev hot-reload instead of a production build:

**For Electron with Vite/webpack:**
1. Start the JS dev server in the background (e.g. `npm run dev` → Vite on port 5173)
2. Start Electron pointing at that dev server: modify `main.js` to `win.loadURL('http://localhost:5173')` instead of `loadFile`
3. Launch via `start-electron.sh`

**For Tauri with Vite:**
1. Start Vite: `npm run dev`
2. Use `tauri dev` instead of `tauri build`. Tauri will spawn both Vite and the Tauri binary. You need to also set `devUrl` in `tauri.conf.json` to the Vite URL.
3. Manually run `tauri dev` with the same env vars as in C.2 above.

Note: HMR feedback loop is slower in this setup because everything goes through the VNC bridge (~200ms latency). For rapid UI iteration, the user may prefer to dev locally and only use this sandbox for final verification.

---

## Part D — Verification & troubleshooting

### D.1 — Health check

```bash
bash /home/z/my-project/scripts/status-bridge.sh
```

Reports:
- Are Xvfb, x11vnc, websockify, twm running?
- Is any Electron/Tauri app on :99? How many WebKit helpers?
- HTTP gateway reachable? WebSocket bridge completes RFB handshake?

### D.2 — Take a screenshot

```bash
bash /home/z/my-project/scripts/screenshot.sh /home/z/my-project/download/preview.png
```

Uses `xwd → xwdtopnm → pnmtopng`. The script handles the known quirk that `xwdtopnm` writes a status line to stdout (which would pollute the .ppm output if naively redirected). 10-second timeout — if it fails, the X server is wedged (probably because a Tauri app crashed leaving stale sockets).

### D.3 — End-to-end click test through noVNC

To verify that mouse events actually reach the app, use Playwright against the noVNC page:

```javascript
// test-click.js
const { chromium } = require('playwright');
(async () => {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  await page.goto('http://localhost:81/', { waitUntil: 'networkidle' });
  await page.waitForFunction(() => {
    const el = document.getElementById('noVNC_status') || document.getElementById('noVNC_status_inner');
    return el && el.textContent.toLowerCase().includes('connected');
  }, { timeout: 15000 });
  await page.waitForTimeout(3000);
  const canvas = await page.$('canvas');
  // Click on a button location in the underlying desktop
  await canvas.click({ position: { x: 168, y: 244 } });
  await page.waitForTimeout(1000);
  await page.screenshot({ path: '/home/z/my-project/download/preview-after-click.png' });
  await browser.close();
})();
```

Run from any directory with `playwright` installed (`/home/z/my-project/scripts/electron-app/node_modules/playwright`).

### D.4 — Common issues

| Symptom | Cause | Fix |
|---|---|---|
| Preview panel shows blank/directory listing | Missing `~/local/novnc/index.html` | Run `bridge-install.sh` |
| xwd times out | Xvfb wedged by crashed Tauri | `stop-bridge.sh` then `start-bridge.sh` |
| Tauri: "Failed to spawn child process /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/WebKitNetworkProcess" | LD_PRELOAD interceptor missing or stale | `gcc -shared -fPIC -o ~/local/interceptors/exec_redirect.so ~/local/interceptors/exec_redirect.c -ldl` |
| Tauri: "Could not create default EGL display: EGL_BAD_PARAMETER" | Mesa EGL not installed | `apt-get download libegl-mesa0 libglx-mesa0 mesa-libgallium; for f in *.deb; do dpkg-deb -x "$f" ~/local/; done` |
| Tauri: window appears but clicks don't change counter | No window manager running | `pgrep -x twm` should return a PID; if empty, run `DISPLAY=:99 twm &` |
| Tauri: Runtime Info tile shows "—" | `window.__TAURI__` undefined | Add `"withGlobalTauri": true` to `tauri.conf.json`'s `app` section + rebuild |
| Tauri: IPC commands return "unauthorized" | No capabilities file | Create `src-tauri/capabilities/main.json` granting `core:default` |
| Electron: "Module did not self-register" | Native module built for wrong Node | `npx electron-rebuild` in the app dir |
| Build fails: "Package webkit2gtk-4.1 was not found" | pkg-config can't find .pc files | `export PKG_CONFIG_PATH=~/local/usr/lib/x86_64-linux-gnu/pkgconfig:~/local/usr/share/pkgconfig` |
| Build fails: "rust-lld: unable to find library -lgdk-3" | Linker can't find .so files | Set `RUSTFLAGS="-Clink-arg=-L$HOME/local/usr/lib/x86_64-linux-gnu -Clink-arg=-Wl,-rpath,$HOME/local/usr/lib/x86_64-linux-gnu"` |
| `cargo install tauri-cli` dies mid-compile | Kernel kills long rustc | Use `npm install @tauri-apps/cli` instead |
| Counter doesn't change despite clicks | Wrong coordinates | Use VLM to find button center; canvas is 1280×800 so coordinates map 1:1 |

### D.5 — Things that do NOT work (don't try)

- `apt install <pkg>` — needs sudo. Use `apt-get download` + `dpkg-deb -x`.
- Binding websockify to port 81 — already taken by Caddy gateway. Bind to 3000; Caddy proxies 81 → 3000.
- `proot` (ptrace) — blocked by seccomp (SIGSYS).
- `bwrap` / bubblewrap (user namespaces) — restricted in this container.
- `cargo install tauri-cli` — rustc killed by watchdog. Use npm package.
- `WEBKIT_FORCE_SANDBOX=0` — deprecated in WebKitGTK 2.52. Use `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1`.
- Tauri 1.x — needs `webkit2gtk-4.0` and `libsoup-2.4`, conflicts with Tauri 2. Use Tauri 2.
- Top-level `await` in a classic `<script>` tag — silently throws SyntaxError; wrap in `async function`.

---

## Part E — Quick reference: file layout

```
/home/z/my-project/
├── scripts/
│   ├── bridge-install.sh              # one-time install (idempotent)
│   ├── fetch-tauri-deps.sh            # Tauri-only: download + extract ~40 system packages
│   ├── start-bridge.sh                # bring up Xvfb+x11vnc+websockify+twm
│   ├── start-electron.sh              # launch any Electron app on the bridge
│   ├── start-tauri.sh                 # launch any Tauri app on the bridge (after tauri build)
│   ├── stop-bridge.sh                 # tear down the bridge + apps
│   ├── status-bridge.sh               # health check
│   ├── screenshot.sh                  # capture PNG of the virtual desktop
│   ├── electron-app/                  # the demo Electron app (reference)
│   └── tauri-app/                     # the demo Tauri app (reference)
├── download/
│   ├── LIVE-DESKTOP-PREVIEW-SETUP.md  # this file
│   ├── electron-preview-frame.png     # verification screenshot (Electron)
│   └── tauri-preview-frame.png        # verification screenshot (Tauri)
└── worklog.md                         # multi-agent work log

/home/z/local/                          # all .deb-extracted system packages
├── usr/
│   ├── bin/x11vnc, xwd, xwdtopnm, pnmtopng, xdotool, xprop, xwininfo, twm
│   ├── lib/x86_64-linux-gnu/...        # all .so files
│   └── include/...                     # headers (for Tauri build)
├── novnc/                              # noVNC v1.5.0 static files + index.html redirect
└── interceptors/
    ├── exec_redirect.c                 # LD_PRELOAD interceptor source
    └── exec_redirect.so                # compiled LD_PRELOAD shim

~/.cargo/                               # Rust toolchain
~/.rustup/                              # Rust toolchains
```

---

## Part F — How to restart after sandbox reset

If the sandbox restarts (everything in `/tmp` wiped, processes killed), only `~/local/`, `~/.cargo/`, and the npm `node_modules/` survive. To bring the preview back:

```bash
# 1. Re-install websockify (pip-installed, may have been wiped)
pip3 install websockify

# 2. For Electron apps: re-install if node_modules was wiped
cd /path/to/electron-app && npm install --no-audit --no-fund
node node_modules/electron/install.js

# 3. For Tauri apps: re-install if node_modules was wiped
cd /path/to/tauri-app && npm install --no-audit --no-fund @tauri-apps/cli@^2 @tauri-apps/api@^2
# The Rust toolchain in ~/.cargo survives. The Tauri binary in src-tauri/target/release/ survives.

# 4. Start the bridge
bash /home/z/my-project/scripts/start-bridge.sh

# 5. Launch the app
bash /home/z/my-project/scripts/start-electron.sh /path/to/electron-app
# OR
bash /home/z/my-project/scripts/start-tauri.sh /path/to/tauri-app
```

---

## Part G — What the user sees

After running `start-bridge.sh` + `start-electron.sh` (or `start-tauri.sh`), opening the preview panel (or clicking "Open in New Tab") shows the noVNC viewer auto-connected to display :99. The user sees the live app and can click, type, scroll, etc.

End-to-end path for a click:
1. User clicks in browser canvas
2. noVNC JS encodes a PointerEvent
3. WebSocket sends to websockify on port 3000 (proxied from port 81 by Caddy)
4. websockify forwards the bytes to x11vnc on port 5900
5. x11vnc translates VNC PointerEvent → X11 button event
6. Xvfb dispatches the event via twm to the window under the cursor
7. Electron/Tauri receives the X11 event
8. The app's renderer (Chromium for Electron, WebKitGTK for Tauri) processes the click
9. The JS handler runs, calls IPC if needed (Electron `ipcRenderer.invoke` / Tauri `invoke`)
10. Main process (Node for Electron, Rust for Tauri) updates state
11. Renderer re-renders, sends damage to Xvfb
12. x11vnc sends framebuffer update back through the chain to the browser
13. User sees the counter increment

Typical total latency: 200-400ms round-trip.
