'use client'

/**
 * GCS v2 Operations Canvas — Status strip (Zone A, M9).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.1 + §2.2 zone A + §2.3 rule 5 (E-stop).
 *
 * Fixed 48px top, full width, pointer-events:auto. Never collapses; the
 * E-STOP button is always visible. Renders:
 *  - Brand chip `RSIM` + fleet phase badge
 *  - Per-vehicle chips `v0 READY · 87 %`
 *  - Per-plane ConnBadge (fleet/sim/catalog) — reuses the v1 §5.4 contract;
 *    reads from the telemetry store's `getPlaneStates()`
 *  - Clock: sim time + wall clock (mono, tabular numerics)
 *  - E-STOP button (amber, rightmost, fires `POST /api/fleet/estop` via the
 *    M9 command bus; single-key `E` also fires it on keyup).
 *
 * §2.3 rule 5: E-stop is one deliberate action, visually firewalled. Single
 * press of `E` or click fires immediately; no hold-to-confirm; amber-on-dark;
 * flashes the status strip for 2s on fire.
 */

import { useState, type JSX } from 'react'
import { useTelemetrySnapshot, getPlaneStates, getFleetSnapshot } from '@/state/telemetry-store'
import { useAppStore, setActiveVehicle } from '@/state/app-store'
import { useHudRef } from '../FlushLoop'
import { PlaneBadges } from './PlaneBadges'
import { command } from '@/state/command-bus'

export function StatusStrip(): JSX.Element {
  const snap = useTelemetrySnapshot()
  const app = useAppStore()
  const fleetSnap = getFleetSnapshot()
  const phase = fleetSnap?.phase ?? 'INIT'
  const [estopFlashing, setEstopFlashing] = useState(false)

  const setPhase = useHudRef('phase_strip')
  const setSimClock = useHudRef('clock_sim')
  const setWallClock = useHudRef('clock_wall')

  const fireEstop = (): void => {
    // §2.3 rule 5: immediate fire, no hold-to-confirm. Flash the strip 2s.
    setEstopFlashing(true)
    setTimeout(() => setEstopFlashing(false), 2000)
    void command('estop_fleet', undefined, {})
  }

  return (
    <div
      data-rsim-zone="A"
      className="pointer-events-auto absolute top-0 left-0 right-0 flex items-center gap-3 px-4 border-b"
      style={{
        height: 48,
        zIndex: 30,
        ...(estopFlashing ? { boxShadow: 'inset 0 0 0 2px var(--rsim-alert)' } : {}),
      }}
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

        {/* E-STOP button — M9: wired through the command bus.
            §2.3 rule 5: single press, no hold-to-confirm, amber, firewalled
            from takeoff-class verbs. The strip flashes 2s on fire. */}
        <button
          type="button"
          className="rsim-estop"
          onClick={fireEstop}
          aria-pressed={estopFlashing}
          title="E-STOP (E) — immediate fleet estop, no hold-to-confirm"
          style={{
            height: 32,
            padding: '0 12px',
            cursor: 'pointer',
            opacity: estopFlashing ? 1 : 0.9,
          }}
        >
          E-STOP
        </button>
      </div>
    </div>
  )
}
