'use client'

/**
 * GCS v2 Operations Canvas — Status strip (Zone A, M8 T-B1).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.1 + §2.2 zone A.
 *
 * Fixed 48px top, full width, pointer-events:auto. Never collapses; the
 * E-STOP button is always visible. Renders:
 *  - Brand chip `RSIM` + fleet phase badge
 *  - Per-vehicle chips `v0 READY · 87 %`
 *  - Per-plane ConnBadge (fleet/sim/catalog) — reuses the v1 §5.4 contract;
 *    M8 reads from the telemetry store's `getPlaneStates()`
 *  - Clock: sim time + wall clock (mono, tabular numerics)
 *  - E-STOP button (amber, rightmost, fires `POST /api/fleet/estop` —
 *    command bus lands M9, so M8 renders the button visibly DISABLED
 *    per §13.1 "E-STOP and verb controls render disabled").
 *
 * The status strip is the M8 ConnBadge home; the v1 `ConnBadge` component
 * retires at M14 (spec §3.1).
 */

import { useTelemetrySnapshot, getPlaneStates, getFleetSnapshot } from '@/state/telemetry-store'
import { useAppStore, setActiveVehicle } from '@/state/app-store'
import { useHudRef } from '../FlushLoop'
import { PlaneBadges } from './PlaneBadges'

export function StatusStrip() {
  const snap = useTelemetrySnapshot()
  const app = useAppStore()
  const fleetSnap = getFleetSnapshot()
  const phase = fleetSnap?.phase ?? 'INIT'

  const setPhase = useHudRef('phase_strip')
  const setSimClock = useHudRef('clock_sim')
  const setWallClock = useHudRef('clock_wall')

  return (
    <div
      data-rsim-zone="A"
      className="pointer-events-auto absolute top-0 left-0 right-0 flex items-center gap-3 px-4 border-b"
      style={{ height: 48, zIndex: 30 }}
    >
      {/* Brand + fleet phase */}
      <div className="flex items-center gap-2">
        <span className="rsim-chip" style={{ background: 'rgba(34, 211, 238, 0.08)' }}>
          RSIM
        </span>
        <span
          ref={setPhase}
          className="rsim-mono"
          style={{
            fontSize: 11,
            fontWeight: 600,
            letterSpacing: '0.04em',
            color: 'var(--rsim-text-dim)',
            textTransform: 'uppercase',
          }}
        >
          {phase}
        </span>
      </div>

      {/* Vehicle chips — click = setActiveVehicle */}
      <div className="flex items-center gap-1">
        {snap.vehicles.map((v) => {
          const active = v.index === app.activeVehicle
          return (
            <button
              key={v.id}
              onClick={() => setActiveVehicle(v.index)}
              className="rsim-chip rsim-mono"
              style={{
                cursor: 'pointer',
                color: active ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)',
                borderColor: active ? 'var(--rsim-accent)' : 'var(--rsim-border)',
                background: active ? 'rgba(34, 211, 238, 0.08)' : 'transparent',
              }}
              aria-pressed={active}
              aria-label={`Vehicle ${v.index}: ${v.fsm}, ${Math.round(v.battery_pct)}%`}
            >
              <span>v{v.index}</span>
              <span>{v.fsm}</span>
              <span>{Math.round(v.battery_pct)}%</span>
            </button>
          )
        })}
        {snap.vehicles.length === 0 && (
          <span className="rsim-chip rsim-mono" style={{ color: 'var(--rsim-text-dim)' }}>
            no vehicles
          </span>
        )}
      </div>

      {/* Plane ConnBadges — fleet/sim/catalog LIVE/SIMULATED/CONNECTING */}
      <div className="flex items-center gap-1">
        <PlaneBadges />
      </div>

      {/* Spacer + clocks + E-STOP */}
      <div className="ml-auto flex items-center gap-3">
        <div className="rsim-mono flex items-center gap-2" style={{ fontSize: 11, color: 'var(--rsim-text-dim)' }}>
          <span ref={setSimClock} aria-label="sim time">—</span>
          <span aria-hidden="true">·</span>
          <span ref={setWallClock} aria-label="wall clock">—</span>
        </div>

        {/* E-STOP button — M8: disabled (command bus lands M9) */}
        <button
          type="button"
          className="rsim-estop"
          disabled
          style={{
            height: 32,
            padding: '0 12px',
            opacity: 0.4,
            cursor: 'not-allowed',
          }}
          aria-pressed={false}
          title="E-STOP — wired at M9 (single-key E or click fires POST /api/fleet/estop)"
        >
          E-STOP
        </button>
      </div>
    </div>
  )
}
