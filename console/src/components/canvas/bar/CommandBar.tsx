'use client'

/**
 * GCS v2 Operations Canvas — Command bar (Zone D, M8 T-B1).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.4 + §2.2 zone D.
 *
 * Fixed 96px bottom, full width, never collapses. Two rows:
 *  Row 1: active-vehicle chip · mode chip · verbs [ARM(hold) · TAKEOFF(hold) ·
 *         LAND · RTL · HOLD · START MISSION(hold)] · MISSION ▸ (strip) ·
 *         goto target chip (when pending).
 *  Row 2: map-mode switch [Fly · Plan · Fence · Corridor] · follow toggle ·
 *         cheat-sheet hint.
 *
 * State-gated per §7.4 (commands disabled when backend state would reject);
 * every verb shows its shortcut key on the button (`ARM (A)`). Busy state =
 * spinner chip 400 ms min display (command bus lands M9, M8 renders the
 * verbs visibly DISABLED per §13.1 "E-STOP and verb controls render disabled").
 *
 * The hold-to-confirm progress fill (§7.4 — 400 ms amber) and the deferred-
 * undo toast (§7.4 — tap + 6 s undo) land with T-C2 at M9.
 */

import { useAppStore, setMapMode, setFollow, setGotoPending, type MapMode } from '@/state/app-store'
import { useTelemetrySnapshot } from '@/state/telemetry-store'
import { isBusy } from '@/state/command-bus'

interface Verb {
  id: 'arm' | 'disarm' | 'takeoff' | 'land' | 'rtl' | 'hold' | 'mission_start'
  label: string
  shortcut: string
  hold?: boolean // §7.4: ARM/TAKEOFF/START MISSION use hold-to-confirm
  danger?: boolean // §7.4: destructive flight verbs
}

const VERBS: Verb[] = [
  { id: 'arm', label: 'ARM', shortcut: 'A', hold: true },
  { id: 'disarm', label: 'DISARM', shortcut: 'D' },
  { id: 'takeoff', label: 'TAKEOFF', shortcut: 'T', hold: true },
  { id: 'land', label: 'LAND', shortcut: 'L' },
  { id: 'rtl', label: 'RTL', shortcut: 'R' },
  { id: 'hold', label: 'HOLD', shortcut: 'H' },
  { id: 'mission_start', label: 'START MISSION', shortcut: 'M', hold: true },
]

const MAP_MODES: { id: MapMode; label: string; shortcut: string }[] = [
  { id: 'fly', label: 'Fly', shortcut: 'F' },
  { id: 'plan', label: 'Plan', shortcut: 'P' },
  { id: 'fence', label: 'Fence', shortcut: '—' },
  { id: 'corridor', label: 'Corridor', shortcut: '—' },
]

export function CommandBar() {
  const app = useAppStore()
  const snap = useTelemetrySnapshot()

  const activeVehicle = snap.vehicles.find((v) => v.index === app.activeVehicle) ?? snap.vehicles[0]

  return (
    <div
      data-rsim-zone="D"
      className="pointer-events-auto absolute bottom-0 left-0 right-0 border-t"
      style={{ height: 96, zIndex: 30, display: 'flex', flexDirection: 'column', padding: '12px 16px' }}
    >
      {/* Row 1: active vehicle + verbs */}
      <div className="flex items-center gap-3" style={{ height: 40 }}>
        {/* Active vehicle chip */}
        <div
          className="rsim-mono rsim-chip"
          style={{ color: 'var(--rsim-accent)', borderColor: 'var(--rsim-accent)', background: 'rgba(34, 211, 238, 0.08)' }}
        >
          {activeVehicle ? `v${activeVehicle.index} · ${activeVehicle.fsm}` : 'no vehicle'}
        </div>

        {/* Mode chip */}
        <div className="rsim-mono rsim-chip" style={{ color: 'var(--rsim-text-dim)' }}>
          {activeVehicle?.mode ?? '—'}
        </div>

        {/* Verb buttons — M8: all disabled (command bus lands M9) */}
        <div className="flex items-center gap-1">
          {VERBS.map((verb) => {
            const busy = isBusy(verb.id)
            return (
              <button
                key={verb.id}
                type="button"
                disabled
                style={{
                  height: 32,
                  padding: '0 10px',
                  borderRadius: 'var(--rsim-radius-control)',
                  border: '1px solid',
                  borderColor: verb.hold ? 'var(--rsim-alert)' : 'var(--rsim-border)',
                  background: verb.hold ? 'rgba(245, 158, 11, 0.06)' : 'rgba(17, 22, 29, 0.4)',
                  color: 'var(--rsim-text-dim)',
                  fontSize: 11,
                  fontWeight: 600,
                  fontFamily: 'var(--rsim-font-mono)',
                  letterSpacing: '0.04em',
                  cursor: 'not-allowed',
                  opacity: 0.4,
                }}
                aria-label={`${verb.label} (${verb.shortcut})${verb.hold ? ' — hold to confirm' : ''}`}
                title={`${verb.label} (${verb.shortcut}) — wired at M9${verb.hold ? ' · hold-to-confirm 400ms' : ''}${busy ? ' · busy' : ''}`}
              >
                {verb.label} <span style={{ opacity: 0.6 }}>({verb.shortcut})</span>
              </button>
            )
          })}

          {/* MISSION ▸ strip toggle (M10 wires the MissionStrip overlay) */}
          <button
            type="button"
            disabled
            style={{
              height: 32,
              padding: '0 10px',
              borderRadius: 'var(--rsim-radius-control)',
              border: '1px solid var(--rsim-border)',
              background: 'rgba(17, 22, 29, 0.4)',
              color: 'var(--rsim-text-dim)',
              fontSize: 11,
              fontWeight: 600,
              fontFamily: 'var(--rsim-font-mono)',
              letterSpacing: '0.04em',
              cursor: 'not-allowed',
              opacity: 0.4,
            }}
            title="Mission strip — wired at M10"
          >
            MISSION ▸ (M)
          </button>

          {/* Goto target chip — visible when goto pending */}
          {app.gotoPending && (
            <div
              className="rsim-mono rsim-chip"
              style={{ color: 'var(--rsim-alert)', borderColor: 'var(--rsim-alert)', background: 'rgba(245, 158, 11, 0.08)' }}
            >
              goto: click target for v{app.activeVehicle}
            </div>
          )}
        </div>

        <div className="ml-auto" />
      </div>

      {/* Row 2: map-mode switch + follow toggle + cheat hint */}
      <div className="flex items-center gap-3 mt-2" style={{ height: 32 }}>
        <div className="flex items-center gap-1" role="group" aria-label="Map mode">
          {MAP_MODES.map((m) => {
            const active = app.mapMode === m.id
            return (
              <button
                key={m.id}
                type="button"
                onClick={() => setMapMode(m.id)}
                style={{
                  height: 26,
                  padding: '0 8px',
                  borderRadius: 'var(--rsim-radius-control)',
                  border: '1px solid',
                  borderColor: active ? 'var(--rsim-accent)' : 'var(--rsim-border)',
                  background: active ? 'rgba(34, 211, 238, 0.08)' : 'transparent',
                  color: active ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)',
                  fontSize: 10,
                  fontWeight: 600,
                  fontFamily: 'var(--rsim-font-mono)',
                  letterSpacing: '0.04em',
                  cursor: 'pointer',
                }}
                aria-pressed={active}
                aria-label={`Map mode: ${m.label} (${m.shortcut})`}
              >
                {m.label}
              </button>
            )
          })}
        </div>

        <button
          type="button"
          onClick={() => setFollow(!app.follow)}
          style={{
            height: 26,
            padding: '0 8px',
            borderRadius: 'var(--rsim-radius-control)',
            border: '1px solid',
            borderColor: app.follow ? 'var(--rsim-accent)' : 'var(--rsim-border)',
            background: app.follow ? 'rgba(34, 211, 238, 0.08)' : 'transparent',
            color: app.follow ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)',
            fontSize: 10,
            fontWeight: 600,
            fontFamily: 'var(--rsim-font-mono)',
            letterSpacing: '0.04em',
            cursor: 'pointer',
          }}
          aria-pressed={app.follow}
          title="Follow camera (F) — suspends on manual pan for 5s"
        >
          FOLLOW
        </button>

        <div className="ml-auto rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>
          press ? for shortcuts
        </div>
      </div>
    </div>
  )
}
