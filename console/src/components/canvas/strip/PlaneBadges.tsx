'use client'

/**
 * GCS v2 Operations Canvas — per-plane ConnBadge row (M8, T-B1).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.1 (link chips) + §5.4 (state colors).
 *
 * The v1 ConnBadge/ConnSubline components retire at M14; this is the
 * v2-native port per spec §3.1. Renders one chip per plane (fleet/sim/catalog)
 * with the §5.4 contract:
 *
 *   LIVE       = accent cyan
 *   SIMULATED  = violet #A78BFA
 *   CONNECTING = dim gray with countdown
 *   OFFLINE    = danger
 *
 * For M8 the sim plane is "connecting" until the first sim socket opens;
 * the SIMULATED ladder kicks in after the retry budget (§9.1).
 */

import { useTelemetrySnapshot, getPlaneStates } from '@/state/telemetry-store'
import type { ConnState } from '@/lib/types'

const PLANES: { id: 'fleet' | 'sim' | 'catalog'; label: string; port: number }[] = [
  { id: 'fleet', label: 'FLEET', port: 8400 },
  { id: 'sim', label: 'SIM', port: 8200 },
  { id: 'catalog', label: 'CATALOG', port: 8300 },
]

export function PlaneBadges() {
  // Subscribe so we re-render when the plane states change.
  useTelemetrySnapshot()
  const planes = getPlaneStates()

  return (
    <>
      {PLANES.map((p) => (
        <ConnBadgeChip
          key={p.id}
          label={p.label}
          port={p.port}
          state={planes[p.id].conn}
          retryAt={planes[p.id].retryAt}
          lastError={planes[p.id].lastError}
        />
      ))}
    </>
  )
}

function ConnBadgeChip({
  label,
  port,
  state,
  retryAt,
  lastError,
}: {
  label: string
  port: number
  state: ConnState
  retryAt: number | null
  lastError: string | null
}) {
  const cls = stateClass(state)
  const title = state === 'live'
    ? `${label} :${port} — live`
    : state === 'simulated'
      ? `${label} :${port} — client-side simulation${lastError ? ` (${lastError})` : ''}`
      : `${label} :${port} — connecting${retryAt ? ` · retry in ${Math.max(0, Math.round((retryAt - Date.now()) / 1000))}s` : ''}${lastError ? ` · ${lastError}` : ''}`
  return (
    <span
      className={`rsim-chip rsim-mono rsim-chip-${state === 'live' ? 'live' : state === 'simulated' ? 'simulated' : state === 'connecting' ? 'connecting' : 'offline'}`}
      title={title}
      aria-label={title}
    >
      <span style={{ opacity: 0.6 }}>{label}</span>
      <span>{state.toUpperCase()}</span>
    </span>
  )
}

function stateClass(state: ConnState): string {
  switch (state) {
    case 'live':
      return 'rsim-chip-live'
    case 'simulated':
      return 'rsim-chip-simulated'
    case 'connecting':
      return 'rsim-chip-connecting'
    default:
      return 'rsim-chip-offline'
  }
}
