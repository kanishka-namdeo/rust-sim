'use client'

/**
 * GCS v2 Operations Canvas — Pre-flight checklist overlay.
 *
 * Competition research: QGC pre-arm checks, Mission Planner pre-flight checklist.
 * The spec §8.3 Pre-flight panel: "5 QGC checks + all_passed; ARM gate reads it.
 * GET /api/vehicles/{i}/prearm-checks (1 s while visible)."
 *
 * Shows the PX4 pre-arm checks (battery, GPS, IMU, sensor calibration, EKF)
 * with pass/fail badges. The ARM button's guard reads all_passed before
 * allowing arming.
 */

import { useState, useEffect, type JSX } from 'react'
import { toggleOverlay } from '@/state/app-store'
import { useTelemetrySnapshot, getFleetSnapshot } from '@/state/telemetry-store'
import { gw, fetchGw } from '@/lib/conn'

const FLEET_PORT = 8400

interface PrearmCheck {
  name: string
  passed: boolean
  message: string
}

export function PreFlightPanel(): JSX.Element {
  const snap = useTelemetrySnapshot()
  const [checks, setChecks] = useState<PrearmCheck[]>([])
  const [loading, setLoading] = useState(false)

  // Poll /api/vehicles/{i}/prearm-checks every 1s while visible.
  useEffect(() => {
    const activeVehicle = snap.vehicles.find((v) => v.index === 0) ?? snap.vehicles[0]
    if (!activeVehicle) return
    let cancelled = false
    const poll = async (): Promise<void> => {
      setLoading(true)
      try {
        const res = await fetchGw(gw(FLEET_PORT, `/api/vehicles/${activeVehicle.index}/prearm-checks`), { method: 'GET' }, 3000)
        const j = (await res.json()) as { ok: boolean; data?: { checks: PrearmCheck[]; all_passed: boolean } }
        if (!cancelled && j.ok && j.data?.checks) {
          setChecks(j.data.checks)
        }
      } catch {
        // network error — keep last result
      } finally {
        if (!cancelled) setLoading(false)
      }
    }
    void poll()
    const timer = setInterval(poll, 1000)
    return () => { cancelled = true; clearInterval(timer) }
  }, [snap.vehicles])

  const allPassed = checks.length > 0 && checks.every((c) => c.passed)

  return (
    <div data-rsim-zone="F" className="pointer-events-auto rsim-canvas" style={{ position: 'absolute', left: 56, top: 48, bottom: 96, width: 340, zIndex: 25, background: 'var(--rsim-surface-solid)', borderRight: '1px solid var(--rsim-border)', display: 'flex', flexDirection: 'column', overflow: 'hidden', fontFamily: 'var(--rsim-font-ui)', color: 'var(--rsim-text)' }}>
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Pre-flight Checklist</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>
            {loading ? 'checking…' : `${checks.length} checks · ${allPassed ? 'all passed' : `${checks.filter((c) => !c.passed).length} failing`}`}
          </div>
        </div>
        <button type="button" onClick={() => toggleOverlay('preflight', { force: false })} style={{ background: 'transparent', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text-dim)', cursor: 'pointer', padding: '4px 8px', fontSize: 11 }}>×</button>
      </div>
      <div style={{ flex: 1, overflow: 'auto', padding: 8 }}>
        {checks.length === 0 && !loading && (
          <div style={{ padding: 16, fontSize: 11, color: 'var(--rsim-text-dim)' }}>No pre-arm checks received. Vehicle may not be connected.</div>
        )}
        {checks.map((c) => (
          <div key={c.name} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '6px 10px', borderBottom: '1px solid var(--rsim-border)' }}>
            <span style={{ fontSize: 14, color: c.passed ? 'var(--rsim-ok)' : 'var(--rsim-danger)' }}>{c.passed ? '✓' : '✗'}</span>
            <div style={{ flex: 1 }}>
              <div style={{ fontSize: 12, fontWeight: 500 }}>{c.name}</div>
              {!c.passed && c.message && <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-danger)' }}>{c.message}</div>}
            </div>
          </div>
        ))}
      </div>
      <div style={{ borderTop: '1px solid var(--rsim-border)', padding: 12 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: 8, borderRadius: 'var(--rsim-radius-control)', border: `1px solid ${allPassed ? 'var(--rsim-ok)' : 'var(--rsim-danger)'}`, background: allPassed ? 'rgba(52, 211, 153, 0.08)' : 'rgba(239, 68, 68, 0.08)' }}>
          <span style={{ fontSize: 18, color: allPassed ? 'var(--rsim-ok)' : 'var(--rsim-danger)' }}>{allPassed ? '✓' : '⚠'}</span>
          <span style={{ fontSize: 12, fontWeight: 600, color: allPassed ? 'var(--rsim-ok)' : 'var(--rsim-danger)' }}>
            {allPassed ? 'Ready to arm' : 'Cannot arm — fix failing checks'}
          </span>
        </div>
      </div>
    </div>
  )
}
