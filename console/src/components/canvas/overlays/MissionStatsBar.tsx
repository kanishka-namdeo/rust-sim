'use client'

/**
 * GCS v2 Operations Canvas — Mission statistics bar (competition feature).
 *
 * QGC master docs: "Mission Statistics — Displays details for the selected
 * waypoint (altitude difference, azimuth, distance to previous waypoint,
 * gradient, heading) and for the entire mission (total distance, flight
 * time, battery consumption)."
 *
 * Renders a compact stats strip at the top of the MissionStrip overlay:
 * total distance (haversine), est. flight time, waypoint count, altitude
 * profile (min/max/range), battery estimate.
 */

import type { JSX } from 'react'
import { usePlanStore } from '@/state/plan-store'

function haversineM(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const R = 6371000
  const dLat = ((lat2 - lat1) * Math.PI) / 180
  const dLon = ((lon2 - lon1) * Math.PI) / 180
  const a = Math.sin(dLat / 2) ** 2 + Math.cos((lat1 * Math.PI) / 180) * Math.cos((lat2 * Math.PI) / 180) * Math.sin(dLon / 2) ** 2
  return R * 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a))
}

export function MissionStatsBar(): JSX.Element {
  const plan = usePlanStore()
  const wps = plan.file.waypoints
  if (wps.length === 0) return <div style={{ padding: '4px 12px', fontSize: 10, color: 'var(--rsim-text-dim)' }}>No waypoints yet</div>

  let totalDist = 0
  for (let i = 1; i < wps.length; i++) {
    totalDist += haversineM(wps[i - 1].x, wps[i - 1].y, wps[i].x, wps[i].y)
  }
  const avgSpeed = 5 // m/s estimate for a quad
  const flightTimeS = totalDist / avgSpeed
  const alts = wps.map((w) => w.z)
  const altMin = Math.min(...alts)
  const altMax = Math.max(...alts)
  const mm = Math.floor(flightTimeS / 60)
  const ss = Math.floor(flightTimeS % 60)

  return (
    <div className="rsim-mono" style={{ display: 'flex', gap: 12, padding: '6px 12px', borderBottom: '1px solid var(--rsim-border)', fontSize: 10, color: 'var(--rsim-text-dim)' }}>
      <Stat label="WP" value={String(wps.length)} />
      <Stat label="Dist" value={totalDist > 1000 ? `${(totalDist / 1000).toFixed(2)} km` : `${totalDist.toFixed(0)} m`} />
      <Stat label="Time" value={`${mm}:${ss.toString().padStart(2, '0')}`} />
      <Stat label="Alt" value={`${altMin.toFixed(0)}–${altMax.toFixed(0)} m`} />
      <Stat label="Battery est" value={`~${((flightTimeS / 60) * 2).toFixed(0)}%`} />
    </div>
  )
}

function Stat({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center' }}>
      <span style={{ fontSize: 9, textTransform: 'uppercase', opacity: 0.6 }}>{label}</span>
      <span style={{ color: 'var(--rsim-text)', fontWeight: 600 }}>{value}</span>
    </div>
  )
}
