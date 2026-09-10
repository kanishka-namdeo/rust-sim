/**
 * GCS v2 Operations Canvas — app store (M8 skeleton, real).
 *
 * Spec: docs/GCS_V2_SPEC.md §9.3 + §13.1 M8 skeleton interface #2.
 *
 * `useSyncExternalStore` module store — the operator-surface state that
 * actually drives React: selection, map mode, overlay toggles, follow,
 * goto-pending, the notification queue. NO telemetry numerics live here —
 * those live in `state/telemetry-store.ts` and flow to the HUD through
 * `FlushLoop.tsx`'s ref registry at 60 fps (the spec's "telemetry numbers
 * never touch React state" rule, §9.2).
 *
 * M8 ships this as a real store (no command wiring — C adds that at M9). The
 * `app-store.mock.ts` sibling exists only for stream B's unit tests; import
 * paths never flip (no double implementation, §13.1 v1.2).
 */

import { useSyncExternalStore } from 'react'

// ---------------------------------------------------------------------------
// Types (M8 contract; C extends at M9 with command dispatch)
// ---------------------------------------------------------------------------

export type MapMode = 'fly' | 'plan' | 'fence' | 'corridor'

/** Overlay panel identifiers (zones A–H + F overlay panels, spec §2.2). */
export type OverlayId =
  | 'mission'
  | 'library'
  | 'fleet'
  | 'sim'
  | 'setup'
  | 'analyze'
  | 'preflight'
  | 'cheat'
  | 'settings'

export interface Notification {
  id: string
  severity: 'info' | 'warn' | 'error'
  title: string
  detail?: string
  /** Wall-clock ms when queued. */
  t: number
  /** Auto-dismiss in 6 s for info/warn; sticky for error (§8.6). */
  sticky?: boolean
  /** Click-through target panel (§8.6). */
  sourcePanel?: OverlayId
}

export interface AppStoreState {
  /** Active vehicle index (single-select; -1 = none selected). */
  activeVehicle: number
  /** Multi-select vehicle indices (Shift+drag box, §7.1; consumed by §8.4 chip row). */
  multiSelect: number[]
  /** Current map editing mode (§2.3 rule 1: Plan-on-live-map). */
  mapMode: MapMode
  /** Overlay panel toggles — max two open at once (§4.2). */
  overlays: Record<OverlayId, boolean>
  /** Follow-camera toggle (§6.5: camera eases to active vehicle ≥1 Hz). */
  follow: boolean
  /** Goto-mode pending (§7.1: G enters goto mode, click places target). */
  gotoPending: boolean
  /** NED inset view toggle (§6.5: V toggles the Fleet NED picture). */
  nedInset: boolean
  /** Notifications queue (max 5 visible, §8.6). */
  notifications: Notification[]
}

const INITIAL: AppStoreState = {
  activeVehicle: 0,
  multiSelect: [],
  mapMode: 'fly',
  overlays: {
    mission: false,
    library: false,
    fleet: false,
    sim: false,
    setup: false,
    analyze: false,
    preflight: false,
    cheat: false,
    settings: false,
  },
  follow: true,
  gotoPending: false,
  nedInset: false,
  notifications: [],
}

// ---------------------------------------------------------------------------
// Module-level store (useSyncExternalStore contract)
// ---------------------------------------------------------------------------

let state: AppStoreState = INITIAL
const listeners = new Set<() => void>()

/** Bump the version so `useSyncExternalStore` re-reads `getSnapshot`. */
let version = 0
const bump = () => {
  version++
  for (const fn of listeners) fn()
}

function setState(next: Partial<AppStoreState>): void {
  state = { ...state, ...next }
  bump()
}

export function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

export function getSnapshot(): AppStoreState {
  return state
}

export function getVersion(): number {
  return version
}

// ---------------------------------------------------------------------------
// Selectors (the only place state-mutation helpers live — C adds command
// dispatch at M9 by routing through command-bus.ts)
// ---------------------------------------------------------------------------

export function setActiveVehicle(i: number): void {
  if (i < 0) return
  setState({ activeVehicle: i, multiSelect: [] })
}

export function setMultiSelect(indices: number[]): void {
  setState({ multiSelect: indices })
}

export function setMapMode(mode: MapMode): void {
  setState({ mapMode: mode })
}

/**
 * Toggle an overlay. Spec §4.2 stacking rule: at ≥ 1600 px a second overlay
 * may open side by side (each ≤ 32 % width, column C auto-collapses while two
 * are open); below 1600 px opening a second collapses the first.
 *
 * For M8 this is the simple toggler; the stacking arithmetic (which depends
 * on viewport width + column C collapse) lands with the OperationsCanvas
 * layout in T-B1.
 */
export function toggleOverlay(id: OverlayId, opts?: { force?: boolean }): void {
  const force = opts?.force
  const next = { ...state.overlays }
  if (force === true) {
    next[id] = true
  } else if (force === false) {
    next[id] = false
  } else {
    next[id] = !next[id]
  }
  setState({ overlays: next })
}

export function setFollow(on: boolean): void {
  setState({ follow: on })
}

export function setGotoPending(on: boolean): void {
  setState({ gotoPending: on })
}

export function setNedInset(on: boolean): void {
  setState({ nedInset: on })
}

// ---------------------------------------------------------------------------
// Notification queue (§8.6 — max 5 visible, 6 s auto-dismiss, errors sticky)
// ---------------------------------------------------------------------------

const NOTIFICATION_LIMIT = 5

export function pushNotification(n: Omit<Notification, 'id' | 't'>): string {
  const id = `n${Date.now()}-${Math.random().toString(36).slice(2, 7)}`
  const note: Notification = { ...n, id, t: Date.now() }
  const next = [...state.notifications, note]
  // Cap at NOTIFICATION_LIMIT (the visible queue; errors remain sticky until
  // acknowledged, so they survive the cap).
  while (next.length > NOTIFICATION_LIMIT) {
    const idx = next.findIndex((x) => !x.sticky)
    if (idx === -1) next.shift()
    else next.splice(idx, 1)
  }
  setState({ notifications: next })
  // Auto-dismiss for info/warn (6 s); errors stay until dismissed.
  if (note.severity !== 'error' && !note.sticky) {
    setTimeout(() => dismissNotification(id), 6000)
  }
  return id
}

export function dismissNotification(id: string): void {
  setState({ notifications: state.notifications.filter((n) => n.id !== id) })
}

// ---------------------------------------------------------------------------
// React hook (consumed by zone furniture + overlays)
// ---------------------------------------------------------------------------

export function useAppStore(): AppStoreState {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}
