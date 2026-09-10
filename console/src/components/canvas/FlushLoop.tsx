'use client'

/**
 * GCS v2 Operations Canvas — the single rAF flush loop (M8, T-A3).
 *
 * Spec: docs/GCS_V2_SPEC.md §9.2 + §13.1 M8 skeleton interface #4.
 *
 * One `requestAnimationFrame` loop, owned by stream A, that every frame:
 *
 *  1. Reads the telemetry store's ring buffers + last fleet snapshot.
 *  2. Writes to the HUD ref registry — module-level `Map<string, HTMLElement>`
 *     registered by zone widgets via the `useHudRef(id)` hook. Writes go
 *     through `textContent` and `style.transform` only — no React state,
 *     no per-frame DOM queries, no prop drilling. This is the A↔B seam
 *     that lets three streams work in parallel without coupling.
 *  3. Increments `window.__rsimTelemetry.framesRendered` (the G-14/G-20
 *     dropped-frame budget assertion target).
 *
 * The loop is started by `OperationsCanvas.tsx` (stream B) on mount and
 * stopped on unmount. The `__rsimMapDebug` map-side counterpart (loadedAtMs,
 * pitch, bearing, layers, mode) is owned by `MapCanvas.tsx` and updated by
 * its own `load`/`moveend`/`sourcedata` handlers — this loop never touches
 * MapLibre directly (no per-frame `setPaintProperty` churn, §6.4).
 *
 * React state commits happen ≤ 5 Hz in the telemetry store's
 * `maybeCommitReactState` (§9.2); this loop never calls React state setters.
 */

import { useEffect, useRef } from 'react'
import { getFleetSnapshot, getStrip, getTrack, _incrementFramesRendered } from '@/state/telemetry-store'

// ---------------------------------------------------------------------------
// HUD ref registry — module-level Map<id, HTMLElement>
// ---------------------------------------------------------------------------

type RefId =
  | `v${number}_alt` // vehicle i alt (m AGL)
  | `v${number}_battery` // vehicle i battery (%)
  | `v${number}_voltage` // vehicle i voltage (V)
  | `v${number}_mode` // vehicle i mode string
  | `v${number}_fsm` // vehicle i FSM badge
  | `v${number}_yaw` // vehicle i yaw (deg)
  | `v${number}_roll` // vehicle i roll (deg) — HUD
  | `v${number}_pitch` // vehicle i pitch (deg) — HUD
  | `v${number}_hud_transform` // vehicle i attitude HUD horizon transform
  | `v${number}_callsign` // vehicle i callsign label (v0/v1)
  | `v${number}_fix` // vehicle i GPS fix (n sat / fix_type)
  | `v${number}_speed` // vehicle i ground speed (m/s)
  | `phase_strip` // fleet phase in status strip
  | `clock_sim` // sim time mm:ss.d
  | `clock_wall` // wall clock HH:MM:SS

const hudRefs = new Map<RefId, HTMLElement>()

/** Register a DOM node as a flush target. Returns an unregister function. */
export function registerHudRef(id: RefId, el: HTMLElement | null): () => void {
  if (el) hudRefs.set(id, el)
  else hudRefs.delete(id)
  return () => hudRefs.delete(id)
}

/**
 * `useHudRef(id)` — the widget-side hook. Returns a ref callback to attach
 * to a DOM node. The loop writes `textContent` / `style.transform` directly
 * each frame; React never re-renders the widget for telemetry changes.
 */
export function useHudRef(id: RefId) {
  const ref = useRef<HTMLElement | null>(null)
  // Set the ref + register/unregister with the module map.
  const setRef = (el: HTMLElement | null) => {
    ref.current = el
    registerHudRef(id, el)
  }
  // Cleanup on unmount.
  useEffect(() => {
    return () => {
      registerHudRef(id, null)
    }
  }, [id])
  return setRef
}

// ---------------------------------------------------------------------------
// Per-frame write helpers
// ---------------------------------------------------------------------------

function setText(id: RefId, value: string): void {
  const el = hudRefs.get(id)
  if (el && el.textContent !== value) el.textContent = value
}

function setTransform(id: RefId, transform: string): void {
  const el = hudRefs.get(id)
  if (el && el.style.transform !== transform) el.style.transform = transform
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
  const pitch = Math.abs(sinp) >= 1 ? Math.sign(sinp) * (Math.PI / 2) : Math.asin(sinp)
  const yaw = Math.atan2(2 * (w * z + x * y), 1 - 2 * (y * y + z * z))
  return {
    roll_deg: (roll * 180) / Math.PI,
    pitch_deg: (pitch * 180) / Math.PI,
    yaw_deg: ((yaw * 180) / Math.PI + 360) % 360,
  }
}

function fmt(v: number, digits = 1, fallback = '—'): string {
  if (!Number.isFinite(v)) return fallback
  return v.toFixed(digits)
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

let rafId: number | null = null
let framesRendered = 0
let lastFlushAt = 0

function flush(): void {
  const snap = getFleetSnapshot()
  framesRendered++
  _incrementFramesRendered() // §9.2: __rsimTelemetry.framesRendered reads via getter
  lastFlushAt = performance.now()

  if (snap) {
    setText('phase_strip', snap.phase)
    // Per-vehicle HUD writes. Vehicles not present in the registry are
    // skipped cheaply (Map.get returns undefined).
    for (const v of snap.vehicles) {
      const i = v.index
      setText(`v${i}_alt`, fmt(v.alt_agl_m ?? 0, 1))
      setText(`v${i}_battery`, fmt(v.battery_pct, 0))
      setText(`v${i}_voltage`, v.voltage_v != null ? fmt(v.voltage_v, 1) : '—')
      setText(`v${i}_mode`, v.mode)
      setText(`v${i}_fsm`, v.fsm)
      setText(`v${i}_yaw`, fmt(v.yaw_deg, 0))
      setText(`v${i}_callsign`, `v${i}`)
      // Speed: horizontal component of velocity_ned_ms.
      const speed = Math.hypot(v.velocity_ned_ms[0], v.velocity_ned_ms[1])
      setText(`v${i}_speed`, fmt(speed, 1))
      // GPS fix/sats — the sim frame was the wire source; with the sim
      // socket ladder removed, this slot shows "—" until the fleet plane
      // surfaces GPS fix/sats on the vehicle record directly.
      setText(`v${i}_fix`, '—')
      // Attitude HUD: roll/pitch from attitude_q_wxyz (P5 fix). Yaw-only
      // fallback (roll/pitch = 0) if absent.
      const q = v.attitude_q_wxyz ?? null
      if (q) {
        const { roll_deg, pitch_deg } = quatToRollPitchYaw(q)
        setText(`v${i}_roll`, fmt(roll_deg, 1))
        setText(`v${i}_pitch`, fmt(pitch_deg, 1))
        // SVG horizon transform: rotate(roll) translateY(pitch * scale).
        // The exact scale is the SVG component's contract; here we use 1.5
        // px/deg (the §8.3 HUD's convention will be wired by stream B's
        // TelemetryColumn component).
        setTransform(
          `v${i}_hud_transform`,
          `rotate(${roll_deg.toFixed(2)}deg) translateY(${(pitch_deg * 1.5).toFixed(2)}px)`,
        )
      } else {
        setText(`v${i}_roll`, '0.0')
        setText(`v${i}_pitch`, '0.0')
        setTransform(`v${i}_hud_transform`, '')
      }
    }
  }

  // Clock: sim time clock is gone with the sim socket ladder; the wall
  // clock remains.
  setText('clock_sim', '—')
  const now = new Date()
  setText('clock_wall', now.toUTCString().slice(17, 25))

  rafId = requestAnimationFrame(flush)
}

// ---------------------------------------------------------------------------
// React mount point — OperationsCanvas.tsx renders <FlushLoop/> once.
// ---------------------------------------------------------------------------

export function FlushLoop(): null {
  useEffect(() => {
    if (rafId == null) {
      rafId = requestAnimationFrame(flush)
    }
    return () => {
      if (rafId != null) {
        cancelAnimationFrame(rafId)
        rafId = null
      }
    }
  }, [])
  return null
}

// Expose a debug getter for gates / human inspector (`?debug=ws` overlay).
if (typeof window !== 'undefined') {
  ;(window as unknown as { __rsimFlushLoop?: unknown }).__rsimFlushLoop = {
    get framesRendered() {
      return framesRendered
    },
    get lastFlushAt() {
      return lastFlushAt
    },
    get registeredHudRefs() {
      return hudRefs.size
    },
  }
}
