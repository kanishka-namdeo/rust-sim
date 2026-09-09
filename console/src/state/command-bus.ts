/**
 * GCS v2 Operations Canvas — command bus (M8 signature only; M9 implementation).
 *
 * Spec: docs/GCS_V2_SPEC.md §9.4 + §13.1 M8 skeleton interface #5.
 *
 * M8 ships the SIGNATURE only — stream B renders busy/error surfaces against
 * it; stream C implements the dispatch at M9. Every flight verb from §7.2/§7.3
 * routes through here at M9 — one place to log, one place to gate.
 *
 * The shape:
 *   `command(name, vehicle?, payload?) → Promise<{ok} | {error:{code,message}}>`
 *
 * Plus the busy registry so buttons/keys can render enabled/disabled against
 * in-flight POSTs (§8.4: "Busy state (in-flight POST) = spinner chip 400 ms
 * min display"), and the deferred-undo wrapper for delete-class verbs
 * (§7.4: "tap + 6 s undo toast").
 *
 * The M8 stub returns a "not implemented in M8" error so any v2 widget that
 * calls `command()` during M8 (before the verb controls are wired) renders
 * visibly disabled per §13.1 ("E-STOP and verb controls render disabled
 * (command bus lands M9)"). Stream B's verbs check `isBusy(name)` and gate
 * themselves on the M9 promise.
 */

export type CommandResult = { ok: true } | { ok: false; error: { code: string; message: string } }

export type CommandName =
  | 'arm' // §7.3.1 A: hold-to-confirm ARM, body {arm:true}
  | 'disarm' // §7.3.1 D: tap disarm, body {arm:false}
  | 'takeoff' // §7.3.1 T: alt chip → hold-to-confirm
  | 'land' // §7.3.1 L: tap, safety-positive
  | 'rtl' // §7.3.1 R: tap, safety-positive
  | 'hold' // §7.3.1 H: tap, safety-positive
  | 'estop_fleet' // §7.3.1 E: immediate POST /api/fleet/estop (keyup-fire)
  | 'estop_sim' // §7.2 M2 SIM E-STOP: POST :8200+i/api/estop (per-vehicle)
  | 'goto' // §7.3.1 G: place → confirm → POST /api/vehicles/{i}/goto
  | 'mission_start' // §7.2 M2: POST /api/mission/start (hold-to-confirm)
  | 'mission_upload' // §8.5 MissionStrip: 3-type via :8400 /mission/upload
  | 'mission_clear' // §7.2 M2: POST /api/mission/clear
  | 'scenario_hotswap' // §7.2 M1: file-open .toml → PUT /api/fleet (ADR-0018)
  | 'fault_inject' // §7.2 M2: fault picker → POST :8200+i/api/faults
  | 'fault_clear' // §7.2 M2: DELETE :8200+i/api/faults/{id}
  | 'calibrate' // §8.5 Setup: POST /api/vehicles/{i}/calibrate
  | 'airframe_apply' // §8.5 Setup: POST /api/vehicles/{i}/airframe (hold-to-confirm)
  | 'param_write' // §8.5 Setup: POST /api/vehicles/{i}/params/write
  | 'mode_set' // §8.5 Setup: POST /api/vehicles/{i}/mode
  | 'preset_save' // §8.5 Setup: POST /api/vehicles/{i}/param-presets
  | 'preset_load' // §8.5 Setup: POST /api/vehicles/{i}/param-presets/{name}/load
  | 'preset_delete' // §8.5 Setup: DELETE /api/vehicles/{i}/param-presets/{name}
  | 'fleet_start' // §8.5 FleetC2: POST /api/fleet/start (hold-to-confirm)
  | 'task_append' // §7.2 M5: POST /api/tasks (re-auction via append)

// ---------------------------------------------------------------------------
// Busy registry — buttons/keys render disabled while a verb is in-flight.
// Module-level so the registry survives re-renders (no React state for the
// registry itself — consumers subscribe via useSyncExternalStore-style hook).
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

export function getBusySnapshot(): { name: CommandName; startedAt: number } | null {
  if (busy.size === 0) return null
  const [name, startedAt] = busy.entries().next().value!
  return { name, startedAt }
}

export function getBusyVersion(): number {
  return busyVersion
}

export function isBusy(name: CommandName): boolean {
  return busy.has(name)
}

// ---------------------------------------------------------------------------
// Deferred-undo wrapper for delete-class verbs (§7.4 + §9.4). M9 implements
// the actual deferred commit; M8 exports the type so B can render the undo
// toast scaffold now and C swaps in the real wiring at M9.
// ---------------------------------------------------------------------------

export interface DeferredUndo<T> {
  /** The id of the deleted/affected item (for undo targeting). */
  id: string
  /** Apply the deletion now (or the action whose undo we hold for 6 s). */
  commit: () => Promise<T>
  /** Undo within the 6 s window (returns true if undo applied, false if committed already). */
  undo: () => Promise<boolean>
  /** The 6 s window in ms (spec §7.4). */
  windowMs: number
}

// ---------------------------------------------------------------------------
// command() — M8 stub.
//
// Returns a deterministic "not_implemented" error so any v2 widget that calls
// `command()` during M8 surfaces a visible "command bus lands at M9" message
// rather than silently executing. Stream B gates on `isBusy(name)` to render
// the disabled state; the stub never enters the busy registry (M9 will).
// ---------------------------------------------------------------------------

export async function command(
  _name: CommandName,
  _vehicle?: number,
  _payload?: Record<string, unknown>,
): Promise<CommandResult> {
  return {
    ok: false,
    error: {
      code: 'not_implemented',
      message: 'command bus lands at M9 (GCS v2 spec §13.1 M8 skeleton interface #5)',
    },
  }
}

// Internal helpers exported for the M9 implementation (no-op at M8 — kept here
// so the M9 commit can flip the implementation without touching the surface).

export function _markBusy(name: CommandName): void {
  busy.set(name, Date.now())
  bumpBusy()
}

export function _clearBusy(name: CommandName): void {
  busy.delete(name)
  bumpBusy()
}
