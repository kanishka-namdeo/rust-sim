'use client'

/**
 * Plan View (GCS_SPEC §5.1 + §8.1 — the QGC Plan View analog).
 *
 * Layout: mission-list panel (left) · Leaflet map (middle) · waypoint table
 * + geofence editor + validation/upload status (right).
 *
 * The mission catalog (:8300, fleet-catalog binary) is the single source of
 * truth — there is no client-side persistence. The catalog answers with the
 * `{"ok":bool,"data":...}` envelope; the `usePlanCatalog` hook unwraps.
 *
 * Edit model (matches §8.1 step-by-step):
 *   1. New Mission      → enter edit mode, mode='waypoint', empty file
 *   2. Map click        → add waypoint (numbered blue marker + blue polyline)
 *   3. Waypoint table   → edit altitude / hold / accept-radius
 *   4. Draw Geofence    → toggle mode='fence', click adds vertex, dbl-click closes
 *   5. Validate         → POST /api/missions/{id}/validate → toast
 *   6. Save             → modal: name → POST or PUT to catalog
 *   7. Upload to Vehicle → modal: vehicle index → POST /api/vehicles/{i}/mission/upload
 *
 * The catalog assigns the ULID on POST (we send `id=""`); PUT bumps the
 * version. The upload path is a version-checked stub at M1 — the actual
 * MAVLink mission protocol upload is M2 scope (the endpoint returns
 * "validated + version-checked (M1 stub)" or PX4_VERSION_UNAVAILABLE).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  CheckCircle2,
  CircleSlash,
  Eraser,
  FilePlus2,
  Hexagon,
  Loader2,
  MapPin,
  PlaneTakeoff,
  RefreshCw,
  Save,
  Send,
  Trash2,
  TriangleAlert,
  Upload,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Progress } from '@/components/ui/progress'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useToast } from '@/hooks/use-toast'
import { usePlanCatalog } from '@/hooks/usePlanCatalog'
import { ConnBadge } from './ConnBadge'
import { PlanMap, type PlanMapMode } from './PlanMap'
import {
  DEFAULT_WP_ACCEPT_M,
  DEFAULT_WP_ALT_M,
  DEFAULT_WP_HOLD_S,
  PX4_TEST_FIELD,
  emptyPlanMission,
  type PlanMissionFile,
  type PlanValidationError,
  type PlanValidationResult,
} from '@/lib/plan-types'

const FLEET_PORT = 8400

/** Edit mode (matches §8.1 step-by-step). */
type EditMode = 'idle' | 'waypoint' | 'fence'

/** Upload progress (three sub-bars per spec §8.1 step 12). */
interface UploadProgress {
  missionTotal: number
  fenceTotal: number
  rallyTotal: number
  /** 'validated' = M1 stub returned successfully; 'failed' = error */
  status: 'validated' | 'failed'
  /** Status message (the catalog's response or error). */
  message: string
  /** Stable error code (e.g. PX4_VERSION_UNAVAILABLE) on failure. */
  code: string | null
  /** HTTP status (0 = network). */
  httpStatus: number
}

/** One row in the saved-missions list. */
interface SavedMissionRow {
  id: string
  name: string
  version: number
  updated_at: string
  waypoint_count: number
  fence_count: number
  rally_count: number
}

const DEFAULT_VEHICLE_COUNT = 4

export function PlanView() {
  const catalog = usePlanCatalog()
  const { toast } = useToast()

  // --------------------------------------------------------------- editor state
  const [file, setFile] = useState<PlanMissionFile>(() => emptyPlanMission('untitled'))
  const [mode, setMode] = useState<EditMode>('idle')
  const [dirty, setDirty] = useState(false)
  const [validated, setValidated] = useState<PlanValidationResult | null>(null)
  const [uploadProgress, setUploadProgress] = useState<UploadProgress | null>(null)
  const [selectedWp, setSelectedWp] = useState<number | null>(null)
  const [saveOpen, setSaveOpen] = useState(false)
  const [saveName, setSaveName] = useState('untitled')
  const [uploadOpen, setUploadOpen] = useState(false)
  const [uploadVehicle, setUploadVehicle] = useState(0)
  const [fleetConn, setFleetConn] = useState<'connecting' | 'live' | 'simulated'>('connecting')
  const [vehicleCount, setVehicleCount] = useState(DEFAULT_VEHICLE_COUNT)

  // Open the Save modal — preset the name input from the current mission
  // (NOT in an effect, to avoid the react-hooks/set-state-in-effect rule).
  const openSave = useCallback(() => {
    setSaveName(file.mission.name || 'untitled')
    setSaveOpen(true)
  }, [file.mission.name])

  // --------------------------------------------------------------- fleet probe
  // Probe the fleet plane (:8400) so the upload-modal vehicle list reflects
  // live vehicles. Best-effort; falls back to 4 simulated vehicles.
  useEffect(() => {
    let cancelled = false
    const probe = async () => {
      try {
        const res = await fetch(`/api/fleet?XTransformPort=${FLEET_PORT}`, { cache: 'no-store' })
        if (!res.ok) throw new Error(`HTTP ${res.status}`)
        const j: unknown = await res.json()
        const env = (j && typeof j === 'object' && 'data' in (j as Record<string, unknown>))
          ? (j as { data: unknown }).data
          : j
        const r = (env && typeof env === 'object' ? env : null) as Record<string, unknown> | null
        const vehicles = Array.isArray(r?.vehicles) ? r!.vehicles as unknown[] : []
        if (!cancelled && vehicles.length > 0) {
          setFleetConn('live')
          setVehicleCount(Math.max(1, vehicles.length))
        } else if (!cancelled) {
          setFleetConn('live')
          setVehicleCount(0)
        }
      } catch {
        if (!cancelled) setFleetConn('simulated')
      }
    }
    void probe()
    const id = setInterval(probe, 12000)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [])

  // --------------------------------------------------------------- helpers
  const addWaypoint = useCallback(
    (lat: number, lng: number) => {
      setFile((f) => ({
        ...f,
        waypoints: [
          ...f.waypoints,
          {
            seq: f.waypoints.length === 0 ? 0 : Math.max(...f.waypoints.map((w) => w.seq)) + 1,
            frame: 3, // MAV_FRAME_GLOBAL_RELATIVE_ALT
            command: 16, // MAV_CMD_NAV_WAYPOINT
            x: lat,
            y: lng,
            z: DEFAULT_WP_ALT_M,
            param1: DEFAULT_WP_HOLD_S,
            param2: DEFAULT_WP_ACCEPT_M,
            param3: 0,
            param4: 0,
          },
        ],
      }))
      setDirty(true)
      setValidated(null)
      setUploadProgress(null)
    },
    [],
  )

  const moveWaypoint = useCallback((seq: number, lat: number, lng: number) => {
    setFile((f) => ({
      ...f,
      waypoints: f.waypoints.map((w) => (w.seq === seq ? { ...w, x: lat, y: lng } : w)),
    }))
    setDirty(true)
    setValidated(null)
    setUploadProgress(null)
  }, [])

  const removeWaypoint = useCallback((seq: number) => {
    setFile((f) => ({
      ...f,
      // re-number seq to be contiguous (QGC behavior)
      waypoints: f.waypoints
        .filter((w) => w.seq !== seq)
        .map((w, i) => ({ ...w, seq: i })),
    }))
    setDirty(true)
    setValidated(null)
    setUploadProgress(null)
    setSelectedWp(null)
  }, [])

  const patchWaypoint = useCallback(
    (seq: number, patch: Partial<PlanMissionFile['waypoints'][number]>) => {
      setFile((f) => ({
        ...f,
        waypoints: f.waypoints.map((w) => (w.seq === seq ? { ...w, ...patch } : w)),
      }))
      setDirty(true)
      setValidated(null)
      setUploadProgress(null)
    },
    [],
  )

  const addFenceVertex = useCallback((lat: number, lng: number) => {
    setFile((f) => ({
      ...f,
      geofence: {
        ...f.geofence,
        inclusion: [...f.geofence.inclusion, [lat, lng]],
      },
    }))
    setDirty(true)
    setValidated(null)
    setUploadProgress(null)
  }, [])

  const closeFence = useCallback(() => {
    if (file.geofence.inclusion.length < 3) {
      toast({
        title: 'Cannot close geofence',
        description: `need at least 3 vertices, got ${file.geofence.inclusion.length}`,
        variant: 'destructive',
      })
      return
    }
    setMode('waypoint')
    toast({
      title: 'Geofence closed',
      description: `${file.geofence.inclusion.length} vertices · ceiling ${file.geofence.ceiling_m} m · floor ${file.geofence.floor_m} m`,
    })
  }, [file.geofence, toast])

  const clearFence = useCallback(() => {
    setFile((f) => ({ ...f, geofence: { ...f.geofence, inclusion: [], exclusion: [] } }))
    setDirty(true)
    setValidated(null)
    setUploadProgress(null)
  }, [])

  const clearWaypoints = useCallback(() => {
    setFile((f) => ({ ...f, waypoints: [] }))
    setDirty(true)
    setValidated(null)
    setUploadProgress(null)
    setSelectedWp(null)
  }, [])

  const newMission = useCallback(() => {
    setFile(emptyPlanMission('untitled'))
    setMode('waypoint')
    setDirty(true)
    setValidated(null)
    setUploadProgress(null)
    setSelectedWp(null)
    toast({
      title: 'New mission',
      description: 'edit mode active · click the map to add waypoint 1',
    })
  }, [toast])

  const loadMission = useCallback(
    async (id: string) => {
      const loaded = await catalog.getMission(id)
      if (!loaded) {
        toast({ title: 'Load failed', description: `mission ${id} not found`, variant: 'destructive' })
        return
      }
      setFile(loaded)
      setMode('idle')
      setDirty(false)
      setValidated(null)
      setUploadProgress(null)
      setSelectedWp(null)
      toast({
        title: `Loaded "${loaded.mission.name}"`,
        description: `v${loaded.mission.version} · ${loaded.waypoints.length} waypoints · ${loaded.geofence.inclusion.length} fence vertices · ${loaded.rally.length} rally`,
      })
    },
    [catalog, toast],
  )

  const deleteMission = useCallback(
    async (id: string, name: string) => {
      const ok = await catalog.deleteMission(id)
      if (ok) {
        toast({ title: `Deleted "${name}"`, description: 'soft-delete (catalog retains tombstone)' })
        await catalog.refresh()
        if (file.mission.id === id) {
          setFile(emptyPlanMission('untitled'))
          setMode('idle')
          setDirty(false)
          setValidated(null)
          setUploadProgress(null)
        }
      } else {
        toast({ title: 'Delete failed', description: `mission ${id} could not be deleted`, variant: 'destructive' })
      }
    },
    [catalog, file.mission.id, toast],
  )

  // --------------------------------------------------------------- validate
  const doValidate = useCallback(async () => {
    if (!file.mission.id) {
      toast({
        title: 'Save first',
        description: 'click Save to persist the mission, then Validate (the catalog validates by id)',
        variant: 'destructive',
      })
      return
    }
    const r = await catalog.runBusy(() => catalog.validateMission(file.mission.id))
    if (!r.result) {
      toast({ title: 'Validation request failed', description: r.error ?? 'unknown error', variant: 'destructive' })
      return
    }
    setValidated(r.result)
    if (r.result.valid) {
      toast({
        title: 'Validation passed',
        description: `${file.waypoints.length} waypoints · ${file.geofence.inclusion.length} fence vertices · ${file.rally.length} rally`,
      })
    } else {
      const first = r.result.errors[0]
      toast({
        title: 'Validation failed',
        description: first ? `${first.rule}: ${first.message}` : `${r.result.errors.length} errors`,
        variant: 'destructive',
      })
    }
  }, [catalog, file.mission.id, file.waypoints.length, file.geofence.inclusion.length, file.rally.length, toast])

  // --------------------------------------------------------------- save
  const doSave = useCallback(
    async (name: string) => {
      const toSave: PlanMissionFile = {
        ...file,
        mission: { ...file.mission, name },
      }
      const r = await catalog.runBusy(() => catalog.saveMission(toSave))
      if (r.ok && r.file) {
        setFile(r.file)
        setDirty(false)
        setValidated(null)
        setUploadProgress(null)
        toast({
          title: r.status === 201 ? `Mission saved as "${name}"` : `Mission "${name}" updated`,
          description: `v${r.version} · id ${r.id?.slice(0, 10)}…`,
        })
        await catalog.refresh()
      } else {
        toast({
          title: 'Save failed',
          description: r.error ?? `HTTP ${r.status}`,
          variant: 'destructive',
        })
      }
    },
    [catalog, file, toast],
  )

  // --------------------------------------------------------------- upload
  const doUpload = useCallback(
    async (vehicleId: number) => {
      if (!file.mission.id) {
        toast({ title: 'Save first', description: 'click Save before uploading', variant: 'destructive' })
        return
      }
      // optimistic: show validated counts immediately (M1 stub returns synchronously)
      const totalMission = file.waypoints.length
      const totalFence = file.geofence.inclusion.length
      const totalRally = file.rally.length
      setUploadProgress({
        missionTotal: totalMission,
        fenceTotal: totalFence,
        rallyTotal: totalRally,
        status: 'validated',
        message: 'in progress…',
        code: null,
        httpStatus: 0,
      })
      const r = await catalog.runBusy(() => catalog.uploadToVehicle(vehicleId, file.mission.id))
      if (r.ok) {
        setUploadProgress({
          missionTotal: totalMission,
          fenceTotal: totalFence,
          rallyTotal: totalRally,
          status: 'validated',
          message: r.message,
          code: null,
          httpStatus: r.status,
        })
        toast({
          title: `Upload accepted (M1 stub)`,
          description: `mission "${file.mission.name}" → vehicle ${vehicleId} · ${totalMission} waypoints · ${totalFence} fence · ${totalRally} rally`,
        })
      } else {
        setUploadProgress({
          missionTotal: totalMission,
          fenceTotal: totalFence,
          rallyTotal: totalRally,
          status: 'failed',
          message: r.message,
          code: r.code,
          httpStatus: r.status,
        })
        toast({
          title: `Upload rejected`,
          description: r.code ? `${r.code}: ${r.message}` : r.message,
          variant: 'destructive',
        })
      }
    },
    [catalog, file.mission.id, file.mission.name, file.waypoints.length, file.geofence.inclusion.length, file.rally.length, toast],
  )

  // --------------------------------------------------------------- derived
  const erroredSeqs = useMemo(() => {
    const s = new Set<number>()
    if (validated && !validated.valid) {
      for (const e of validated.errors) {
        if (typeof e.seq === 'number' && e.seq > 0) s.add(e.seq)
      }
    }
    return s
  }, [validated])

  const planMode: PlanMapMode = mode === 'fence' ? 'fence' : mode === 'waypoint' ? 'waypoint' : 'idle'

  const savedMissions: SavedMissionRow[] = useMemo(
    () =>
      catalog.missions.map((m) => ({
        id: m.id,
        name: m.name,
        version: m.version,
        updated_at: m.updated_at,
        waypoint_count: m.waypoint_count,
        fence_count: m.fence_count,
        rally_count: m.rally_count,
      })),
    [catalog.missions],
  )

  const canSave = file.waypoints.length > 0
  const canValidate = !!file.mission.id && file.waypoints.length > 0
  const canUpload = !!file.mission.id && validated?.valid === true

  return (
    <div className="flex flex-col gap-4">
      {/* ------------------------------------------------------------- header */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <CardTitle className="flex items-center gap-2 text-base">
              <MapPin className="size-4 text-sky-600 dark:text-sky-400" aria-hidden="true" />
              Plan View
              <span className="text-xs font-normal text-muted-foreground">— QGC Plan analog · mission editor</span>
            </CardTitle>
            <Badge variant={file.mission.id ? 'default' : 'secondary'} className="gap-1 font-mono text-[11px]">
              {file.mission.id ? `${file.mission.name} · v${file.mission.version}` : 'unsaved'}
              {dirty ? ' · unsaved' : ''}
            </Badge>
            <ConnBadge conn={catalog.conn} port={8300} retryAt={catalog.retryAt} lastError={catalog.lastError} />
            <span className="ml-auto hidden font-mono text-[11px] text-muted-foreground md:inline">
              catalog :8300 · {PX4_TEST_FIELD.lat.toFixed(5)}, {PX4_TEST_FIELD.lon.toFixed(5)} · zoom 16
            </span>
          </div>
          <CardDescription>
            Click the map to add waypoints, draw a geofence polygon, validate against ADR-0026 rules, save to the
            catalog, then upload to a vehicle. The map anchors on the PX4 test field — the same origin the SITL
            vehicles fly from.
          </CardDescription>
        </CardHeader>
      </Card>

      <div className="grid gap-4 xl:grid-cols-4">
        {/* ---------------------------------------------------- mission list */}
        <Card className="xl:col-span-1">
          <CardHeader className="pb-2">
            <div className="flex items-center gap-2">
              <CardTitle className="text-sm">Mission library</CardTitle>
              <Button
                size="icon"
                variant="ghost"
                className="ml-auto size-6"
                onClick={() => void catalog.refresh()}
                aria-label="refresh mission list"
                disabled={catalog.busy}
              >
                {catalog.busy ? <Loader2 className="size-3 animate-spin" aria-hidden="true" /> : <RefreshCw className="size-3" aria-hidden="true" />}
              </Button>
            </div>
            <CardDescription className="text-[11px]">
              {catalog.conn === 'live'
                ? `GET /api/missions · ${savedMissions.length} saved`
                : catalog.conn === 'simulated'
                  ? 'catalog offline — live retry every 12 s'
                  : 'probing gateway route :8300 …'}
            </CardDescription>
          </CardHeader>
          <CardContent className="p-0">
            <ScrollArea className="h-[420px]">
              <ul className="divide-y divide-border">
                {savedMissions.length === 0 && (
                  <li className="px-4 py-6 text-center text-xs text-muted-foreground">
                    No saved missions.
                    <br />
                    Click <span className="font-medium">New Mission</span> to start editing.
                  </li>
                )}
                {savedMissions.map((m) => {
                  const active = file.mission.id === m.id
                  return (
                    <li
                      key={m.id}
                      className={`group flex flex-col gap-1 px-3 py-2.5 transition-colors hover:bg-muted/40 ${active ? 'bg-sky-500/10' : ''}`}
                    >
                      <button
                        type="button"
                        onClick={() => void loadMission(m.id)}
                        className="flex items-start gap-2 text-left"
                        aria-label={`load mission ${m.name} version ${m.version}`}
                      >
                        <span
                          className={`mt-0.5 size-2 shrink-0 rounded-full ${active ? 'bg-sky-500' : 'bg-muted-foreground/40'}`}
                          aria-hidden="true"
                        />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-xs font-medium">{m.name}</span>
                          <span className="block font-mono text-[10px] text-muted-foreground">
                            v{m.version} · {m.waypoint_count} wp · {m.fence_count} fv · {m.rally_count} rp
                          </span>
                          <span className="block font-mono text-[10px] text-muted-foreground/70">
                            {m.updated_at.replace('T', ' ').replace(/:\d{2}Z$/, 'Z')}
                          </span>
                        </span>
                      </button>
                      <div className="flex items-center justify-end gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                        <Button
                          size="icon"
                          variant="ghost"
                          className="size-6"
                          aria-label={`delete mission ${m.name}`}
                          onClick={() => {
                            if (window.confirm(`Delete mission "${m.name}" (v${m.version})?\nThe catalog retains a tombstone.`)) {
                              void deleteMission(m.id, m.name)
                            }
                          }}
                        >
                          <Trash2 className="size-3" aria-hidden="true" />
                        </Button>
                      </div>
                    </li>
                  )
                })}
              </ul>
            </ScrollArea>
          </CardContent>
        </Card>

        {/* ------------------------------------------------------------- map */}
        <Card className="xl:col-span-2">
          <CardHeader className="pb-2">
            <div className="flex flex-wrap items-center gap-1.5">
              <CardTitle className="mr-2 text-sm">Map</CardTitle>
              <Button
                size="sm"
                variant={mode === 'waypoint' ? 'secondary' : 'outline'}
                className="gap-1.5"
                onClick={() => setMode((m) => (m === 'waypoint' ? 'idle' : 'waypoint'))}
                aria-pressed={mode === 'waypoint'}
                title="click the map to add waypoints"
              >
                <MapPin className="size-3.5" aria-hidden="true" />
                Waypoint{mode === 'waypoint' ? ' ✓' : ''}
              </Button>
              <Button
                size="sm"
                variant={mode === 'fence' ? 'secondary' : 'outline'}
                className="gap-1.5"
                onClick={() => setMode((m) => (m === 'fence' ? 'waypoint' : 'fence'))}
                aria-pressed={mode === 'fence'}
                title="click to add fence vertices, double-click to close"
              >
                <Hexagon className="size-3.5" aria-hidden="true" />
                Draw Geofence{mode === 'fence' ? ' ✓' : ''}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                className="gap-1.5"
                onClick={clearWaypoints}
                disabled={file.waypoints.length === 0}
                title="clear all waypoints"
              >
                <Eraser className="size-3.5" aria-hidden="true" /> Clear wps
              </Button>
              <Button
                size="sm"
                variant="ghost"
                className="gap-1.5"
                onClick={clearFence}
                disabled={file.geofence.inclusion.length === 0}
                title="clear the geofence polygon"
              >
                <Eraser className="size-3.5" aria-hidden="true" /> Clear fence
              </Button>
            </div>
            {mode === 'fence' && (
              <CardDescription className="text-[11px] text-emerald-600 dark:text-emerald-400">
                Fence drawing: click to add vertices ({file.geofence.inclusion.length} so far), double-click to close.
                Need ≥ 3 to enclose.
              </CardDescription>
            )}
            {mode === 'waypoint' && (
              <CardDescription className="text-[11px] text-sky-600 dark:text-sky-400">
                Waypoint mode: click to add a numbered waypoint, drag to move, right-click to remove.
              </CardDescription>
            )}
          </CardHeader>
          <CardContent className="p-2">
            <div className="h-[460px] w-full overflow-hidden rounded-md border border-border">
              <PlanMap
                centerLat={PX4_TEST_FIELD.lat}
                centerLon={PX4_TEST_FIELD.lon}
                zoom={16}
                waypoints={file.waypoints}
                geofence={file.geofence}
                erroredSeqs={erroredSeqs}
                mode={planMode}
                fenceDrawing={mode === 'fence'}
                onSelectWaypoint={setSelectedWp}
                onAddWaypoint={addWaypoint}
                onMoveWaypoint={moveWaypoint}
                onRemoveWaypoint={removeWaypoint}
                onAddFenceVertex={addFenceVertex}
                onCloseFence={closeFence}
              />
            </div>
          </CardContent>
        </Card>

        {/* ----------------------------------------------------------- editor */}
        <Card className="xl:col-span-1">
          <CardHeader className="pb-2">
            <div className="flex flex-wrap items-center gap-1.5">
              <CardTitle className="mr-1 text-sm">Editor</CardTitle>
              <Button
                size="sm"
                variant="outline"
                className="ml-auto gap-1.5"
                onClick={newMission}
                title="discard the current edits and start a new mission"
              >
                <FilePlus2 className="size-3.5" aria-hidden="true" /> New Mission
              </Button>
            </div>
            <CardDescription className="text-[11px]">
              {file.mission.id
                ? `editing "${file.mission.name}" v${file.mission.version}${dirty ? ' · unsaved changes' : ''}`
                : 'no mission loaded — click New Mission to start'}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            {/* waypoint table */}
            <div className="rounded-md border border-border">
              <div className="flex items-center justify-between gap-2 border-b border-border bg-muted/40 px-2.5 py-1.5">
                <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                  Waypoints
                </span>
                <span className="font-mono text-[10px] text-muted-foreground">{file.waypoints.length} items</span>
              </div>
              <div className="max-h-56 overflow-auto">
                <Table>
                  <TableHeader>
                    <TableRow className="hover:bg-transparent">
                      <TableHead className="h-7 w-6 px-1 text-[10px]">#</TableHead>
                      <TableHead className="h-7 w-20 px-1 text-[10px]">lat, lon</TableHead>
                      <TableHead className="h-7 w-14 px-1 text-[10px]">AGL m</TableHead>
                      <TableHead className="h-7 w-12 px-1 text-[10px]">hold s</TableHead>
                      <TableHead className="h-7 w-14 px-1 text-[10px]">accept m</TableHead>
                      <TableHead className="h-7 w-6 px-1" />
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {file.waypoints.length === 0 && (
                      <TableRow>
                        <TableCell colSpan={6} className="py-3 text-center text-[11px] text-muted-foreground">
                          {mode === 'waypoint' ? 'click the map to add waypoint 1' : 'click Waypoint mode, then the map'}
                        </TableCell>
                      </TableRow>
                    )}
                    {file.waypoints.map((w, i) => {
                      const err = erroredSeqs.has(w.seq)
                      return (
                        <TableRow
                          key={`${w.seq}-${i}`}
                          className={`hover:bg-muted/40 ${err ? 'bg-rose-500/10' : ''} ${selectedWp === w.seq ? 'bg-sky-500/10' : ''}`}
                        >
                          <TableCell className={`py-1 px-1 font-mono text-[11px] ${err ? 'text-rose-600 dark:text-rose-400' : ''}`}>
                            {i + 1}
                          </TableCell>
                          <TableCell className="py-1 px-1 font-mono text-[9px] leading-tight">
                            {w.x.toFixed(5)},
                            <br />
                            {w.y.toFixed(5)}
                          </TableCell>
                          <TableCell className="py-1 px-1">
                            <Input
                              type="number"
                              min={0}
                              max={file.geofence.ceiling_m}
                              step={0.5}
                              value={w.z}
                              onChange={(e) => patchWaypoint(w.seq, { z: Number(e.target.value) || 0 })}
                              className="h-6 w-12 px-1 text-[11px]"
                              aria-label={`waypoint ${i + 1} altitude AGL metres`}
                            />
                          </TableCell>
                          <TableCell className="py-1 px-1">
                            <Input
                              type="number"
                              min={0}
                              step={0.5}
                              value={w.param1}
                              onChange={(e) => patchWaypoint(w.seq, { param1: Number(e.target.value) || 0 })}
                              className="h-6 w-10 px-1 text-[11px]"
                              aria-label={`waypoint ${i + 1} hold seconds`}
                            />
                          </TableCell>
                          <TableCell className="py-1 px-1">
                            <Input
                              type="number"
                              min={0}
                              step={0.5}
                              value={w.param2}
                              onChange={(e) => patchWaypoint(w.seq, { param2: Number(e.target.value) || 0 })}
                              className="h-6 w-12 px-1 text-[11px]"
                              aria-label={`waypoint ${i + 1} accept radius metres`}
                            />
                          </TableCell>
                          <TableCell className="py-1 px-1">
                            <Button
                              size="icon"
                              variant="ghost"
                              className="size-5"
                              onClick={() => removeWaypoint(w.seq)}
                              aria-label={`remove waypoint ${i + 1}`}
                            >
                              <Trash2 className="size-3" aria-hidden="true" />
                            </Button>
                          </TableCell>
                        </TableRow>
                      )
                    })}
                  </TableBody>
                </Table>
              </div>
            </div>

            {/* geofence editor */}
            <div className="rounded-md border border-border">
              <div className="flex items-center justify-between gap-2 border-b border-border bg-muted/40 px-2.5 py-1.5">
                <span className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                  Geofence
                </span>
                <span className="font-mono text-[10px] text-muted-foreground">
                  {file.geofence.inclusion.length >= 3 ? 'closed' : 'open'} · {file.geofence.inclusion.length} verts
                </span>
              </div>
              <div className="grid grid-cols-2 gap-2 p-2">
                <Label className="flex flex-col gap-1 text-[10px]">
                  ceiling (m AGL)
                  <Input
                    type="number"
                    min={0}
                    max={500}
                    step={5}
                    value={file.geofence.ceiling_m}
                    onChange={(e) => {
                      const v = Number(e.target.value) || 0
                      setFile((f) => ({ ...f, geofence: { ...f.geofence, ceiling_m: v } }))
                      setDirty(true)
                    }}
                    className="h-7 text-[11px]"
                  />
                </Label>
                <Label className="flex flex-col gap-1 text-[10px]">
                  floor (m AGL)
                  <Input
                    type="number"
                    min={0}
                    max={500}
                    step={5}
                    value={file.geofence.floor_m}
                    onChange={(e) => {
                      const v = Number(e.target.value) || 0
                      setFile((f) => ({ ...f, geofence: { ...f.geofence, floor_m: v } }))
                      setDirty(true)
                    }}
                    className="h-7 text-[11px]"
                  />
                </Label>
              </div>
            </div>

            {/* action bar */}
            <div className="grid grid-cols-2 gap-1.5">
              <Button
                size="sm"
                variant="outline"
                className="gap-1.5"
                onClick={() => void doValidate()}
                disabled={!canValidate || catalog.busy}
                title={file.mission.id ? 'POST /api/missions/{id}/validate' : 'save the mission first'}
              >
                {catalog.busy ? <Loader2 className="size-3.5 animate-spin" aria-hidden="true" /> : <CheckCircle2 className="size-3.5" aria-hidden="true" />}
                Validate
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="gap-1.5"
                onClick={openSave}
                disabled={!canSave || catalog.busy}
                title={canSave ? 'POST or PUT /api/missions' : 'add at least one waypoint first'}
              >
                <Save className="size-3.5" aria-hidden="true" />
                Save
              </Button>
              <Button
                size="sm"
                className="col-span-2 gap-1.5"
                onClick={() => setUploadOpen(true)}
                disabled={!canUpload || catalog.busy}
                title={canUpload ? 'POST /api/vehicles/{i}/mission/upload' : 'validate first (must pass)'}
              >
                <Upload className="size-3.5" aria-hidden="true" />
                Upload to Vehicle
              </Button>
            </div>

            {/* validation status */}
            {validated && (
              <div
                className={`rounded-md border p-2 text-[11px] ${
                  validated.valid
                    ? 'border-emerald-600/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400'
                    : 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
                }`}
                role="status"
                aria-live="polite"
              >
                <div className="flex items-center gap-1.5">
                  {validated.valid ? (
                    <CheckCircle2 className="size-3.5" aria-hidden="true" />
                  ) : (
                    <TriangleAlert className="size-3.5" aria-hidden="true" />
                  )}
                  <span className="font-semibold">
                    {validated.valid ? 'Validation passed' : `Validation failed · ${validated.errors.length} error(s)`}
                  </span>
                </div>
                {!validated.valid && (
                  <ul className="mt-1 space-y-0.5 pl-5 font-mono text-[10px]">
                    {validated.errors.slice(0, 8).map((e: PlanValidationError, i: number) => (
                      <li key={i}>
                        {e.rule}
                        {typeof e.seq === 'number' && e.seq > 0 ? ` · wp ${e.seq}` : ''}: {e.message}
                        {typeof e.distance === 'number' ? ` (${e.distance.toFixed(1)} m)` : ''}
                      </li>
                    ))}
                    {validated.errors.length > 8 && (
                      <li className="text-muted-foreground">… +{validated.errors.length - 8} more</li>
                    )}
                  </ul>
                )}
              </div>
            )}

            {/* upload progress (M1 stub status) */}
            {uploadProgress && (
              <div
                className={`rounded-md border p-2 text-[11px] ${
                  uploadProgress.status === 'validated'
                    ? 'border-sky-600/40 bg-sky-500/10 text-sky-700 dark:text-sky-400'
                    : 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
                }`}
                role="status"
                aria-live="polite"
              >
                <div className="mb-1.5 flex items-center gap-1.5">
                  {uploadProgress.status === 'validated' ? (
                    <Send className="size-3.5" aria-hidden="true" />
                  ) : (
                    <CircleSlash className="size-3.5" aria-hidden="true" />
                  )}
                  <span className="font-semibold">
                    {uploadProgress.status === 'validated' ? 'Upload accepted (M1 stub)' : `Upload failed · ${uploadProgress.code ?? 'error'}`}
                  </span>
                </div>
                <UploadBar label="Mission (flight plan)" total={uploadProgress.missionTotal} ok={uploadProgress.status === 'validated'} />
                <UploadBar label="Fence" total={uploadProgress.fenceTotal} ok={uploadProgress.status === 'validated'} />
                <UploadBar label="Rally" total={uploadProgress.rallyTotal} ok={uploadProgress.status === 'validated'} />
                <p className="mt-1.5 font-mono text-[10px] text-muted-foreground">
                  HTTP {uploadProgress.httpStatus} · {uploadProgress.message}
                </p>
                {uploadProgress.status === 'validated' && (
                  <p className="mt-0.5 text-[10px] text-muted-foreground/70">
                    M1 stub — MAVLink upload is M2 scope (see GCS_SPEC §5.1 AC-5.1.6)
                  </p>
                )}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      {/* --------------------------------------------------- save modal */}
      <AlertDialog open={saveOpen} onOpenChange={setSaveOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="flex items-center gap-2">
              <Save className="size-4 text-muted-foreground" aria-hidden="true" />
              Save Mission
            </AlertDialogTitle>
            <AlertDialogDescription>
              {file.mission.id
                ? `Update "${file.mission.name}" (v${file.mission.version}) in the catalog — the catalog bumps the version.`
                : 'Create a new mission in the catalog — the catalog assigns the ULID.'}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="flex flex-col gap-1.5 py-1">
            <Label htmlFor="mission-name" className="text-xs">
              Mission name
            </Label>
            <Input
              id="mission-name"
              value={saveName}
              onChange={(e) => setSaveName(e.target.value)}
              placeholder="demo-square"
              className="font-mono text-sm"
              autoFocus
              onKeyDown={(e) => {
                if (e.key === 'Enter' && saveName.trim().length > 0) {
                  void doSave(saveName.trim())
                  setSaveOpen(false)
                }
              }}
            />
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={saveName.trim().length === 0 || catalog.busy}
              onClick={() => {
                void doSave(saveName.trim())
                setSaveOpen(false)
              }}
            >
              {catalog.busy ? 'Saving…' : file.mission.id ? 'Update' : 'Create'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* ------------------------------------------------- upload modal */}
      <AlertDialog open={uploadOpen} onOpenChange={setUploadOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="flex items-center gap-2">
              <PlaneTakeoff className="size-4 text-muted-foreground" aria-hidden="true" />
              Select target vehicle
            </AlertDialogTitle>
            <AlertDialogDescription>
              {fleetConn === 'live'
                ? `Upload "${file.mission.name}" (v${file.mission.version}) to a connected SITL vehicle. The catalog runs version-check (ADR-0029) before the MAVLink upload (M2).`
                : `Fleet plane :${FLEET_PORT} is offline — you can still attempt the upload (it will fail with PX4_VERSION_UNAVAILABLE) to verify the catalog's error path.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="flex flex-col gap-1.5 py-1">
            <Label htmlFor="vehicle-select" className="text-xs">
              Vehicle
            </Label>
            <Select
              value={String(uploadVehicle)}
              onValueChange={(v) => setUploadVehicle(Number(v))}
            >
              <SelectTrigger id="vehicle-select" className="font-mono text-sm">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Array.from({ length: Math.max(vehicleCount, 4) }).map((_, i) => (
                  <SelectItem key={i} value={String(i)}>
                    Vehicle {i} (sysid {i + 1})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={catalog.busy}
              onClick={() => {
                void doUpload(uploadVehicle)
                setUploadOpen(false)
              }}
            >
              {catalog.busy ? 'Uploading…' : 'Upload'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

/** One of the three upload sub-bars (Mission / Fence / Rally). */
function UploadBar({ label, total, ok }: { label: string; total: number; ok: boolean }) {
  return (
    <div className="mb-1 last:mb-0">
      <div className="flex items-center justify-between text-[10px]">
        <span>{label}</span>
        <span className="font-mono text-muted-foreground">
          {total} item{total === 1 ? '' : 's'} {ok ? '✓ validated' : '— failed'}
        </span>
      </div>
      <Progress
        value={ok ? 100 : 0}
        className={`mt-0.5 h-1.5 ${
          ok ? '[&>div]:bg-sky-500' : '[&>div]:bg-rose-500'
        }`}
        aria-label={`${label} upload progress`}
      />
    </div>
  )
}
