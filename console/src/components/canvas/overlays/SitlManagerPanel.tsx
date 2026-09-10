'use client'

/**
 * GCS v2 Operations Canvas — SITL Manager overlay (Task 7e, 2026-09-10).
 *
 * Industry pattern (QGC + Mission Planner): the GCS does NOT auto-spawn SITL
 * when it launches — the operator starts SITL on demand. This panel is the
 * operator-facing SITL lifecycle control:
 *   - Status: RUNNING (with vehicle count, scenario, started_at) or STOPPED
 *   - Start: hold-to-confirm 400ms, picks a scenario TOML from the dropdown
 *   - Stop: safety-positive single tap (kills mavfleet + its px4/sim children)
 *
 * Calls the fleet-supervisor REST API on :8500 (the supervisor owns the
 * mavfleet child process). The fleet's vehicles appear in the FleetFrame
 * (:8400) a few seconds after start; the Fly View + Operator Map pick
 * them up automatically.
 *
 * Different from the removed SimControlPanel: that was sim-internal physics
 * + fault injection UI; this is fleet lifecycle (start/stop the mavfleet
 * process + N PX4 SITL pairs).
 */

import { useState, type JSX } from 'react'
import { useSitlSupervisor } from '@/hooks/useSitlSupervisor'
import { toggleOverlay } from '@/state/app-store'
import { command } from '@/state/command-bus'

export function SitlManagerPanel(): JSX.Element {
  const { status, scenarios, starting, stopping, refresh } = useSitlSupervisor(1000)
  const [selectedScenario, setSelectedScenario] = useState<string | null>(null)
  const [holdProgress, setHoldProgress] = useState(0)
  const [holdTimer, setHoldTimer] = useState<ReturnType<typeof setTimeout> | null>(null)

  const running = status?.running ?? false
  const vehicleCount = status?.vehicle_count ?? 0
  const scenarioName = status?.scenario ?? null
  const startedAt = status?.started_at_ms ?? null
  const pid = status?.pid ?? null

  // Effective scenario for the start button: the user's pick, or the default
  // from the supervisor.
  const effectiveScenario = selectedScenario ?? scenarios?.default ?? 'operator_session.toml'

  // Hold-to-confirm for the Start button (400ms).
  const startHoldBegin = (): void => {
    if (running || starting) return
    setHoldProgress(0)
    const start = Date.now()
    const id = setInterval(() => {
      const elapsed = Date.now() - start
      const pct = Math.min(100, (elapsed / 400) * 100)
      setHoldProgress(pct)
      if (pct >= 100) {
        clearInterval(id)
        setHoldTimer(null)
        void command('sitl_start', undefined, { scenario: effectiveScenario })
      }
    }, 50)
    setHoldTimer(id)
  }
  const startHoldCancel = (): void => {
    if (holdTimer) {
      clearInterval(holdTimer)
      setHoldTimer(null)
    }
    setHoldProgress(0)
  }

  const onStop = (): void => {
    if (!running || stopping) return
    void command('sitl_stop', undefined, {})
  }

  const formatStartedAt = (ms: number): string => {
    const d = new Date(ms)
    return d.toLocaleString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
  }

  return (
    <div
      data-rsim-zone="F"
      role="dialog"
      aria-modal="true"
      className="pointer-events-auto rsim-canvas"
      style={{
        position: 'absolute',
        left: 56,
        top: 48,
        bottom: 96,
        width: 360,
        zIndex: 25,
        background: 'var(--rsim-surface-solid)',
        borderRight: '1px solid var(--rsim-border)',
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
        fontFamily: 'var(--rsim-font-ui)',
        color: 'var(--rsim-text)',
      }}
    >
      {/* Header */}
      <div
        style={{
          padding: 12,
          borderBottom: '1px solid var(--rsim-border)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
        }}
      >
        <div>
          <div style={{ fontSize: 13, fontWeight: 600 }}>SITL Manager</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>
            :8500 · fleet-supervisor
          </div>
        </div>
        <button
          type="button"
          onClick={() => toggleOverlay('sitl', { force: false })}
          style={{
            background: 'transparent',
            border: '1px solid var(--rsim-border)',
            borderRadius: 'var(--rsim-radius-control)',
            color: 'var(--rsim-text-dim)',
            cursor: 'pointer',
            padding: '4px 8px',
            fontSize: 11,
          }}
        >
          ×
        </button>
      </div>

      {/* Status block */}
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)' }}>
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            marginBottom: 8,
          }}
        >
          <span
            className="rsim-mono"
            style={{
              padding: '2px 8px',
              borderRadius: 'var(--rsim-radius-chip)',
              fontSize: 10,
              fontWeight: 700,
              background: running ? 'rgba(34, 211, 238, 0.15)' : 'rgba(150, 150, 150, 0.1)',
              color: running ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)',
              border: `1px solid ${running ? 'var(--rsim-accent)' : 'var(--rsim-border)'}`,
            }}
          >
            {running ? '● RUNNING' : '○ STOPPED'}
          </span>
          <span style={{ fontSize: 11, color: 'var(--rsim-text-dim)' }}>
            {running ? `${vehicleCount} vehicle${vehicleCount === 1 ? '' : 's'}` : 'no vehicles'}
          </span>
        </div>
        {running && (
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)', display: 'flex', flexDirection: 'column', gap: 2 }}>
            {scenarioName && <div>scenario: {scenarioName}</div>}
            {startedAt && <div>started: {formatStartedAt(startedAt)}</div>}
            {pid != null && <div>pid: {pid}</div>}
          </div>
        )}
        {!running && (
          <div style={{ fontSize: 11, color: 'var(--rsim-text-dim)' }}>
            SITL is not running. Pick a scenario and hold Start to spawn PX4 SITL.
          </div>
        )}
      </div>

      {/* Scenario picker */}
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)' }}>
        <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)', marginBottom: 6 }}>
          Scenario (TOML in fleet/tests/)
        </div>
        <select
          value={effectiveScenario}
          onChange={(e) => setSelectedScenario(e.target.value)}
          disabled={running || starting}
          style={{
            width: '100%',
            padding: '6px 8px',
            borderRadius: 'var(--rsim-radius-control)',
            border: '1px solid var(--rsim-border)',
            background: 'rgba(17, 22, 29, 0.6)',
            color: 'var(--rsim-text)',
            fontSize: 11,
            fontFamily: 'var(--rsim-font-mono)',
            cursor: running || starting ? 'not-allowed' : 'pointer',
          }}
        >
          {scenarios?.scenarios.map((s) => (
            <option key={s} value={s}>
              {s}
              {s === scenarios.default ? ' (default)' : ''}
            </option>
          ))}
        </select>
      </div>

      {/* Action buttons */}
      <div style={{ padding: 12, display: 'flex', flexDirection: 'column', gap: 8 }}>
        {!running ? (
          <button
            type="button"
            onMouseDown={startHoldBegin}
            onMouseUp={startHoldCancel}
            onMouseLeave={startHoldCancel}
            disabled={starting}
            style={{
              position: 'relative',
              padding: '10px 12px',
              borderRadius: 'var(--rsim-radius-control)',
              border: `1px solid ${starting ? 'var(--rsim-accent)' : 'var(--rsim-border)'}`,
              background: starting ? 'rgba(34, 211, 238, 0.08)' : 'transparent',
              color: starting ? 'var(--rsim-accent)' : 'var(--rsim-text)',
              fontSize: 12,
              fontWeight: 600,
              cursor: starting ? 'wait' : 'pointer',
              overflow: 'hidden',
            }}
            aria-label="Start SITL (hold 400ms)"
          >
            {holdProgress > 0 && holdProgress < 100 && (
              <span
                style={{
                  position: 'absolute',
                  left: 0,
                  top: 0,
                  bottom: 0,
                  width: `${holdProgress}%`,
                  background: 'rgba(34, 211, 238, 0.25)',
                  transition: 'width 50ms linear',
                }}
              />
            )}
            <span style={{ position: 'relative' }}>
              {starting ? 'Starting SITL…' : '▶ Start SITL (hold 400ms)'}
            </span>
          </button>
        ) : (
          <button
            type="button"
            onClick={onStop}
            disabled={stopping}
            style={{
              padding: '10px 12px',
              borderRadius: 'var(--rsim-radius-control)',
              border: `1px solid ${stopping ? 'var(--rsim-danger)' : 'var(--rsim-border)'}`,
              background: stopping ? 'rgba(239, 68, 68, 0.08)' : 'transparent',
              color: stopping ? 'var(--rsim-danger)' : 'var(--rsim-text)',
              fontSize: 12,
              fontWeight: 600,
              cursor: stopping ? 'wait' : 'pointer',
            }}
            aria-label="Stop SITL"
          >
            {stopping ? 'Stopping SITL…' : '■ Stop SITL'}
          </button>
        )}
        <button
          type="button"
          onClick={() => void refresh()}
          style={{
            padding: '6px 10px',
            borderRadius: 'var(--rsim-radius-control)',
            border: '1px solid var(--rsim-border)',
            background: 'transparent',
            color: 'var(--rsim-text-dim)',
            fontSize: 10,
            cursor: 'pointer',
          }}
        >
          ↻ Refresh status
        </button>
        <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)', marginTop: 8, lineHeight: 1.5 }}>
          <strong style={{ color: 'var(--rsim-text)' }}>Industry pattern</strong> (QGC + Mission Planner): the GCS does not auto-spawn SITL on launch. The operator starts/stops SITL on demand. The supervisor (:8500) spawns the mavfleet process which spawns N PX4 SITL + sim pairs.
        </div>
      </div>
    </div>
  )
}
