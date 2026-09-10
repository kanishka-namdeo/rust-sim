'use client'

/**
 * GCS v2 Operations Canvas — Setup drawer (M12, T-B4).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.5 Setup drawer + §3.2 (Vehicle Setup extensions)
 * + §5.3 (QGC-style configuration workflow, ADR-0016).
 *
 * Full v1 Vehicle Setup as a drawer, per selected vehicle: sections (summary/
 * airframe/sensors/power/safety/flight-modes/params), airframe apply + reboot,
 * calibrate buttons, params (download/search/group/edit/diff/presets).
 *
 * P11 fix: vehicle count derived from the fleet snapshot (not hardcoded 2).
 *
 * Data: :8400 setup/params REST + :8300 presets via useVehicleSetup hook.
 */

import { useState, useEffect, type JSX } from 'react'
import { useVehicleSetup } from '@/hooks/useVehicleSetup'
import { useTelemetrySnapshot } from '@/state/telemetry-store'
import { useAppStore, setActiveVehicle, toggleOverlay, pushNotification } from '@/state/app-store'
import { command } from '@/state/command-bus'

type Section = 'summary' | 'airframe' | 'sensors' | 'params' | 'presets'

export function SetupDrawer(): JSX.Element {
  const snap = useTelemetrySnapshot()
  const app = useAppStore()
  // P11 fix: vehicle count from the fleet snapshot, not hardcoded.
  const setup = useVehicleSetup(snap.vehicles.length || 1)
  const [section, setSection] = useState<Section>('summary')

  return (
    <div data-rsim-zone="F" className="pointer-events-auto rsim-canvas" style={{ position: 'absolute', right: 0, top: 48, bottom: 96, width: 380, zIndex: 25, background: 'var(--rsim-surface-solid)', borderLeft: '1px solid var(--rsim-border)', display: 'flex', flexDirection: 'column', overflow: 'hidden', fontFamily: 'var(--rsim-font-ui)', color: 'var(--rsim-text)' }}>
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Vehicle Setup</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>v{setup.index} · {setup.conn}</div>
        </div>
        <button type="button" onClick={() => toggleOverlay('setup', { force: false })} style={{ background: 'transparent', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text-dim)', cursor: 'pointer', padding: '4px 8px', fontSize: 11 }} aria-label="Close">×</button>
      </div>

      {/* Vehicle selector */}
      <div style={{ padding: 8, borderBottom: '1px solid var(--rsim-border)', display: 'flex', gap: 4, flexWrap: 'wrap' }}>
        {snap.vehicles.map((v) => (
          <button key={v.id} type="button" onClick={() => setup.setIndex(v.index)} style={{ ...btnStyle, padding: '2px 8px', fontSize: 10, borderBottom: setup.index === v.index ? '2px solid var(--rsim-accent)' : 'none', color: setup.index === v.index ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)' }}>v{v.index}</button>
        ))}
      </div>

      {/* Section tabs */}
      <div style={{ display: 'flex', borderBottom: '1px solid var(--rsim-border)' }}>
        {(['summary', 'airframe', 'sensors', 'params', 'presets'] as const).map((s) => (
          <button key={s} type="button" onClick={() => setSection(s)} style={{ flex: 1, padding: '6px 4px', background: 'transparent', border: 'none', borderBottom: section === s ? '2px solid var(--rsim-accent)' : '2px solid transparent', color: section === s ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)', fontSize: 10, fontWeight: 600, cursor: 'pointer', textTransform: 'capitalize' }}>{s}</button>
        ))}
      </div>

      <div style={{ flex: 1, overflow: 'auto', padding: 12 }}>
        {section === 'summary' && <SummarySection setup={setup} />}
        {section === 'airframe' && <AirframeSection setup={setup} />}
        {section === 'sensors' && <SensorsSection setup={setup} />}
        {section === 'params' && <ParamsSection setup={setup} />}
        {section === 'presets' && <PresetsSection setup={setup} />}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Summary — the QGC setup Summary (fsm, mode, armed, airframe, params state).
// ---------------------------------------------------------------------------

function SummarySection({ setup }: { setup: ReturnType<typeof useVehicleSetup> }): JSX.Element {
  const s = setup.summary
  if (!s) return <div style={{ fontSize: 11, color: 'var(--rsim-text-dim)' }}>Loading… ({setup.conn})</div>
  return (
    <div style={{ fontSize: 11, display: 'flex', flexDirection: 'column', gap: 8 }}>
      <Row label="FSM" value={s.fsm} />
      <Row label="Mode" value={s.mode} />
      <Row label="Armed" value={s.armed ? 'yes' : 'no'} />
      <Row label="Battery" value={`${Math.round(s.battery_pct)}%`} />
      <Row label="Airframe" value={s.airframe.name} />
      <Row label="Params" value={`${s.params?.state ?? '—'} (${s.params?.received ?? 0}/${s.params?.total ?? 0})`} />
      <Row label="Autopilot" value={`${s.autopilot.type} ${s.autopilot.version}`} />
      {s.restart_pending && <div style={{ color: 'var(--rsim-alert)', fontSize: 10 }}>restart pending…</div>}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Airframe — apply + controlled restart.
// ---------------------------------------------------------------------------

function AirframeSection({ setup }: { setup: ReturnType<typeof useVehicleSetup> }): JSX.Element {
  const [selected, setSelected] = useState<number | null>(null)
  return (
    <div style={{ fontSize: 11, display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>Select an airframe to apply (hold-to-confirm via the command bus; PX4 restarts).</div>
      {setup.groups.map((g) => (
        <div key={g.category}>
          <div style={{ fontSize: 10, fontWeight: 700, color: 'var(--rsim-accent)', textTransform: 'uppercase', letterSpacing: '0.04em', marginBottom: 4 }}>{g.category}</div>
          {g.airframes.map((a) => (
            <button key={a.id} type="button" onClick={() => setSelected(a.id)} style={{ ...btnStyle, width: '100%', textAlign: 'left', marginBottom: 2, background: selected === a.id ? 'rgba(34, 211, 238, 0.08)' : 'transparent' }}>
              {a.name} {a.sim_model ? `(${a.sim_model})` : ''}
            </button>
          ))}
        </div>
      ))}
      {selected != null && (
        <button type="button" onClick={() => { void command('airframe_apply', setup.index, { sys_autostart: selected }).then((r) => { if (r.ok) pushNotification({ severity: 'info', title: 'Airframe applied', detail: `v${setup.index} · restart pending` }) }) }} style={{ ...btnStyle, color: 'var(--rsim-alert)', borderColor: 'var(--rsim-alert)', marginTop: 8 }} disabled={setup.busy} title="POST /api/vehicles/:i/airframe (hold-to-confirm via command bus)">Apply airframe</button>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Sensors — calibrate buttons.
// ---------------------------------------------------------------------------

function SensorsSection({ setup }: { setup: ReturnType<typeof useVehicleSetup> }): JSX.Element {
  const sensors = ['gyro', 'accel', 'mag', 'level', 'baro'] as const
  return (
    <div style={{ fontSize: 11, display: 'flex', flexDirection: 'column', gap: 6 }}>
      <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>Calibrate sensors. The command bus routes to POST /api/vehicles/:i/calibrate.</div>
      {sensors.map((s) => (
        <button key={s} type="button" onClick={() => { void command('calibrate', setup.index, { sensor: s }).then((r) => { pushNotification({ severity: r.ok ? 'info' : 'error', title: `${s} calibrate`, detail: r.ok ? 'started' : r.error.message }) }) }} style={btnStyle} disabled={setup.busy}>Calibrate {s}</button>
      ))}
      {setup.summary?.calibration && (
        <div style={{ marginTop: 8, fontSize: 10, color: 'var(--rsim-text-dim)' }}>
          <div>gyro: {setup.summary.calibration.gyro ? '✓' : '—'}</div>
          <div>accel: {setup.summary.calibration.accel ? '✓' : '—'}</div>
          <div>mag0: {setup.summary.calibration.mag0 ? '✓' : '—'}</div>
          <div>level: {setup.summary.calibration.level_horizon ? '✓' : '—'}</div>
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Params — download + search + edit.
// ---------------------------------------------------------------------------

function ParamsSection({ setup }: { setup: ReturnType<typeof useVehicleSetup> }): JSX.Element {
  const [search, setSearch] = useState('')
  const ps = setup.paramStore
  return (
    <div style={{ fontSize: 11, display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div style={{ display: 'flex', gap: 4 }}>
        <input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="search params…" style={{ ...inputStyle, flex: 1 }} />
        <button type="button" onClick={() => setup.searchParams(search, '')} style={btnStyle} disabled={setup.busy}>Search</button>
      </div>
      <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>{ps?.state ?? '—'} ({ps?.received ?? 0}/{ps?.total ?? 0})</div>
      <div style={{ maxHeight: 300, overflow: 'auto', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)' }}>
        {(ps?.params ?? []).filter((p) => !search || p.id.toLowerCase().includes(search.toLowerCase())).slice(0, 100).map((p) => (
          <div key={p.id} style={{ display: 'grid', gridTemplateColumns: '1fr 70px', gap: 4, padding: '2px 6px', borderBottom: '1px solid var(--rsim-border)', fontSize: 10, fontFamily: 'var(--rsim-font-mono)', background: p.is_changed ? 'rgba(245, 158, 11, 0.06)' : 'transparent' }}>
            <span style={{ color: p.is_changed ? 'var(--rsim-alert)' : 'var(--rsim-text)' }}>{p.id}</span>
            <input type="number" value={p.value} onChange={(e) => { void setup.writeParam(p.id, parseFloat(e.target.value) || 0) }} style={{ ...inputStyle, padding: '1px 3px', fontSize: 9 }} step="any" />
          </div>
        ))}
        {(ps?.params ?? []).length === 0 && <div style={{ padding: 12, color: 'var(--rsim-text-dim)' }}>No params. Click Search to download.</div>}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Presets — save/load/delete (catalog :8300).
// ---------------------------------------------------------------------------

function PresetsSection({ setup }: { setup: ReturnType<typeof useVehicleSetup> }): JSX.Element {
  const [name, setName] = useState('')
  const [presets, setPresets] = useState<{ name: string; param_count: number }[]>([])
  // Refresh the preset list when the section mounts.
  useEffect(() => { void setup.listPresets().then((r) => { if (r.ok) setPresets(r.presets) }) }, [setup])
  return (
    <div style={{ fontSize: 11, display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div style={{ display: 'flex', gap: 4 }}>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="preset name…" style={{ ...inputStyle, flex: 1 }} />
        <button type="button" onClick={async () => { if (!name) return; const r = await setup.savePreset(name); pushNotification({ severity: r.ok ? 'info' : 'error', title: 'Save preset', detail: r.ok ? name : r.detail ?? 'failed' }); if (r.ok) { const lr = await setup.listPresets(); if (lr.ok) setPresets(lr.presets) } }} style={btnStyle} disabled={setup.busy || !name}>Save</button>
      </div>
      <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>Saved presets ({presets.length}):</div>
      {presets.map((p) => (
        <div key={p.name} style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: 4, borderBottom: '1px solid var(--rsim-border)' }}>
          <span className="rsim-mono" style={{ fontSize: 10 }}>{p.name} ({p.param_count} params)</span>
          <div style={{ display: 'flex', gap: 4 }}>
            <button type="button" onClick={async () => { const r = await setup.loadPreset(p.name); pushNotification({ severity: r.ok ? 'info' : 'error', title: 'Load preset', detail: r.ok ? `${p.name} (${r.applied} applied)` : 'failed' }) }} style={{ ...btnStyle, padding: '2px 6px', fontSize: 9 }} disabled={setup.busy}>Load</button>
            <button type="button" onClick={async () => { const r = await setup.deletePreset(p.name); pushNotification({ severity: r.ok ? 'info' : 'error', title: 'Delete preset', detail: r.ok ? p.name : r.detail ?? 'failed' }); if (r.ok) { const lr = await setup.listPresets(); if (lr.ok) setPresets(lr.presets) } }} style={{ ...btnStyle, padding: '2px 6px', fontSize: 9, color: 'var(--rsim-danger)' }} disabled={setup.busy}>Del</button>
          </div>
        </div>
      ))}
      {presets.length === 0 && <div style={{ color: 'var(--rsim-text-dim)' }}>No presets saved.</div>}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function Row({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div style={{ display: 'flex', justifyContent: 'space-between' }}>
      <span style={{ color: 'var(--rsim-text-dim)' }}>{label}</span>
      <span className="rsim-mono">{value}</span>
    </div>
  )
}

const inputStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-chip)', color: 'var(--rsim-text)', fontSize: 11, padding: '2px 4px', fontFamily: 'var(--rsim-font-mono)' }
const btnStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text)', fontSize: 11, padding: '4px 10px', cursor: 'pointer', fontFamily: 'var(--rsim-font-mono)' }
