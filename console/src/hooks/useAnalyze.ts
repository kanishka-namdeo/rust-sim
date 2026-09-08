'use client'

/**
 * Analyze View data hook (:8300 fleet-catalog plane, GCS_SPEC §5.5 / §8.5).
 *
 * Six API methods, all routed through `src/lib/conn.ts` gateway mode with
 * `?XTransformPort=8300`:
 *   - listReplays()                         GET /api/replays
 *   - listUlogs()                           GET /api/ulogs
 *   - getReplayMeta(filename)                GET /api/replays/{file}/meta
 *   - getReplayTopics(filename)              GET /api/replays/{file}/topics
 *   - getReplayData(filename, from, to, t)   GET /api/replays/{file}/data?from_tick&to_tick&topic
 *   - getUlogTopics(filename)                GET /api/ulogs/{file}/topics
 *   - getUlogTopicData(file, topic, a, b)    GET /api/ulogs/{file}/topics/{topic}/data?from_s&to_s
 *
 * The catalog plane is shared with the M2/M4 mission + preset CRUD endpoints
 * (ADR-0027). All Analyze responses are tolerant-shaped: missing fields
 * (e.g. `geo_origin`, `final_pos_ned_m`) are common — the UI degrades
 * gracefully (an empty map, an empty plot list) instead of crashing.
 *
 * When the backend's replay/ULog endpoints are not yet implemented (M6-Backend
 * track in flight in parallel), the methods return empty arrays / null so
 * the UI shows an empty-state panel instead of erroring.
 */

import { useCallback, useState } from 'react'
import { fetchGw, gw, unwrapEnvelope } from '@/lib/conn'
import type {
  ReplayFile,
  ReplayMeta,
  ReplayTopicData,
  UlogFile,
  UlogTopicData,
} from '@/lib/types'

export const CATALOG_PORT = 8300

// ---------------------------------------------------------------------------
// tolerant scalar helpers (mirrors the shape of conn.ts' private helpers)
// ---------------------------------------------------------------------------

type Rec = Record<string, unknown>

function asRec(v: unknown): Rec | null {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Rec) : null
}

function num(...cands: unknown[]): number | null {
  for (const c of cands) {
    if (typeof c === 'number' && Number.isFinite(c)) return c
    if (typeof c === 'string' && c.trim() !== '' && Number.isFinite(Number(c))) return Number(c)
  }
  return null
}

function str(...cands: unknown[]): string | null {
  for (const c of cands) if (typeof c === 'string' && c.length > 0) return c
  return null
}

function numArr(v: unknown): number[] | null {
  if (!Array.isArray(v)) return null
  const out: number[] = []
  for (const x of v) {
    const n = num(x)
    if (n == null) return null
    out.push(n)
  }
  return out
}

function numVec3(v: unknown): [number, number, number] | null {
  if (!Array.isArray(v) || v.length < 3) return null
  const a = num(v[0])
  const b = num(v[1])
  const c = num(v[2])
  if (a == null || b == null || c == null) return null
  return [a, b, c]
}

function numVec4(v: unknown): [number, number, number, number] | null {
  if (!Array.isArray(v) || v.length < 4) return null
  const a = num(v[0])
  const b = num(v[1])
  const c = num(v[2])
  const d = num(v[3])
  if (a == null || b == null || c == null || d == null) return null
  return [a, b, c, d]
}

/** Pull the catalog's per-file mtime into a JS epoch ms (server returns either
 *  s, ms, or an ISO 8601 string). Falls back to 0 (so sorting stays stable). */
function coerceMtime(...cands: unknown[]): number {
  const n = num(...cands)
  if (n != null) {
    // heuristic: > 1e11 means ms; < 1e11 means s
    return n > 1e11 ? n : n * 1000
  }
  const s = str(...cands)
  if (s) {
    const t = Date.parse(s)
    if (Number.isFinite(t)) return t
  }
  return 0
}

// ---------------------------------------------------------------------------
// normalizers
// ---------------------------------------------------------------------------

function normReplayFile(raw: unknown): ReplayFile | null {
  const r = asRec(raw) ?? {}
  const filename = str(r.filename, r.name, r.file)
  if (!filename) return null
  return {
    filename,
    duration_s: num(r.duration_s, r.duration, r.virtual_duration_s) ?? 0,
    vehicle_count: num(r.vehicle_count, r.vehicles) ?? 1,
    scenario_sha256: str(r.scenario_sha256, r.scenario_hash, r.sha256) ?? '',
    mtime: coerceMtime(r.mtime, r.modified, r.modified_at),
  }
}

function normUlogFile(raw: unknown): UlogFile | null {
  const r = asRec(raw) ?? {}
  const filename = str(r.filename, r.name, r.file)
  if (!filename) return null
  return {
    filename,
    size_bytes: num(r.size_bytes, r.size, r.bytes) ?? 0,
    mtime: coerceMtime(r.mtime, r.modified, r.modified_at),
  }
}

function normReplayMeta(raw: unknown): ReplayMeta | null {
  const r = asRec(raw) ?? {}
  const records = num(r.records, r.tick_count, r.total_ticks) ?? 0
  const virtual_duration_s = num(r.virtual_duration_s, r.duration_s, r.duration) ?? 0
  const tick_rate_hz = num(r.tick_rate_hz, r.rate_hz, r.tick_rate) ?? 0
  // Records / rate fallback when virtual_duration_s is missing or zero.
  const duration = virtual_duration_s > 0 ? virtual_duration_s : tick_rate_hz > 0 ? records / tick_rate_hz : 0
  const originRec = asRec(r.geo_origin) ?? asRec(r.origin)
  const geo_origin = originRec
    ? {
        lat_deg: num(originRec.lat_deg, originRec.lat) ?? 47.39777,
        lon_deg: num(originRec.lon_deg, originRec.lon) ?? 8.54558,
        alt_m: num(originRec.alt_m, originRec.alt) ?? 500,
      }
    : null
  return {
    tick_rate_hz,
    seed: r.seed != null ? (typeof r.seed === 'number' ? r.seed : String(r.seed)) : 0,
    scenario_sha256: str(r.scenario_sha256, r.scenario_hash) ?? '',
    records,
    virtual_duration_s: duration,
    final_pos_ned_m: numVec3(r.final_pos_ned_m) ?? undefined,
    final_q_wxyz: numVec4(r.final_q_wxyz) ?? undefined,
    geo_origin,
  }
}

function normReplayData(raw: unknown): ReplayTopicData | null {
  const r = asRec(raw) ?? {}
  const ticks = numArr(r.ticks) ?? numArr(r.t) ?? []
  const valuesRaw = r.values
  let values: number[] | number[][] = []
  if (Array.isArray(valuesRaw)) {
    if (valuesRaw.length > 0 && Array.isArray(valuesRaw[0])) {
      // vector topic — [[n,e,d], ...]
      values = (valuesRaw as unknown[]).map((v) => numArr(v) ?? [])
    } else {
      // scalar topic — [v, v, ...]
      values = numArr(valuesRaw) ?? []
    }
  }
  return { ticks, values }
}

function normUlogData(raw: unknown): UlogTopicData | null {
  const r = asRec(raw) ?? {}
  const t = numArr(r.t) ?? numArr(r.timestamps, r.time) ?? []
  const fieldsRec = asRec(r.fields) ?? {}
  const fields: Record<string, number[]> = {}
  for (const [k, v] of Object.entries(fieldsRec)) {
    const a = numArr(v)
    if (a) fields[k] = a
  }
  // Some servers flatten single-field topics as a top-level `values` array.
  if (Object.keys(fields).length === 0) {
    const a = numArr(r.values)
    if (a) fields.value = a
  }
  return { t, fields }
}

// ---------------------------------------------------------------------------

export interface AnalyzeApi {
  /** Catalog plane reachable (last probe result). */
  catalogAlive: boolean
  /** True while any list/meta/data fetch is in flight (for the header badge). */
  busy: boolean
  /** Last error message surfaced to the UI (null = no error). */
  lastError: string | null
  /** GET /api/replays → list of .replay files (sorted by mtime desc). */
  listReplays: () => Promise<ReplayFile[]>
  /** GET /api/ulogs → list of .ulg files (sorted by mtime desc). */
  listUlogs: () => Promise<UlogFile[]>
  /** GET /api/replays/{file}/meta → header info (tick rate, duration, origin). */
  getReplayMeta: (filename: string) => Promise<ReplayMeta | null>
  /** GET /api/replays/{file}/topics → topic names available in this replay. */
  getReplayTopics: (filename: string) => Promise<string[]>
  /** GET /api/replays/{file}/data?from_tick&to_tick&topic → tick samples. */
  getReplayData: (filename: string, fromTick: number, toTick: number, topic: string) => Promise<ReplayTopicData | null>
  /** GET /api/ulogs/{file}/topics → topic names in this ULog. */
  getUlogTopics: (filename: string) => Promise<string[]>
  /** GET /api/ulogs/{file}/topics/{topic}/data?from_s&to_s → field samples. */
  getUlogTopicData: (filename: string, topic: string, fromS: number, toS: number) => Promise<UlogTopicData | null>
  /** Force a refresh of the catalog-alive flag (called by the header Refresh). */
  probe: () => Promise<boolean>
}

export function useAnalyze(): AnalyzeApi {
  const [catalogAlive, setCatalogAlive] = useState(false)
  const [busy, setBusy] = useState(false)
  const [lastError, setLastError] = useState<string | null>(null)

  /** Wrap any catalog fetch: marks busy, surfaces a friendly error, returns
   *  the parsed JSON (envelope-stripped) or null on failure. */
  const getJson = useCallback(
    async (path: string, query: Record<string, string | number> = {}, timeoutMs = 5000): Promise<unknown> => {
      setBusy(true)
      try {
        const res = await fetchGw(gw(CATALOG_PORT, path, query), { method: 'GET' }, timeoutMs)
        if (!res.ok) {
          const msg = `HTTP ${res.status} on ${path}`
          setLastError(msg)
          setCatalogAlive(false)
          return null
        }
        const j = await res.json().catch(() => null)
        setCatalogAlive(true)
        setLastError(null)
        return unwrapEnvelope(j)
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        setLastError(msg)
        setCatalogAlive(false)
        return null
      } finally {
        setBusy(false)
      }
    },
    [],
  )

  const listReplays = useCallback(async (): Promise<ReplayFile[]> => {
    const data = await getJson('/api/replays', {}, 4000)
    if (!data) return []
    const arr = Array.isArray(data)
      ? data
      : Array.isArray((asRec(data) ?? {}).files)
        ? (asRec(data) as { files: unknown[] }).files
        : []
    const list = arr.map(normReplayFile).filter((f): f is ReplayFile => f != null)
    list.sort((a, b) => b.mtime - a.mtime)
    return list
  }, [getJson])

  const listUlogs = useCallback(async (): Promise<UlogFile[]> => {
    const data = await getJson('/api/ulogs', {}, 4000)
    if (!data) return []
    const arr = Array.isArray(data)
      ? data
      : Array.isArray((asRec(data) ?? {}).files)
        ? (asRec(data) as { files: unknown[] }).files
        : []
    const list = arr.map(normUlogFile).filter((f): f is UlogFile => f != null)
    list.sort((a, b) => b.mtime - a.mtime)
    return list
  }, [getJson])

  const getReplayMeta = useCallback(
    async (filename: string): Promise<ReplayMeta | null> => {
      const data = await getJson(`/api/replays/${encodeURIComponent(filename)}/meta`, {}, 5000)
      if (!data) return null
      return normReplayMeta(data)
    },
    [getJson],
  )

  const getReplayTopics = useCallback(
    async (filename: string): Promise<string[]> => {
      const data = await getJson(`/api/replays/${encodeURIComponent(filename)}/topics`, {}, 5000)
      if (!data) return []
      const arr = Array.isArray(data)
        ? data
        : Array.isArray((asRec(data) ?? {}).topics)
          ? (asRec(data) as { topics: unknown[] }).topics
          : []
      return arr
        .map((t) => {
          if (typeof t === 'string') return t
          const r = asRec(t)
          return r ? str(r.name, r.topic) ?? '' : ''
        })
        .filter((t) => t.length > 0)
    },
    [getJson],
  )

  const getReplayData = useCallback(
    async (filename: string, fromTick: number, toTick: number, topic: string): Promise<ReplayTopicData | null> => {
      const data = await getJson(
        `/api/replays/${encodeURIComponent(filename)}/data`,
        { from_tick: fromTick, to_tick: toTick, topic },
        8000,
      )
      if (!data) return null
      return normReplayData(data)
    },
    [getJson],
  )

  const getUlogTopics = useCallback(
    async (filename: string): Promise<string[]> => {
      const data = await getJson(`/api/ulogs/${encodeURIComponent(filename)}/topics`, {}, 5000)
      if (!data) return []
      const arr = Array.isArray(data)
        ? data
        : Array.isArray((asRec(data) ?? {}).topics)
          ? (asRec(data) as { topics: unknown[] }).topics
          : []
      return arr
        .map((t) => {
          if (typeof t === 'string') return t
          const r = asRec(t)
          return r ? str(r.name, r.topic) ?? '' : ''
        })
        .filter((t) => t.length > 0)
    },
    [getJson],
  )

  const getUlogTopicData = useCallback(
    async (filename: string, topic: string, fromS: number, toS: number): Promise<UlogTopicData | null> => {
      const data = await getJson(
        `/api/ulogs/${encodeURIComponent(filename)}/topics/${encodeURIComponent(topic)}/data`,
        { from_s: fromS, to_s: toS },
        8000,
      )
      if (!data) return null
      return normUlogData(data)
    },
    [getJson],
  )

  const probe = useCallback(async (): Promise<boolean> => {
    const data = await getJson('/api/health', {}, 2500)
    const alive = data != null
    setCatalogAlive(alive)
    return alive
  }, [getJson])

  return {
    catalogAlive,
    busy,
    lastError,
    listReplays,
    listUlogs,
    getReplayMeta,
    getReplayTopics,
    getReplayData,
    getUlogTopics,
    getUlogTopicData,
    probe,
  }
}

export type AnalyzeHookApi = ReturnType<typeof useAnalyze>
