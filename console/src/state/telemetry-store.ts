/**
 * GCS v2 Operations Canvas — telemetry store (M8, T-A3).
 *
 * Spec: docs/GCS_V2_SPEC.md §9.1 + §9.2 + §13.1 M8 skeleton interface #3.
 *
 * One module-level store replacing v1's per-hook sockets:
 *
 *  - Fleet socket (`:8400` WS) — 10 Hz `FleetFrame` → `normalizeFleetSnapshot`
 *    (consolidated from the 3 v1 copies, P8 — `lib/normalize.ts`) → ring
 *    buffers (tracks 1800, strips 600, events 200) + last-frame ref.
 *  - Sim sockets (`:8200+i`) — one per vehicle present in the fleet snapshot
 *    (P11 fix: count discovered from fleet frame, NOT hardcoded). SimFrame
 *    normalizer is the same `lib/normalize.ts`. Sim frame is the only wire
 *    source of GPS fix/sats (`sensors.gps_fix`/`gps_sat`); instruments join
 *    by vehicle index (§8.3).
 *  - Catalog probe (:8300 REST) — health only at M8.
 *  - Probe/retry ladder (v1 contract preserved): probe → LIVE: WS + REST
 *    fallback → 3 × 2.5 s retry → SIMULATED mock engine (5–10 Hz) → 12 s
 *    re-probe. Mock engines port from `lib/mock-fleet.ts` / `lib/mock-sim.ts`
 *    unchanged.
 *
 * Telemetry numerics NEVER touch React state (§9.2). The `FlushLoop.tsx` rAF
 * loop reads this store every frame and writes to the HUD ref registry
 * (textContent / style.transform). React state commits ≤ 5 Hz for
 * armed/mode/fsm/phase/battery-chip changes only — tracked here as
 * `lastStateChangeAt`, asserted by `window.__rsimCommits` (§9.2).
 *
 * Gate instruments (§9.2 — always on, negligible cost):
 *   `window.__rsimTelemetry = { framesIn, framesRendered, lastFrameAt,
 *                                simSockets, commits }`
 *   `window.__rsimCommits` increments in the state-commit path; gates read it
 *   to assert ≤ 5 Hz over 60 s (G-14 mock-engine soak leg + G-20).
 */

import { useSyncExternalStore } from 'react'
import { gw, probePlane, wsUrl } from '../lib/conn'
import {
  normalizeFleetSnapshot,
  normalizeSimFrame,
  SIM_FALLBACK_FRAME,
} from '../lib/normalize'
import type {
  ConnState,
  FleetEvent,
  FleetSnapshot,
  FleetVehicle,
  SimFrame,
} from '../lib/types'
import { RingBuffer } from './ring-buffer'
import { FleetMockEngine } from '../lib/mock-fleet'
import { SimMockEngine } from '../lib/mock-sim'
import { DEFAULT_ORIGIN } from '../lib/geo'

// ---------------------------------------------------------------------------
// Constants (spec §9.1 ring buffer sizes; §9.1 probe/retry ladder)
// ---------------------------------------------------------------------------

const TRACK_CAP = 1800 // §9.1: per-vehicle [lon,lat] ring
const STRIP_CAP = 600 // §9.1: per-vehicle strip-chart samples per key
const EVENT_CAP = 200 // §9.1: events tail
const PROBE_TIMEOUT_MS = 2500 // §9.1: initial probe
const RETRY_MS = 2500 // §9.1: 3 × 2.5 s retry
const RETRY_TRIES = 3
const REPROBE_MS = 12000 // §9.1: 12 s re-probe after going SIMULATED
const SIM_TICK_MS = 100 // 10 Hz mock (spec §9.1: "5–10 Hz")

const FLEET_PORT = 8400
const CATALOG_PORT = 8300
const SIM_PORT_BASE = 8200 // :8200+i

// ---------------------------------------------------------------------------
// Per-strip keys (consumed by §8.3 mini-plots + the column C attitude HUD)
// ---------------------------------------------------------------------------

export type StripKey = 'alt_m' | 'battery_pct' | 'ground_speed_ms' | 'yaw_deg' | 'roll_deg' | 'pitch_deg'

const STRIP_KEYS: StripKey[] = ['alt_m', 'battery_pct', 'ground_speed_ms', 'yaw_deg', 'roll_deg', 'pitch_deg']

// ---------------------------------------------------------------------------
// Module-level state — NO React state for telemetry numerics (§9.2)
// ---------------------------------------------------------------------------

interface PlaneState {
  conn: ConnState
  retryAt: number | null // ms epoch when next probe fires (null = live)
  lastError: string | null
}

interface VehicleBuffers {
  /** Track ring: [lon, lat] for the operator map / Fleet NED inset (L2 layer). */
  track: RingBuffer<[number, number]>
  /** Per-strip-key ring: {t (epoch ms), v} samples. */
  strips: Record<StripKey, RingBuffer<{ t: number; v: number }>>
}

const planes: Record<'fleet' | 'catalog' | 'sim', PlaneState> = {
  fleet: { conn: 'connecting', retryAt: Date.now() + PROBE_TIMEOUT_MS, lastError: null },
  catalog: { conn: 'connecting', retryAt: Date.now() + PROBE_TIMEOUT_MS, lastError: null },
  sim: { conn: 'connecting', retryAt: Date.now() + PROBE_TIMEOUT_MS, lastError: null },
}

let snapshot: FleetSnapshot | null = null
let vehicles: FleetVehicle[] = [] // last normalized vehicles (the snapshot.vehicles)
let events: FleetEvent[] = [] // tail ring (latest 200)
let simFrames: SimFrame[] = [] // per-vehicle last sim frame (index → frame)
let simSockets: number[] = [] // open sim socket vehicle indices (P11 assertion target)
const vehicleBuffers = new Map<number, VehicleBuffers>()

// The catalog plane is a plain REST probe (no WS at M8). Track its conn state.
let catalogProbeTimer: ReturnType<typeof setTimeout> | null = null
let fleetSocket: WebSocket | null = null
let fleetSocketRetry = 0
const simSocketsMap = new Map<number, WebSocket>() // vehicle index → socket
let mockFleet: FleetMockEngine | null = null
let mockSims: Map<number, SimMockEngine> = new Map()
let mockTimer: ReturnType<typeof setInterval> | null = null

const listeners = new Set<() => void>()
let version = 0
let lastStateChangeAt = 0 // §9.2: armed/mode/fsm/phase/battery-chip changes only
let lastReactSnapshot: {
  activeVehicle: number
  armed: boolean
  mode: string
  fsm: string
  phase: string
  batteryPct: number
} | null = null

// Gate instruments (§9.2 — always on)
const inst = {
  framesIn: 0, // WS frames normalized into the store
  framesRendered: 0, // rAF passes that flushed work (incremented by FlushLoop via _incrementFramesRendered)
  lastFrameAt: 0,
  commits: 0, // React state commits (§9.2)
}

/** Incremented by FlushLoop.tsx on each rAF pass (§9.2 binding contract).
 *  Exposed so the FlushLoop owns the write; __rsimTelemetry.framesRendered
 *  reads via a getter so gates can assert it. */
export function _incrementFramesRendered(): void {
  inst.framesRendered++
}

if (typeof window !== 'undefined') {
  ;(window as unknown as { __rsimTelemetry?: unknown }).__rsimTelemetry = {
    get framesIn() {
      return inst.framesIn
    },
    get framesRendered() {
      return inst.framesRendered
    },
    get lastFrameAt() {
      return inst.lastFrameAt
    },
    get simSockets() {
      return simSockets.length
    },
    get commits() {
      return inst.commits
    },
  }
  ;(window as unknown as { __rsimCommits?: number }).__rsimCommits = 0
}

// ---------------------------------------------------------------------------
// External subscription (useSyncExternalStore contract)
// ---------------------------------------------------------------------------

export function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

let snapshotCache: ReturnType<typeof getSnapshot> | null = null
let snapshotCacheVersion = -1

export function getSnapshot(): {
  vehicles: FleetVehicle[]
  lastFrameAt: number
  planes: typeof planes
  simSockets: number[]
} {
  // `useSyncExternalStore` requires `getSnapshot` to return the SAME object
  // when state hasn't changed — otherwise it detects "changes" every call
  // and re-renders infinitely (the M8 spike: React error #185 "Maximum
  // update depth exceeded"). Cache by `version` and only rebuild on bump.
  if (snapshotCache && snapshotCacheVersion === version) {
    return snapshotCache
  }
  snapshotCache = {
    vehicles,
    lastFrameAt: inst.lastFrameAt,
    planes,
    simSockets: simSockets.slice(),
  }
  snapshotCacheVersion = version
  return snapshotCache
}

export function getVersion(): number {
  return version
}

// `useSyncExternalStore` React binding — consumed by zone furniture.
export function useTelemetrySnapshot() {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}

function bump(): void {
  version++
  for (const fn of listeners) fn()
}

// ---------------------------------------------------------------------------
// Ring-buffer accessors (consumed by FlushLoop / MapCanvas / column C plots)
// ---------------------------------------------------------------------------

export function getTrack(i: number): [number, number][] {
  return vehicleBuffers.get(i)?.track.snapshot() ?? []
}

export function getStrip(i: number, key: StripKey): { t: number; v: number }[] {
  return vehicleBuffers.get(i)?.strips[key].snapshot() ?? []
}

export function getSimFrame(i: number): SimFrame | null {
  return simFrames[i] ?? null
}

/** Last normalized fleet snapshot (the source of truth for vehicles/tasks/fence). */
export function getFleetSnapshot(): FleetSnapshot | null {
  return snapshot
}

// ---------------------------------------------------------------------------
// Internal: per-vehicle buffer management
// ---------------------------------------------------------------------------

function ensureVehicleBuffers(i: number): VehicleBuffers {
  let buf = vehicleBuffers.get(i)
  if (!buf) {
    buf = {
      track: new RingBuffer<[number, number]>(TRACK_CAP),
      strips: {
        alt_m: new RingBuffer<{ t: number; v: number }>(STRIP_CAP),
        battery_pct: new RingBuffer<{ t: number; v: number }>(STRIP_CAP),
        ground_speed_ms: new RingBuffer<{ t: number; v: number }>(STRIP_CAP),
        yaw_deg: new RingBuffer<{ t: number; v: number }>(STRIP_CAP),
        roll_deg: new RingBuffer<{ t: number; v: number }>(STRIP_CAP),
        pitch_deg: new RingBuffer<{ t: number; v: number }>(STRIP_CAP),
      },
    }
    vehicleBuffers.set(i, buf)
  }
  return buf
}

// ---------------------------------------------------------------------------
// Internal: strip sampling from a normalized FleetVehicle.
// ---------------------------------------------------------------------------

function sampleStrips(v: FleetVehicle, simFrame: SimFrame | null): void {
  const buf = ensureVehicleBuffers(v.index)
  const now = Date.now()
  // Track: [lon, lat] only when the vehicle has a GPS fix.
  if (v.lat != null && v.lon != null) {
    buf.track.push([v.lon, v.lat])
  }
  // Strip samples.
  const speed = Math.hypot(v.velocity_ned_ms[0], v.velocity_ned_ms[1])
  buf.strips.alt_m.push({ t: now, v: v.alt_agl_m ?? 0 })
  buf.strips.battery_pct.push({ t: now, v: v.battery_pct })
  buf.strips.ground_speed_ms.push({ t: now, v: speed })
  buf.strips.yaw_deg.push({ t: now, v: v.yaw_deg })
  // Roll/pitch from attitude_q_wxyz (P5 fix); fall back to 0 if absent
  // (the v1 yaw-only behavior). Use sim frame's q as redundancy (§9.1).
  const q = v.attitude_q_wxyz ?? simFrame?.state.q_wxyz ?? null
  if (q) {
    const { roll_deg, pitch_deg } = quatToRollPitchYaw(q)
    buf.strips.roll_deg.push({ t: now, v: roll_deg })
    buf.strips.pitch_deg.push({ t: now, v: pitch_deg })
  } else {
    buf.strips.roll_deg.push({ t: now, v: 0 })
    buf.strips.pitch_deg.push({ t: now, v: 0 })
  }
}

// Tiny inline ZYX NED quaternion → roll/pitch/yaw (matches lib/format.ts:quatToEulerDeg).
function quatToRollPitchYaw(q: [number, number, number, number]): {
  roll_deg: number
  pitch_deg: number
  yaw_deg: number
} {
  const [w, x, y, z] = q
  const roll = Math.atan2(2 * (w * x + y * z), 1 - 2 * (x * x + y * y))
  const sinp = 2 * (w * y - z * x)
  const pitch = Math.abs(sinp) >= 1 ? Math.sign(sinp) * Math.PI / 2 : Math.asin(sinp)
  const yaw = Math.atan2(2 * (w * z + x * y), 1 - 2 * (y * y + z * z))
  return {
    roll_deg: (roll * 180) / Math.PI,
    pitch_deg: (pitch * 180) / Math.PI,
    yaw_deg: ((yaw * 180) / Math.PI + 360) % 360,
  }
}

// ---------------------------------------------------------------------------
// Internal: commit a normalized fleet frame → ring buffers + state
// ---------------------------------------------------------------------------

function commitFleetFrame(
  norm: { snapshot: FleetSnapshot; events: FleetEvent[]; auctions: unknown[] } | null,
  source: 'live' | 'mock',
): void {
  if (!norm) return
  inst.framesIn++
  inst.lastFrameAt = Date.now()

  snapshot = norm.snapshot
  vehicles = norm.snapshot.vehicles

  // Append events to the ring (EVENT_CAP); preserve order.
  if (norm.events.length > 0) {
    events = [...events, ...norm.events].slice(-EVENT_CAP)
  }

  // P11: spawn/close sim sockets to match the fleet frame's vehicle count.
  // The sim socket count is the G-14 assertion target (`__rsimTelemetry.simSockets
  // == fleet-frame vehicle count`). Live WS for each vehicle present in the
  // snapshot; only the live plane does this — the mock ladder uses SimMockEngine.
  if (source === 'live' && planes.fleet.conn === 'live') {
    syncSimSockets(norm.snapshot.vehicles)
  }

  // Sample strips + tracks for each vehicle (the rAF loop reads these).
  for (const v of norm.snapshot.vehicles) {
    sampleStrips(v, simFrames[v.index] ?? null)
  }

  // React state commit (≤ 5 Hz): only when armed/mode/fsm/phase/battery-chip
  // changed (§9.2). This is the ONLY path that increments __rsimCommits.
  maybeCommitReactState(norm.snapshot)

  // Welcome notification — fires once when the first fleet frame arrives.
  maybeShowWelcome(norm.snapshot)

  // Battery ladder notifications (MDPI spec: 30% warn / 20% caution /
  // 10% auto-RTL notice). Only fires on threshold crossings.
  checkBatteryLadder(norm.snapshot.vehicles)

  bump()
}

// Battery ladder — fires notifications on threshold crossings.
let lastBatteryThreshold: Record<number, string> = {}
function checkBatteryLadder(vehicles: FleetVehicle[]): void {
  for (const v of vehicles) {
    const pct = v.battery_pct
    let level = 'ok'
    if (pct <= 10) level = 'critical'
    else if (pct <= 20) level = 'caution'
    else if (pct <= 30) level = 'warn'

    const prev = lastBatteryThreshold[v.index] ?? 'ok'
    if (level !== prev && level !== 'ok') {
      const severity = level === 'critical' ? 'error' : level === 'caution' ? 'warn' : 'info'
      const title = level === 'critical' ? 'Battery critical — auto-RTL recommended' : level === 'caution' ? 'Battery caution' : 'Battery low'
      const detail = `v${v.index}: ${pct.toFixed(0)}% (${level})`
      // Dynamic import to avoid circular dep.
      import('./app-store').then(({ pushNotification }) => {
        pushNotification({ severity, title, detail, sticky: level === 'critical' })
      })
    }
    lastBatteryThreshold[v.index] = level
  }
}

// React-state commit gate. The state-commit path is the budget the G-14/G-20
// soaks assert via window.__rsimCommits ≤ 5 Hz over 60 s.
function maybeCommitReactState(snap: FleetSnapshot): void {
  const v0 = snap.vehicles[0]
  const next = {
    activeVehicle: v0?.index ?? 0,
    armed: Boolean(v0?.armed),
    mode: v0?.mode ?? 'UNKNOWN',
    fsm: v0?.fsm ?? 'INIT',
    phase: snap.phase,
    batteryPct: v0?.battery_pct ?? 0,
  }
  if (
    !lastReactSnapshot ||
    lastReactSnapshot.armed !== next.armed ||
    lastReactSnapshot.mode !== next.mode ||
    lastReactSnapshot.fsm !== next.fsm ||
    lastReactSnapshot.phase !== next.phase ||
    Math.abs(lastReactSnapshot.batteryPct - next.batteryPct) >= 1
  ) {
    lastReactSnapshot = next
    lastStateChangeAt = Date.now()
    inst.commits++
    if (typeof window !== 'undefined') {
      ;(window as unknown as { __rsimCommits?: number }).__rsimCommits = inst.commits
    }
  }
}

// ---------------------------------------------------------------------------
// Sim socket management — P11 fix (count from fleet snapshot, not hardcoded)
// ---------------------------------------------------------------------------

function syncSimSockets(fleetVehicles: FleetVehicle[]): void {
  const liveIndices = fleetVehicles.map((v) => v.index).sort((a, b) => a - b)
  const openIndices = Array.from(simSocketsMap.keys()).sort((a, b) => a - b)

  // Spawn new sockets.
  for (const i of liveIndices) {
    if (!simSocketsMap.has(i)) openSimSocket(i)
  }
  // Close sockets whose vehicle disappeared.
  for (const i of openIndices) {
    if (!liveIndices.includes(i)) closeSimSocket(i)
  }
  simSockets = Array.from(simSocketsMap.keys()).sort((a, b) => a - b)
}

function openSimSocket(i: number): void {
  if (typeof window === 'undefined') return
  const port = SIM_PORT_BASE + i
  try {
    const ws = new WebSocket(wsUrl(port))
    ws.onopen = () => {
      simSocketsMap.set(i, ws)
      simSockets = Array.from(simSocketsMap.keys()).sort((a, b) => a - b)
      bump()
    }
    ws.onmessage = (ev) => {
      try {
        const raw = JSON.parse(typeof ev.data === 'string' ? ev.data : '{}')
        const frame = normalizeSimFrame(raw, simFrames[i] ?? null)
        simFrames[i] = frame
        // Sample strips now if we have a matching fleet vehicle — sim frame
        // carries the authoritative attitude quaternion and gps_fix/sats.
        const v = vehicles.find((x) => x.index === i)
        if (v) sampleStrips(v, frame)
        inst.framesIn++
        inst.lastFrameAt = Date.now()
        // Per-frame bumps are cheap but matter for the "framesRendered" budget;
        // the state-commit bump happens through maybeCommitReactState above.
        bump()
      } catch (e) {
        console.warn('[telemetry-store] sim frame parse failed', i, e)
      }
    }
    ws.onerror = () => {
      // Fall back to retry; the socket will close on its own.
    }
    ws.onclose = () => {
      simSocketsMap.delete(i)
      simSockets = Array.from(simSocketsMap.keys()).sort((a, b) => a - b)
      bump()
    }
    // Register optimistically so a rapid onopen/onerror doesn't race.
    simSocketsMap.set(i, ws)
  } catch (e) {
    console.warn('[telemetry-store] sim socket open failed', i, e)
  }
}

function closeSimSocket(i: number): void {
  const ws = simSocketsMap.get(i)
  if (!ws) return
  try {
    ws.close()
  } catch {
    // ignore
  }
  simSocketsMap.delete(i)
}

// ---------------------------------------------------------------------------
// Fleet socket — probe/retry/ladder
// ---------------------------------------------------------------------------

async function startFleetSocket(): Promise<void> {
  if (typeof window === 'undefined') return
  // Probe first (cheap).
  const ok = await probePlane(gw(FLEET_PORT, '/api/fleet'), PROBE_TIMEOUT_MS)
  if (!ok) {
    fleetSocketRetry++
    if (fleetSocketRetry > RETRY_TRIES) {
      // Go SIMULATED.
      planes.fleet = { conn: 'simulated', retryAt: Date.now() + REPROBE_MS, lastError: 'no fleet plane' }
      startMockLadder()
      bump()
      // Schedule a re-probe.
      setTimeout(startFleetSocket, REPROBE_MS)
      return
    }
    planes.fleet = { conn: 'connecting', retryAt: Date.now() + RETRY_MS, lastError: 'probe failed' }
    bump()
    setTimeout(startFleetSocket, RETRY_MS)
    return
  }
  // Probe succeeded — open WS.
  fleetSocketRetry = 0
  try {
    const ws = new WebSocket(wsUrl(FLEET_PORT))
    ws.onopen = () => {
      planes.fleet = { conn: 'live', retryAt: null, lastError: null }
      // Mock ladder off (if it was on).
      stopMockLadder()
      bump()
    }
    ws.onmessage = (ev) => {
      try {
        const raw = JSON.parse(typeof ev.data === 'string' ? ev.data : '{}')
        const norm = normalizeFleetSnapshot(raw, vehiclesById())
        commitFleetFrame(norm, 'live')
      } catch (e) {
        console.warn('[telemetry-store] fleet frame parse failed', e)
      }
    }
    ws.onerror = () => {
      planes.fleet = { conn: 'connecting', retryAt: Date.now() + RETRY_MS, lastError: 'socket error' }
      bump()
    }
    ws.onclose = () => {
      fleetSocket = null
      planes.fleet = { conn: 'connecting', retryAt: Date.now() + RETRY_MS, lastError: 'socket closed' }
      bump()
      setTimeout(startFleetSocket, RETRY_MS)
    }
    fleetSocket = ws
  } catch (e) {
    planes.fleet = { conn: 'connecting', retryAt: Date.now() + RETRY_MS, lastError: String(e) }
    bump()
    setTimeout(startFleetSocket, RETRY_MS)
  }
}

function vehiclesById(): Record<string, FleetVehicle> {
  const out: Record<string, FleetVehicle> = {}
  for (const v of vehicles) out[v.id] = v
  return out
}

// ---------------------------------------------------------------------------
// Mock ladder — SIMULATED badge + 12 s re-probe (§9.1, dual-mode contract)
// ---------------------------------------------------------------------------

function startMockLadder(): void {
  if (mockTimer) return
  mockFleet = new FleetMockEngine()
  // Build a 2-vehicle mock at first; engine can be re-tuned if more vehicles
  // appear. The mock emits FleetSnapshot + events + auctions every tick.
  mockTimer = setInterval(() => {
    if (!mockFleet) return
    const tick = mockFleet.tick(SIM_TICK_MS / 1000)
    // The mock emits a FleetSnapshot directly (no envelope); normalizeFleetSnapshot
    // accepts bare frames.
    const norm = normalizeFleetSnapshot(tick.snapshot, vehiclesById())
    if (norm) {
      // Re-source sim frames from per-vehicle mock engines so strip samples
      // and the GPS fix/sat instruments work in SIMULATED mode too.
      for (const v of norm.snapshot.vehicles) {
        let sm = mockSims.get(v.index)
        if (!sm) {
          sm = new SimMockEngine()
          mockSims.set(v.index, sm)
        }
        const simFrame = sm.tick(SIM_TICK_MS / 1000)
        simFrames[v.index] = simFrame
      }
      commitFleetFrame(norm, 'mock')
    }
  }, SIM_TICK_MS)
}

function stopMockLadder(): void {
  if (mockTimer) {
    clearInterval(mockTimer)
    mockTimer = null
  }
  mockFleet = null
  mockSims.clear()
}

// ---------------------------------------------------------------------------
// Catalog probe — REST health only at M8 (no WS per ADR-0024 stays Proposed)
// ---------------------------------------------------------------------------

async function probeCatalog(): Promise<void> {
  if (typeof window === 'undefined') return
  const ok = await probePlane(gw(CATALOG_PORT, '/api/health'), PROBE_TIMEOUT_MS)
  planes.catalog = ok
    ? { conn: 'live', retryAt: null, lastError: null }
    : { conn: 'simulated', retryAt: Date.now() + REPROBE_MS, lastError: 'no catalog plane' }
  bump()
  // Re-probe every 12 s while down.
  if (!ok) {
    catalogProbeTimer = setTimeout(probeCatalog, REPROBE_MS)
  } else {
    catalogProbeTimer = setTimeout(probeCatalog, REPROBE_MS * 5) // slow health re-check while live
  }
}

// ---------------------------------------------------------------------------
// Init / teardown — called from CanvasLoader.tsx (useEffect mount)
// ---------------------------------------------------------------------------

export function startTelemetry(): void {
  if (typeof window === 'undefined') return
  // Don't double-start (React strict-mode double-mounts in dev; v1's
  // next.config.ts already has reactStrictMode:false, but the guard holds).
  if (fleetSocket || mockTimer) return
  void startFleetSocket()
  void probeCatalog()
  // The sim plane's "live"/"simulated" status derives from whether any sim
  // sockets are open. We start the sim plane as "connecting" and the ladder
  // (mock or live) will flip it.
  planes.sim = { conn: 'connecting', retryAt: Date.now() + PROBE_TIMEOUT_MS, lastError: null }
}

// Welcome notification — fires once when the first fleet frame with vehicles arrives.
let welcomeNotified = false
function maybeShowWelcome(snap: FleetSnapshot): void {
  if (welcomeNotified) return
  if (snap.vehicles.length > 0) {
    welcomeNotified = true
    import('./app-store').then(({ pushNotification }) => {
      pushNotification({
        severity: 'info',
        title: `${snap.vehicles.length} SITL vehicle${snap.vehicles.length > 1 ? 's' : ''} connected`,
        detail: 'press ? for shortcuts · right-click map for context menu',
      })
    })
  }
}

export function stopTelemetry(): void {
  if (fleetSocket) {
    try {
      fleetSocket.close()
    } catch {
      // ignore
    }
    fleetSocket = null
  }
  for (const i of Array.from(simSocketsMap.keys())) closeSimSocket(i)
  simSocketsMap.clear()
  simSockets = []
  if (catalogProbeTimer) {
    clearTimeout(catalogProbeTimer)
    catalogProbeTimer = null
  }
  stopMockLadder()
}

// ---------------------------------------------------------------------------
// Public read helpers (consumed by zone furniture + MapCanvas)
// ---------------------------------------------------------------------------

/** Convenience: count of vehicles in the latest fleet snapshot (P11 source). */
export function getVehicleCount(): number {
  return vehicles.length
}

/** Per-plane state for ConnBadge components (the §5.4 contract). */
export function getPlaneStates(): typeof planes {
  return planes
}
