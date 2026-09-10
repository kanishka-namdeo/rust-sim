'use client'

/**
 * GCS v2 Operations Canvas — Command bar (Zone D, M9 T-C2).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.4 + §2.2 zone D + §7.4 (command safety) +
 * §7.3.1 (keymap — verb buttons show their shortcut key).
 *
 * Fixed 96px bottom, full width, never collapses. Two rows:
 *  Row 1: active-vehicle chip · mode chip · verbs [ARM(hold) · TAKEOFF(hold) ·
 *         LAND · RTL · HOLD · START MISSION(hold)] · MISSION ▸ (strip) ·
 *         goto target chip (when pending).
 *  Row 2: map-mode switch [Fly · Plan · Fence · Corridor] · follow toggle ·
 *         cheat-sheet hint.
 *
 * M9 wiring:
 *  - State-gated per §7.4: each verb calls `guardVerb(name, vehicle)` to
 *    determine enabled/disabled; disabled verbs show the reason as tooltip
 *    (§2.3 rule 2).
 *  - Hold-to-confirm (§7.4): ARM/TAKEOFF/START MISSION use a 400ms press-and-
 *    hold with an amber progress fill. Released early = no fire.
 *  - Busy state (§8.4): spinner chip 400ms min display via `useBusy()`.
 *  - Verbs fire through `command(name, vehicle, payload)` — the command bus
 *    routes to the correct REST endpoint and surfaces errors via the
 *    notification queue.
 */

import { useState, useRef, useEffect, type JSX } from 'react'
import { useAppStore, setMapMode, setFollow, type MapMode } from '@/state/app-store'
import { useTelemetrySnapshot, getVehicleCount } from '@/state/telemetry-store'
import {
  command,
  guardVerb,
  isBusy,
  isHoldConfirm,
  useBusy,
  HOLD_CONFIRM_MS,
  type CommandName,
} from '@/state/command-bus'

interface Verb {
  id: CommandName
  label: string
  shortcut: string
  hold?: boolean // §7.4: ARM/TAKEOFF/START MISSION use hold-to-confirm
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

export function CommandBar(): JSX.Element {
  const app = useAppStore()
  const snap = useTelemetrySnapshot()
  const busy = useBusy()

  const activeVehicle = snap.vehicles.find((v) => v.index === app.activeVehicle) ?? snap.vehicles[0]
  const vehicleIndex = activeVehicle?.index

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

        {/* Verb buttons — M9: wired through the command bus */}
        <div className="flex items-center gap-1">
          {VERBS.map((verb) => (
            <VerbButton
              key={verb.id}
              verb={verb}
              vehicle={vehicleIndex}
              disabled={busy != null}
            />
          ))}

          {/* MISSION ▸ strip toggle — enabled (M key + button both work) */}
          <button
            type="button"
            onClick={() => import('@/state/app-store').then(({ toggleOverlay }) => toggleOverlay('mission'))}
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
              cursor: 'pointer',
            }}
            title="Toggle mission strip overlay (M)"
          >
            MISSION ▸ (M)
          </button>

          {/* Busy chip — §8.4 spinner, 400ms min display */}
          {busy && (
            <div
              className="rsim-mono rsim-chip"
              style={{ color: 'var(--rsim-alert)', borderColor: 'var(--rsim-alert)', background: 'rgba(245, 158, 11, 0.08)' }}
              aria-label={`busy: ${busy.name}`}
            >
              ⏳ {busy.name}…
            </div>
          )}

          {/* Goto target chip — visible when goto pending */}
          {app.gotoPending && (
            <div
              className="rsim-mono rsim-chip"
              style={{ color: 'var(--rsim-alert)', borderColor: 'var(--rsim-alert)', background: 'rgba(245, 158, 11, 0.08)' }}
            >
              goto: click target for v{app.activeVehicle} · Esc cancel
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

// ---------------------------------------------------------------------------
// VerbButton — the hold-to-confirm + state-gated verb button.
// §7.4: ARM/TAKEOFF/START MISSION/START FLEET/airframe-apply = hold 400ms
// with an amber progress fill. DISARM/LAND/RTL/HOLD/estop = single tap
// (safety-positive — never gate behind confirmation).
// ---------------------------------------------------------------------------

function VerbButton({ verb, vehicle, disabled }: { verb: Verb; vehicle: number | undefined; disabled: boolean }): JSX.Element {
  const guard = guardVerb(verb.id, vehicle)
  const busy = isBusy(verb.id)
  const enabled = guard.enabled && !busy && !disabled
  const [holdProgress, setHoldProgress] = useState(0) // 0..1 for the amber fill
  const holdTimerRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const holdStartRef = useRef<number>(0)
  const firedRef = useRef<boolean>(false)

  // Cleanup the hold timer on unmount.
  useEffect(() => {
    return () => {
      if (holdTimerRef.current) clearInterval(holdTimerRef.current)
    }
  }, [])

  const startHold = (e: React.MouseEvent<HTMLButtonElement>): void => {
    if (!enabled || !verb.hold) return
    e.preventDefault()
    firedRef.current = false
    holdStartRef.current = Date.now()
    setHoldProgress(0)
    // Tick every 50ms to update the amber fill; fire at 400ms.
    holdTimerRef.current = setInterval(() => {
      const elapsed = Date.now() - holdStartRef.current
      const progress = Math.min(1, elapsed / HOLD_CONFIRM_MS)
      setHoldProgress(progress)
      if (progress >= 1 && !firedRef.current) {
        firedRef.current = true
        if (holdTimerRef.current) clearInterval(holdTimerRef.current)
        holdTimerRef.current = null
        setHoldProgress(0)
        void command(verb.id, vehicle, verb.id === 'takeoff' ? { alt_m: 5 } : {})
      }
    }, 50)
  }

  const cancelHold = (): void => {
    if (holdTimerRef.current) {
      clearInterval(holdTimerRef.current)
      holdTimerRef.current = null
    }
    setHoldProgress(0)
    firedRef.current = false
  }

  const handleClick = (e: React.MouseEvent<HTMLButtonElement>): void => {
    if (!enabled) return
    if (verb.hold) {
      // Hold verbs: the mousedown starts the hold; click only fires if the
      // mouse is released after 400ms (the interval already fired). If the
      // mouse is released early, onMouseUp cancels. The click handler is a
      // no-op for hold verbs — the interval owns the fire.
      e.preventDefault()
      return
    }
    // Single-tap verb — fire immediately.
    void command(verb.id, vehicle, verb.id === 'takeoff' ? { alt_m: 5 } : {})
  }

  const tooltip = enabled
    ? `${verb.label} (${verb.shortcut})${verb.hold ? ' — hold 400ms' : ''}`
    : `${verb.label} (${verb.shortcut}) — ${guard.reason ?? 'disabled'}`

  return (
    <button
      type="button"
      disabled={!enabled}
      onMouseDown={startHold}
      onMouseUp={cancelHold}
      onMouseLeave={cancelHold}
      onClick={handleClick}
      style={{
        position: 'relative',
        height: 32,
        padding: '0 10px',
        borderRadius: 'var(--rsim-radius-control)',
        border: '1px solid',
        borderColor: verb.hold ? 'var(--rsim-alert)' : enabled ? 'var(--rsim-border)' : 'var(--rsim-border)',
        background: verb.hold ? 'rgba(245, 158, 11, 0.06)' : enabled ? 'rgba(17, 22, 29, 0.6)' : 'rgba(17, 22, 29, 0.2)',
        color: enabled ? (verb.hold ? 'var(--rsim-alert)' : 'var(--rsim-text)') : 'var(--rsim-text-dim)',
        fontSize: 11,
        fontWeight: 600,
        fontFamily: 'var(--rsim-font-mono)',
        letterSpacing: '0.04em',
        cursor: enabled ? 'pointer' : 'not-allowed',
        opacity: enabled ? 1 : 0.4,
        overflow: 'hidden',
      }}
      aria-label={tooltip}
      title={tooltip}
    >
      {/* Hold-to-confirm amber progress fill (§7.4 — 400ms amber) */}
      {verb.hold && holdProgress > 0 && (
        <span
          aria-hidden="true"
          style={{
            position: 'absolute',
            left: 0,
            top: 0,
            bottom: 0,
            width: `${holdProgress * 100}%`,
            background: 'rgba(245, 158, 11, 0.25)',
            pointerEvents: 'none',
          }}
        />
      )}
      <span style={{ position: 'relative' }}>
        {verb.label} <span style={{ opacity: 0.6 }}>({verb.shortcut})</span>
      </span>
    </button>
  )
}
