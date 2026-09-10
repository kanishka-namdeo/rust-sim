'use client'

/**
 * GCS v2 Operations Canvas — MissionStrip overlay (M10, T-B2).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.5 MissionStrip + §6.4 L3/L4 + §7.2 M3/M4.
 *
 * The Unified Plan Tree (QGC 5.1 pattern): mission/fence/rally as collapsible
 * sections, one expanded at a time = the active map-editing layer. Contains:
 *  - Waypoint table (seq/lat,lon/AGL/hold/accept inline edit)
 *  - Validation status + errors list (V-1..V-13 rules)
 *  - Save (catalog POST/PUT) + Validate (catalog POST /validate)
 *  - Upload actions (3 sub-bar progress: Mission/Fence/Rally via :8400
 *    /api/vehicles/{i}/mission/upload mission_type 0/1/2 — real per-item ack)
 *  - "Add exclusion polygon" + fence ceiling/floor inputs
 *  - Pattern generators (survey/corridor/perimeter params)
 *  - Undo stack (6 s undo toast per §7.4)
 */

import { useState, type JSX } from 'react'
import {
  usePlanStore,
  setSelectedWp,
  patchWaypoint,
  removeWaypoint,
  setFenceCeilingFloor,
  addExclusionPolygon,
  clearFence,
  undo,
  setValidated,
  setUploadProgress,
  onSaved,
  newMission,
  type UploadProgress,
} from '@/state/plan-store'
import { toggleOverlay, pushNotification } from '@/state/app-store'
import { usePlanCatalog } from '@/hooks/usePlanCatalog'
import { MissionStatsBar } from './MissionStatsBar'
import { gw, fetchGw } from '@/lib/conn'
import type { PlanWaypoint, MissionType } from '@/lib/plan-types'
import { MISSION_TYPE_ID } from '@/lib/plan-types'

const FLEET_PORT = 8400

type Section = 'mission' | 'fence' | 'rally' | 'patterns'

export function MissionStrip(): JSX.Element {
  const plan = usePlanStore()
  const catalog = usePlanCatalog()
  const [expanded, setExpanded] = useState<Section>('mission')

  const canUpload = !!plan.file.mission.id && plan.validated?.valid === true
  const erroredSeqs = plan.validated && !plan.validated.valid
    ? new Set(plan.validated.errors.filter((e) => e.seq != null && e.seq > 0).map((e) => e.seq as number))
    : new Set<number>()

  return (
    <div
      data-rsim-zone="F" role="dialog" aria-modal="true"
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
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Mission Strip</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>
            {plan.file.mission.id ? `${plan.file.mission.name} · v${plan.file.mission.version}` : 'untitled'} {plan.dirty ? '· unsaved' : ''}
          </div>
        </div>
        <button type="button" onClick={() => toggleOverlay('library', { force: false })} style={{ background: 'transparent', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text-dim)', cursor: 'pointer', padding: '4px 8px', fontSize: 11 }} aria-label="Close">×</button>
      </div>

      {/* Mission statistics bar (QGC master — distance/time/alt/battery est) */}
      <MissionStatsBar />

      <div style={{ flex: 1, overflow: 'auto' }}>
        <Section title="Mission" count={plan.file.waypoints.length} expanded={expanded === 'mission'} onToggle={() => setExpanded('mission')}>
          <WaypointTable waypoints={plan.file.waypoints} selectedWp={plan.selectedWp} erroredSeqs={erroredSeqs} onSelect={setSelectedWp} onPatch={patchWaypoint} onRemove={removeWaypoint} />
        </Section>
        <Section title="Geofence" count={plan.file.geofence.inclusion.length} expanded={expanded === 'fence'} onToggle={() => setExpanded('fence')}>
          <FencePanel ceiling_m={plan.file.geofence.ceiling_m} floor_m={plan.file.geofence.floor_m} inclusionCount={plan.file.geofence.inclusion.length} exclusionCount={plan.file.geofence.exclusion.length} onSetCeilingFloor={setFenceCeilingFloor} onAddExclusion={addExclusionPolygon} onClearFence={clearFence} />
        </Section>
        <Section title="Rally" count={plan.file.rally.length} expanded={expanded === 'rally'} onToggle={() => setExpanded('rally')}>
          <div style={{ padding: 8, fontSize: 11, color: 'var(--rsim-text-dim)' }}>{plan.file.rally.length} rally points (max 5 per V-12).</div>
        </Section>
        <Section title="Patterns" count={0} expanded={expanded === 'patterns'} onToggle={() => setExpanded('patterns')}>
          <div style={{ padding: 8, fontSize: 11, color: 'var(--rsim-text-dim)' }}>Survey / Corridor / Perimeter generators in lib/patterns.ts.</div>
        </Section>
      </div>

      <div style={{ borderTop: '1px solid var(--rsim-border)', padding: 12, display: 'flex', flexDirection: 'column', gap: 8 }}>
        {plan.validated && !plan.validated.valid && (
          <div style={{ fontSize: 11, color: 'var(--rsim-danger)', background: 'rgba(239, 68, 68, 0.08)', padding: 8, borderRadius: 'var(--rsim-radius-control)', border: '1px solid var(--rsim-danger)' }}>
            <strong>Validation failed:</strong> {plan.validated.errors.length} error(s)
            <ul style={{ margin: '4px 0 0 16px', padding: 0, fontSize: 10, color: 'var(--rsim-text-dim)' }}>
              {plan.validated.errors.slice(0, 5).map((e, i) => (<li key={i}>{e.rule}: {e.message}{e.seq != null ? ` (WP ${e.seq})` : ''}</li>))}
              {plan.validated.errors.length > 5 && <li>... +{plan.validated.errors.length - 5} more</li>}
            </ul>
          </div>
        )}
        {plan.validated?.valid && (
          <div style={{ fontSize: 11, color: 'var(--rsim-ok)', background: 'rgba(52, 211, 153, 0.08)', padding: 8, borderRadius: 'var(--rsim-radius-control)', border: '1px solid var(--rsim-ok)' }}>Validation passed</div>
        )}
        <div style={{ display: 'flex', gap: 6 }}>
          <ActionButton label="Validate" disabled={catalog.busy || !plan.file.mission.id} onClick={async () => { if (!plan.file.mission.id) return; const r = await catalog.validateMission(plan.file.mission.id); if (r.result) setValidated(r.result) }} title="POST /api/missions/:id/validate" />
          <ActionButton label={plan.file.mission.id ? 'Save' : 'Save as…'} disabled={catalog.busy} onClick={async () => { const r = await catalog.saveMission(plan.file); if (r.ok && r.file) onSaved(r.file) }} title={plan.file.mission.id ? 'PUT' : 'POST'} />
          <ActionButton label="Upload" disabled={catalog.busy || !canUpload} onClick={() => { void doUpload3Type(plan.file, 0, setUploadProgress) }} title="POST :8400 /api/vehicles/:i/mission/upload" />
          <ActionButton label="Undo" disabled={plan.undoStack.length === 0} onClick={undo} title="Undo last edit" />
        </div>
        {plan.uploadProgress && <UploadProgressView progress={plan.uploadProgress} />}
        <div style={{ display: 'flex', gap: 6, marginTop: 4 }}>
          <ActionButton label="Library" disabled={false} onClick={() => toggleOverlay('library')} title="Open library" />
          <ActionButton label="New" disabled={false} onClick={() => newMission()} title="New mission" />
        </div>
      </div>
    </div>
  )
}

function Section({ title, count, expanded, onToggle, children }: { title: string; count: number; expanded: boolean; onToggle: () => void; children: React.ReactNode }): JSX.Element {
  return (
    <div style={{ borderBottom: '1px solid var(--rsim-border)' }}>
      <button type="button" onClick={onToggle} style={{ width: '100%', padding: '8px 12px', background: 'transparent', border: 'none', color: 'var(--rsim-text)', fontSize: 12, fontWeight: 600, cursor: 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <span>{title}</span>
        <span className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>{count} {expanded ? '▲' : '▼'}</span>
      </button>
      {expanded && <div style={{ padding: 4 }}>{children}</div>}
    </div>
  )
}

const inputStyle: React.CSSProperties = { width: '100%', background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-chip)', color: 'var(--rsim-text)', fontSize: 10, padding: '2px 4px', fontFamily: 'var(--rsim-font-mono)' }
const btnStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text)', fontSize: 11, padding: '4px 10px', cursor: 'pointer', fontFamily: 'var(--rsim-font-mono)' }

function WaypointTable({ waypoints, selectedWp, erroredSeqs, onSelect, onPatch, onRemove }: { waypoints: PlanWaypoint[]; selectedWp: number | null; erroredSeqs: Set<number>; onSelect: (s: number | null) => void; onPatch: (s: number, p: Partial<PlanWaypoint>) => void; onRemove: (s: number) => void }): JSX.Element {
  return (
    <div style={{ fontSize: 10, fontFamily: 'var(--rsim-font-mono)' }}>
      {waypoints.length > 0 && (
        <div style={{ display: 'grid', gridTemplateColumns: '24px 1fr 56px 40px 36px 24px', gap: 2, padding: '4px 8px', fontWeight: 600, color: 'var(--rsim-text-dim)', borderBottom: '1px solid var(--rsim-border)' }}>
          <span>#</span><span>lat, lon</span><span>AGL m</span><span>hold</span><span>acc</span><span></span>
        </div>
      )}
      {waypoints.map((w) => (
        <div key={w.seq} onClick={() => onSelect(w.seq)} style={{ display: 'grid', gridTemplateColumns: '24px 1fr 56px 40px 36px 24px', gap: 2, padding: '4px 8px', cursor: 'pointer', background: w.seq === selectedWp ? 'rgba(34, 211, 238, 0.08)' : erroredSeqs.has(w.seq) ? 'rgba(239, 68, 68, 0.06)' : 'transparent', borderLeft: erroredSeqs.has(w.seq) ? '2px solid var(--rsim-danger)' : w.seq === selectedWp ? '2px solid var(--rsim-accent)' : '2px solid transparent' }}>
          <span>{w.seq}</span>
          <span style={{ fontSize: 9, color: 'var(--rsim-text-dim)' }}>{w.x.toFixed(5)}, {w.y.toFixed(5)}</span>
          <input type="number" value={w.z} onChange={(e) => onPatch(w.seq, { z: parseFloat(e.target.value) || 0 })} style={inputStyle} onClick={(e) => e.stopPropagation()} />
          <input type="number" value={w.param1} onChange={(e) => onPatch(w.seq, { param1: parseFloat(e.target.value) || 0 })} style={inputStyle} onClick={(e) => e.stopPropagation()} />
          <input type="number" value={w.param2} onChange={(e) => onPatch(w.seq, { param2: parseFloat(e.target.value) || 0 })} style={inputStyle} onClick={(e) => e.stopPropagation()} />
          <button type="button" onClick={(e) => { e.stopPropagation(); onRemove(w.seq) }} style={{ background: 'transparent', border: 'none', color: 'var(--rsim-danger)', cursor: 'pointer', fontSize: 14, padding: 0 }} title="Delete">×</button>
        </div>
      ))}
      {waypoints.length === 0 && (
        <div className="rsim-empty-state" style={{ padding: '16px 8px' }}>
          <div className="rsim-empty-state-icon">📍</div>
          <div className="rsim-empty-state-title">No waypoints yet</div>
          <div className="rsim-empty-state-detail">
            Switch to Plan mode (P) and click the map to add waypoints.<br />
            Or use a pattern generator to create a survey grid.
          </div>
          <div className="rsim-empty-state-cta">
            <button type="button" onClick={() => { import('@/state/app-store').then(({ setMapMode }) => setMapMode('plan')) }}>Go to Plan mode</button>
          </div>
        </div>
      )}
    </div>
  )
}

function FencePanel({ ceiling_m, floor_m, inclusionCount, exclusionCount, onSetCeilingFloor, onAddExclusion, onClearFence }: { ceiling_m: number; floor_m: number; inclusionCount: number; exclusionCount: number; onSetCeilingFloor: (c: number, f: number) => void; onAddExclusion: () => void; onClearFence: () => void }): JSX.Element {
  return (
    <div style={{ padding: 8, display: 'flex', flexDirection: 'column', gap: 8, fontSize: 11 }}>
      <div style={{ color: 'var(--rsim-text-dim)' }}>Inclusion: {inclusionCount} vertices · Exclusion: {exclusionCount} polygons</div>
      <div style={{ display: 'flex', gap: 8 }}>
        <label style={{ display: 'flex', flexDirection: 'column', gap: 2, fontSize: 10, color: 'var(--rsim-text-dim)' }}>Ceiling (m AGL)<input type="number" value={ceiling_m} onChange={(e) => onSetCeilingFloor(parseFloat(e.target.value) || 0, floor_m)} style={inputStyle} /></label>
        <label style={{ display: 'flex', flexDirection: 'column', gap: 2, fontSize: 10, color: 'var(--rsim-text-dim)' }}>Floor (m AGL)<input type="number" value={floor_m} onChange={(e) => onSetCeilingFloor(ceiling_m, parseFloat(e.target.value) || 0)} style={inputStyle} /></label>
      </div>
      <div style={{ display: 'flex', gap: 6 }}>
        <button type="button" onClick={onAddExclusion} style={btnStyle}>+ Exclusion</button>
        <button type="button" onClick={onClearFence} style={{ ...btnStyle, color: 'var(--rsim-danger)' }}>Clear fence</button>
      </div>
      <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>Fence mode (left rail) + click map to add vertices. Dblclick closes.</div>
    </div>
  )
}

function UploadProgressView({ progress }: { progress: UploadProgress }): JSX.Element {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 10, fontFamily: 'var(--rsim-font-mono)' }}>
      <UploadBar label="Mission" bar={progress.mission} />
      <UploadBar label="Fence" bar={progress.fence} />
      <UploadBar label="Rally" bar={progress.rally} />
    </div>
  )
}

function UploadBar({ label, bar }: { label: string; bar: UploadProgress['mission'] }): JSX.Element {
  const pct = bar.itemsSent > 0 ? (bar.itemsAcked / bar.itemsSent) * 100 : bar.status === 'success' ? 100 : 0
  const color = bar.status === 'success' ? 'var(--rsim-ok)' : bar.status === 'failed' ? 'var(--rsim-danger)' : 'var(--rsim-alert)'
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', color }}>
        <span>{label}</span>
        <span>{bar.status === 'pending' ? '…' : bar.status === 'success' ? '✓' : '✗'} {bar.itemsAcked}/{bar.itemsSent}</span>
      </div>
      <div style={{ height: 4, background: 'rgba(17, 22, 29, 0.6)', borderRadius: 2, overflow: 'hidden' }}>
        <div style={{ height: '100%', width: `${pct}%`, background: color, transition: 'width 200ms' }} />
      </div>
      {bar.message && <div style={{ fontSize: 9, color: 'var(--rsim-text-dim)' }}>{bar.message}</div>}
    </div>
  )
}

function ActionButton({ label, disabled, onClick, title }: { label: string; disabled: boolean; onClick: () => void; title?: string }): JSX.Element {
  return (
    <button type="button" disabled={disabled} onClick={onClick} title={title} style={{ ...btnStyle, opacity: disabled ? 0.4 : 1, cursor: disabled ? 'not-allowed' : 'pointer', flex: 1 }}>{label}</button>
  )
}

// 3-type upload via :8400 with real MAVLink ack counts. §8.7: rollback on fail.
async function doUpload3Type(file: { waypoints: PlanWaypoint[]; geofence: { inclusion: [number, number][]; ceiling_m: number }; rally: { seq: number; frame: number; x: number; y: number; z: number }[] }, vehicle: number, setProgress: (p: UploadProgress | null) => void): Promise<void> {
  const types: { type: MissionType; items: PlanWaypoint[] }[] = [
    { type: 'mission', items: file.waypoints },
    { type: 'fence', items: file.geofence.inclusion.map(([lat, lon], i) => ({ seq: i, frame: 3, command: 16, x: lat, y: lon, z: file.geofence.ceiling_m, param1: 0, param2: 0, param3: 0, param4: 0 })) },
    { type: 'rally', items: file.rally.map((r) => ({ seq: r.seq, frame: r.frame, command: 16, x: r.x, y: r.y, z: r.z, param1: 0, param2: 0, param3: 0, param4: 0 })) },
  ]
  const progress: UploadProgress = { mission: { status: 'idle', itemsSent: 0, itemsAcked: 0, message: '' }, fence: { status: 'idle', itemsSent: 0, itemsAcked: 0, message: '' }, rally: { status: 'idle', itemsSent: 0, itemsAcked: 0, message: '' } }
  setProgress({ ...progress })
  for (const { type, items } of types) {
    if (items.length === 0) continue
    const missionType = MISSION_TYPE_ID[type]
    progress[type] = { status: 'pending', itemsSent: 0, itemsAcked: 0, message: `uploading ${type}…` }
    setProgress({ ...progress })
    const wireItems = items.map((w, i) => ({ seq: i, frame: w.frame, command: w.command, current: i === 0 ? 1 : 0, autocontinue: 1, x: Math.round(w.x * 1e7), y: Math.round(w.y * 1e7), z: w.z, param1: w.param1, param2: w.param2, param3: w.param3, param4: w.param4, mission_type: missionType }))
    try {
      const res = await fetchGw(gw(FLEET_PORT, `/api/vehicles/${vehicle}/mission/upload`), { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ items: wireItems, mission_type: missionType }) }, 40000)
      const j = (await res.json()) as { ok: boolean; data?: { status: string; items_sent: number; items_acked: number; reason?: string }; error?: { code: string; message: string } }
      if (j.ok && j.data) {
        progress[type] = { status: j.data.status === 'ok' ? 'success' : 'failed', itemsSent: j.data.items_sent ?? 0, itemsAcked: j.data.items_acked ?? 0, message: j.data.reason ?? `${j.data.items_acked}/${j.data.items_sent} ack'd` }
      } else {
        progress[type] = { status: 'failed', itemsSent: 0, itemsAcked: 0, message: j.error?.message ?? `HTTP ${res.status}` }
      }
      setProgress({ ...progress })
      if (!j.ok || j.data?.status !== 'ok') {
        await fetchGw(gw(FLEET_PORT, `/api/vehicles/${vehicle}/mission/clear`), { method: 'POST' })
        pushNotification({ severity: 'error', title: `${type} upload failed`, detail: j.error?.message ?? j.data?.reason ?? `HTTP ${res.status}`, sticky: true })
        return
      }
    } catch (e) {
      progress[type] = { status: 'failed', itemsSent: 0, itemsAcked: 0, message: String(e) }
      setProgress({ ...progress })
      pushNotification({ severity: 'error', title: `${type} upload failed`, detail: String(e), sticky: true })
      return
    }
  }
}
