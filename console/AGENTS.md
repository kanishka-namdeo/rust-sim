# console/ — RustSim operator console

## Purpose

Single-page Next.js operator console with two permanently-mounted telemetry
engines: the **Sim Console** (rustsitsim plane on `:8200` — 10 Hz physics
telemetry, strip charts, fault injection) and **Fleet C2** (mavfleet plane on
`:8400` — fleet table, NED map, task board, event log, e-stop). Both run a
LIVE/SIMULATED dual mode: if the Rust backend answers through the gateway,
everything is live; otherwise a client-side mock engine keeps the UI operable
and the live endpoint is re-probed every 12 s.

## Ownership

Owned here: pages/components/hooks/libs under `src/`, the tolerant protocol
normalizers (`src/lib/conn.ts`), the dual-mode lifecycle, styling.

Not owned here: the backend schemas (sim SPEC §4, fleet spec §3.4 — the
console normalizes tolerantly but does not redefine them), the gateway
itself (see `Caddyfile.example`).

## Local Contracts

- No database, no auth, no server state: the console is a pure client of the
  two Rust control planes.
- API routing styles (see `src/lib/conn.ts`):
  - `gateway` (default): relative fetches + `?XTransformPort=<port>`, WS at
    `/?XTransformPort=<port>` — used behind the Caddy gateway
    (`Caddyfile.example`) and preview proxies.
  - `direct` (`NEXT_PUBLIC_RSIM_API_STYLE=direct`): absolute
    `http://127.0.0.1:<port>` REST + `ws://127.0.0.1:<port>/` for local runs
    without the gateway.
- Both consoles stay mounted (hidden by CSS) so telemetry engines survive tab
  switches.
- Dependencies are trimmed to what `src/` actually imports (radix
  tabs/select/label/progress/scroll-area/alert-dialog/toast/slot, lucide,
  next-themes, cn util, Tailwind v4). No prisma, no template cruft.

## Work Guidance

- `npm install && npm run build && npm start` (or `npm run dev`). Port 3000.
- For the gateway mode, run Caddy with `Caddyfile.example` and open :81.
- Frame normalization is deliberately tolerant (key aliases, envelope
  unwrapping) — extend the normalizers rather than hard-coding backend
  shapes.
- Mock engines (`src/lib/mock-sim.ts`, `mock-fleet.ts`) exist for offline
  demo/development, not to mask backend regressions: live-vs-mock is always
  visible in the UI.

## Verification

- `npm run lint` and `npm run build` must be clean.
- End-to-end: `../scripts/browser_live_test.sh` — opens the console through
  the gateway, asserts the LIVE badge on both consoles, asserts telemetry is
  moving (two snapshots differ), and captures screenshots.

## Child DOX Index

No child AGENTS.md files yet. Candidates when they become durable
boundaries: `src/components/dashboard/` (the console widgets),
`src/lib/conn.ts` + `src/hooks/` (the telemetry engines).
