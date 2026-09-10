'use client'

/**
 * * GCS v2 Operations Canvas — SITL Manager hook (Task 7e, 2026-09-10).
 *
 * The SITL lifecycle is now operator-driven (QGC/MP pattern): the GCS does
 * NOT auto-spawn SITL on launch. The operator uses the SITL Manager panel
 * (or the command bar) to start/stop the mavfleet process via the
 * fleet-supervisor REST API on :8500.
 *
 * This hook is a thin wrapper over the supervisor's REST endpoints:
 *   GET  /api/sitl/status     → {running, vehicle_count, pid, started_at_ms, run_dir, scenario}
 *   POST /api/sitl/start      → body {scenario?: string}
 *   POST /api/sitl/stop
 *   GET  /api/sitl/scenarios   → {scenarios: string[], default: string}
 *
 * Polls /api/sitl/status at 1 Hz when the panel is open (the supervisor
 * reads the live fleet frame from :8400 to populate vehicle_count).
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { gw, fetchGw } from '@/lib/conn'

export interface SitlStatus {
  running: boolean
  vehicle_count: number
  pid: number | null
  started_at_ms: number | null
  run_dir: string | null
  scenario: string | null
}

export interface SitlScenarios {
  scenarios: string[]
  default: string
}

const SUPERVISOR_PORT = 8500

/** Fetch the current SITL status from the supervisor. */
async function fetchStatus(): Promise<SitlStatus | null> {
  try {
    const res = await fetchGw(gw(SUPERVISOR_PORT, '/api/sitl/status'))
    if (!res.ok) return null
    const j = (await res.json()) as { ok: boolean; data?: SitlStatus }
    return j.ok && j.data ? j.data : null
  } catch {
    return null
  }
}

/** Fetch the list of available scenario TOMLs. */
async function fetchScenarios(): Promise<SitlScenarios | null> {
  try {
    const res = await fetchGw(gw(SUPERVISOR_PORT, '/api/sitl/scenarios'))
    if (!res.ok) return null
    const j = (await res.json()) as { ok: boolean; data?: SitlScenarios }
    return j.ok && j.data ? j.data : null
  } catch {
    return null
  }
}

/** Start the fleet (spawns mavfleet + N PX4 SITL pairs). */
async function startSitl(scenario?: string): Promise<{ ok: boolean; error?: string }> {
  try {
    const res = await fetchGw(gw(SUPERVISOR_PORT, '/api/sitl/start'), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ scenario: scenario ?? null }),
    })
    if (!res.ok) {
      const j = (await res.json().catch(() => ({}))) as { error?: { message?: string } }
      return { ok: false, error: j.error?.message ?? `HTTP ${res.status}` }
    }
    return { ok: true }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) }
  }
}

/** Stop the fleet (kills mavfleet + its px4/sim children). */
async function stopSitl(): Promise<{ ok: boolean; error?: string }> {
  try {
    const res = await fetchGw(gw(SUPERVISOR_PORT, '/api/sitl/stop'), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({}),
    })
    if (!res.ok) {
      const j = (await res.json().catch(() => ({}))) as { error?: { message?: string } }
      return { ok: false, error: j.error?.message ?? `HTTP ${res.status}` }
    }
    return { ok: true }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) }
  }
}

/**
 * React hook: polls /api/sitl/status at 1 Hz, exposes start/stop/scenarios.
 *
 * @param pollMs — poll interval in ms (default 1000). Set to 0 to disable
 *   polling (e.g. when the SITL Manager panel is closed).
 */
export function useSitlSupervisor(pollMs = 1000): {
  status: SitlStatus | null
  scenarios: SitlScenarios | null
  starting: boolean
  stopping: boolean
  start: (scenario?: string) => Promise<{ ok: boolean; error?: string }>
  stop: () => Promise<{ ok: boolean; error?: string }>
  refresh: () => Promise<void>
} {
  const [status, setStatus] = useState<SitlStatus | null>(null)
  const [scenarios, setScenarios] = useState<SitlScenarios | null>(null)
  const [starting, setStarting] = useState(false)
  const [stopping, setStopping] = useState(false)
  const mountedRef = useRef(true)

  const refresh = useCallback(async () => {
    const [s, sc] = await Promise.all([fetchStatus(), fetchScenarios()])
    if (!mountedRef.current) return
    setStatus(s)
    setScenarios(sc)
  }, [])

  useEffect(() => {
    mountedRef.current = true
    void refresh()
    if (pollMs <= 0) return () => { mountedRef.current = false }
    const id = setInterval(() => { void refresh() }, pollMs)
    return () => {
      mountedRef.current = false
      clearInterval(id)
    }
  }, [pollMs, refresh])

  const start = useCallback(async (scenario?: string) => {
    setStarting(true)
    try {
      const r = await startSitl(scenario)
      await refresh()
      return r
    } finally {
      if (mountedRef.current) setStarting(false)
    }
  }, [refresh])

  const stop = useCallback(async () => {
    setStopping(true)
    try {
      const r = await stopSitl()
      await refresh()
      return r
    } finally {
      if (mountedRef.current) setStopping(false)
    }
  }, [refresh])

  return { status, scenarios, starting, stopping, start, stop, refresh }
}
