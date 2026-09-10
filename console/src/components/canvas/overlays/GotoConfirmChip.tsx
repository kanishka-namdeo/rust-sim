'use client'

/**
 * GCS v2 Operations Canvas — Goto confirm chip (competition feature).
 *
 * QGC goto flow: select target on map → enter altitude → confirm.
 * Spec §7.1: G enters goto mode → click places L9 goto-target → alt input
 * chip → Enter confirms POST /api/vehicles/{i}/goto / Esc cancels.
 *
 * Renders a floating chip at the bottom-center of the canvas when goto is
 * pending. Shows the target lat/lon + an altitude input + Enter/Esc hints.
 */

import { useState, type JSX } from 'react'
import { useAppStore, setGotoPending, pushNotification } from '@/state/app-store'
import { command } from '@/state/command-bus'

export function GotoConfirmChip(): JSX.Element | null {
  const app = useAppStore()
  const [alt, setAlt] = useState(5)
  const [target, setTarget] = useState<{ lat: number; lon: number } | null>(null)

  if (!app.gotoPending && !target) return null

  // When goto is pending but no target yet, show the "click target" hint.
  if (app.gotoPending && !target) {
    return (
      <div style={{ position: 'absolute', bottom: 108, left: '50%', transform: 'translateX(-50%)', zIndex: 40, pointerEvents: 'none' }}>
        <div className="rsim-chip rsim-mono" style={{ color: 'var(--rsim-alert)', borderColor: 'var(--rsim-alert)', background: 'rgba(245, 158, 11, 0.12)', padding: '6px 12px', fontSize: 12 }}>
          goto: click target for v{app.activeVehicle} · Esc to cancel
        </div>
      </div>
    )
  }

  // When a target has been placed (the map click handler sets it via a
  // window event), show the alt input + confirm/cancel.
  if (!target) return null

  const confirm = (): void => {
    void command('goto', app.activeVehicle, { lat: target.lat, lon: target.lon, alt_m: alt }).then((r) => {
      pushNotification({ severity: r.ok ? 'info' : 'error', title: r.ok ? 'goto sent' : 'goto failed', detail: r.ok ? `v${app.activeVehicle} → ${target.lat.toFixed(5)}, ${target.lon.toFixed(5)} @ ${alt}m` : r.error.message })
    })
    setTarget(null)
    setGotoPending(false)
  }

  const cancel = (): void => {
    setTarget(null)
    setGotoPending(false)
  }

  return (
    <div style={{ position: 'absolute', bottom: 108, left: '50%', transform: 'translateX(-50%)', zIndex: 40 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '8px 12px', borderRadius: 'var(--rsim-radius-panel)', background: 'var(--rsim-surface-solid)', border: '1px solid var(--rsim-alert)', boxShadow: 'var(--rsim-shadow-overlay)' }}>
        <span className="rsim-mono" style={{ fontSize: 11, color: 'var(--rsim-text-dim)' }}>
          goto v{app.activeVehicle}: {target.lat.toFixed(5)}, {target.lon.toFixed(5)}
        </span>
        <label style={{ display: 'flex', alignItems: 'center', gap: 4, fontSize: 10, color: 'var(--rsim-text-dim)' }}>
          alt
          <input
            type="number"
            value={alt}
            onChange={(e) => setAlt(parseFloat(e.target.value) || 0)}
            onKeyDown={(e) => { if (e.key === 'Enter') confirm(); if (e.key === 'Escape') cancel() }}
            style={{ width: 50, background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-chip)', color: 'var(--rsim-text)', fontSize: 11, padding: '2px 4px', fontFamily: 'var(--rsim-font-mono)' }}
            autoFocus
          />
          m
        </label>
        <button type="button" onClick={confirm} style={{ background: 'var(--rsim-accent)', border: 'none', borderRadius: 'var(--rsim-radius-control)', color: '#0B0F14', fontSize: 11, fontWeight: 600, padding: '4px 12px', cursor: 'pointer' }}>Enter ↵</button>
        <button type="button" onClick={cancel} style={{ background: 'transparent', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text-dim)', fontSize: 11, padding: '4px 10px', cursor: 'pointer' }}>Esc</button>
      </div>
    </div>
  )
}
