'use client'

/**
 * Plan catalog client (fleet-catalog :8300, GCS_SPEC §5.1 / §8.1).
 *
 * Same dual-mode lifecycle pattern as the other console hooks: probe the
 * REST plane through the gateway → LIVE (list/poll the mission summaries)
 * or SIMULATED (catalog offline; UI stays operable for offline editing
 * with a clear "SIMULATED DATA — backend offline" badge). Live retries
 * happen every 12 s while the catalog is down.
 *
 * All API calls route through `src/lib/conn.ts`'s gateway mode, so every
 * fetch URL carries `?XTransformPort=8300`. The catalog answers with the
 * `{"ok":bool,"data":...} | {"ok":false,"error":{...}}` envelope; the
 * helpers here unwrap the envelope and surface structured results to the
 * Plan View component.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchGw, gw, unwrapEnvelope } from '@/lib/conn'
import type { ConnState } from '@/lib/types'
import {
  normalizePlanMissionFile,
  type PlanMissionFile,
  type PlanMissionSummary,
  type PlanValidationResult,
} from '@/lib/plan-types'

export const CATALOG_PORT = 8300

/** Connection probe interval (catalog offline → retry every 12 s). */
const RETRY_MS = 12_000

/** Result of a save (POST or PUT) call. */
export interface SaveOutcome {
  ok: boolean
  /** Assigned id (POST) or updated id (PUT). null on failure. */
  id: string | null
  /** New version (bumped). */
  version: number
  /** Full mission file returned by the catalog (null on failure). */
  file: PlanMissionFile | null
  /** Human-readable error (null on success). */
  error: string | null
  /** HTTP status code (0 = network error). */
  status: number
}

/** Result of an upload call. */
export interface UploadOutcome {
  ok: boolean
  /** HTTP status code (0 = network error). */
  status: number
  /** Stable error code from the catalog's error envelope (null on success). */
  code: string | null
  /** Human-readable message. */
  message: string
  /** Raw unwrapped response body (for the UI to display details). */
  data: unknown
}

/** Result of a validate call. null = network/parse failure. */
export interface ValidateOutcome {
  ok: boolean
  /** HTTP status code (0 = network error). */
  status: number
  /** Parsed validation result (null if the body couldn't be parsed). */
  result: PlanValidationResult | null
  /** Human-readable error (null when the body parsed). */
  error: string | null
}

export function usePlanCatalog() {
  const [conn, setConn] = useState<ConnState>('connecting')
  const [lastError, setLastError] = useState<string | null>(null)
  const [retryAt, setRetryAt] = useState<number | null>(null)
  const [missions, setMissions] = useState<PlanMissionSummary[]>([])
  const [busy, setBusy] = useState(false)

  const connRef = useRef<ConnState>('connecting')

  const setConnBoth = useCallback((c: ConnState) => {
    connRef.current = c
    setConn(c)
  }, [])

  // --------------------------------------------------------------- list/probe
  const refresh = useCallback(async (): Promise<boolean> => {
    try {
      const res = await fetchGw(gw(CATALOG_PORT, '/api/missions'), { method: 'GET' }, 3000)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const j = await res.json()
      const data = unwrapEnvelope(j)
      const list = Array.isArray(data) ? (data as PlanMissionSummary[]) : []
      setMissions(list)
      if (connRef.current !== 'live') {
        setConnBoth('live')
        setLastError(null)
        setRetryAt(null)
      }
      return true
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      if (connRef.current !== 'simulated') {
        setConnBoth('simulated')
      }
      setLastError(msg)
      setRetryAt(Date.now() + RETRY_MS)
      return false
    }
  }, [setConnBoth])

  // initial probe + retry loop while offline
  useEffect(() => {
    let cancelled = false
    let timer: ReturnType<typeof setInterval> | null = null
    const tick = async () => {
      if (cancelled) return
      await refresh()
    }
    void tick()
    timer = setInterval(tick, RETRY_MS)
    return () => {
      cancelled = true
      if (timer) clearInterval(timer)
    }
  }, [refresh])

  // ------------------------------------------------------------ get mission
  const getMission = useCallback(async (id: string): Promise<PlanMissionFile | null> => {
    if (!id) return null
    try {
      const res = await fetchGw(gw(CATALOG_PORT, `/api/missions/${id}`), { method: 'GET' }, 4000)
      if (!res.ok) return null
      const j = await res.json()
      const data = unwrapEnvelope(j)
      return normalizePlanMissionFile(data)
    } catch {
      return null
    }
  }, [])

  // ------------------------------------------------------------ save (POST/PUT)
  const saveMission = useCallback(async (file: PlanMissionFile): Promise<SaveOutcome> => {
    const isUpdate = file.mission.id.length > 0
    const url = isUpdate
      ? gw(CATALOG_PORT, `/api/missions/${file.mission.id}`)
      : gw(CATALOG_PORT, '/api/missions')
    const method = isUpdate ? 'PUT' : 'POST'
    try {
      const res = await fetchGw(
        url,
        {
          method,
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(file),
        },
        5000,
      )
      const j = (await res.json().catch(() => null)) as
        | { ok: boolean; data?: unknown; error?: { code?: string; message?: string } }
        | null
      if (!res.ok || !j) {
        const err = j?.error?.message ?? `HTTP ${res.status}`
        return { ok: false, id: null, version: file.mission.version, file: null, error: err, status: res.status }
      }
      const data = normalizePlanMissionFile(unwrapEnvelope(j))
      return {
        ok: true,
        id: data.mission.id,
        version: data.mission.version,
        file: data,
        error: null,
        status: res.status,
      }
    } catch (e) {
      return {
        ok: false,
        id: null,
        version: file.mission.version,
        file: null,
        error: e instanceof Error ? e.message : String(e),
        status: 0,
      }
    }
  }, [])

  // -------------------------------------------------------------- delete
  const deleteMission = useCallback(async (id: string): Promise<boolean> => {
    if (!id) return false
    try {
      const res = await fetchGw(gw(CATALOG_PORT, `/api/missions/${id}`), { method: 'DELETE' }, 3000)
      return res.ok
    } catch {
      return false
    }
  }, [])

  // -------------------------------------------------------------- validate
  const validateMission = useCallback(async (id: string): Promise<ValidateOutcome> => {
    if (!id) {
      return { ok: false, status: 0, result: null, error: 'mission not saved yet (click Save first)' }
    }
    try {
      const res = await fetchGw(gw(CATALOG_PORT, `/api/missions/${id}/validate`), { method: 'POST' }, 5000)
      const j = (await res.json().catch(() => null)) as
        | { ok: boolean; data?: { valid?: boolean; errors?: unknown }; error?: { message?: string } }
        | null
      if (!j) {
        return { ok: false, status: res.status, result: null, error: `HTTP ${res.status} (no body)` }
      }
      // The catalog returns { ok: result.valid, data: result } — when invalid,
      // ok=false but data still carries the errors. We surface both.
      const data = unwrapEnvelope(j) as { valid?: boolean; errors?: unknown }
      const errors = Array.isArray(data?.errors) ? data!.errors! : []
      const result: PlanValidationResult = {
        valid: Boolean(data?.valid ?? false),
        errors: errors as PlanValidationResult['errors'],
      }
      return {
        ok: res.ok || result.valid,
        status: res.status,
        result,
        error: null,
      }
    } catch (e) {
      return {
        ok: false,
        status: 0,
        result: null,
        error: e instanceof Error ? e.message : String(e),
      }
    }
  }, [])

  // -------------------------------------------------------------- upload
  const uploadToVehicle = useCallback(async (vehicleId: number, missionId: string): Promise<UploadOutcome> => {
    if (!missionId) {
      return { ok: false, status: 0, code: 'NO_MISSION', message: 'mission not saved (click Save first)', data: null }
    }
    const url = gw(CATALOG_PORT, `/api/vehicles/${vehicleId}/mission/upload`, { mission_id: missionId })
    try {
      const res = await fetchGw(url, { method: 'POST' }, 8000)
      const j = (await res.json().catch(() => null)) as
        | { ok: boolean; data?: unknown; error?: { code?: string; message?: string; details?: unknown } }
        | null
      if (res.ok && j?.ok) {
        return {
          ok: true,
          status: res.status,
          code: null,
          message: 'validated + version-checked (M1 stub — MAVLink upload is M2 scope)',
          data: j?.data ?? null,
        }
      }
      return {
        ok: false,
        status: res.status,
        code: j?.error?.code ?? null,
        message: j?.error?.message ?? `HTTP ${res.status}`,
        data: j ?? null,
      }
    } catch (e) {
      return {
        ok: false,
        status: 0,
        code: 'NETWORK_ERROR',
        message: e instanceof Error ? e.message : String(e),
        data: null,
      }
    }
  }, [])

  // -------------------------------------------------------------- run-with-busy
  const runBusy = useCallback(
    async <T,>(fn: () => Promise<T>): Promise<T> => {
      setBusy(true)
      try {
        return await fn()
      } finally {
        setBusy(false)
      }
    },
    [],
  )

  return {
    conn,
    lastError,
    retryAt,
    missions,
    busy,
    refresh,
    getMission,
    saveMission,
    deleteMission,
    validateMission,
    uploadToVehicle,
    runBusy,
  }
}

export type PlanCatalogApi = ReturnType<typeof usePlanCatalog>
