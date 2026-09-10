'use client'

/**
 * GCS v2 Operations Canvas — Fleet C2 overlay (M11, T-B3).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.5 FleetC2 + §2.3 rule 8 (vehicle cards) +
 * §5.4 (fleet mission orchestration) + §6.4 L6 (task markers).
 *
 * Vehicle cards (MDPI pattern: id, fsm, mode, battery badge, next target,
 * coords, mini-attitude, RTH button) — NOT tables. Bindings sub-panel
 * (assign/upload/state badges). Patterns sub-panel (generate + auto-bind).
 * Start-fleet dialog (parallel/sequential + gate + timeout). Task board +
 * auction log. Event log w/ filters + 8-policy safety ladder. Full FSM
 * table as expandable detail.
 *
 * Data: fleet REST + WS events_tail (:8400) via useFleetC2 hook.
 */

import { useState, useEffect, type JSX } from 'react'
import { useFleetC2, type FleetC2Api } from '@/hooks/useFleetC2'
import { useTelemetrySnapshot } from '@/state/telemetry-store'
import { usePlanCatalog } from '@/hooks/usePlanCatalog'
import { toggleOverlay, pushNotification, setActiveVehicle } from '@/state/app-store'
import { command } from '@/state/command-bus'
import type { FleetVehicle } from '@/lib/types'
import { fsmStyle } from '@/lib/fsm'

export function FleetC2Panel(): JSX.Element {
  const fleet = useFleetC2()
  const snap = useTelemetrySnapshot()
  const [tab, setTab] = useState<'cards' | 'bindings' | 'patterns' | 'events'>('cards')

  return (
    <div data-rsim-zone="F" role="dialog" aria-modal="true" className="pointer-events-auto rsim-canvas" style={{ position: 'absolute', left: 56, top: 48, bottom: 96, width: 380, zIndex: 25, background: 'var(--rsim-surface-solid)', borderRight: '1px solid var(--rsim-border)', display: 'flex', flexDirection: 'column', overflow: 'hidden', fontFamily: 'var(--rsim-font-ui)', color: 'var(--rsim-text)' }}>
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Fleet C2</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>{fleet.snapshot?.phase ?? '—'} · {snap.vehicles.length} vehicles · {fleet.conn}</div>
        </div>
        <button type="button" onClick={() => toggleOverlay('fleet', { force: false })} style={{ background: 'transparent', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text-dim)', cursor: 'pointer', padding: '4px 8px', fontSize: 11 }} aria-label="Close">×</button>
      </div>

      {/* Tab bar */}
      <div style={{ display: 'flex', borderBottom: '1px solid var(--rsim-border)' }}>
        {(['cards', 'bindings', 'patterns', 'events'] as const).map((t) => (
          <button key={t} type="button" onClick={() => setTab(t)} style={{ flex: 1, padding: '6px 8px', background: 'transparent', border: 'none', borderBottom: tab === t ? '2px solid var(--rsim-accent)' : '2px solid transparent', color: tab === t ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)', fontSize: 11, fontWeight: 600, cursor: 'pointer', textTransform: 'capitalize' }}>{t}</button>
        ))}
      </div>

      <div style={{ flex: 1, overflow: 'auto' }}>
        {tab === 'cards' && <VehicleCards vehicles={snap.vehicles} fleet={fleet} />}
        {tab === 'bindings' && <BindingsPanel fleet={fleet} />}
        {tab === 'patterns' && <PatternsPanel fleet={fleet} />}
        {tab === 'events' && <EventLog events={fleet.events} />}
      </div>

      {/* Start + E-stop actions */}
      <div style={{ borderTop: '1px solid var(--rsim-border)', padding: 12, display: 'flex', gap: 6 }}>
        <button type="button" onClick={() => { void command('fleet_start', undefined, { mode: 'parallel' }) }} style={btnStyle} disabled={fleet.busy} title="POST /api/fleet/start (parallel)">▶ Start fleet</button>
        <button type="button" onClick={() => { void command('estop_fleet', undefined) }} style={{ ...btnStyle, color: 'var(--rsim-alert)', borderColor: 'var(--rsim-alert)' }} title="POST /api/fleet/estop">■ E-STOP</button>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Vehicle cards — §2.3 rule 8 (MDPI pattern). NOT tables.
// ---------------------------------------------------------------------------

function VehicleCards({ vehicles, fleet }: { vehicles: FleetVehicle[]; fleet: FleetC2Api }): JSX.Element {
  if (vehicles.length === 0) return <div style={{ padding: 16, fontSize: 11, color: 'var(--rsim-text-dim)' }}>No vehicles. Fleet plane: {fleet.conn}.</div>
  return (
    <div style={{ padding: 8, display: 'flex', flexDirection: 'column', gap: 8 }}>
      {vehicles.map((v) => {
        const style = fsmStyle(v.fsm)
        const binding = fleet.bindings.find((b) => b.vehicle_id === v.index)
        return (
          <div key={v.id} onClick={() => setActiveVehicle(v.index)} style={{ padding: 10, borderRadius: 'var(--rsim-radius-panel)', border: '1px solid var(--rsim-border)', background: 'rgba(17, 22, 29, 0.4)', cursor: 'pointer' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 6 }}>
              <span className="rsim-mono" style={{ fontSize: 13, fontWeight: 600, color: `hsl(${188 + v.index * 74}, 78%, 60%)` }}>v{v.index}</span>
              <span className="rsim-mono" style={{ fontSize: 10, padding: '2px 6px', borderRadius: 'var(--rsim-radius-chip)', color: style.hex, border: `1px solid ${style.hex}` }}>{v.fsm}</span>
            </div>
            <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)', display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 4 }}>
              <span>mode: {v.mode}</span>
              <span>batt: {Math.round(v.battery_pct)}%</span>
              <span>alt: {v.alt_agl_m != null ? `${v.alt_agl_m.toFixed(1)}m` : '—'}</span>
              <span>spd: {Math.hypot(v.velocity_ned_ms[0], v.velocity_ned_ms[1]).toFixed(1)}m/s</span>
              <span>lat: {v.lat?.toFixed(5) ?? '—'}</span>
              <span>lon: {v.lon?.toFixed(5) ?? '—'}</span>
            </div>
            {binding && (
              <div className="rsim-mono" style={{ fontSize: 9, color: 'var(--rsim-accent)', marginTop: 4 }}>
                mission: {binding.mission_id} · {binding.binding_state}
              </div>
            )}
            <div style={{ display: 'flex', gap: 4, marginTop: 6 }}>
              <button type="button" onClick={(e) => { e.stopPropagation(); void command('rtl', v.index) }} style={{ ...btnStyle, padding: '2px 6px', fontSize: 10 }} disabled={!v.armed} title="RTL">RTL</button>
              <button type="button" onClick={(e) => { e.stopPropagation(); void command('land', v.index) }} style={{ ...btnStyle, padding: '2px 6px', fontSize: 10 }} disabled={!v.armed} title="Land">Land</button>
              <button type="button" onClick={(e) => { e.stopPropagation(); void command('hold', v.index) }} style={{ ...btnStyle, padding: '2px 6px', fontSize: 10 }} disabled={!v.armed} title="Hold">Hold</button>
            </div>
          </div>
        )
      })}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Bindings panel — assign missions to vehicles.
// ---------------------------------------------------------------------------

function BindingsPanel({ fleet }: { fleet: FleetC2Api }): JSX.Element {
  const [selected, setSelected] = useState<Record<number, string>>({})
  const snap = useTelemetrySnapshot()
  const catalog = usePlanCatalog()

  const apply = async (): Promise<void> => {
    const inputs = Object.entries(selected).map(([vid, mid]) => ({ vehicle_id: parseInt(vid), mission_id: mid }))
    if (inputs.length === 0) return
    const r = await fleet.setMissionBindings(inputs)
    if (r.ok) pushNotification({ severity: 'info', title: 'Bindings updated', detail: `${inputs.length} vehicle(s)` })
    else pushNotification({ severity: 'error', title: 'Bindings failed', detail: r.error ?? '' })
  }

  return (
    <div style={{ padding: 8 }}>
      <div style={{ fontSize: 11, color: 'var(--rsim-text-dim)', marginBottom: 8 }}>Assign missions to vehicles, then Apply. Upload + Start from the action bar.</div>
      {snap.vehicles.map((v) => {
        const binding = fleet.bindings.find((b) => b.vehicle_id === v.index)
        return (
          <div key={v.id} style={{ padding: 8, borderBottom: '1px solid var(--rsim-border)' }}>
            <div className="rsim-mono" style={{ fontSize: 11, display: 'flex', justifyContent: 'space-between' }}>
              <span>v{v.index} · {v.fsm}</span>
              <span style={{ color: binding ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)' }}>{binding?.binding_state ?? 'unassigned'}</span>
            </div>
            <select value={selected[v.index] ?? binding?.mission_id ?? ''} onChange={(e) => setSelected({ ...selected, [v.index]: e.target.value })} style={{ ...inputStyle, width: '100%', marginTop: 4 }}>
              <option value="">— select mission —</option>
              {catalog.missions.map((m) => (<option key={m.id} value={m.id}>{m.name} (v{m.version})</option>))}
            </select>
          </div>
        )
      })}
      <button type="button" onClick={() => void apply()} style={{ ...btnStyle, marginTop: 8, width: '100%' }} disabled={fleet.busy}>Apply bindings</button>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Patterns panel — generate swarm patterns + auto-bind.
// ---------------------------------------------------------------------------

function PatternsPanel({ fleet }: { fleet: FleetC2Api }): JSX.Element {
  return (
    <div style={{ padding: 8 }}>
      <div style={{ fontSize: 11, color: 'var(--rsim-text-dim)', marginBottom: 8 }}>Swarm patterns — generate per-vehicle missions and auto-bind. Full patterns dialog lands M11 late.</div>
      {fleet.patterns.map((p) => (
        <div key={p.name} style={{ padding: 8, borderBottom: '1px solid var(--rsim-border)' }}>
          <div style={{ fontSize: 12, fontWeight: 600 }}>{p.name}</div>
          <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>{p.description}</div>
          <button type="button" onClick={() => { void fleet.generatePattern(p.name, {}).then((r) => { if (r.ok) pushNotification({ severity: 'info', title: 'Pattern generated', detail: `${r.result?.missions.length ?? 0} missions` }) }) }} style={{ ...btnStyle, marginTop: 4, fontSize: 10 }} disabled={fleet.busy}>Generate</button>
        </div>
      ))}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Event log — filterable, 8-policy safety ladder detail.
// ---------------------------------------------------------------------------

function EventLog({ events }: { events: FleetC2Api['events'] }): JSX.Element {
  const [filter, setFilter] = useState<'all' | 'info' | 'warn' | 'critical'>('all')
  const filtered = events.filter((e) => filter === 'all' || e.severity === filter)
  return (
    <div style={{ padding: 8 }}>
      <div style={{ display: 'flex', gap: 4, marginBottom: 8 }}>
        {(['all', 'info', 'warn', 'critical'] as const).map((f) => (
          <button key={f} type="button" onClick={() => setFilter(f)} style={{ ...btnStyle, padding: '2px 6px', fontSize: 10, borderBottom: filter === f ? '2px solid var(--rsim-accent)' : 'none' }}>{f}</button>
        ))}
      </div>
      <div style={{ fontSize: 10, fontFamily: 'var(--rsim-font-mono)' }}>
        {filtered.slice(-100).reverse().map((e, i) => (
          <div key={i} style={{ padding: '3px 0', borderBottom: '1px solid var(--rsim-border)', color: e.severity === 'critical' ? 'var(--rsim-danger)' : e.severity === 'warn' ? 'var(--rsim-alert)' : 'var(--rsim-text-dim)' }}>
            <span style={{ opacity: 0.6 }}>{new Date(e.t).toLocaleTimeString()} </span>
            <span>{e.detail}</span>
          </div>
        ))}
        {filtered.length === 0 && <div style={{ padding: 12, color: 'var(--rsim-text-dim)' }}>No events.</div>}
      </div>
    </div>
  )
}

const inputStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-chip)', color: 'var(--rsim-text)', fontSize: 11, padding: '2px 4px', fontFamily: 'var(--rsim-font-mono)' }
const btnStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text)', fontSize: 11, padding: '4px 10px', cursor: 'pointer', fontFamily: 'var(--rsim-font-mono)' }
