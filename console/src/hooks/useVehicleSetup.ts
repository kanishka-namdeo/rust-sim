'use client'

/**
 * Vehicle Setup data hook (:8400 fleet REST plane + :8300 catalog
 * param-presets, ADR-0016 — the QGC/Mission-Planner configuration workflow).
 *
 * Same dual-mode lifecycle as the other console hooks: probe the REST
 * plane → LIVE (poll the setup summary + param store, run the actions
 * against the endpoints) or SIMULATED (MockSetupEngine, with periodic
 * live retries). Catalog and modes are static-shaped in both modes.
 *
 * v1 (GCS_SPEC §5.3) adds four preset round-trip methods that hit the
 * catalog on :8300 — `listPresets`, `savePreset`, `loadPreset`,
 * `deletePreset` — and a `searchParams` helper that re-queries the
 * :8400 param endpoint with `?search=&group=` filters (server-side
 * filtering, AC-5.3.1). `loadPreset` fans each loaded param out through
 * the existing `writeParam` path so the write hits PX4 via PARAM_SET.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchGw, gw, probePlane, unwrapEnvelope } from '@/lib/conn'
import { MOCK_AIRFRAME_GROUPS, MOCK_MODES, MockSetupEngine } from '@/lib/mock-setup'
import type {
  AirframeGroup,
  CalSensor,
  ModeEntry,
  ParamEntry,
  ParamStoreView,
  PresetSummary,
  VehicleSetupSummary,
} from '@/lib/types'

export const FLEET_PORT = 8400
export const CATALOG_PORT = 8300

export type SetupActionOutcome = {
  ok: boolean
  /** Human detail for the toast (live: backend message; sim: demo text). */
  detail?: string
}

/** Structured result of `searchParams` — the filtered subset of the store. */
export type SearchOutcome = {
  ok: boolean
  /** Filtered ParamStoreView (or null on hard failure). */
  store: ParamStoreView | null
  /** Source flag: 'live' (server filtered) | 'simulated' (client filtered) | 'error'. */
  source: 'live' | 'simulated' | 'error'
  detail?: string
}

/** Structured result of `listPresets`. */
export type ListPresetsOutcome = {
  ok: boolean
  presets: PresetSummary[]
  detail?: string
}

/** Structured result of `savePreset` / `deletePreset`. */
export type PresetOpOutcome = {
  ok: boolean
  detail?: string
}

/** Structured result of `loadPreset` — the loaded params + how many
 *  were applied to the vehicle via PARAM_SET. */
export type LoadPresetOutcome = {
  ok: boolean
  /** Params the catalog returned from the preset file. */
  params: { id: string; value: number; type: number }[]
  /** How many of those were successfully written via PARAM_SET. */
  applied: number
  detail?: string
}

export function useVehicleSetup(vehicleCount: number) {
  const [conn, setConn] = useState<'connecting' | 'live' | 'simulated'>('connecting')
  const [lastError, setLastError] = useState<string | null>(null)
  const [retryAt, setRetryAt] = useState<number | null>(null)
  const [index, setIndex] = useState(0)
  const [summary, setSummary] = useState<VehicleSetupSummary | null>(null)
  const [paramStore, setParamStore] = useState<ParamStoreView | null>(null)
  const [groups, setGroups] = useState<AirframeGroup[]>(MOCK_AIRFRAME_GROUPS)
  const [modes, setModes] = useState<ModeEntry[]>(MOCK_MODES)
  const [busy, setBusy] = useState(false)

  const engineRef = useRef<MockSetupEngine | null>(null)
  const connRef = useRef<'connecting' | 'live' | 'simulated'>('connecting')
  const idxRef = useRef(0)
  // mirror of `paramStore` so the preset-save action can read the latest
  // cached params without re-binding its useCallback on every poll tick.
  const paramStoreRef = useRef<ParamStoreView | null>(null)

  const setIndexBoth = useCallback((i: number) => {
    idxRef.current = i
    setIndex(i)
  }, [])

  const setConnBoth = useCallback((c: 'connecting' | 'live' | 'simulated') => {
    connRef.current = c
    setConn(c)
  }, [])

  // -- live polling --------------------------------------------------------

  const pollOnce = useCallback(async () => {
    const i = idxRef.current
    try {
      const res = await fetchGw(gw(FLEET_PORT, `/api/vehicles/${i}/setup`), { method: 'GET' }, 2500)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const j = await res.json()
      const data = unwrapEnvelope(j)
      setSummary(normalizeSummary(data))
    } catch {
      // summary polling is best-effort; conn state is probed separately
    }
    try {
      const res = await fetchGw(gw(FLEET_PORT, `/api/vehicles/${i}/params`), { method: 'GET' }, 2500)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const j = await res.json()
      const next = normalizeParamStore(unwrapEnvelope(j))
      paramStoreRef.current = next
      setParamStore(next)
    } catch {
      /* params polling is best-effort too */
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    let pollTimer: ReturnType<typeof setInterval> | null = null
    let retryTimer: ReturnType<typeof setInterval> | null = null

    const engine = new MockSetupEngine()
    engineRef.current = engine

    const stopPolling = () => {
      if (pollTimer) {
        clearInterval(pollTimer)
        pollTimer = null
      }
    }

    const startMock = (reason: string) => {
      if (cancelled) return
      stopPolling()
      setConnBoth('simulated')
      setLastError(reason)
      setRetryAt(Date.now() + 12000)
      if (!pollTimer) {
        const fresh = new MockSetupEngine()
        engineRef.current = fresh
        pollTimer = setInterval(() => {
          fresh.step()
          const v = fresh.vehicles[idxRef.current] ?? fresh.vehicles[0]
          if (v) {
            const sum = v.summary()
            const store = v.paramStoreView()
            paramStoreRef.current = store
            setSummary(sum)
            setParamStore(store)
          }
        }, 1000)
      }
    }

    const goLive = async () => {
      if (cancelled) return
      stopPolling()
      setConnBoth('live')
      setLastError(null)
      setRetryAt(null)
      try {
        // catalog + modes are static-shaped: fetch once, keep mock shapes as fallback
        const [afRes, modeRes] = await Promise.all([
          fetchGw(gw(FLEET_PORT, '/api/airframes'), { method: 'GET' }, 2500),
          fetchGw(gw(FLEET_PORT, '/api/modes'), { method: 'GET' }, 2500),
        ])
        if (afRes.ok) {
          const j = unwrapEnvelope(await afRes.json()) as { groups?: AirframeGroup[] }
          if (Array.isArray(j.groups) && j.groups.length > 0) setGroups(j.groups)
        }
        if (modeRes.ok) {
          const j = unwrapEnvelope(await modeRes.json()) as { modes?: ModeEntry[] }
          if (Array.isArray(j.modes) && j.modes.length > 0) setModes(j.modes)
        }
      } catch {
        /* keep mock shapes */
      }
      await pollOnce()
      pollTimer = setInterval(() => void pollOnce(), 1000)
    }

    probePlane(gw(FLEET_PORT, '/api/vehicles/0/setup')).then((alive) => {
      if (cancelled) return
      if (alive) void goLive()
      else startMock('backend :8400 unreachable')
    })

    retryTimer = setInterval(() => {
      if (cancelled || connRef.current === 'live') return
      probePlane(gw(FLEET_PORT, '/api/vehicles/0/setup'), 2000).then((alive) => {
        if (cancelled || !alive || connRef.current === 'live') return
        void goLive()
      })
      if (connRef.current === 'simulated') setRetryAt(Date.now() + 12000)
    }, 12000)

    return () => {
      cancelled = true
      stopPolling()
      if (retryTimer) clearInterval(retryTimer)
    }
  }, [pollOnce, setConnBoth])

  // -- actions (live endpoints / mock engine) ------------------------------

  const act = useCallback(
    async (
      path: string,
      body: Record<string, unknown>,
      mock: () => SetupActionOutcome,
    ): Promise<SetupActionOutcome> => {
      setBusy(true)
      try {
        if (connRef.current === 'live') {
          try {
            const res = await fetchGw(gw(FLEET_PORT, path), {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(body),
            }, 5000)
            const j: unknown = await res.json().catch(() => null)
            const env = (j && typeof j === 'object' && 'ok' in (j as Record<string, unknown>)) ? (j as { ok: unknown; error?: unknown }) : null
            if (!res.ok || (env && env.ok === false)) {
              const msg = env?.error ?? `HTTP ${res.status}`
              return { ok: false, detail: String(msg) }
            }
            const d = (env && 'data' in (env as Record<string, unknown>)) ? unwrapEnvelope(j) : {}
            const data = (d ?? {}) as Record<string, unknown>
            return { ok: true, detail: detailFor(path, data) }
          } catch (e) {
            return { ok: false, detail: `network: ${e instanceof Error ? e.message : 'failed'}` }
          }
        }
        // simulated
        await new Promise((r) => setTimeout(r, 350)) // demo latency
        return mock()
      } finally {
        setBusy(false)
        // refresh the views right after an action in both modes
        if (connRef.current === 'live') void pollOnce()
      }
    },
    [pollOnce],
  )

  const refreshParams = useCallback(
    () =>
      act(`/api/vehicles/${idxRef.current}/params/refresh`, {}, () => {
        engineRef.current?.vehicles[idxRef.current]?.requestParamList()
        return { ok: true, detail: 'parameter download started (simulated)' }
      }),
    [act],
  )

  // -- v1: server-side parameter search/filter (:8400 /api/vehicles/{i}/params?search=&group=)
  //
  // Hits the same endpoint the 1 Hz polling uses, but with the
  // `?search=&group=` query (server-side substring + group filter, AC-5.3.1).
  // In SIMULATED mode the backend is offline, so we client-filter the mock
  // store (the mock has 31 params, well under the 100 ms AC budget).
  const searchParams = useCallback(
    async (query: string, group?: string): Promise<SearchOutcome> => {
      const q = query.trim()
      const g = (group ?? '').trim()
      // SIMULATED: filter the mock store locally.
      if (connRef.current !== 'live') {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) {
          return { ok: false, store: null, source: 'error', detail: 'no vehicle' }
        }
        await new Promise((r) => setTimeout(r, 120)) // demo latency
        const store = v.paramStoreView()
        const ql = q.toUpperCase()
        const filtered: ParamEntry[] = store.params.filter((p) => {
          const okSearch = ql === '' || p.id.toUpperCase().includes(ql)
          const okGroup = g === '' || g === '__all__' || p.group === g
          return okSearch && okGroup
        })
        return {
          ok: true,
          source: 'simulated',
          store: { ...store, params: filtered, received: filtered.length },
          detail: `${filtered.length} of ${store.params.length} (simulated filter)`,
        }
      }
      // LIVE: hand the query to the backend.
      const qs: Record<string, string | number> = {}
      if (q) qs.search = q
      if (g && g !== '__all__') qs.group = g
      const url = gw(FLEET_PORT, `/api/vehicles/${idxRef.current}/params`, qs)
      try {
        const res = await fetchGw(url, { method: 'GET' }, 4000)
        if (!res.ok) {
          return { ok: false, store: null, source: 'error', detail: `HTTP ${res.status}` }
        }
        const j = await res.json()
        const store = normalizeParamStore(unwrapEnvelope(j))
        return { ok: true, source: 'live', store, detail: store ? `${store.params.length} rows` : 'no store' }
      } catch (e) {
        return { ok: false, store: null, source: 'error', detail: `network: ${e instanceof Error ? e.message : 'failed'}` }
      }
    },
    [],
  )

  // -- v1: param-presets (:8300 catalog) ------------------------------------
  //
  // GET /api/vehicles/{i}/param-presets → [{name, created_at, param_count}]
  const listPresets = useCallback(async (): Promise<ListPresetsOutcome> => {
    // SIMULATED: return an in-memory list (mock engine keeps a tiny map).
    if (connRef.current !== 'live') {
      const v = engineRef.current?.vehicles[idxRef.current]
      const presets = v ? v.listPresets() : []
      await new Promise((r) => setTimeout(r, 150))
      return { ok: true, presets, detail: `${presets.length} preset(s) (simulated)` }
    }
    const url = gw(CATALOG_PORT, `/api/vehicles/${idxRef.current}/param-presets`)
    try {
      const res = await fetchGw(url, { method: 'GET' }, 4000)
      if (!res.ok) {
        return { ok: false, presets: [], detail: `HTTP ${res.status}` }
      }
      const j = unwrapEnvelope(await res.json())
      const arr = Array.isArray(j) ? j : (Array.isArray((j as Record<string, unknown>)?.presets) ? (j as { presets: unknown[] }).presets : [])
      const presets = arr.map(normalizePresetSummary).filter((p): p is PresetSummary => p != null)
      return { ok: true, presets, detail: `${presets.length} preset(s)` }
    } catch (e) {
      return { ok: false, presets: [], detail: `network: ${e instanceof Error ? e.message : 'failed'}` }
    }
  }, [])

  // POST /api/vehicles/{i}/param-presets {name, params:[{id,value,type}, ...]}
  const savePreset = useCallback(
    async (name: string): Promise<PresetOpOutcome> => {
      const trimmed = name.trim()
      if (!trimmed) return { ok: false, detail: 'preset name required' }
      // SIMULATED: hand the params to the mock engine.
      if (connRef.current !== 'live') {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) return { ok: false, detail: 'no vehicle' }
        const count = v.savePreset(trimmed, v.paramStoreView().params)
        await new Promise((r) => setTimeout(r, 250))
        return { ok: true, detail: `Preset '${trimmed}' saved (${count} params, simulated)` }
      }
      // LIVE: collect the current polled paramStore, post to the catalog.
      const store = paramStoreRef.current
      if (!store || store.params.length === 0) {
        return { ok: false, detail: 'no params cached — press Download first' }
      }
      const body = {
        name: trimmed,
        params: store.params.map((p) => ({ id: p.id, value: p.value, type: p.type })),
      }
      const url = gw(CATALOG_PORT, `/api/vehicles/${idxRef.current}/param-presets`)
      try {
        const res = await fetchGw(url, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        }, 5000)
        const j: unknown = await res.json().catch(() => null)
        const env = (j && typeof j === 'object' && 'ok' in (j as Record<string, unknown>))
          ? (j as { ok: unknown; error?: unknown; data?: unknown })
          : null
        if (!res.ok || (env && env.ok === false)) {
          const msg = env?.error ?? `HTTP ${res.status}`
          return { ok: false, detail: String(msg) }
        }
        const data = (env && env.data ? env.data : {}) as { param_count?: number; name?: string }
        const count = typeof data.param_count === 'number' ? data.param_count : store.params.length
        return { ok: true, detail: `Preset '${trimmed}' saved (${count} params)` }
      } catch (e) {
        return { ok: false, detail: `network: ${e instanceof Error ? e.message : 'failed'}` }
      }
    },
    [],
  )

  // POST /api/vehicles/{i}/param-presets/{name}/load → {params:[{id,value,type},...]}
  //
  // Then fans each loaded param out through the live PARAM_SET write path
  // (`act` against `/api/vehicles/{i}/params`) so PX4 actually receives and
  // echo-confirms each value — the spec's "applied via the existing
  // writeParam method" (§5.3) and the QGC §8.3 step-10 post-condition.
  const loadPreset = useCallback(
    async (name: string): Promise<LoadPresetOutcome> => {
      const trimmed = name.trim()
      if (!trimmed) return { ok: false, params: [], applied: 0, detail: 'preset name required' }
      // SIMULATED: pull params from the mock engine and write them locally.
      if (connRef.current !== 'live') {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) return { ok: false, params: [], applied: 0, detail: 'no vehicle' }
        const params = v.loadPreset(trimmed)
        if (params.length === 0) {
          return { ok: false, params: [], applied: 0, detail: `preset '${trimmed}' not found` }
        }
        let applied = 0
        for (const p of params) {
          if (v.writeParam(p.id, p.value).ok) applied++
        }
        await new Promise((r) => setTimeout(r, 400))
        return { ok: true, params, applied, detail: `${applied}/${params.length} params written (simulated)` }
      }
      // LIVE: ask the catalog for the params, then write each via PARAM_SET.
      const url = gw(CATALOG_PORT, `/api/vehicles/${idxRef.current}/param-presets/${encodeURIComponent(trimmed)}/load`)
      let params: { id: string; value: number; type: number }[]
      try {
        const res = await fetchGw(url, { method: 'POST' }, 6000)
        if (!res.ok) {
          return { ok: false, params: [], applied: 0, detail: `HTTP ${res.status}` }
        }
        const j = unwrapEnvelope(await res.json())
        const arr = Array.isArray(j) ? j : (Array.isArray((j as Record<string, unknown>)?.params) ? (j as { params: unknown[] }).params : [])
        params = arr
          .map((p) => {
            const r = (p && typeof p === 'object' ? p : {}) as Record<string, unknown>
            const id = typeof r.id === 'string' ? r.id : ''
            const value = typeof r.value === 'number' ? r.value : Number(r.value)
            const type = typeof r.type === 'number' ? r.type : 9
            if (!id || !Number.isFinite(value)) return null
            return { id, value, type }
          })
          .filter((p): p is { id: string; value: number; type: number } => p != null)
      } catch (e) {
        return { ok: false, params: [], applied: 0, detail: `network: ${e instanceof Error ? e.message : 'failed'}` }
      }
      // fan each loaded param out through PARAM_SET (the live write path)
      let applied = 0
      for (const p of params) {
        const r = await act(`/api/vehicles/${idxRef.current}/params`, { id: p.id, value: p.value }, () => ({ ok: true, detail: '' }))
        if (r.ok) applied++
      }
      return { ok: true, params, applied, detail: `${applied}/${params.length} params written via PARAM_SET` }
    },
    [act],
  )

  // DELETE /api/vehicles/{i}/param-presets/{name}
  const deletePreset = useCallback(
    async (name: string): Promise<PresetOpOutcome> => {
      const trimmed = name.trim()
      if (!trimmed) return { ok: false, detail: 'preset name required' }
      if (connRef.current !== 'live') {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) return { ok: false, detail: 'no vehicle' }
        const ok = v.deletePreset(trimmed)
        await new Promise((r) => setTimeout(r, 200))
        return ok
          ? { ok: true, detail: `Preset '${trimmed}' deleted (simulated)` }
          : { ok: false, detail: `preset '${trimmed}' not found` }
      }
      const url = gw(CATALOG_PORT, `/api/vehicles/${idxRef.current}/param-presets/${encodeURIComponent(trimmed)}`)
      try {
        const res = await fetchGw(url, { method: 'DELETE' }, 4000)
        if (!res.ok) {
          const j: unknown = await res.json().catch(() => null)
          const env = (j && typeof j === 'object' && 'error' in (j as Record<string, unknown>))
            ? (j as { error?: unknown })
            : null
          const msg = env?.error ?? `HTTP ${res.status}`
          return { ok: false, detail: String(msg) }
        }
        return { ok: true, detail: `Preset '${trimmed}' deleted` }
      } catch (e) {
        return { ok: false, detail: `network: ${e instanceof Error ? e.message : 'failed'}` }
      }
    },
    [],
  )

  const writeParam = useCallback(
    (id: string, value: number) =>
      act(`/api/vehicles/${idxRef.current}/params`, { id, value }, () => {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) return { ok: false, detail: 'no vehicle' }
        const r = v.writeParam(id, value)
        return { ok: r.ok, detail: r.ok ? `${id} = ${value} (echo confirmed, simulated)` : 'write rejected' }
      }),
    [act],
  )

  const applyAirframe = useCallback(
    (sysAutostart: number) =>
      act(`/api/vehicles/${idxRef.current}/airframe`, { sys_autostart: sysAutostart }, () => {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) return { ok: false, detail: 'no vehicle' }
        const r = v.applyAirframe(sysAutostart)
        return {
          ok: r.ok,
          detail: r.ok
            ? `SYS_AUTOSTART=${sysAutostart} written — rebooting with new airframe (simulated)`
            : 'apply rejected (armed?)',
        }
      }),
    [act],
  )

  const calibrate = useCallback(
    (sensor: CalSensor) =>
      act(`/api/vehicles/${idxRef.current}/calibrate`, { sensor }, () => {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v) return { ok: false, detail: 'no vehicle' }
        const r = v.calibrate(sensor)
        return { ok: r.accepted, detail: r.accepted ? `${r.description} calibration accepted (simulated)` : 'rejected (armed?)' }
      }),
    [act],
  )

  const setMode = useCallback(
    (mode: string) =>
      act(`/api/vehicles/${idxRef.current}/mode`, { mode }, () => {
        const v = engineRef.current?.vehicles[idxRef.current]
        if (!v || !v.setMode(mode)) return { ok: false, detail: 'mode rejected' }
        return { ok: true, detail: `mode ${mode} accepted (simulated)` }
      }),
    [act],
  )

  return {
    conn,
    port: FLEET_PORT,
    catalogPort: CATALOG_PORT,
    lastError,
    retryAt,
    index,
    setIndex: setIndexBoth,
    vehicleCount,
    summary,
    paramStore,
    groups,
    modes,
    busy,
    refreshParams,
    writeParam,
    applyAirframe,
    calibrate,
    setMode,
    // v1 — param search/filter + presets
    searchParams,
    listPresets,
    savePreset,
    loadPreset,
    deletePreset,
  }
}

export type VehicleSetupApi = ReturnType<typeof useVehicleSetup>

// ---------------------------------------------------------------------------
// tolerant normalizers (envelope shapes from ADR-0016's REST plane)
// ---------------------------------------------------------------------------

function num(v: unknown): number | null {
  if (typeof v === 'number' && Number.isFinite(v)) return v
  if (typeof v === 'string' && v.trim() !== '' && Number.isFinite(Number(v))) return Number(v)
  return null
}

function str(v: unknown, fallback = ''): string {
  return typeof v === 'string' && v.length > 0 ? v : fallback
}

function rec(v: unknown): Record<string, number | null> | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null
  const r = v as Record<string, unknown>
  const out: Record<string, number | null> = {}
  for (const [k, val] of Object.entries(r)) out[k] = num(val)
  return out
}

function normalizeSummary(raw: unknown): VehicleSetupSummary | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const r = raw as Record<string, unknown>
  const air = (r.airframe && typeof r.airframe === 'object' ? r.airframe : {}) as Record<string, unknown>
  const cal = (r.calibration && typeof r.calibration === 'object' && !Array.isArray(r.calibration) ? r.calibration : null) as Record<string, unknown> | null
  const params = (r.params && typeof r.params === 'object' && !Array.isArray(r.params) ? r.params : null) as Record<string, unknown> | null
  const autopilot = (r.autopilot && typeof r.autopilot === 'object' ? r.autopilot : {}) as Record<string, unknown>
  return {
    index: num(r.index) ?? 0,
    sysid: num(r.sysid) ?? 1,
    compid: num(r.compid) ?? 1,
    fsm: str(r.fsm, 'INIT'),
    mode: str(r.mode, 'UNKNOWN'),
    mode_word: num(r.mode_word) ?? 0,
    armed: r.armed === true,
    battery_pct: num(r.battery_pct) ?? 0,
    voltage_v: num(r.voltage_v),
    restart_pending: r.restart_pending === true,
    autopilot: {
      type: str(autopilot.type, 'PX4'),
      version: str(autopilot.version, ''),
    },
    airframe: {
      sys_autostart: num(air.sys_autostart),
      name: str(air.name, 'Unknown airframe'),
      frame_type: str(air.frame_type),
      category: str(air.category, 'Other'),
      dynamics_compatible: air.dynamics_compatible === true,
    },
    params: params
      ? {
          total: num(params.total) ?? 0,
          received: num(params.received) ?? 0,
          state: str(params.state, 'Idle'),
        }
      : null,
    calibration: cal
      ? {
          accel: cal.accel === true ? true : cal.accel === false ? false : null,
          gyro: cal.gyro === true ? true : cal.gyro === false ? false : null,
          mag0: cal.mag0 === true ? true : cal.mag0 === false ? false : null,
          mag1: cal.mag1 === true ? true : cal.mag1 === false ? false : null,
          mag2: cal.mag2 === true ? true : cal.mag2 === false ? false : null,
          level_horizon: cal.level_horizon === true ? true : cal.level_horizon === false ? false : null,
        }
      : null,
    power: rec(r.power),
    safety: rec(r.safety),
  }
}

function normalizeParamStore(raw: unknown): ParamStoreView | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const r = raw as Record<string, unknown>
  const params = Array.isArray(r.params)
    ? r.params
        .map((p, i) => {
          const e = (p && typeof p === 'object' ? p : {}) as Record<string, unknown>
          const id = str(e.id)
          if (!id) return null
          const value = num(e.value) ?? 0
          const type = num(e.type) ?? 9
          const defaultVal = num(e.default)
          // group: explicit `group` field, else fall back to the id's
          // leading token (PX4 convention: `MPC_XY_VEL_MAX` → `MPC`).
          const groupStr = str(e.group)
          const group = groupStr !== '' ? groupStr : id.split('_')[0] ?? ''
          const isChanged =
            e.is_changed === true
              ? true
              : e.is_changed === false
                ? false
                : defaultVal == null
                  ? false
                  : Math.abs(defaultVal - value) > 1e-9
          return {
            id,
            value,
            raw: num(e.raw) ?? value,
            type,
            kind: str(e.kind, type === 9 ? 'real32' : type === 6 ? 'int32' : 'custom'),
            index: num(e.index) ?? i,
            group,
            default: defaultVal,
            is_changed: isChanged,
          }
        })
        .filter((p): p is ParamStoreView['params'][number] => p != null)
    : []
  return {
    total: num(r.total) ?? 0,
    received: num(r.received) ?? params.length,
    state: str(r.state, 'Idle'),
    requested_ms: num(r.requested_ms),
    last_value_ms: num(r.last_value_ms),
    params,
  }
}

/** Tolerant normalizer for a `PresetSummary` row. */
function normalizePresetSummary(raw: unknown): PresetSummary | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const r = raw as Record<string, unknown>
  const name = str(r.name)
  if (!name) return null
  return {
    name,
    created_at: str(r.created_at, r.createdAt ?? ''),
    param_count: num(r.param_count, r.paramCount) ?? 0,
  }
}

/** Turn an action response body into a short toast detail. */
function detailFor(path: string, data: Record<string, unknown>): string | undefined {
  if (path.endsWith('/params/refresh')) {
    return data.requested === true ? 'PARAM_REQUEST_LIST sent — watch progress on this page' : 'request not queued'
  }
  if (path.endsWith('/params')) {
    const c = data.confirmed
    return c != null ? `${data.id} = ${c} (PARAM_VALUE echo confirmed)` : 'write UNCONFIRMED (no echo)'
  }
  if (path.endsWith('/airframe')) {
    return data.write_confirmed === true
      ? `SYS_AUTOSTART=${data.sys_autostart} written — restart queued, airframe loads at next boot`
      : 'write unconfirmed — restart NOT queued'
  }
  if (path.endsWith('/calibrate')) {
    return data.accepted === true
      ? `MAV_CMD 241 (${data.description}) ACCEPTED — CAL_* params update after the run`
      : `command not accepted (result=${data.result}) — disarmed + idle required`
  }
  if (path.endsWith('/mode')) {
    return data.accepted === true ? `mode ${data.mode} ACCEPTED` : `mode change not accepted (result=${data.result})`
  }
  return undefined
}
