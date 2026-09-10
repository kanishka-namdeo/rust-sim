'use client'

/**
 * GCS v2 Operations Canvas — Library overlay (M10, T-B2).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.5 Library + §3.2 (mission library).
 *
 * Mission list (load/delete/refresh), new mission, download-from-vehicle +
 * comparison table. Data: :8300 /api/missions* (library), :8400
 * /api/vehicles/{i}/mission?type= (vehicle download).
 */

import { useState, type JSX } from 'react'
import { usePlanCatalog } from '@/hooks/usePlanCatalog'
import { loadMission, setDownloadResult } from '@/state/plan-store'
import { toggleOverlay } from '@/state/app-store'
import { gw, fetchGw } from '@/lib/conn'
import type { MissionType } from '@/lib/plan-types'

const FLEET_PORT = 8400

export function LibraryPanel(): JSX.Element {
  const catalog = usePlanCatalog()
  const [downloadVehicle, setDownloadVehicle] = useState(0)
  const [downloadType, setDownloadType] = useState<MissionType>('mission')

  return (
    <div data-rsim-zone="F" className="pointer-events-auto rsim-canvas" style={{ position: 'absolute', left: 56, top: 48, bottom: 96, width: 360, zIndex: 25, background: 'var(--rsim-surface-solid)', borderRight: '1px solid var(--rsim-border)', display: 'flex', flexDirection: 'column', overflow: 'hidden', fontFamily: 'var(--rsim-font-ui)', color: 'var(--rsim-text)' }}>
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Mission Library</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>{catalog.missions.length} saved · {catalog.conn}</div>
        </div>
        <button type="button" onClick={() => toggleOverlay('library', { force: false })} style={{ background: 'transparent', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text-dim)', cursor: 'pointer', padding: '4px 8px', fontSize: 11 }} aria-label="Close">×</button>
      </div>

      <div style={{ flex: 1, overflow: 'auto' }}>
        <div style={{ padding: 8, display: 'flex', gap: 6 }}>
          <button type="button" onClick={() => catalog.refresh()} style={btnStyle} disabled={catalog.busy}>⟳ Refresh</button>
          <button type="button" onClick={() => { import('@/state/plan-store').then(({ newMission }) => newMission()) }} style={btnStyle}>+ New</button>
        </div>

        {/* Mission list — empty state with CTA */}
        <div style={{ padding: 4 }}>
          {catalog.missions.length === 0 && (
            <div className="rsim-empty-state">
              <div className="rsim-empty-state-icon">🗺️</div>
              <div className="rsim-empty-state-title">No saved missions</div>
              <div className="rsim-empty-state-detail">
                Save a mission from the Mission Strip (M), or create one from Plan mode (P) + click the map.
              </div>
              <div className="rsim-empty-state-cta">
                <button type="button" onClick={() => { import('@/state/plan-store').then(({ newMission }) => newMission()) }}>+ New Mission</button>
                <button type="button" onClick={() => catalog.refresh()}>⟳ Refresh</button>
              </div>
            </div>
          )}
          {catalog.missions.map((m) => (
            <div key={m.id} style={{ padding: '8px 12px', borderBottom: '1px solid var(--rsim-border)', display: 'flex', flexDirection: 'column', gap: 4 }}>
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                <span style={{ fontSize: 12, fontWeight: 600 }}>{m.name}</span>
                <span className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>v{m.version} · {m.waypoint_count}wp</span>
              </div>
              <div style={{ display: 'flex', gap: 6 }}>
                <button type="button" onClick={async () => { const f = await catalog.getMission(m.id); if (f) loadMission(f) }} style={btnStyle} disabled={catalog.busy}>Load</button>
                <button type="button" onClick={async () => { if (confirm(`Delete "${m.name}"?`)) { await catalog.deleteMission(m.id) } }} style={{ ...btnStyle, color: 'var(--rsim-danger)' }} disabled={catalog.busy}>Delete</button>
              </div>
            </div>
          ))}
        </div>

        {/* Download from vehicle */}
        <div style={{ padding: 12, borderTop: '1px solid var(--rsim-border)' }}>
          <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 8 }}>Download from vehicle</div>
          <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
            <label style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>v</label>
            <input type="number" min={0} value={downloadVehicle} onChange={(e) => setDownloadVehicle(parseInt(e.target.value) || 0)} style={{ ...inputStyle, width: 40 }} />
            <label style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>type</label>
            <select value={downloadType} onChange={(e) => setDownloadType(e.target.value as MissionType)} style={{ ...inputStyle, width: 80 }}>
              <option value="mission">mission</option>
              <option value="fence">fence</option>
              <option value="rally">rally</option>
            </select>
            <button type="button" onClick={async () => {
              const r = await fetchGw(gw(FLEET_PORT, `/api/vehicles/${downloadVehicle}/mission`), { method: 'GET' }, 15000)
              const j = (await res_json(r)) as { ok: boolean; data?: { status: string; mission_type: number; items: { seq: number; frame: number; command: number; x: number; y: number; z: number; param1: number; param2: number; param3: number; param4: number }[] }; error?: { code: string; message: string } }
              if (j.ok && j.data) {
                setDownloadResult({ ok: true, status: 200, code: null, error: null, missionType: downloadType, items: j.data.items.map((it) => ({ seq: it.seq, frame: it.frame, command: it.command, x: it.x / 1e7, y: it.y / 1e7, z: it.z, param1: it.param1, param2: it.param2, param3: it.param3, param4: it.param4 })), rawItems: j.data.items })
              } else {
                setDownloadResult({ ok: false, status: r.status, code: j.error?.code ?? null, error: j.error?.message ?? null, missionType: downloadType, items: null, rawItems: null })
              }
            }} style={btnStyle}>Download</button>
          </div>
        </div>
      </div>
    </div>
  )
}

async function res_json(r: Response): Promise<unknown> {
  try { return await r.json() } catch { return {} }
}

const inputStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-chip)', color: 'var(--rsim-text)', fontSize: 11, padding: '2px 4px', fontFamily: 'var(--rsim-font-mono)' }
const btnStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text)', fontSize: 11, padding: '4px 10px', cursor: 'pointer', fontFamily: 'var(--rsim-font-mono)' }
