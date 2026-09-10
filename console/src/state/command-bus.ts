/**
 * GCS v2 Operations Canvas — command bus (M9 implementation).
 *
 * Spec: docs/GCS_V2_SPEC.md §9.4 + §7.4 + §7.3.1 + §13.1 M8 skeleton interface #5.
 *
 * Every flight verb from §7.2/§7.3 routes through here — one place to log,
 * one place to gate. M9 implements the actual dispatch:
 *
 *  - `command(name, vehicle?, payload?) → Promise<CommandResult>`
 *  - Busy registry: `isBusy(name)` / `subscribeBusy` / `useBusy()` hook so
 *    buttons/keys render enabled/disabled against in-flight POSTs (§8.4
 *    "Busy state = spinner chip 400 ms min display").
 *  - State gating: §7.4 — verbs dim with reason tooltips when the backend
 *    state would reject them (vehicle not READY, mission not flying, etc.).
 *  - Hold-to-confirm: §7.4 — ARM, TAKEOFF, START MISSION, START FLEET,
 *    airframe-apply use a 400 ms amber progress fill (implemented in the
 *    verb button component; the command bus just exposes `holdConfirmMs`).
 *  - Deferred-undo: §7.4 — delete-class verbs (delete waypoint/vertex,
 *    delete mission/preset) get a 6 s undo toast (the commit is deferred).
 *  - E-stop: §2.3 rule 5 — fires on keyup, no hold-to-confirm, single
 *    deliberate action, amber-on-dark, firewalled from takeoff-class verbs.
 *
 * The command bus NEVER calls React state setters directly — success/error
 * surfaces through the notification queue (app-store `pushNotification`) and
 * the busy registry's `useSyncExternalStore`-style subscription.
 */

import { useSyncExternalStore } from 'react'
import { gw, fetchGw } from '@/lib/conn'
import { pushNotification } from '@/state/app-store'
import type { FleetVehicle } from '@/lib/types'
import { getFleetSnapshot } from '@/state/telemetry-store'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type CommandResult = { ok: true; data?: unknown } | { ok: false; error: { code: string; message: string } }

export type CommandName =
  | 'arm'
  | 'disarm'
  | 'takeoff'
  | 'land'
  | 'rtl'
  | 'hold'
  | 'estop_fleet'
  | 'goto'
  | 'mission_start'
  | 'mission_upload'
  | 'mission_clear'
  | 'sitl_start'
  | 'sitl_stop'
  | 'calibrate'
  | 'airframe_apply'
  | 'param_write'
  | 'mode_set'
  | 'preset_save'
  | 'preset_load'
  | 'preset_delete'
  | 'fleet_start'

/** Spec §7.4: hold-to-confirm verbs (400 ms amber progress fill). */
const HOLD_CONFIRM_COMMANDS: ReadonlySet<CommandName> = new Set([
  'arm',
  'takeoff',
  'mission_start',
  'fleet_start',
  'airframe_apply',
  'sitl_start',
])

export const HOLD_CONFIRM_MS = 400 // §7.4

/** Spec §7.4: safety-positive verbs (single tap, no hold-to-confirm). */
const SAFETY_POSITIVE: ReadonlySet<CommandName> = new Set([
  'disarm',
  'land',
  'rtl',
  'hold',
  'estop_fleet',
  'mission_clear',
  'sitl_stop',
])

export function isHoldConfirm(name: CommandName): boolean {
  return HOLD_CONFIRM_COMMANDS.has(name)
}

export function isSafetyPositive(name: CommandName): boolean {
  return SAFETY_POSITIVE.has(name)
}

// ---------------------------------------------------------------------------
// Busy registry — buttons/keys render disabled while a verb is in-flight.
// ---------------------------------------------------------------------------

const busy = new Map<CommandName, number>() // value = started_at ms
const busyListeners = new Set<() => void>()
let busyVersion = 0

function bumpBusy(): void {
  busyVersion++
  for (const fn of busyListeners) fn()
}

export function subscribeBusy(fn: () => void): () => void {
  busyListeners.add(fn)
  return () => busyListeners.delete(fn)
}

/** Returns the first busy verb (for the §8.4 spinner chip) or null. */
export function getBusySnapshot(): { name: CommandName; startedAt: number } | null {
  if (busy.size === 0) return null
  const [name, startedAt] = busy.entries().next().value as [CommandName, number]
  return { name, startedAt }
}

export function getBusyVersion(): number {
  return busyVersion
}

export function isBusy(name: CommandName): boolean {
  return busy.has(name)
}

/** React hook: re-renders the caller when the busy registry changes. */
export function useBusy(): { name: CommandName; startedAt: number } | null {
  return useSyncExternalStore(
    subscribeBusy,
    getBusySnapshot,
    getBusySnapshot,
  )
}

// ---------------------------------------------------------------------------
// State gating — §7.4 + §2.3 rule 2. Determines whether a verb is enabled
// given the current fleet snapshot + active vehicle. Returns a reason
// string when disabled (rendered as a tooltip per §2.3 rule 2).
// ---------------------------------------------------------------------------

export interface VerbGuardResult {
  enabled: boolean
  reason?: string
}

/**
 * Check whether a verb is enabled for the given vehicle, given the live
 * fleet snapshot. The command bar never renders an enabled verb the backend
 * would reject (§2.3 rule 2).
 */
export function guardVerb(name: CommandName, vehicle: number | undefined): VerbGuardResult {
  // E-stop is always enabled (§7.3.1: "always enabled, fires on keyup").
  if (name === 'estop_fleet') {
    return { enabled: true }
  }
  // SITL lifecycle verbs are always enabled (no vehicle required) — the
  // supervisor owns the mavfleet process; the verbs work whether the fleet
  // is running or not.
  if (name === 'sitl_start' || name === 'sitl_stop') {
    return { enabled: true }
  }

  if (vehicle == null) {
    return { enabled: false, reason: 'no active vehicle' }
  }

  const snap = getFleetSnapshot()
  if (!snap) {
    return { enabled: false, reason: 'no fleet snapshot (SIMULATED or connecting)' }
  }

  const v = snap.vehicles.find((x) => x.index === vehicle)
  if (!v) {
    return { enabled: false, reason: `vehicle v${vehicle} not in fleet` }
  }

  switch (name) {
    case 'arm':
      // §7.3.1 A: prearm checks + fsm READY. M9 doesn't fetch prearm here —
      // the PreFlight panel does that; the ARM button's full gate is
      // "fsm === 'READY' && !armed". M10 wires the prearm-check fetch.
      if (v.armed) return { enabled: false, reason: `v${vehicle} already armed` }
      if (v.fsm !== 'READY') return { enabled: false, reason: `v${vehicle} not READY (fsm=${v.fsm})` }
      return { enabled: true }
    case 'disarm':
      if (!v.armed) return { enabled: false, reason: `v${vehicle} not armed` }
      return { enabled: true }
    case 'takeoff':
      // §7.3.1 T: armed, disarmed→auto-arm attempt? NO — arm separately.
      if (!v.armed) return { enabled: false, reason: 'arm first (A)' }
      return { enabled: true }
    case 'land':
    case 'rtl':
      // §7.3.1 L/R: armed
      if (!v.armed) return { enabled: false, reason: `v${vehicle} not armed` }
      return { enabled: true }
    case 'hold':
      // §7.3.1 H: missionFlying or armed
      if (!v.armed) return { enabled: false, reason: `v${vehicle} not armed` }
      return { enabled: true }
    case 'goto':
      // §7.3.1 G: vehicle selected (always true here since vehicle != null)
      return { enabled: true }
    case 'mission_start':
      // §7.2 M2: armed (the mission needs to be uploaded first — M10 wires upload)
      if (!v.armed) return { enabled: false, reason: 'arm first (A)' }
      return { enabled: true }
    case 'mission_clear':
      return { enabled: true }
    case 'mission_upload':
      return { enabled: true }
    default:
      return { enabled: true }
  }
}

// ---------------------------------------------------------------------------
// Deferred-undo wrapper for delete-class verbs (§7.4 + §9.4).
// ---------------------------------------------------------------------------

export interface DeferredUndo<T> {
  id: string
  commit: () => Promise<T>
  undo: () => Promise<boolean>
  windowMs: number
}

const DEFERRED_UNDO_MS = 6000 // §7.4

/**
 * Wrap a delete-class verb in a 6 s deferred-undo window. The caller
 * schedules the commit; if `undo()` is called within the window, the
 * commit is cancelled. Used by §7.4 delete-waypoint / delete-vertex /
 * delete-mission / delete-preset (M10..M13 overlay panels).
 */
export function withDeferredUndo<T>(
  id: string,
  commitFn: () => Promise<T>,
  undoFn: () => Promise<void>,
): DeferredUndo<T> {
  let committed = false
  let timer: ReturnType<typeof setTimeout> | null = null
  const commit = async () => {
    if (committed) return commitFn()
    committed = true
    if (timer) clearTimeout(timer)
    return commitFn()
  }
  const undo = async () => {
    if (committed) return false
    committed = true
    if (timer) clearTimeout(timer)
    await undoFn()
    return true
  }
  timer = setTimeout(() => {
    if (!committed) {
      committed = true
      void commitFn().catch((e) => {
        pushNotification({
          severity: 'error',
          title: 'deferred commit failed',
          detail: String(e),
        })
      })
    }
  }, DEFERRED_UNDO_MS)
  return { id, commit, undo, windowMs: DEFERRED_UNDO_MS }
}

// ---------------------------------------------------------------------------
// command() — the M9 dispatch.
//
// Routes every flight verb to its REST endpoint via the gateway (gw()).
// Marks the verb busy for the duration of the POST so buttons/keys render
// disabled (§8.4 spinner chip). On success: optional success toast. On
// error: notification + busy clear. E-stop fires immediately (no hold-
// to-confirm — the hold-to-confirm is a UI concern, not a command-bus
// concern; the verb button component implements the 400 ms press timer).
// ---------------------------------------------------------------------------

const FLEET_PORT = 8400
const SUPERVISOR_PORT = 8500

export async function command(
  name: CommandName,
  vehicle?: number,
  payload?: Record<string, unknown>,
): Promise<CommandResult> {
  // Gate first — refuse if the verb isn't enabled for the current state.
  const guard = guardVerb(name, vehicle)
  if (!guard.enabled) {
    const reason = guard.reason ?? 'verb not enabled'
    pushNotification({
      severity: 'warn',
      title: `${name} blocked`,
      detail: reason,
    })
    return { ok: false, error: { code: 'guard_failed', message: reason } }
  }

  busy.set(name, Date.now())
  bumpBusy()

  try {
    const result = await dispatch(name, vehicle, payload)
    if (result.ok) {
      // Success toast for safety-positive verbs (the operator wants
      // confirmation that LAND/RTL/HOLD landed). Hold-to-confirm verbs
      // get their success toast from the verb button component.
      if (isSafetyPositive(name) && name !== 'estop_fleet') {
        pushNotification({
          severity: 'info',
          title: `${name} OK`,
          detail: vehicle != null ? `v${vehicle}` : undefined,
        })
      }
    } else {
      pushNotification({
        severity: 'error',
        title: `${name} failed`,
        detail: `${result.error.code}: ${result.error.message}`,
      })
    }
    return result
  } finally {
    busy.delete(name)
    bumpBusy()
  }
}

// ---------------------------------------------------------------------------
// dispatch() — the actual REST call per verb. One switch, one place.
// ---------------------------------------------------------------------------

async function dispatch(
  name: CommandName,
  vehicle: number | undefined,
  payload: Record<string, unknown> = {},
): Promise<CommandResult> {
  try {
    switch (name) {
      // --- Flight verbs (per-vehicle, via :8400) -------------------------
      case 'arm':
        return await postVehicle(vehicle, '/arm', { arm: true })
      case 'disarm':
        return await postVehicle(vehicle, '/arm', { arm: false })
      case 'takeoff':
        return await postVehicle(vehicle, '/takeoff', { alt_m: payload.alt_m ?? 5 })
      case 'land':
        return await postVehicle(vehicle, '/land', {})
      case 'rtl':
        return await postVehicle(vehicle, '/rtl', {})
      case 'hold':
        return await postVehicle(vehicle, '/hold', {})
      case 'goto': {
        // §7.1: POST /api/vehicles/{i}/goto with {lat, lon, alt_m?}
        if (payload.lat == null || payload.lon == null) {
          return { ok: false, error: { code: 'bad_payload', message: 'goto requires lat + lon' } }
        }
        return await postVehicle(vehicle, '/goto', {
          lat: payload.lat,
          lon: payload.lon,
          ...(payload.alt_m != null ? { alt_m: payload.alt_m } : {}),
        })
      }
      // --- Mission verbs (per-vehicle, via :8400) -----------------------
      case 'mission_start':
        return await postVehicle(vehicle, '/mission/start', {})
      case 'mission_clear':
        return await postVehicle(vehicle, '/mission/clear', {})
      case 'mission_upload':
        // §8.5 MissionStrip: 3-type via :8400 /api/vehicles/{i}/mission/upload
        return await postVehicle(vehicle, '/mission/upload', payload)

      // --- Fleet verbs (via :8400 fleet endpoints) ---------------------
      case 'estop_fleet': {
        // §7.3.1 E: immediate POST /api/fleet/estop (keyup-fire)
        const res = await fetchGw(gw(FLEET_PORT, '/api/fleet/estop'), {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({}),
        })
        return await unwrapResult(res)
      }
      case 'fleet_start':
        return await unwrapResult(
          await fetchGw(gw(FLEET_PORT, '/api/fleet/start'), {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(payload),
          }),
        )

      // --- SITL lifecycle verbs (via :8500 supervisor) -------------------
      case 'sitl_start': {
        const scenario = typeof payload.scenario === 'string' ? payload.scenario : undefined
        const res = await fetchGw(gw(SUPERVISOR_PORT, '/api/sitl/start'), {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ scenario: scenario ?? null }),
        })
        return await unwrapResult(res)
      }
      case 'sitl_stop': {
        const res = await fetchGw(gw(SUPERVISOR_PORT, '/api/sitl/stop'), {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({}),
        })
        return await unwrapResult(res)
      }

      // --- Setup verbs (per-vehicle, via :8400) ------------------------
      case 'calibrate':
        return await postVehicle(vehicle, '/calibrate', payload)
      case 'airframe_apply':
        return await postVehicle(vehicle, '/airframe', payload)
      case 'param_write':
        return await postVehicle(vehicle, '/params/write', payload)
      case 'mode_set':
        return await postVehicle(vehicle, '/mode', payload)
      // --- Preset verbs (per-vehicle, via :8300 catalog) ---------------
      case 'preset_save':
        return await postCatalogVehicle(vehicle, '/param-presets', payload)
      case 'preset_load': {
        const presetName = payload.name as string | undefined
        if (!presetName) return { ok: false, error: { code: 'bad_payload', message: 'preset_load requires name' } }
        return await postCatalogVehicle(vehicle, `/param-presets/${encodeURIComponent(presetName)}/load`, {})
      }
      case 'preset_delete': {
        const presetName = payload.name as string | undefined
        if (!presetName) return { ok: false, error: { code: 'bad_payload', message: 'preset_delete requires name' } }
        if (vehicle == null) return { ok: false, error: { code: 'no_vehicle', message: 'preset_delete needs a vehicle' } }
        const res = await fetchGw(gw(8300, `/api/vehicles/${vehicle}/param-presets/${encodeURIComponent(presetName)}`), {
          method: 'DELETE',
        })
        return await unwrapResult(res)
      }

      default:
        return { ok: false, error: { code: 'unknown_verb', message: `unknown command: ${name}` } }
    }
  } catch (e) {
    return { ok: false, error: { code: 'network', message: e instanceof Error ? e.message : String(e) } }
  }
}

// ---------------------------------------------------------------------------
// Helpers — POST to a per-vehicle endpoint on :8400 or :8300, unwrap the
// {ok,data} envelope into a CommandResult.
// ---------------------------------------------------------------------------

async function postVehicle(
  vehicle: number | undefined,
  path: string,
  body: Record<string, unknown>,
): Promise<CommandResult> {
  if (vehicle == null) return { ok: false, error: { code: 'no_vehicle', message: `${path} needs a vehicle` } }
  const url = gw(FLEET_PORT, `/api/vehicles/${vehicle}${path}`)
  const res = await fetchGw(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  return await unwrapResult(res)
}

async function postCatalogVehicle(
  vehicle: number | undefined,
  path: string,
  body: Record<string, unknown>,
): Promise<CommandResult> {
  if (vehicle == null) return { ok: false, error: { code: 'no_vehicle', message: `${path} needs a vehicle` } }
  const url = gw(8300, `/api/vehicles/${vehicle}${path}`)
  const res = await fetchGw(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  return await unwrapResult(res)
}

async function unwrapResult(res: Response): Promise<CommandResult> {
  if (!res.ok) {
    let code = `http_${res.status}`
    let message = res.statusText
    try {
      const j = (await res.json()) as { error?: { code?: string; message?: string } }
      if (j?.error?.code) code = j.error.code
      if (j?.error?.message) message = j.error.message
    } catch {
      // not JSON — keep the status text
    }
    return { ok: false, error: { code, message } }
  }
  try {
    const j = (await res.json()) as { ok?: boolean; data?: unknown; error?: { code: string; message: string } }
    if (j && j.ok === false && j.error) {
      return { ok: false, error: j.error }
    }
    return { ok: true, data: j?.data }
  } catch {
    // 2xx non-JSON — treat as success
    return { ok: true }
  }
}

// ---------------------------------------------------------------------------
// Internal helpers kept for backward compat with M8 callers (no-op now).
// ---------------------------------------------------------------------------

export function _markBusy(name: CommandName): void {
  busy.set(name, Date.now())
  bumpBusy()
}

export function _clearBusy(name: CommandName): void {
  busy.delete(name)
  bumpBusy()
}
