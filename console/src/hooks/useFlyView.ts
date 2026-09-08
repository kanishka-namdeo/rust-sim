'use client'

/**
 * Fly View data + command hook (mavfleet :8400, GCS_SPEC §5.2).
 *
 * The Fly View is the QGC Fly View analog: live map + instrument widgets +
 * pre-arm checklist + action bar. It composes the existing `useOperatorMap`
 * hook (which already polls :8400 at 10 Hz WS + REST fallback and owns the
 * command surface — arm/takeoff/land/rtl/hold/goto/mission) with:
 *
 *   - active-vehicle selection (default vehicle index 0)
 *   - 60 s strip-chart series (altitude / battery / velocity) for the
 *     active vehicle, derived from snapshot deltas at ~10 Hz
 *   - the pre-arm-checks endpoint (GET /api/vehicles/{i}/prearm-checks on
 *     :8400 — built by a parallel agent; this hook handles 404 gracefully)
 *
 * The hook takes the page-level `op: OperatorMapApi` as input so the fleet
 * polling engine survives tab switches (it lives in page.tsx, not here).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { fetchGw, gw } from '@/lib/conn'
import type { OperatorMapApi } from './useOperatorMap'
import type { FleetVehicle, GuidedResult } from '@/lib/types'

const STRIP_CAP = 600 // 60 s @ 10 Hz
const PREARM_TIMEOUT_MS = 8000

/** One pre-arm check row from the backend. */
export interface PrearmCheck {
  name: string
  passed: boolean
  message: string
}

/** Pre-arm-checks endpoint result. */
export interface PrearmChecksResult {
  ok: boolean
  checks: PrearmCheck[]
  all_passed: boolean
  /** Set when the endpoint is not yet implemented (parallel agent). */
  unavailable?: boolean
  /** Error message when the request failed (network, 5xx, etc.). */
  error?: string
}

/** Per-vehicle strip-chart series (60 s, ~10 Hz). */
export interface FlySeries {
  alt: { t: number; v: number }[]
  battery: { t: number; v: number }[]
  velocity: { t: number; v: number }[]
}

const EMPTY_SERIES: FlySeries = { alt: [], battery: [], velocity: [] }

export function useFlyView(op: OperatorMapApi) {
  const [activeVehicleId, setActiveVehicleId] = useState<number>(0)
  const [prearm, setPrearm] = useState<PrearmChecksResult | null>(null)
  const [prearmLoading, setPrearmLoading] = useState(false)
  const [series, setSeries] = useState<FlySeries>(EMPTY_SERIES)

  // --- last-sample watermark per vehicle id, so we only append on deltas ---
  const seriesRef = useRef<FlySeries>(EMPTY_SERIES)
  const lastSampleRef = useRef<Record<string, number>>({})

  const snapshot = op.snapshot
  const vehicles = snapshot?.vehicles ?? []

  // ----------------------------------------------------------- active vehicle
  const activeVehicle: FleetVehicle | null = useMemo(() => {
    if (vehicles.length === 0) return null
    return vehicles.find((v) => v.index === activeVehicleId) ?? vehicles[0] ?? null
  }, [vehicles, activeVehicleId])

  /** Effective active id (clamped when the selected vehicle vanishes). */
  const effectiveActiveId = activeVehicle?.index ?? 0

  // --- tracks the active id we last appended a sample for, so we can reset
  // the series (and stale per-vehicle watermarks) on a vehicle switch ---
  const lastActiveIdRef = useRef<number>(-1)

  // ----------------------------------------------------- strip-chart builder
  // Re-derive the series from snapshot deltas. Each snapshot tick (10 Hz live
  // or 5 Hz mock) appends one sample for the active vehicle. When the active
  // vehicle changes, the series is reset to a single seed sample for the new
  // vehicle (the alternative — splicing the previous vehicle's 60 s of
  // samples onto the new one — would be misleading).
  useEffect(() => {
    if (!activeVehicle) return
    const tS = snapshot?.t_s ?? 0

    const idChanged = lastActiveIdRef.current !== effectiveActiveId
    lastActiveIdRef.current = effectiveActiveId
    if (idChanged) {
      // reset on vehicle switch
      lastSampleRef.current = {}
      const alt = -activeVehicle.position_ned_m[2]
      const battery = activeVehicle.battery_pct
      const vel = Math.hypot(
        activeVehicle.velocity_ned_ms[0],
        activeVehicle.velocity_ned_ms[1],
        activeVehicle.velocity_ned_ms[2],
      )
      const seed: FlySeries = {
        alt: [{ t: tS, v: alt }],
        battery: [{ t: tS, v: battery }],
        velocity: [{ t: tS, v: vel }],
      }
      seriesRef.current = seed
      setSeries(seed)
      lastSampleRef.current[activeVehicle.id] = tS
      return
    }

    // skip if we already appended at this virtual time for this vehicle
    const lastT = lastSampleRef.current[activeVehicle.id] ?? -Infinity
    if (tS <= lastT) return
    lastSampleRef.current[activeVehicle.id] = tS

    const alt = -activeVehicle.position_ned_m[2]
    const battery = activeVehicle.battery_pct
    const vel = Math.hypot(
      activeVehicle.velocity_ned_ms[0],
      activeVehicle.velocity_ned_ms[1],
      activeVehicle.velocity_ned_ms[2],
    )

    const next: FlySeries = {
      alt: [...seriesRef.current.alt, { t: tS, v: alt }].slice(-STRIP_CAP),
      battery: [...seriesRef.current.battery, { t: tS, v: battery }].slice(-STRIP_CAP),
      velocity: [...seriesRef.current.velocity, { t: tS, v: vel }].slice(-STRIP_CAP),
    }
    seriesRef.current = next
    setSeries(next)
  }, [activeVehicle, effectiveActiveId, snapshot?.t_s])

  // ------------------------------------------------------- pre-arm checks
  const runPrearmChecks = useCallback(
    async (vehicleId: number): Promise<PrearmChecksResult> => {
      setPrearmLoading(true)
      try {
        const res = await fetchGw(
          gw(8400, `/api/vehicles/${vehicleId}/prearm-checks`),
          { method: 'GET' },
          PREARM_TIMEOUT_MS,
        )
        if (res.status === 404) {
          // The parallel agent's endpoint isn't deployed yet.
          const result: PrearmChecksResult = {
            ok: false,
            checks: [],
            all_passed: false,
            unavailable: true,
          }
          setPrearm(result)
          return result
        }
        const j = (await res.json().catch(() => null)) as {
          ok?: boolean
          data?: { checks?: PrearmCheck[]; all_passed?: boolean }
          error?: string
        } | null
        if (!res.ok || !j?.ok) {
          const result: PrearmChecksResult = {
            ok: false,
            checks: j?.data?.checks ?? [],
            all_passed: j?.data?.all_passed ?? false,
            error: j?.error ?? `HTTP ${res.status}`,
          }
          setPrearm(result)
          return result
        }
        const data = j.data ?? {}
        const result: PrearmChecksResult = {
          ok: true,
          checks: data.checks ?? [],
          all_passed: data.all_passed ?? false,
        }
        setPrearm(result)
        return result
      } catch (e) {
        const result: PrearmChecksResult = {
          ok: false,
          checks: [],
          all_passed: false,
          error: e instanceof Error ? e.message : String(e),
        }
        setPrearm(result)
        return result
      } finally {
        setPrearmLoading(false)
      }
    },
    [],
  )

  // ---------------------------------------------------------- action methods
  // Delegate to the page-level `op` (useOperatorMap). All these POST to :8400
  // via the gateway; the mock engine implements the same semantics when the
  // backend is offline.
  const arm = useCallback(
    (index: number): Promise<GuidedResult> => op.arm(index, true),
    [op],
  )
  const disarm = useCallback(
    (index: number): Promise<GuidedResult> => op.arm(index, false),
    [op],
  )
  const takeoff = useCallback(
    (index: number, altM: number): Promise<GuidedResult> => op.takeoff(index, altM),
    [op],
  )
  const land = useCallback(
    (index: number): Promise<GuidedResult> => op.land(index),
    [op],
  )
  const rtl = useCallback(
    (index: number): Promise<GuidedResult> => op.rtl(index),
    [op],
  )
  const hold = useCallback(
    (index: number): Promise<GuidedResult> => op.hold(index),
    [op],
  )
  const startMission = useCallback(
    (): Promise<{ started: boolean; reason: string | null }> => op.startMission(),
    [op],
  )

  return {
    // state
    activeVehicleId: effectiveActiveId,
    setActiveVehicleId,
    activeVehicle,
    vehicles,
    snapshot,
    conn: op.conn,
    prearm,
    prearmLoading,
    series,
    // actions
    arm,
    disarm,
    takeoff,
    land,
    rtl,
    hold,
    startMission,
    runPrearmChecks,
  }
}

export type FlyViewApi = ReturnType<typeof useFlyView>
