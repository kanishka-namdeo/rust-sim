# GCS v2 UX Bridge Spec — Onboarding, Empty States, Loading, Errors & Delight

**Status:** Draft v1.0 — 2026-09-10 (bridges the competitive audit + gap analysis → implementation)
**Scope:** `console/` — UX-only, zero backend changes (§10 carries)
**Owners:** RustSim core team
**Related:** `docs/GCS_V2_SPEC.md` (the engineering contract), `docs/GCS_V2_READINESS_REVIEW.md`

---

## 1. Purpose

The competitive audit (7 tools: QGC, MP, AMC, UgCS, DJI, FlytBase, DroneDeploy)
+ the live gap analysis identified 30+ UX gaps. This spec bridges them into a
prioritized implementation that puts our GCS v2 at or above market standards
for operator delight.

**The north star:** A first-time operator opens the app, sees a guided
welcome, understands the 3-zone layout in <30s, creates their first mission
from a template, validates it, and uploads — all without reading docs.

---

## 2. Top 10 UX patterns to adopt (from the audit)

| # | Pattern | Source | Our implementation |
|---|---------|--------|-------------------|
| 1 | Guided first-flight wizard (not a blank shell) | UgCS + QGC intent | §3 — first-run welcome overlay + 3-step highlight tour |
| 2 | Mission-template wizard as the empty state | UgCS | §4 — empty-state CTAs with template buttons |
| 3 | Empty states that never feel empty | 2026 best practice | §4 — every panel has illustration + reason + CTA + link |
| 4 | Skeleton screens for every async load | NN/G + LogRocket | §5 — shimmer skeletons for telemetry column + REST panels |
| 5 | Actionable "Waiting For Vehicle" diagnostic panel | fixes QGC's #1 friction | §6 — connection diagnostic with Retry + auto-reconnect |
| 6 | Cloud mission sync + org template library | Auterion | §7 — catalog "starter templates" seeded on first run |
| 7 | Overlay-based actions preserving context | FlytBase | Already done (§4.2 — overlays never navigate) |
| 8 | Discoverable keyboard shortcuts + ? cheat sheet | FlytBase | Already done (§7.3) + §3 first-run auto-open |
| 9 | Pre-flight checklist as the first-run flow | DroneDeploy | §8 — pre-flight panel opens after first vehicle READY |
| 10 | Published WCAG 2.1 AA + signature delight | DJI | §9 — focus-visible ring + overlay dialog semantics + pulse |

---

## 3. Onboarding — first-run welcome + guided tour (O1, O2, O3)

### 3.1 First-run welcome overlay

On the first visit (no `localStorage["rsim.onboarded"]` key), show a
centered welcome dialog:

```
┌─────────────────────────────────────────────────────┐
│                  Welcome to RustSim GCS              │
│                                                      │
│  The single-screen operator surface for PX4 SITL.   │
│                                                      │
│  • Map-first: everything is on one canvas            │
│  • Single-key: A to arm, L to land, ? for shortcuts  │
│  • 2 SITL vehicles are already connected             │
│                                                      │
│  [Take the 30s tour]  [Skip — I know my way]          │
│                                                      │
│  Tip: press ? anytime to see all shortcuts            │
└─────────────────────────────────────────────────────┘
```

- **Take the tour** → 3-step highlight tour (§3.2)
- **Skip** → sets `localStorage["rsim.onboarded"] = true`, closes dialog
- Auto-opens the cheat sheet after the welcome (if not skipped)

### 3.2 Three-step highlight tour

| Step | Highlights | Text |
|------|-----------|------|
| 1 | Left rail (zone B) | "Modes + panels live here. Click Plan to start a mission, Fleet C2 to see your vehicles." |
| 2 | Command bar (zone D) | "Flight verbs here. Single-key: A = ARM, L = land, E = estop. Hold ARM for 400ms." |
| 3 | Telemetry column (zone C) | "Live telemetry for the active vehicle. Click a vehicle chip (top) to switch." |

Each step: dim the rest of the canvas, highlight the zone with a cyan border,
show the text in a tooltip. "Next" / "Skip" / "Done" buttons.

### 3.3 First-run hint chips

After onboarding, show dismissible hint chips:
- On the map: "Right-click for context menu · scroll to zoom" (fades after first interaction)
- On the verb bar: "verbs act on selected vehicle v0" (fades after 10s)

---

## 4. Empty states — every panel has a CTA (E1–E6)

### 4.1 Empty state template

Every panel/section that can be empty renders:

```
┌─────────────────────────────────────────┐
│            [icon illustration]            │
│                                          │
│         No missions yet                   │
│  Create one from Plan mode or a template  │
│                                          │
│     [+ New Mission]  [Browse Templates]  │
│                                          │
│  Tip: switch to Plan mode (P) and click  │
│  the map to add waypoints                 │
└─────────────────────────────────────────┘
```

### 4.2 Per-panel empty states

| Panel | Empty state text | CTA |
|-------|-----------------|-----|
| MissionStrip (no waypoints) | "No waypoints yet" + "Plan mode (P) + click map to add" | [New Mission] [Browse Templates] |
| Library (no saved missions) | "No saved missions" + "Save from the Mission Strip (M)" | [+ New] [Refresh] |
| Fleet C2 → Bindings (no missions to bind) | "No missions to bind" + "Create a mission first in Plan mode" | [Go to Plan] |
| Fleet C2 → Patterns (stub) | "Coming soon" badge with disabled styling | disabled |
| Analyze → Plots (stub) | "Coming soon" badge with disabled styling | disabled |
| TelemetryColumn mini-plots (no data) | "connecting…" pulsing label → "no telemetry yet" after timeout | — |
| Notifications (first load) | Welcome info toast: "X SITL vehicles connected · press ? for shortcuts" | [dismiss] |

### 4.3 Mission templates (starter library)

Seed the catalog with 3 starter missions on first run:
1. **Hover test** — single waypoint at 10m AGL (default altitude)
2. **Square patrol** — 4 waypoints in a 50m square at 15m AGL
3. **Line survey** — 6 waypoints in a 100m line at 20m AGL with camera triggers

These appear in the Library empty state as "Browse Templates" buttons.

---

## 5. Loading states — skeleton screens (L1–L3)

### 5.1 Skeleton screen pattern

A shimmer skeleton (CSS animation, no images) renders while data loads:

```css
.rsim-skeleton {
  background: linear-gradient(90deg, var(--rsim-surface-solid) 25%, var(--rsim-border) 50%, var(--rsim-surface-solid) 75%);
  background-size: 200% 100%;
  animation: rsim-shimmer 1.5s infinite;
}
@keyframes rsim-shimmer {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}
```

### 5.2 Per-zone loading states

| Zone | Loading state | Trigger |
|------|---------------|---------|
| TelemetryColumn | Pulsing "connecting…" label + shimmer on instrument tiles | `tel.conn === 'connecting'` |
| Map | Subtle top loading bar (cyan, 2px, animated width) | `mapDebug.loadedAtMs === null` |
| Verb bar vehicle chip | Shimmer on "no vehicle" text | `snap.vehicles.length === 0` |
| REST panels (Library, Setup) | Skeleton rows (3 shimmer lines) | `catalog.conn === 'connecting'` |

---

## 6. Error states — actionable diagnostics (R1–R4)

### 6.1 Connection diagnostic panel

When SIM badge stays CONNECTING for >15s, escalate to `⚠ UNREACHABLE` with a
diagnostic tooltip:
```
SIM :8200 — unreachable
The per-vehicle sim plane (:8200+i) is not responding.
Fleet telemetry (:8400) is live — vehicle state is still available.
Click to retry.
```

### 6.2 Command-bus error toasts

All command-bus failures surface as notifications:
```
[✗ ARM failed] v0 not READY (fsm=BOOTING)
[Retry] [Dismiss]
```

### 6.3 REST panel error envelopes

When a REST call fails, the panel renders:
```
┌─────────────────────────────────────────┐
│  ⚠ Failed to load missions              │
│  network: connection refused             │
│  [Retry]                                  │
└─────────────────────────────────────────┘
```

---

## 7. Accessibility — WCAG 2.1 AA (A1–A6)

### 7.1 Focus-visible ring

```css
.rsim-canvas :focus-visible {
  outline: 2px solid var(--rsim-accent);
  outline-offset: 2px;
}
```

### 7.2 Overlay dialog semantics

Every overlay panel gets `role="dialog" aria-modal="true" aria-labelledby`
+ Escape-to-close + focus return to the invoking element.

### 7.3 Assertive alert region for safety-critical events

Add `aria-live="assertive"` for E-STOP, geofence breach, battery critical.

### 7.4 Mini-plot accessibility

Each mini-plot canvas gets `role="img" aria-label="alt history sparkline"`.

### 7.5 Status chip aria-labels

Each ConnBadge gets a descriptive `aria-label`:
`aria-label="FLEET service: live, 10 Hz telemetry"`.

---

## 8. Delight — micro-interactions (D1–D4)

### 8.1 Overlay open/close animations

150ms translate + fade for drawers:
```css
.rsim-overlay-enter { transform: translateX(-100%); opacity: 0; }
.rsim-overlay-enter-active { transform: translateX(0); opacity: 1; transition: all 150ms ease-out; }
```

### 8.2 Success micro-interactions

On ARM/Takeoff/Mission-start ACK, brief pulse:
```css
@keyframes rsim-success-pulse {
  0% { box-shadow: 0 0 0 0 var(--rsim-ok); }
  100% { box-shadow: 0 0 0 12px transparent; }
}
```

### 8.3 E-STOP idle pulse

A subtle 2s pulse on the E-STOP button when idle (not when active):
```css
@keyframes rsim-estop-idle {
  0%, 100% { opacity: 0.9; }
  50% { opacity: 1; box-shadow: 0 0 4px var(--rsim-alert); }
}
```

---

## 9. Discoverability — align visible state with behavior (C1–C6)

### 9.1 Rail "(—)" labels

Replace "(—)" with a clearer tooltip: "No shortcut — click to open."
Only visually disable truly unimplemented items (Patterns, Plots stubs).

### 9.2 MISSION ▸ button

Enable the MISSION ▸ button (it already works via the M key shortcut).

### 9.3 On-map hint chip

Transient hint that fades after first interaction:
"Right-click for menu · scroll to zoom · ? for shortcuts"

---

## 10. Implementation order

| Priority | Items | Effort |
|----------|-------|--------|
| P0 | §3 first-run welcome + tour; §4 empty states; §5 loading skeletons | 2h |
| P1 | §6 error states; §7 accessibility; §9 discoverability | 1h |
| P2 | §8 delight animations; §4.3 mission templates | 1h |
