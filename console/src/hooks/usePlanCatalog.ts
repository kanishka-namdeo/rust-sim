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
  normalizeMissionItemIntWire,
  normalizePlanMissionFile,
  wireItemToWaypoint,
  type MissionDownloadResult,
  type MissionItemIntWire,
  type MissionType,
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

/** Result of an upload call (M2 — real MAVLink mission protocol). */
export interface UploadOutcome {
  ok: boolean
  /** HTTP status code (0 = network error). */
  status: number
  /** Stable error code from the catalog's error envelope (null on success). */
  code: string | null
  /** Human-readable message. */
  message: string
  /** Numeric mission_type the catalog uploaded (0=mission, 1=fence, 2=rally). */
  missionType: number | null
  /** Items sent to the vehicle via MISSION_ITEM_INT (0 on failure before send). */
  itemsSent: number
  /** Items the vehicle ack'd with MISSION_ACK result=0 (0 on failure). */
  itemsAcked: number
  /** Raw unwrapped response body (for the UI to display extra details). */
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

  // -------------------------------------------------------------- upload (M2)
  // The catalog proxies this through :8400 which speaks the real MAVLink
  // mission protocol. The response carries items_sent + items_acked from
  // the protocol exchange (or an UPLOAD_FAILED error envelope with the
  // partial counts in `details`).
  const uploadToVehicle = useCallback(async (vehicleId: number, missionId: string): Promise<UploadOutcome> => {
    const zero = (code: string | null, message: string, status: number, data: unknown): UploadOutcome => ({
      ok: false,
      status,
      code,
      message,
      missionType: null,
      itemsSent: 0,
      itemsAcked: 0,
      data,
    })
    if (!missionId) {
      return zero('NO_MISSION', 'mission not saved (click Save first)', 0, null)
    }
    const url = gw(CATALOG_PORT, `/api/vehicles/${vehicleId}/mission/upload`, { mission_id: missionId })
    try {
      const res = await fetchGw(url, { method: 'POST' }, 35_000)
      const j = (await res.json().catch(() => null)) as
        | {
            ok: boolean
            data?: { mission_type?: number; items_sent?: number; items_acked?: number }
            error?: { code?: string; message?: string; details?: { mission_type?: number; items_sent?: number; items_acked?: number } }
          }
        | null
      if (res.ok && j?.ok) {
        const d = j.data ?? {}
        const mt = typeof d.mission_type === 'number' ? d.mission_type : null
        const sent = typeof d.items_sent === 'number' ? d.items_sent : 0
        const acked = typeof d.items_acked === 'number' ? d.items_acked : 0
        return {
          ok: true,
          status: res.status,
          code: null,
          message: `MAVLink upload complete · ${acked}/${sent} items ack'd`,
          missionType: mt,
          itemsSent: sent,
          itemsAcked: acked,
          data: j.data ?? null,
        }
      }
      // Failure path — the error envelope carries partial counts in details.
      const err = j?.error ?? {}
      const det = err.details ?? {}
      const mt = typeof det.mission_type === 'number' ? det.mission_type : null
      const sent = typeof det.items_sent === 'number' ? det.items_sent : 0
      const acked = typeof det.items_acked === 'number' ? det.items_acked : 0
      return {
        ok: false,
        status: res.status,
        code: err.code ?? null,
        message: err.message ?? `HTTP ${res.status}`,
        missionType: mt,
        itemsSent: sent,
        itemsAcked: acked,
        data: j ?? null,
      }
    } catch (e) {
      return zero('NETWORK_ERROR', e instanceof Error ? e.message : String(e), 0, null)
    }
  }, [])

  // ------------------------------------------------------- download (M2 — new)
  // GET /api/vehicles/{i}/mission?type={mission|fence|rally} proxies to :8400
  // which speaks the MAVLink mission protocol's download side: GCS sends
  // MISSION_REQUEST_LIST, the vehicle replies with MISSION_COUNT, GCS polls
  // each MISSION_ITEM_INT, GCS sends MISSION_ACK. The response carries the
  // items in wire format (int32 E7 lat/lon); we normalize to PlanWaypoint
  // for the comparison view.
  const downloadMission = useCallback(
    async (vehicleId: number, missionType: MissionType): Promise<MissionDownloadResult> => {
      const fail = (status: number, code: string | null, error: string): MissionDownloadResult => ({
        ok: false,
        status,
        code,
        error,
        missionType: null,
        items: null,
        rawItems: null,
      })
      try {
        const url = gw(CATALOG_PORT, `/api/vehicles/${vehicleId}/mission`, { type: missionType })
        const res = await fetchGw(url, { method: 'GET' }, 35_000)
        const j = (await res.json().catch(() => null)) as
          | {
              ok: boolean
              data?: { mission_type?: number; items?: unknown[] }
              error?: { code?: string; message?: string }
            }
          | null
        if (!res.ok || !j || !j.ok) {
          return fail(res.status, j?.error?.code ?? null, j?.error?.message ?? `HTTP ${res.status}`)
        }
        const data = j.data ?? {}
        const mt = typeof data.mission_type === 'number' ? data.mission_type : null
        const itemsRaw = Array.isArray(data.items) ? data.items : []
        const rawItems: MissionItemIntWire[] = itemsRaw.map((it, i) => normalizeMissionItemIntWire(it, i))
        const items = rawItems.map((w, i) => wireItemToWaypoint(w, i))
        return {
          ok: true,
          status: res.status,
          code: null,
          error: null,
          missionType: mt,
          items,
          rawItems,
        }
      } catch (e) {
        return fail(0, 'NETWORK_ERROR', e instanceof Error ? e.message : String(e))
      }
    },
    [],
  )

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
    downloadMission,
    runBusy,
  }
}

export type PlanCatalogApi = ReturnType<typeof usePlanCatalog>
