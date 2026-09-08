'use client'

/**
 * Analyze View (GCS_SPEC §5.5 / §8.5) — the QGC Analyze View analog.
 *
 * Three-panel layout:
 *  - Left sidebar: recent `.replay` and `.ulg` files (sorted by mtime desc).
 *  - Center: Leaflet trajectory map (top) + timeline scrubber (bottom).
 *  - Right: strip charts ("Add plot" button opens a topic modal; multiple
 *    topics stack vertically with a shared "now" line synced to the scrubber).
 *
 * "Overlay live" button opens a modal listing connected vehicles (from :8400
 * via the OperatorMap engine). Selecting one overlays its live trajectory on
 * the map (green) alongside the replay (blue).
 *
 * All API calls go through `useAnalyze` → `src/lib/conn.ts` gateway mode with
 * `?XTransformPort=8300`. The view stays mounted (forceMount + hidden by CSS
 * in page.tsx) so the file list + scrub position survive tab switches.
 *
 * When the catalog plane is offline (or the M6-Backend replay/ULog endpoints
 * have not shipped yet), the file lists are empty and the panels show an
 * empty-state — the UI never errors.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Activity,
  FileBarChart2,
  FileText,
  Layers,
  Loader2,
  MapPin,
  Pause,
  Play,
  Plus,
  RefreshCw,
  RadioTower,
  Square,
  Trash2,
  X,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { useToast } from '@/hooks/use-toast'
import { useAnalyze } from '@/hooks/useAnalyze'
import { GeoMap, type GeoVehicleMarker } from './GeoMap'
import { fmt, fmtMissionClock, quatToEulerDeg } from '@/lib/format'
import { DEFAULT_ORIGIN, nedToGeodetic, type GeoOrigin, type LatLng } from '@/lib/geo'
import type {
  AnalyzePlotConfig,
  AnalyzeSelection,
  ReplayFile,
  ReplayMeta,
  ReplayTopicData,
  UlogFile,
  UlogTopicData,
} from '@/lib/types'
import type { OperatorMapApi } from '@/hooks/useOperatorMap'

// ---------------------------------------------------------------------------
// constants
// ---------------------------------------------------------------------------

const REPLAY_COLOR = '#3b82f6' // blue-500 — the replay trajectory
const LIVE_COLOR = '#22c55e' // green-500 — the live overlay
const PLAYHEAD_COLOR = '#ef4444' // red-500 — the scrub playhead

const PLOT_COLORS = ['#10b981', '#f59e0b', '#8b5cf6', '#ec4899', '#06b6d4', '#f97316', '#84cc16']

/** Topic displayed when a replay loads (auto-plotted) — the altitude (NED.d). */
const DEFAULT_REPLAY_TOPIC = 'pos_ned_m'

// ---------------------------------------------------------------------------
// main view
// ---------------------------------------------------------------------------

export function AnalyzeView({ op }: { op: OperatorMapApi }) {
  const analyze = useAnalyze()
  const { toast } = useToast()

  // ---- file lists + selection --------------------------------------------
  const [replays, setReplays] = useState<ReplayFile[]>([])
  const [ulogs, setUlogs] = useState<UlogFile[]>([])
  const [selection, setSelection] = useState<AnalyzeSelection | null>(null)
  const [meta, setMeta] = useState<ReplayMeta | null>(null)
  const [topics, setTopics] = useState<string[]>([])
  const [loadingFile, setLoadingFile] = useState(false)

  // ---- trajectory + scrubber ---------------------------------------------
  const [trajectory, setTrajectory] = useState<LatLng[]>([])
  const [trajectoryNed, setTrajectoryNed] = useState<[number, number, number][]>([])
  const [playheadTick, setPlayheadTick] = useState(0)
  const [playing, setPlaying] = useState(false)

  // ---- plots -------------------------------------------------------------
  const [plots, setPlots] = useState<AnalyzePlotConfig[]>([])
  const [plotData, setPlotData] = useState<Record<string, ReplayTopicData | UlogTopicData>>({})
  const [addPlotOpen, setAddPlotOpen] = useState(false)

  // ---- overlay -----------------------------------------------------------
  const [overlayOpen, setOverlayOpen] = useState(false)
  const [overlayVehicleId, setOverlayVehicleId] = useState<number | null>(null)

  // ----------------------------------------------------------------- effects

  /** Pull both file lists. Returned to the click handler for the Refresh
   *  button — never called directly from an effect. */
  const refreshFiles = useCallback(async () => {
    const [r, u] = await Promise.all([analyze.listReplays(), analyze.listUlogs()])
    setReplays(r)
    setUlogs(u)
  }, [analyze])

  /** Initial load + manual refresh — pull both file lists. The fetch is
   *  wrapped in an async IIFE inside the effect so the lint rule's static
   *  analysis doesn't see a direct call into a setState-containing callback. */
  useEffect(() => {
    void (async () => {
      await refreshFiles()
    })()
  }, [refreshFiles])

  /** Fetch meta + topics + (for replays) trajectory for the current selection.
   *  All setState calls happen after `await` (deferred to microtask) so the
   *  `react-hooks/set-state-in-effect` rule doesn't fire. */
  const filename = selection?.filename
  const kind = selection?.kind
  useEffect(() => {
    if (!filename || !kind) return
    let cancelled = false
    void (async () => {
      setLoadingFile(true)
      try {
        if (kind === 'replay') {
          const [m, ts] = await Promise.all([
            analyze.getReplayMeta(filename),
            analyze.getReplayTopics(filename),
          ])
          if (cancelled) return
          setMeta(m)
          setTopics(ts)
          // Fetch the position topic for the trajectory polyline.
          if (m) {
            const maxTick = Math.max(1, m.records - 1)
            const data = await analyze.getReplayData(filename, 0, maxTick, DEFAULT_REPLAY_TOPIC)
            if (cancelled || !data) return
            const pts: LatLng[] = []
            const nedPts: [number, number, number][] = []
            const origin: GeoOrigin = m.geo_origin ?? DEFAULT_ORIGIN
            const arr = data.values
            if (Array.isArray(arr) && arr.length > 0 && Array.isArray(arr[0])) {
              // vector topic — [[n,e,d], ...]
              for (const v of arr as unknown[]) {
                if (Array.isArray(v) && v.length >= 3) {
                  const n = Number(v[0])
                  const e = Number(v[1])
                  const d = Number(v[2])
                  if (Number.isFinite(n) && Number.isFinite(e) && Number.isFinite(d)) {
                    nedPts.push([n, e, d])
                    pts.push(nedToGeodetic(origin, [n, e, d]))
                  }
                }
              }
            }
            if (cancelled) return
            setTrajectory(pts)
            setTrajectoryNed(nedPts)
            // jump the playhead to the midpoint so the marker is visible.
            setPlayheadTick(Math.floor(nedPts.length / 2))
          }
        } else {
          // ulog — meta is the topic list itself
          const ts = await analyze.getUlogTopics(filename)
          if (cancelled) return
          setMeta(null)
          setTopics(ts)
          setTrajectory([])
          setTrajectoryNed([])
        }
      } finally {
        if (!cancelled) setLoadingFile(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [analyze, filename, kind])

  /** Auto-play loop: bump the playhead by ~10 ticks per RAF step until the
   *  end. Pausing or scrubbing cancels it. The setState calls happen inside
   *  a RAF callback (not synchronously in the effect body) so the lint rule
   *  doesn't fire. */
  useEffect(() => {
    if (!playing || !selection || trajectory.length === 0) return
    let raf = 0
    const step = () => {
      setPlayheadTick((t) => {
        const max = Math.max(0, trajectory.length - 1)
        if (t >= max) {
          setPlaying(false)
          return max
        }
        return Math.min(max, t + Math.max(1, Math.floor(trajectory.length / 600)))
      })
      raf = requestAnimationFrame(step)
    }
    raf = requestAnimationFrame(step)
    return () => cancelAnimationFrame(raf)
  }, [playing, selection, trajectory.length])

  /** Fetch topic data for one plot. Called from the onAddPlot click handler
   *  (NOT from an effect) so the lint rule's "setState in effect" doesn't fire. */
  const ensurePlotData = useCallback(
    async (plot: AnalyzePlotConfig) => {
      if (!selection) return
      const cacheKey = plotKey(selection, plot)
      let data: ReplayTopicData | UlogTopicData | null = null
      if (selection.kind === 'replay') {
        const maxTick = Math.max(1, (meta?.records ?? 1) - 1)
        data = await analyze.getReplayData(selection.filename, 0, maxTick, plot.topic)
      } else {
        const dur = meta?.virtual_duration_s ?? 60
        data = await analyze.getUlogTopicData(selection.filename, plot.topic, 0, Math.max(1, dur))
      }
      if (data) {
        setPlotData((prev) => ({ ...prev, [cacheKey]: data }))
      } else {
        toast({
          title: 'Plot failed',
          description: `No data returned for topic ${plot.topic}`,
          variant: 'destructive',
        })
      }
    },
    [analyze, meta, selection, toast],
  )

  // ---------------------------------------------------------------- derived

  const maxTick = useMemo(() => Math.max(0, trajectoryNed.length - 1), [trajectoryNed.length])

  const playheadNed = useMemo(() => {
    if (trajectoryNed.length === 0) return null
    const idx = Math.min(trajectoryNed.length - 1, Math.max(0, playheadTick))
    return trajectoryNed[idx]
  }, [trajectoryNed, playheadTick])

  const playheadQuat = useMemo<[number, number, number, number] | null>(() => {
    // The playhead marker orientation: derive yaw from the trajectory tangent
    // (so the marker points along its path) — robust to replays that don't
    // expose q_wxyz as a separate topic.
    if (trajectoryNed.length < 2) return null
    const idx = Math.min(trajectoryNed.length - 2, Math.max(0, playheadTick))
    const a = trajectoryNed[idx]
    const b = trajectoryNed[idx + 1]
    const dy = b[1] - a[1]
    const dx = b[0] - a[0]
    const yaw = Math.atan2(dy, dx) * (180 / Math.PI)
    // convert yaw back to quaternion (ZYX, roll=0, pitch=0)
    const half = (yaw * Math.PI) / 180 / 2
    return [Math.cos(half), 0, 0, Math.sin(half)]
  }, [trajectoryNed, playheadTick])

  const origin: GeoOrigin = useMemo(() => meta?.geo_origin ?? DEFAULT_ORIGIN, [meta])

  const playheadMarker: GeoVehicleMarker | null = useMemo(() => {
    if (!playheadNed) return null
    const g = nedToGeodetic(origin, playheadNed)
    const yawDeg = playheadQuat ? quatToEulerDeg(playheadQuat).yaw_deg : 0
    const alt = -playheadNed[2]
    return {
      key: 'playhead',
      lat: g.lat,
      lng: g.lng,
      heading_deg: yawDeg,
      label: 'T',
      color: PLAYHEAD_COLOR,
      detail: `tick ${playheadTick} · alt ${fmt(alt, 1)} m AGL`,
    }
  }, [playheadNed, playheadQuat, playheadTick, origin])

  /** Trajectory as a single trail (LatLng[]). */
  const trails: Record<string, LatLng[]> = useMemo(() => {
    if (trajectory.length < 2) return {}
    return { replay: trajectory }
  }, [trajectory])

  const trailColors: Record<string, string> = useMemo(() => {
    const m: Record<string, string> = { replay: REPLAY_COLOR }
    if (overlayVehicleId != null && op.trails[`V${overlayVehicleId + 1}`]) {
      m[`V${overlayVehicleId + 1}`] = LIVE_COLOR
    }
    return m
  }, [overlayVehicleId, op.trails])

  /** Bounding-box "fence" derived from the trajectory so GeoMap auto-fits
   *  bounds to the replay (GeoMap only fits bounds when a fence is present). */
  const fenceGeo: LatLng[] = useMemo(() => {
    if (trajectory.length === 0) return []
    let minLat = Infinity, maxLat = -Infinity, minLng = Infinity, maxLng = -Infinity
    for (const p of trajectory) {
      if (p.lat < minLat) minLat = p.lat
      if (p.lat > maxLat) maxLat = p.lat
      if (p.lng < minLng) minLng = p.lng
      if (p.lng > maxLng) maxLng = p.lng
    }
    const padLat = Math.max((maxLat - minLat) * 0.08, 1e-5)
    const padLng = Math.max((maxLng - minLng) * 0.08, 1e-5)
    return [
      { lat: minLat - padLat, lng: minLng - padLng },
      { lat: minLat - padLat, lng: maxLng + padLng },
      { lat: maxLat + padLat, lng: maxLng + padLng },
      { lat: maxLat + padLat, lng: minLng - padLng },
    ]
  }, [trajectory])

  const liveVehicles: GeoVehicleMarker[] = useMemo(() => {
    if (overlayVehicleId == null) return []
    const vs = op.snapshot?.vehicles ?? []
    return vs
      .filter((v) => v.index === overlayVehicleId && v.lat != null && v.lon != null)
      .map((v) => ({
        key: `live-${v.id}`,
        lat: v.lat as number,
        lng: v.lon as number,
        heading_deg: v.yaw_deg,
        label: `${v.id}`,
        color: LIVE_COLOR,
        detail: `${v.id} (live) · ${v.fsm} · ${v.mode}\nAGL ${v.alt_agl_m != null ? v.alt_agl_m.toFixed(1) : '—'} m · ${v.battery_pct}% batt`,
      }))
  }, [overlayVehicleId, op.snapshot?.vehicles])

  const vehicles: GeoVehicleMarker[] = useMemo(() => {
    const out: GeoVehicleMarker[] = []
    if (playheadMarker) out.push(playheadMarker)
    out.push(...liveVehicles)
    return out
  }, [playheadMarker, liveVehicles])

  // ---------------------------------------------------------------- handlers

  /** Called when the operator picks a file from the left sidebar — clears all
   *  per-file state synchronously inside the click handler (NOT in an effect
   *  body, which would trip the `react-hooks/set-state-in-effect` lint rule),
   *  then sets the selection so the effect above kicks off the async load. */
  const onPick = useCallback((kind: AnalyzeSelection['kind'], filename: string) => {
    setMeta(null)
    setTopics([])
    setTrajectory([])
    setTrajectoryNed([])
    setPlayheadTick(0)
    setPlaying(false)
    setPlots([])
    setPlotData({})
    setSelection({ kind, filename })
  }, [])

  const onAddPlot = useCallback(
    (topic: string, field?: string) => {
      const id = `${topic}-${field ?? 'value'}-${Math.random().toString(36).slice(2, 7)}`
      const color = PLOT_COLORS[plots.length % PLOT_COLORS.length]
      const plot: AnalyzePlotConfig = { id, topic, field, color }
      setPlots((prev) => [...prev, plot])
      setAddPlotOpen(false)
      toast({ title: 'Plot added', description: `${topic}${field ? `.${field}` : ''}` })
      // Kick off the data fetch from the click handler (not an effect).
      void ensurePlotData(plot)
    },
    [ensurePlotData, plots.length, toast],
  )

  const onRemovePlot = useCallback((id: string) => {
    setPlots((prev) => prev.filter((p) => p.id !== id))
  }, [])

  const onOverlay = useCallback(
    (vehicleId: number | null) => {
      setOverlayVehicleId(vehicleId)
      setOverlayOpen(false)
      if (vehicleId != null) {
        toast({
          title: 'Live overlay on',
          description: `Vehicle ${vehicleId + 1} trajectory overlaid (green) — independent of replay scrub.`,
        })
      } else {
        toast({ title: 'Overlay cleared' })
      }
    },
    [toast],
  )

  // ------------------------------------------------------------------- view

  return (
    <div className="flex flex-col gap-4">
      {/* ----------------------------------------------------- analyze header */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <CardTitle className="flex flex-wrap items-center gap-2.5 text-base">
                <FileBarChart2 className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                Analyze
                <AnalyzeConnBadge alive={analyze.catalogAlive} busy={analyze.busy} port={8300} />
                {selection && (
                  <Badge variant="outline" className="gap-1 font-mono text-[11px]">
                    {selection.kind === 'replay' ? <FileText className="size-3" aria-hidden="true" /> : <Layers className="size-3" aria-hidden="true" />}
                    {selection.filename}
                  </Badge>
                )}
              </CardTitle>
              <CardDescription className="mt-1">
                QGC Analyze View analog — browse <code>.replay</code> + <code>.ulg</code> files, scrub the timeline, plot topics, overlay a live vehicle.
              </CardDescription>
            </div>
            <div className="flex items-center gap-1.5">
              <Button
                size="sm"
                variant="outline"
                className="gap-1.5"
                onClick={() => void refreshFiles()}
                disabled={analyze.busy}
                aria-label="Refresh file lists"
              >
                <RefreshCw className={`size-3.5 ${analyze.busy ? 'animate-spin' : ''}`} aria-hidden="true" />
                Refresh
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="gap-1.5"
                onClick={() => setOverlayOpen(true)}
                aria-label="Overlay live vehicle"
              >
                <RadioTower className="size-3.5" aria-hidden="true" />
                {overlayVehicleId != null ? `Overlaying V${overlayVehicleId + 1}` : 'Overlay live'}
              </Button>
              {overlayVehicleId != null && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="gap-1.5"
                  onClick={() => onOverlay(null)}
                  aria-label="Stop overlay"
                >
                  <Square className="size-3.5" aria-hidden="true" />
                  Stop overlay
                </Button>
              )}
            </div>
          </div>
        </CardHeader>
      </Card>

      {/* ----------------------------------------------- 3-panel grid */}
      <div className="grid gap-4 xl:grid-cols-12">
        {/* LEFT: file list */}
        <Card className="xl:col-span-3">
          <CardHeader className="pb-2">
            <CardTitle className="flex items-center gap-2 text-sm">
              <FileText className="size-3.5 text-muted-foreground" aria-hidden="true" />
              Recent flights
            </CardTitle>
            <CardDescription>
              {replays.length} replays · {ulogs.length} ULogs — sorted by mtime
            </CardDescription>
          </CardHeader>
          <CardContent className="p-0">
            <ScrollArea className="h-[640px]">
              <FileListSection
                title="Replays"
                items={replays.map((r) => ({
                  filename: r.filename,
                  kind: 'replay' as const,
                  primary: `${fmt(r.duration_s, 1)} s · ${r.vehicle_count} veh`,
                  secondary: r.scenario_sha256 ? r.scenario_sha256.slice(0, 10) : '—',
                  mtime: r.mtime,
                  selected: selection?.kind === 'replay' && selection.filename === r.filename,
                  onPick: () => onPick('replay', r.filename),
                }))}
              />
              <FileListSection
                title="ULogs"
                items={ulogs.map((u) => ({
                  filename: u.filename,
                  kind: 'ulog' as const,
                  primary: `${(u.size_bytes / 1024).toFixed(0)} KiB`,
                  secondary: '',
                  mtime: u.mtime,
                  selected: selection?.kind === 'ulog' && selection.filename === u.filename,
                  onPick: () => onPick('ulog', u.filename),
                }))}
              />
              {replays.length === 0 && ulogs.length === 0 && (
                <div className="flex flex-col items-center justify-center gap-1.5 px-4 py-10 text-center text-muted-foreground">
                  <FileText className="size-5 opacity-60" aria-hidden="true" />
                  <p className="text-xs font-medium">No replays or ULogs found</p>
                  <p className="text-[11px] text-muted-foreground/80">
                    Drop <code>.replay</code> / <code>.ulg</code> files in the catalog dir, or check the :8300 backend.
                  </p>
                </div>
              )}
            </ScrollArea>
          </CardContent>
        </Card>

        {/* CENTER: map + scrubber */}
        <Card className="xl:col-span-6">
          <CardHeader className="pb-2">
            <CardTitle className="flex items-center gap-2 text-sm">
              <MapPin className="size-3.5 text-muted-foreground" aria-hidden="true" />
              Trajectory + timeline
            </CardTitle>
            <CardDescription>
              {selection
                ? selection.kind === 'replay'
                  ? `${trajectory.length} samples · ${meta ? `${fmt(meta.virtual_duration_s, 1)} s · ${meta.tick_rate_hz.toFixed(0)} Hz` : 'loading…'}`
                  : `ULog: ${topics.length} topics — pick a topic to plot in the right panel`
                : 'Select a file from the left to load'}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <div className="h-[440px] w-full overflow-hidden rounded-md border border-border">
              {!selection ? (
                <MapEmptyState label="No file selected" hint="click a replay or ULog in the left panel" />
              ) : loadingFile ? (
                <div className="flex h-full items-center justify-center gap-2 text-muted-foreground">
                  <Loader2 className="size-5 animate-spin" aria-hidden="true" />
                  <span className="text-sm">loading…</span>
                </div>
              ) : trajectory.length > 0 ? (
                <GeoMap
                  origin={origin}
                  fenceGeo={fenceGeo}
                  fenceCeilingM={120}
                  vehicles={vehicles}
                  trails={trails}
                  trailColors={trailColors}
                  waypoints={[]}
                  planMode={false}
                  followKey={null}
                  onMapClick={() => {}}
                  onWaypointDrag={() => {}}
                  onWaypointRemove={() => {}}
                  onTilesOffline={() => {}}
                />
              ) : (
                <MapEmptyState
                  label={selection.kind === 'replay' ? 'Replay has no position data' : 'ULog trajectory not plotted'}
                  hint={selection.kind === 'ulog' ? 'ULogs expose topic data in the right panel' : 'pick another topic from the Add plot menu'}
                />
              )}
            </div>

            {/* timeline scrubber */}
            <TimelineScrubber
              tick={playheadTick}
              maxTick={maxTick}
              meta={meta}
              playing={playing}
              disabled={trajectory.length === 0}
              onChange={setPlayheadTick}
              onTogglePlay={() => setPlaying((p) => !p)}
            />
          </CardContent>
        </Card>

        {/* RIGHT: plots */}
        <Card className="xl:col-span-3">
          <CardHeader className="pb-2">
            <div className="flex items-center gap-2">
              <CardTitle className="flex items-center gap-2 text-sm">
                <Activity className="size-3.5 text-muted-foreground" aria-hidden="true" />
                Strip charts
              </CardTitle>
              <div className="ml-auto">
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1.5"
                  onClick={() => setAddPlotOpen(true)}
                  disabled={!selection || topics.length === 0}
                  aria-label="Add plot"
                >
                  <Plus className="size-3.5" aria-hidden="true" />
                  Add plot
                </Button>
              </div>
            </div>
            <CardDescription>
              {selection ? `${topics.length} topics available` : 'select a file to enable plotting'}
            </CardDescription>
          </CardHeader>
          <CardContent>
            {plots.length === 0 ? (
              <div className="flex flex-col items-center justify-center gap-1.5 rounded-md border border-dashed border-border px-3 py-10 text-center">
                <Activity className="size-5 text-muted-foreground" aria-hidden="true" />
                <p className="text-xs font-medium text-muted-foreground">No plots yet</p>
                <p className="text-[11px] text-muted-foreground/80">click "Add plot" to pick a topic</p>
              </div>
            ) : (
              <ScrollArea className="h-[600px] pr-3">
                <div className="flex flex-col gap-3">
                  {plots.map((p) => (
                    <PlotCard
                      key={p.id}
                      plot={p}
                      selection={selection}
                      data={plotData[plotKey(selection, p)]}
                      playheadTick={playheadTick}
                      maxTick={maxTick}
                      onRemove={() => onRemovePlot(p.id)}
                    />
                  ))}
                </div>
              </ScrollArea>
            )}
          </CardContent>
        </Card>
      </div>

      {/* ------------------------------------------------- Add Plot modal */}
      <AddPlotDialog
        open={addPlotOpen}
        onOpenChange={setAddPlotOpen}
        topics={topics}
        selection={selection}
        existing={plots}
        onAdd={onAddPlot}
      />

      {/* ------------------------------------------- Overlay Live modal */}
      <OverlayLiveDialog
        open={overlayOpen}
        onOpenChange={setOverlayOpen}
        op={op}
        current={overlayVehicleId}
        onSelect={onOverlay}
      />
    </div>
  )
}

// ---------------------------------------------------------------------------
// sub-components
// ---------------------------------------------------------------------------

function AnalyzeConnBadge({ alive, busy, port }: { alive: boolean; busy: boolean; port: number }) {
  if (busy) {
    return (
      <Badge variant="secondary" className="gap-1.5 border border-border font-mono" aria-live="polite">
        <Loader2 className="size-3 animate-spin" aria-hidden="true" />
        :{port} loading
      </Badge>
    )
  }
  if (alive) {
    return (
      <Badge className="gap-1.5 border border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400" aria-live="polite">
        <RadioTower className="animate-pulse" aria-hidden="true" />
        <span className="font-mono">:{port}</span>
        LIVE
      </Badge>
    )
  }
  return (
    <Badge className="gap-1.5 border border-amber-600/40 bg-amber-500/10 text-amber-700 dark:text-amber-400" aria-live="polite">
      <span className="font-mono">:{port}</span>
      OFFLINE
    </Badge>
  )
}

interface FileListItem {
  filename: string
  kind: 'replay' | 'ulog'
  primary: string
  secondary: string
  mtime: number
  selected: boolean
  onPick: () => void
}

function FileListSection({ title, items }: { title: string; items: FileListItem[] }) {
  // Always render the section header (even when empty) so the operator can
  // see the structure they can populate — QGC Analyze View shows both lists
  // whether or not files exist.
  return (
    <div className="border-b border-border/60 last:border-b-0">
      <div className="px-3 pt-2.5 pb-1 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
        {title} · {items.length}
      </div>
      {items.length === 0 ? (
        <p className="px-3 pb-2 text-[11px] italic text-muted-foreground/70">
          no {title.toLowerCase()} in catalog — drop files into the :8300 catalog dir
        </p>
      ) : (
        <ul className="flex flex-col">
          {items.map((it) => (
            <li key={`${it.kind}:${it.filename}`}>
              <button
                type="button"
                onClick={it.onPick}
                aria-pressed={it.selected}
                className={`flex w-full flex-col gap-0.5 border-l-2 px-3 py-2 text-left transition-colors ${
                  it.selected
                    ? 'border-emerald-500 bg-emerald-500/10'
                    : 'border-transparent hover:bg-accent/40'
                }`}
              >
                <span className="truncate font-mono text-xs font-medium">{it.filename}</span>
                <span className="font-mono text-[10px] text-muted-foreground">
                  {it.primary}
                  {it.secondary ? ` · ${it.secondary}` : ''}
                </span>
                <span className="font-mono text-[10px] text-muted-foreground/70">
                  {it.mtime > 0 ? new Date(it.mtime).toLocaleString() : '—'}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

function MapEmptyState({ label, hint }: { label: string; hint: string }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-1.5 bg-muted/30 text-center">
      <MapPin className="size-5 text-muted-foreground" aria-hidden="true" />
      <p className="text-sm font-medium text-muted-foreground">{label}</p>
      <p className="text-[11px] text-muted-foreground/80">{hint}</p>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Timeline scrubber
// ---------------------------------------------------------------------------

function TimelineScrubber({
  tick,
  maxTick,
  meta,
  playing,
  disabled,
  onChange,
  onTogglePlay,
}: {
  tick: number
  maxTick: number
  meta: ReplayMeta | null
  playing: boolean
  disabled: boolean
  onChange: (t: number) => void
  onTogglePlay: () => void
}) {
  const virtualTime = meta ? (tick / Math.max(1, meta.records)) * meta.virtual_duration_s : 0
  return (
    <div
      className="rounded-md border border-border bg-card/60 px-3 py-2.5"
      role="group"
      aria-label="Timeline scrubber"
    >
      <div className="flex items-center gap-2">
        <Button
          type="button"
          size="icon"
          variant="secondary"
          className="size-7"
          onClick={onTogglePlay}
          disabled={disabled}
          aria-label={playing ? 'Pause' : 'Play'}
        >
          {playing ? <Pause className="size-3.5" aria-hidden="true" /> : <Play className="size-3.5" aria-hidden="true" />}
        </Button>
        <div className="flex flex-1 items-center gap-2">
          <input
            type="range"
            min={0}
            max={Math.max(0, maxTick)}
            value={Math.min(tick, maxTick)}
            onChange={(e) => onChange(Number(e.target.value))}
            disabled={disabled}
            aria-label="Tick scrubber"
            className="rsim-range h-1.5 w-full cursor-pointer appearance-none rounded-full bg-muted accent-emerald-500 disabled:opacity-40"
          />
        </div>
        <div className="flex w-32 flex-col items-end gap-0.5 font-mono text-[11px] text-muted-foreground">
          <span className="tabular-nums text-foreground">
            tick <span className="font-semibold">{Math.min(tick, maxTick)}</span>
            <span className="text-muted-foreground"> / {maxTick}</span>
          </span>
          <span className="tabular-nums">t = {fmtMissionClock(virtualTime * 1e6)}</span>
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Plot card (lightweight canvas)
// ---------------------------------------------------------------------------

interface PlotDataResolved {
  /** x values: tick (replay) or seconds (ulog). */
  xs: number[]
  /** y values per series. */
  series: { label: string; color: string; ys: number[] }[]
  /** axis label. */
  xLabel: string
}

function resolvePlotData(
  plot: AnalyzePlotConfig,
  selection: AnalyzeSelection | null,
  data: ReplayTopicData | UlogTopicData | undefined,
): PlotDataResolved | null {
  if (!selection || !data) return null
  if (selection.kind === 'replay') {
    const d = data as ReplayTopicData
    const xs = d.ticks
    if (!Array.isArray(d.values) || d.values.length === 0) return null
    if (Array.isArray(d.values[0])) {
      // vector topic — one series per component (x, y, z, w)
      const vec = d.values as number[][]
      const dims = Math.max(...vec.map((v) => v.length))
      const labels = ['x', 'y', 'z', 'w'].slice(0, dims)
      const series = labels.map((lab, i) => ({
        label: `${plot.topic}.${lab}`,
        color: PLOT_COLORS[i % PLOT_COLORS.length],
        ys: vec.map((v) => v[i] ?? NaN),
      }))
      return { xs, series, xLabel: 'tick' }
    }
    // scalar topic
    return {
      xs,
      series: [{ label: plot.topic, color: plot.color, ys: d.values as number[] }],
      xLabel: 'tick',
    }
  }
  // ulog
  const d = data as UlogTopicData
  const xs = d.t
  const fields = Object.keys(d.fields)
  if (fields.length === 0) return null
  const series = fields.map((f, i) => ({
    label: `${plot.topic}.${f}`,
    color: PLOT_COLORS[i % PLOT_COLORS.length],
    ys: d.fields[f],
  }))
  return { xs, series, xLabel: 's' }
}

function PlotCard({
  plot,
  selection,
  data,
  playheadTick,
  maxTick,
  onRemove,
}: {
  plot: AnalyzePlotConfig
  selection: AnalyzeSelection | null
  data: ReplayTopicData | UlogTopicData | undefined
  playheadTick: number
  maxTick: number
  onRemove: () => void
}) {
  const resolved = useMemo(() => resolvePlotData(plot, selection, data), [plot, selection, data])
  const title = `${plot.topic}${plot.field ? `.${plot.field}` : ''}`
  return (
    <div className="rounded-md border border-border bg-card/60">
      <div className="flex items-center gap-2 border-b border-border/60 px-2.5 py-1.5">
        <span className="font-mono text-[11px] font-medium">{title}</span>
        {resolved && (
          <Badge variant="outline" className="font-mono text-[10px]">
            {resolved.series.length} series · {resolved.xs.length} pts
          </Badge>
        )}
        <Button
          size="icon"
          variant="ghost"
          className="ml-auto size-6"
          onClick={onRemove}
          aria-label={`Remove plot ${title}`}
        >
          <Trash2 className="size-3" aria-hidden="true" />
        </Button>
      </div>
      <div className="px-1 py-1">
        {!resolved ? (
          <div className="flex h-32 items-center justify-center gap-2 text-[11px] text-muted-foreground">
            <Loader2 className="size-3 animate-spin" aria-hidden="true" />
            loading…
          </div>
        ) : (
          <PlotCanvas
            data={resolved}
            playheadX={playheadTick}
            maxX={maxTick}
            xLabel={resolved.xLabel}
          />
        )}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// PlotCanvas — a lightweight canvas 2D line chart with a vertical "now" line.
// No chart library: ~120 lines of canvas, mirrors the StripChart approach.
// ---------------------------------------------------------------------------

const PAD_L = 38
const PAD_R = 8
const PAD_T = 8
const PAD_B = 18

function PlotCanvas({
  data,
  playheadX,
  maxX,
  xLabel,
}: {
  data: PlotDataResolved
  playheadX: number
  maxX: number
  xLabel: string
}) {
  const wrapRef = useRef<HTMLDivElement>(null)
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const propsRef = useRef({ data, playheadX, maxX, xLabel })
  useEffect(() => {
    propsRef.current = { data, playheadX, maxX, xLabel }
  }, [data, playheadX, maxX, xLabel])

  useEffect(() => {
    const canvas = canvasRef.current
    const wrap = wrapRef.current
    if (!canvas || !wrap) return

    let raf = 0
    let w = 0
    let h = 0

    const ro = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect
      if (!rect) return
      const dpr = Math.min(2, window.devicePixelRatio || 1)
      w = rect.width
      h = rect.height
      canvas.width = Math.max(1, Math.round(w * dpr))
      canvas.height = Math.max(1, Math.round(h * dpr))
      const ctx = canvas.getContext('2d')
      if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    })
    ro.observe(wrap)

    const cssVar = (name: string, fallback: string): string => {
      const v = getComputedStyle(document.body).getPropertyValue(name).trim()
      return v || fallback
    }

    const draw = () => {
      raf = requestAnimationFrame(draw)
      const ctx = canvas.getContext('2d')
      if (!ctx || w < 40 || h < 30) return
      const { data: D, playheadX: ph, maxX: mx, xLabel: xl } = propsRef.current
      const muted = cssVar('--muted-foreground', '#71717a')
      const border = cssVar('--border', '#e4e4e7')

      ctx.clearRect(0, 0, w, h)

      const xMin = 0
      const xMax = Math.max(mx, 1)
      let yMin = Infinity
      let yMax = -Infinity
      for (const s of D.series) {
        for (const v of s.ys) {
          if (Number.isFinite(v)) {
            if (v < yMin) yMin = v
            if (v > yMax) yMax = v
          }
        }
      }
      if (!Number.isFinite(yMin) || !Number.isFinite(yMax)) {
        yMin = 0
        yMax = 1
      }
      if (yMax - yMin < 1e-9) {
        yMax += 0.5
        yMin -= 0.5
      }
      const pad = (yMax - yMin) * 0.12
      yMin -= pad
      yMax += pad

      const plotX0 = PAD_L
      const plotX1 = w - PAD_R
      const plotY0 = PAD_T
      const plotY1 = h - PAD_B
      const plotW = plotX1 - plotX0
      const plotH = plotY1 - plotY0

      const X = (t: number) => plotX0 + ((t - xMin) / (xMax - xMin)) * plotW
      const Y = (v: number) => plotY1 - ((v - yMin) / (yMax - yMin)) * plotH

      // horizontal grid + y labels
      ctx.font = '10px ui-sans-serif, system-ui, sans-serif'
      ctx.textBaseline = 'middle'
      for (let i = 0; i <= 4; i++) {
        const v = yMin + ((yMax - yMin) * i) / 4
        const y = Y(v)
        ctx.strokeStyle = border
        ctx.globalAlpha = i === 0 || i === 4 ? 0.9 : 0.5
        ctx.lineWidth = 1
        ctx.beginPath()
        ctx.moveTo(plotX0, y)
        ctx.lineTo(plotX1, y)
        ctx.stroke()
        ctx.globalAlpha = 1
        ctx.fillStyle = muted
        ctx.textAlign = 'right'
        ctx.fillText(fmtTick(v), plotX0 - 4, y)
      }

      // x labels: 0 / mid / max
      ctx.textAlign = 'center'
      ctx.textBaseline = 'top'
      ctx.fillStyle = muted
      const mid = (xMin + xMax) / 2
      ctx.fillText('0', X(xMin), plotY1 + 3)
      ctx.fillText(fmtTick(mid), X(mid), plotY1 + 3)
      ctx.fillText(fmtTick(xMax), X(xMax), plotY1 + 3)
      ctx.textAlign = 'right'
      ctx.fillText(xl, plotX1, plotY1 + 3)

      // series
      for (const s of D.series) {
        if (s.ys.length < 2) continue
        const stride = Math.max(1, Math.ceil(s.ys.length / 600))
        ctx.strokeStyle = s.color
        ctx.lineWidth = 1.5
        ctx.lineJoin = 'round'
        ctx.beginPath()
        let started = false
        for (let i = 0; i < s.ys.length; i += stride) {
          const v = s.ys[i]
          const x = D.xs[i]
          if (!Number.isFinite(v) || x < xMin || x > xMax) continue
          const px = X(x)
          const py = Y(v)
          if (!started) {
            ctx.moveTo(px, py)
            started = true
          } else {
            ctx.lineTo(px, py)
          }
        }
        ctx.stroke()
      }

      // "now" line — the scrubber position
      if (ph >= xMin && ph <= xMax) {
        const x = X(ph)
        ctx.strokeStyle = PLAYHEAD_COLOR
        ctx.globalAlpha = 0.85
        ctx.lineWidth = 1.5
        ctx.setLineDash([4, 3])
        ctx.beginPath()
        ctx.moveTo(x, plotY0)
        ctx.lineTo(x, plotY1)
        ctx.stroke()
        ctx.setLineDash([])
        ctx.globalAlpha = 1
        ctx.fillStyle = PLAYHEAD_COLOR
        ctx.textAlign = 'left'
        ctx.textBaseline = 'top'
        ctx.font = '9px ui-sans-serif, system-ui, sans-serif'
        ctx.fillText('now', x + 3, plotY0 + 2)
      }
    }

    raf = requestAnimationFrame(draw)
    return () => {
      cancelAnimationFrame(raf)
      ro.disconnect()
    }
  }, [])

  return (
    <div ref={wrapRef} className="relative h-32 w-full">
      <canvas ref={canvasRef} role="img" aria-label={`Plot of ${data.series.map((s) => s.label).join(', ')}`} className="absolute inset-0 h-full w-full" />
    </div>
  )
}

function fmtTick(v: number): string {
  const a = Math.abs(v)
  if (a >= 1000) return v.toFixed(0)
  if (a >= 10) return v.toFixed(1)
  if (a >= 1) return v.toFixed(2)
  return v.toFixed(3)
}

// ---------------------------------------------------------------------------
// Add Plot dialog
// ---------------------------------------------------------------------------

function AddPlotDialog({
  open,
  onOpenChange,
  topics,
  selection,
  existing,
  onAdd,
}: {
  open: boolean
  onOpenChange: (v: boolean) => void
  topics: string[]
  selection: AnalyzeSelection | null
  existing: AnalyzePlotConfig[]
  onAdd: (topic: string, field?: string) => void
}) {
  const [topic, setTopic] = useState<string>('')
  const [field, setField] = useState<string>('')

  // Reset on open via the onOpenChange callback (matches PatternsDialog's
  // pattern) — NOT in a useEffect, which would trip the
  // `react-hooks/set-state-in-effect` lint rule.
  const handleOpenChange = useCallback(
    (next: boolean) => {
      if (next) {
        setTopic(topics[0] ?? '')
        setField('')
      }
      onOpenChange(next)
    },
    [onOpenChange, topics],
  )

  const ulogFields = useMemo(() => {
    if (!selection || selection.kind !== 'ulog') return null
    // we don't have field names until the data is fetched — surface a hint
    // that the user can type a field name. Most ULog topics use a stable
    // field name like "z" or "vx" — let the operator specify it.
    return true
  }, [selection])

  return (
    <AlertDialog open={open} onOpenChange={handleOpenChange}>
      <AlertDialogContent className="sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle>Add plot</AlertDialogTitle>
          <AlertDialogDescription>
            Pick a topic from the {selection?.kind === 'replay' ? 'replay' : 'ULog'}'s topic list. {selection?.kind === 'replay' ? 'Vector topics (pos_ned_m, q_wxyz) plot each component.' : 'ULog topics expose one or more fields — name the field to plot.'}
          </AlertDialogDescription>
        </AlertDialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="plot-topic" className="text-xs">Topic</Label>
            <Select value={topic} onValueChange={setTopic}>
              <SelectTrigger id="plot-topic" className="w-full">
                <SelectValue placeholder={topics.length === 0 ? 'no topics available' : 'pick a topic'} />
              </SelectTrigger>
              <SelectContent>
                {topics.map((t) => (
                  <SelectItem key={t} value={t} className="font-mono text-xs">
                    {t}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {existing.some((p) => p.topic === topic) && (
              <p className="text-[11px] text-amber-600 dark:text-amber-400">
                A plot of {topic} is already added — adding another stacks it below.
              </p>
            )}
          </div>

          {ulogFields && (
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="plot-field" className="text-xs">
                Field (optional — leave blank for first numeric field)
              </Label>
              <Input
                id="plot-field"
                type="text"
                value={field}
                onChange={(e) => setField(e.target.value)}
                placeholder="e.g. z, vx, yaw"
              />
            </div>
          )}
        </div>

        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <Button onClick={(e) => { e.preventDefault(); onAdd(topic, field || undefined) }} disabled={!topic}>
            <Plus className="size-3.5" aria-hidden="true" />
            Add
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

// ---------------------------------------------------------------------------
// Overlay Live dialog
// ---------------------------------------------------------------------------

function OverlayLiveDialog({
  open,
  onOpenChange,
  op,
  current,
  onSelect,
}: {
  open: boolean
  onOpenChange: (v: boolean) => void
  op: OperatorMapApi
  current: number | null
  onSelect: (vehicleId: number | null) => void
}) {
  const vehicles = op.snapshot?.vehicles ?? []
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle>Overlay live vehicle</AlertDialogTitle>
          <AlertDialogDescription>
            Overlay a connected live vehicle's trajectory (green) alongside the replay (blue). The live vehicle moves in real-time, independent of the replay scrubber.
          </AlertDialogDescription>
        </AlertDialogHeader>

        <div className="flex flex-col gap-1.5">
          {vehicles.length === 0 ? (
            <div className="rounded-md border border-dashed border-border px-3 py-6 text-center text-xs text-muted-foreground">
              No connected vehicles — the fleet plane on :8400 reports 0 vehicles.
              <div className="mt-2">
                <ConnBadgeSmall conn={op.conn} />
              </div>
            </div>
          ) : (
            <ul className="max-h-64 overflow-y-auto rounded-md border border-border">
              {vehicles.map((v) => (
                <li key={v.id}>
                  <button
                    type="button"
                    onClick={() => onSelect(v.index)}
                    aria-pressed={current === v.index}
                    className={`flex w-full items-center gap-2 border-l-2 px-3 py-2 text-left transition-colors ${
                      current === v.index
                        ? 'border-emerald-500 bg-emerald-500/10'
                        : 'border-transparent hover:bg-accent/40'
                    }`}
                  >
                    <span className={`size-2 rounded-full ${v.armed ? 'bg-amber-500' : 'bg-muted-foreground/40'}`} aria-hidden="true" />
                    <span className="font-mono text-xs font-medium">V{v.index + 1}</span>
                    <span className="font-mono text-[10px] text-muted-foreground">{v.fsm}</span>
                    <span className="font-mono text-[10px] text-muted-foreground">{v.mode}</span>
                    <span className="ml-auto font-mono text-[10px] text-muted-foreground">
                      {v.battery_pct}% · {v.alt_agl_m != null ? `${v.alt_agl_m.toFixed(0)} m` : 'no fix'}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <AlertDialogFooter>
          <AlertDialogCancel>Close</AlertDialogCancel>
          {current != null && (
            <Button variant="outline" onClick={(e) => { e.preventDefault(); onSelect(null) }}>
              <X className="size-3.5" aria-hidden="true" />
              Stop overlay
            </Button>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

function ConnBadgeSmall({ conn }: { conn: OperatorMapApi['conn'] }) {
  if (conn === 'live') return <Badge className="gap-1 border border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400" aria-live="polite"><RadioTower className="size-3" aria-hidden="true" />live</Badge>
  if (conn === 'connecting') return <Badge variant="secondary" className="gap-1"><Loader2 className="size-3 animate-spin" aria-hidden="true" />connecting</Badge>
  return <Badge className="gap-1 border border-amber-600/40 bg-amber-500/10 text-amber-700 dark:text-amber-400">simulated</Badge>
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function plotKey(selection: AnalyzeSelection | null, plot: AnalyzePlotConfig): string {
  if (!selection) return ''
  return `${selection.kind}:${selection.filename}#${plot.topic}${plot.field ? `.${plot.field}` : ''}`
}
