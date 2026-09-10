'use client'

/**
 * Analyze View data hook (:8300 fleet-catalog plane, GCS_SPEC §5.5 / §8.5).
 *
 * Four API methods, all routed through `src/lib/conn.ts` gateway mode with
 * `?XTransformPort=8300`:
 *   - listUlogs()                           GET /api/ulogs
 *   - getUlogTopics(filename)                GET /api/ulogs/{file}/topics
 *   - getUlogTopicData(file, topic, a, b)    GET /api/ulogs/{file}/topics/{topic}/data?from_s&to_s
 *
 * The catalog plane is shared with the M2/M4 mission + preset CRUD endpoints
 * (ADR-0027). All Analyze responses are tolerant-shaped: missing fields are
 * common — the UI degrades gracefully (an empty plot list) instead of crashing.
 *
 * When the backend's ULog endpoints are not yet implemented (M6-Backend track
 * in flight in parallel), the methods return empty arrays / null so the UI
 * shows an empty-state panel instead of erroring.
 */

import { useCallback, useState } from 'react'
import { fetchGw, gw, unwrapEnvelope } from '@/lib/conn'
import type {
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

function normUlogData(raw: unknown): UlogTopicData | null {
  const r = asRec(raw) ?? {}
  const t = numArr(r.t) ?? numArr(r.timestamps) ?? numArr(r.time) ?? []
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
  /** GET /api/ulogs → list of .ulg files (sorted by mtime desc). */
  listUlogs: () => Promise<UlogFile[]>
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
    listUlogs,
    getUlogTopics,
    getUlogTopicData,
    probe,
  }
}

export type AnalyzeHookApi = ReturnType<typeof useAnalyze>
