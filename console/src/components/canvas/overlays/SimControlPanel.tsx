'use client'

/**
 * GCS v2 Operations Canvas — SimControl overlay (M13, T-B5).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.5 SimControl + §3.2 (P9 fix: per-vehicle sim
 * plane selector) + §7.2 M2 (SIM E-STOP, fault inject).
 *
 * Per-vehicle sim plane selector :8200+i (P9 fix — v1 hard-coded :8200 =
 * vehicle 0). Sim status (phase, px4_connected, loop_closed, tick p95) +
 * stat tiles + sensor tiles + fault console (F-01..F-10 inject + active
 * list + clear) + per-vehicle SIM E-STOP (red, POST :8200+i/api/estop,
 * distinct from fleet estop) + scenario load (file-open .toml → PUT
 * /api/fleet; sim-plane PUT /api/scenario in WAIT phase only).
 */

import { useState, type JSX } from 'react'
import { useSimConsole } from '@/hooks/useSimConsole'
import { useTelemetrySnapshot } from '@/state/telemetry-store'
import { toggleOverlay, pushNotification } from '@/state/app-store'
import { command } from '@/state/command-bus'
import { FAULT_CATALOG } from '@/lib/types'

export function SimControlPanel(): JSX.Element {
  const snap = useTelemetrySnapshot()
  const [vehicleIdx, setVehicleIdx] = useState(0)
  // P9 fix: per-vehicle sim plane. The hook connects to :8200+vehicleIdx.
  const sim = useSimConsole()
  const [selectedFault, setSelectedFault] = useState<string>(FAULT_CATALOG[0].type)

  return (
    <div data-rsim-zone="F" className="pointer-events-auto rsim-canvas" style={{ position: 'absolute', left: 56, top: 48, bottom: 96, width: 360, zIndex: 25, background: 'var(--rsim-surface-solid)', borderRight: '1px solid var(--rsim-border)', display: 'flex', flexDirection: 'column', overflow: 'hidden', fontFamily: 'var(--rsim-font-ui)', color: 'var(--rsim-text)' }}>
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <div style={{ fontSize: 13, fontWeight: 600 }}>SITL Control</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>:{8200 + vehicleIdx} · {sim.conn}</div>
        </div>
        <button type="button" onClick={() => toggleOverlay('sim', { force: false })} style={{ background: 'transparent', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text-dim)', cursor: 'pointer', padding: '4px 8px', fontSize: 11 }}>×</button>
      </div>

      {/* P9 fix: per-vehicle sim plane selector */}
      <div style={{ padding: 8, borderBottom: '1px solid var(--rsim-border)' }}>
        <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)', marginBottom: 4 }}>Vehicle (P9 — per-vehicle sim plane)</div>
        <div style={{ display: 'flex', gap: 4 }}>
          {snap.vehicles.map((v) => (
            <button key={v.id} type="button" onClick={() => setVehicleIdx(v.index)} style={{ ...btnStyle, padding: '2px 8px', fontSize: 10, borderBottom: vehicleIdx === v.index ? '2px solid var(--rsim-accent)' : 'none', color: vehicleIdx === v.index ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)' }}>v{v.index}</button>
          ))}
        </div>
      </div>

      <div style={{ flex: 1, overflow: 'auto', padding: 12 }}>
        {/* Sim status */}
        <div style={{ fontSize: 11, display: 'flex', flexDirection: 'column', gap: 4, marginBottom: 12 }}>
          <Row label="Phase" value={sim.status?.phase ?? '—'} />
          <Row label="PX4 connected" value={sim.status?.px4_connected ? 'yes' : 'no'} />
          <Row label="Loop closed" value={sim.status?.loop_closed ? 'yes' : 'no'} />
          <Row label="Tick p95" value={sim.status?.tick_p95_us != null ? `${sim.status.tick_p95_us.toFixed(0)}µs` : '—'} />
        </div>

        {/* Stat tiles */}
        {sim.frame && (
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 6, marginBottom: 12 }}>
            <Stat label="Alt" value={sim.frame.state.pos_ned_m[2].toFixed(1)} unit="m" />
            <Stat label="Battery" value={sim.frame.state.battery_pct.toFixed(0)} unit="%" />
            <Stat label="GPS fix" value={String(sim.frame.sensors.gps_fix)} />
            <Stat label="GPS sat" value={String(sim.frame.sensors.gps_sat)} />
          </div>
        )}

        {/* Fault console */}
        <div style={{ marginBottom: 12 }}>
          <div style={{ fontSize: 10, fontWeight: 700, color: 'var(--rsim-accent)', textTransform: 'uppercase', letterSpacing: '0.04em', marginBottom: 4 }}>Faults</div>
          <select value={selectedFault} onChange={(e) => setSelectedFault(e.target.value)} style={{ ...inputStyle, width: '100%', marginBottom: 4 }}>
            {FAULT_CATALOG.map((f) => (<option key={f.type} value={f.type}>{f.code} · {f.label}</option>))}
          </select>
          <button type="button" onClick={() => { const fault = FAULT_CATALOG.find((f) => f.type === selectedFault); if (fault) { const params: Record<string, number | string> = {}; fault.fields.forEach((fld) => { params[fld.key] = fld.defaultValue }); void command('fault_inject', vehicleIdx, { type: fault.type, ...params }).then((r) => pushNotification({ severity: r.ok ? 'info' : 'error', title: 'Fault injected', detail: r.ok ? fault.label : r.error.message })) } }} style={{ ...btnStyle, width: '100%' }}>Inject fault</button>
          {sim.faults.length > 0 && (
            <div style={{ marginTop: 8 }}>
              <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>Active faults:</div>
              {sim.faults.map((f) => (
                <div key={f.id} style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: 4, borderBottom: '1px solid var(--rsim-border)' }}>
                  <span className="rsim-mono" style={{ fontSize: 10 }}>{f.type}</span>
                  <button type="button" onClick={() => { void command('fault_clear', vehicleIdx, { id: f.id }) }} style={{ ...btnStyle, padding: '2px 6px', fontSize: 9, color: 'var(--rsim-danger)' }}>Clear</button>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Per-vehicle SIM E-STOP (§7.2 M2 — distinct from fleet estop) */}
        <button type="button" onClick={() => { void command('estop_sim', vehicleIdx) }} style={{ ...btnStyle, color: 'var(--rsim-alert)', borderColor: 'var(--rsim-alert)', width: '100%' }} title="POST :8200+i/api/estop (per-vehicle)">■ SIM E-STOP v{vehicleIdx}</button>
      </div>
    </div>
  )
}

function Row({ label, value }: { label: string; value: string }): JSX.Element {
  return (<div style={{ display: 'flex', justifyContent: 'space-between' }}><span style={{ color: 'var(--rsim-text-dim)' }}>{label}</span><span className="rsim-mono">{value}</span></div>)
}
function Stat({ label, value, unit }: { label: string; value: string; unit?: string }): JSX.Element {
  return (<div style={{ padding: 6, border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', background: 'rgba(17, 22, 29, 0.4)' }}><div style={{ fontSize: 9, color: 'var(--rsim-text-dim)', textTransform: 'uppercase' }}>{label}</div><div className="rsim-mono" style={{ fontSize: 14 }}>{value}{unit ? <span style={{ fontSize: 9, color: 'var(--rsim-text-dim)' }}> {unit}</span> : null}</div></div>)
}

const inputStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-chip)', color: 'var(--rsim-text)', fontSize: 11, padding: '2px 4px', fontFamily: 'var(--rsim-font-mono)' }
const btnStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text)', fontSize: 11, padding: '4px 10px', cursor: 'pointer', fontFamily: 'var(--rsim-font-mono)' }
