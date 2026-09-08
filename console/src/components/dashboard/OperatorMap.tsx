'use client'

/**
 * Operator Map (ADR-0017): the QGroundControl Fly/Plan-style geo map where
 * the user controls SITL themselves.
 *
 * - Live Leaflet map: vehicle markers with heading, trajectories, home,
 *   the real geofence; map click in Fly mode = Go To Location (confirm
 *   bar), in Plan mode = add a waypoint.
 * - Mission panel: numbered waypoints, per-waypoint altitude/hover, drag
 *   to edit, Upload (append/replace) → the manager validates against the
 *   fence and the auction flies them; Start Mission / Clear.
 * - Guided action bar: ARM / DISARM / TAKEOFF / LAND / RTL / HOLD for the
 *   selected vehicle, E-STOP for the fleet — every button is the same
 *   MAVLink the supervisor itself sends.
 *
 * LIVE/SIMULATED dual-mode like every console view.
 */

import { useCallback, useMemo, useState } from 'react'
import {
  Compass,
  Crosshair,
  Eraser,
  Flag,
  Landmark,
  Layers,
  MapPin,
  Mountain,
  OctagonX,
  PlaneTakeoff,
  Play,
  RadioTower,
  RotateCcw,
  Send,
  Square,
  Trash2,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useToast } from '@/hooks/use-toast'
import { cn } from '@/lib/utils'
import type { MapWaypoint } from '@/lib/types'
import { ConnBadge } from './ConnBadge'
import { GeoMap, type GeoVehicleMarker } from './GeoMap'
import type { OperatorMapApi } from '@/hooks/useOperatorMap'

const FSM_TONE: Record<string, string> = {
  READY: 'text-sky-600 dark:text-sky-400',
  ACTIVE: 'text-amber-600 dark:text-amber-400',
  RTL: 'text-orange-600 dark:text-orange-400',
  LANDED: 'text-emerald-600 dark:text-emerald-400',
  FAULT: 'text-red-600 dark:text-red-400',
}

const TASK_TONE: Record<string, string> = {
  pending: 'text-muted-foreground',
  queued: 'text-sky-600 dark:text-sky-400',
  active: 'text-amber-600 dark:text-amber-400',
  complete: 'text-emerald-600 dark:text-emerald-400',
  done: 'text-emerald-600 dark:text-emerald-400',
  cleared: 'text-muted-foreground line-through',
  rejected: 'text-red-600 dark:text-red-400',
}

export function OperatorMap({ op }: { op: OperatorMapApi }) {
  const { toast } = useToast()
  const [selected, setSelected] = useState(0)
  const [planMode, setPlanMode] = useState(false)
  const [waypoints, setWaypoints] = useState<MapWaypoint[]>([])
  const [wpSeq, setWpSeq] = useState(1)
  const [defaultAlt, setDefaultAlt] = useState(12)
  const [defaultHover, setDefaultHover] = useState(2)
  const [uploadMode, setUploadMode] = useState<'append' | 'replace'>('append')
  const [gotoPending, setGotoPending] = useState<{ lat: number; lng: number } | null>(null)
  const [gotoAlt, setGotoAlt] = useState(10)
  const [busy, setBusy] = useState(false)
  const [follow, setFollow] = useState(true)

  const snap = op.snapshot
  const vehicles = snap?.vehicles ?? []
  const vehicle = vehicles.find((v) => v.index === selected) ?? null
  const missionFlying = vehicle != null && vehicle.fsm === 'ACTIVE' && vehicle.task_id != null

  // ------------------------------------------------------------ map markers
  const markers: GeoVehicleMarker[] = useMemo(
    () =>
      vehicles
        .filter((v) => v.lat != null && v.lon != null)
        .map((v) => ({
          key: v.id,
          lat: v.lat as number,
          lng: v.lon as number,
          heading_deg: v.yaw_deg,
          label: `${v.id}${v.armed ? ' ⚔' : ''}`,
          color: op.vehicleColors[v.index % op.vehicleColors.length],
          detail:
            `${v.id} (V${v.index + 1} · sysid ${v.sysid})\n` +
            `${v.fsm} · ${v.mode}${v.armed ? ' · ARMED' : ' · disarmed'}\n` +
            `AGL ${v.alt_agl_m != null ? v.alt_agl_m.toFixed(1) : '—'} m · ${v.battery_pct}% batt\n` +
            `${v.lat!.toFixed(6)}, ${v.lon!.toFixed(6)}` +
            (v.task_id ? `\ntask ${v.task_id}` : ''),
        })),
    [vehicles, op.vehicleColors],
  )

  const trailColors = useMemo(() => {
    const m: Record<string, string> = {}
    for (const v of vehicles) m[v.id] = op.vehicleColors[v.index % op.vehicleColors.length]
    return m
  }, [vehicles, op.vehicleColors])

  // ---------------------------------------------------------- map handlers
  const onMapClick = useCallback(
    (lat: number, lng: number) => {
      if (planMode) {
        const key = `wp${wpSeq}`
        setWpSeq((n) => n + 1)
        setWaypoints((wps) => [...wps, { key, lat, lng, alt_m: defaultAlt, hover_s: defaultHover }])
      } else if (vehicle && vehicle.lat != null) {
        // QGC map position action: Go To Location (confirm bar)
        setGotoPending({ lat, lng })
      }
    },
    [planMode, wpSeq, defaultAlt, defaultHover, vehicle],
  )

  const onWaypointDrag = useCallback((key: string, lat: number, lng: number) => {
    setWaypoints((wps) => wps.map((w) => (w.key === key ? { ...w, lat, lng } : w)))
  }, [])

  const onWaypointRemove = useCallback((key: string) => {
    setWaypoints((wps) => wps.filter((w) => w.key !== key))
  }, [])

  const patchWaypoint = useCallback((key: string, patch: Partial<MapWaypoint>) => {
    setWaypoints((wps) => wps.map((w) => (w.key === key ? { ...w, ...patch } : w)))
  }, [])

  // -------------------------------------------------------------- commands
  const run = useCallback(
    async (label: string, fn: () => Promise<string | null>) => {
      if (busy) return
      setBusy(true)
      try {
        const err = await fn()
        if (err) toast({ title: `${label} — rejected`, description: err, variant: 'destructive' })
      } catch (e) {
        toast({
          title: `${label} failed`,
          description: e instanceof Error ? e.message : String(e),
          variant: 'destructive',
        })
      } finally {
        setBusy(false)
      }
    },
    [busy, toast],
  )

  const cmd = useCallback(
    (label: string, fn: () => Promise<unknown>) =>
      run(label, async () => {
        const r = (await fn()) as { accepted?: boolean; started?: boolean; result?: number; reason?: string }
        const ok = r.accepted === true || r.started === true
        if (!ok) {
          return r.reason ?? `not accepted (MAVLink result ${r.result ?? '—'})`
        }
        toast({ title: `${label} accepted`, description: 'check the event log for the supervisor echo' })
        return null
      }),
    [run, toast],
  )

  const doUpload = useCallback(
    () =>
      run('Mission upload', async () => {
        if (waypoints.length === 0) return 'no waypoints planned — click the map in Plan mode'
        const r = await op.uploadMission(waypoints, uploadMode === 'replace')
        const parts: string[] = []
        if (r.accepted.length > 0) parts.push(`${r.accepted.length} accepted (${r.accepted.join(', ')})`)
        if (r.rejected.length > 0) parts.push(`${r.rejected.length} rejected`)
        toast({
          title: 'Mission uploaded',
          description: parts.join(' · ') + ` — pool ${r.pool}`,
        })
        if (r.rejected.length > 0) {
          toast({
            title: 'Rejected waypoints',
            description: r.rejected.map((x) => `${x.label}: ${x.reason}`).join(' · '),
            variant: 'destructive',
          })
        }
        setWaypoints([])
        return null
      }),
    [run, toast, op, waypoints, uploadMode],
  )

  // ------------------------------------------------------------------ view
  const fence = snap?.geofence

  return (
    <div className="flex flex-col gap-4">
      {/* header */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <CardTitle className="flex items-center gap-2 text-base">
              <MapPin className="size-4 text-sky-600 dark:text-sky-400" aria-hidden="true" />
              Operator Map
              <span className="text-xs font-normal text-muted-foreground">— QGC Fly/Plan on real PX4 SITL</span>
            </CardTitle>
            <Badge variant={snap?.phase === 'RUNNING' ? 'default' : 'secondary'} className="font-mono">
              {snap?.phase ?? '—'}
            </Badge>
            <ConnBadge conn={op.conn} port={op.port} retryAt={op.retryAt} lastError={op.lastError} />
            {op.tilesOffline && (
              <Badge variant="outline" className="gap-1 border-amber-500/40 text-amber-600 dark:text-amber-400">
                <Layers aria-hidden="true" className="size-3" /> tiles offline — graticule
              </Badge>
            )}
            <span className="ml-auto hidden font-mono text-[11px] text-muted-foreground md:inline">
              origin {op.origin.lat_deg.toFixed(6)}, {op.origin.lon_deg.toFixed(6)} · {op.origin.alt_m.toFixed(0)} m MSL
              {fence ? ` · fence ±${Math.abs(fence.points[1][0]).toFixed(0)} m / ${fence.ceiling_m.toFixed(0)} m` : ''}
            </span>
          </div>
          <CardDescription>
            The geo map anchors to the scenario <code>[env] origin</code> the sims' HIL_GPS reports from — what you
            see is PX4's own GLOBAL_POSITION_INT estimate. Click the map to Go To (Fly) or add waypoints (Plan).
          </CardDescription>
        </CardHeader>
      </Card>

      <div className="grid gap-4 xl:grid-cols-3">
        {/* map */}
        <Card className="xl:col-span-2">
          <CardHeader className="pb-2">
            <div className="flex flex-wrap items-center gap-2">
              <CardTitle className="text-sm">Live map</CardTitle>
              <div className="ml-auto flex items-center gap-1.5">
                <Button
                  size="sm"
                  variant={planMode ? 'outline' : 'secondary'}
                  className="gap-1.5"
                  onClick={() => setPlanMode(false)}
                  aria-pressed={!planMode}
                >
                  <Crosshair className="size-3.5" aria-hidden="true" /> Fly
                </Button>
                <Button
                  size="sm"
                  variant={planMode ? 'secondary' : 'outline'}
                  className="gap-1.5"
                  onClick={() => {
                    setPlanMode(true)
                    setGotoPending(null)
                  }}
                  aria-pressed={planMode}
                >
                  <MapPin className="size-3.5" aria-hidden="true" /> Plan
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="gap-1.5"
                  onClick={() => setFollow((f) => !f)}
                  aria-pressed={follow}
                  title="keep the selected vehicle centered"
                >
                  <RadioTower className="size-3.5" aria-hidden="true" />
                  {follow ? 'following' : 'free'}
                </Button>
              </div>
            </div>
            {planMode && (
              <CardDescription>
                Plan mode: click the map to append waypoints, drag to move, right-click a waypoint to remove, then
                Upload → Start Mission.
              </CardDescription>
            )}
          </CardHeader>
          <CardContent className="p-2">
            <div className="h-[520px] w-full overflow-hidden rounded-md border border-border">
              <GeoMap
                origin={op.origin}
                fenceGeo={op.fenceGeo}
                fenceCeilingM={fence?.ceiling_m ?? 60}
                vehicles={markers}
                trails={op.trails}
                trailColors={trailColors}
                waypoints={waypoints}
                planMode={planMode}
                followKey={follow && !planMode ? (vehicle?.id ?? null) : null}
                onMapClick={onMapClick}
                onWaypointDrag={onWaypointDrag}
                onWaypointRemove={onWaypointRemove}
                onTilesOffline={() => op.setTilesOffline(true)}
              />
            </div>
            {/* go-to confirm bar (QGC map position action) */}
            {gotoPending && (
              <div
                className="mt-2 flex flex-wrap items-center gap-2 rounded-md border border-sky-500/40 bg-sky-500/10 px-3 py-2"
                role="group"
                aria-label="Go to location confirm"
              >
                <Send className="size-3.5 text-sky-600 dark:text-sky-400" aria-hidden="true" />
                <span className="font-mono text-xs">
                  Go To {gotoPending.lat.toFixed(6)}, {gotoPending.lng.toFixed(6)}
                </span>
                <Label className="ml-2 flex items-center gap-1 text-xs">
                  <Mountain className="size-3" aria-hidden="true" />
                  AGL
                  <Input
                    type="number"
                    min={1}
                    max={60}
                    value={gotoAlt}
                    onChange={(e) => setGotoAlt(Number(e.target.value) || 10)}
                    className="h-7 w-16 text-xs"
                    aria-label="go-to altitude AGL metres"
                  />
                  m
                </Label>
                <span className="hidden text-[11px] text-muted-foreground sm:inline">
                  vehicle flies there in OFFBOARD and holds
                </span>
                <div className="ml-auto flex gap-1.5">
                  <Button
                    size="sm"
                    className="gap-1.5"
                    disabled={busy || !vehicle}
                    onClick={() => {
                      const g = gotoPending
                      setGotoPending(null)
                      void cmd(`Go To ${g.lat.toFixed(5)}, ${g.lng.toFixed(5)}`, () =>
                        op.goto(selected, g.lat, g.lng, gotoAlt),
                      )
                    }}
                  >
                    Go
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setGotoPending(null)}>
                    Cancel
                  </Button>
                </div>
              </div>
            )}
          </CardContent>
        </Card>

        {/* vehicle + commands + mission panel */}
        <div className="flex flex-col gap-4">
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm">Vehicle &amp; guided actions</CardTitle>
              <CardDescription>the same MAVLink the supervisor itself sends (ADR-0017)</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              <div className="flex flex-wrap gap-1.5">
                {vehicles.map((v) => (
                  <Button
                    key={v.id}
                    size="sm"
                    variant={v.index === selected ? 'secondary' : 'ghost'}
                    className="gap-1.5"
                    onClick={() => setSelected(v.index)}
                    aria-pressed={v.index === selected}
                  >
                    <span className={cn('font-mono text-xs', FSM_TONE[v.fsm] ?? '')}>{v.fsm}</span>
                    {v.id}
                  </Button>
                ))}
              </div>
              {vehicle ? (
                <div className="grid grid-cols-2 gap-x-3 gap-y-1 font-mono text-[11px] text-muted-foreground">
                  <span>
                    mode <span className="text-foreground">{vehicle.mode}</span>
                  </span>
                  <span>
                    fsm <span className={cn('text-foreground', FSM_TONE[vehicle.fsm] ?? '')}>{vehicle.fsm}</span>
                  </span>
                  <span>
                    armed{' '}
                    <span className={vehicle.armed ? 'text-amber-600 dark:text-amber-400' : 'text-foreground'}>
                      {vehicle.armed ? 'YES' : 'no'}
                    </span>
                  </span>
                  <span>
                    AGL{' '}
                    <span className="text-foreground">
                      {vehicle.alt_agl_m != null ? `${vehicle.alt_agl_m.toFixed(1)} m` : '—'}
                    </span>
                  </span>
                  <span>
                    battery <span className="text-foreground">{vehicle.battery_pct}%</span>
                  </span>
                  <span>
                    task <span className="text-foreground">{vehicle.task_id ?? '—'}</span>
                  </span>
                  <span className="col-span-2">
                    GPS{' '}
                    <span className="text-foreground">
                      {vehicle.lat != null ? `${vehicle.lat.toFixed(6)}, ${vehicle.lon?.toFixed(6)}` : 'no fix yet'}
                    </span>
                  </span>
                </div>
              ) : (
                <p className="text-xs text-muted-foreground">no vehicles in the snapshot yet</p>
              )}

              <div className="grid grid-cols-3 gap-1.5">
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1"
                  disabled={busy || !vehicle || vehicle.armed || missionFlying}
                  title={missionFlying ? 'vehicle is flying an autonomous mission' : 'COMPONENT_ARM_DISARM(1)'}
                  onClick={() => void cmd('ARM', () => op.arm(selected, true))}
                >
                  <PlaneTakeoff className="size-3.5" aria-hidden="true" /> ARM
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1"
                  disabled={busy || !vehicle || !vehicle.armed}
                  title="COMPONENT_ARM_DISARM(0)"
                  onClick={() => void cmd('DISARM', () => op.arm(selected, false))}
                >
                  <Square className="size-3.5" aria-hidden="true" /> DISARM
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1"
                  disabled={busy || !vehicle || missionFlying}
                  title={missionFlying ? 'vehicle is flying an autonomous mission' : 'MAV_CMD_NAV_TAKEOFF'}
                  onClick={() =>
                    void cmd('TAKEOFF', () => op.takeoff(selected, Math.min(defaultAlt, fence?.ceiling_m ?? 60)))
                  }
                >
                  <Mountain className="size-3.5" aria-hidden="true" /> TAKEOFF
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1"
                  disabled={busy || !vehicle || missionFlying}
                  title="AUTO.LAND + stream stop"
                  onClick={() => void cmd('LAND', () => op.land(selected))}
                >
                  <Landmark className="size-3.5" aria-hidden="true" /> LAND
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1"
                  disabled={busy || !vehicle || missionFlying}
                  title="AUTO.RTL + stream stop"
                  onClick={() => void cmd('RTL', () => op.rtl(selected))}
                >
                  <RotateCcw className="size-3.5" aria-hidden="true" /> RTL
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1"
                  disabled={busy || !vehicle || missionFlying}
                  title="AUTO.LOITER (pause) + stream stop"
                  onClick={() => void cmd('HOLD', () => op.hold(selected))}
                >
                  <Compass className="size-3.5" aria-hidden="true" /> HOLD
                </Button>
              </div>

              <Button
                variant="destructive"
                size="sm"
                className="mt-1 gap-1.5"
                disabled={busy}
                onClick={() =>
                  void run('E-STOP', async () => {
                    await op.estop()
                    toast({ title: 'E-STOP requested', description: 'policy 1: all vehicles LAND, run ABORTED' })
                    return null
                  })
                }
              >
                <OctagonX className="size-4" aria-hidden="true" /> E-STOP (fleet)
              </Button>
            </CardContent>
          </Card>

          {/* mission panel */}
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm">Mission (Plan)</CardTitle>
              <CardDescription>
                fence-validated upload → sequential auction → offboard runners (op* tasks)
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              <div className="grid grid-cols-2 gap-2">
                <Label className="flex flex-col gap-1 text-xs">
                  default altitude (AGL m)
                  <Input
                    type="number"
                    min={1}
                    max={fence?.ceiling_m ?? 60}
                    value={defaultAlt}
                    onChange={(e) => setDefaultAlt(Number(e.target.value) || 12)}
                    className="h-8"
                  />
                </Label>
                <Label className="flex flex-col gap-1 text-xs">
                  default hover (s)
                  <Input
                    type="number"
                    min={0}
                    max={60}
                    value={defaultHover}
                    onChange={(e) => setDefaultHover(Number(e.target.value) || 0)}
                    className="h-8"
                  />
                </Label>
              </div>

              <div className="max-h-44 overflow-auto rounded-md border border-border">
                <Table>
                  <TableHeader>
                    <TableRow className="hover:bg-transparent">
                      <TableHead className="h-8 w-8 text-[11px]">#</TableHead>
                      <TableHead className="h-8 text-[11px]">lat, lon</TableHead>
                      <TableHead className="h-8 w-16 text-[11px]">AGL m</TableHead>
                      <TableHead className="h-8 w-14 text-[11px]">hover s</TableHead>
                      <TableHead className="h-8 w-8" />
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {waypoints.length === 0 && (
                      <TableRow>
                        <TableCell colSpan={5} className="py-3 text-center text-xs text-muted-foreground">
                          {planMode ? 'click the map to add waypoints' : 'switch to Plan mode to edit'}
                        </TableCell>
                      </TableRow>
                    )}
                    {waypoints.map((w, i) => (
                      <TableRow key={w.key} className="hover:bg-muted/40">
                        <TableCell className="py-1.5 font-mono text-xs">{i + 1}</TableCell>
                        <TableCell className="py-1.5 font-mono text-[10px]">
                          {w.lat.toFixed(6)}, {w.lng.toFixed(6)}
                        </TableCell>
                        <TableCell className="py-1.5">
                          <Input
                            type="number"
                            min={1}
                            max={fence?.ceiling_m ?? 60}
                            value={w.alt_m}
                            onChange={(e) => patchWaypoint(w.key, { alt_m: Number(e.target.value) || 1 })}
                            className="h-7 w-14 px-1 text-xs"
                            aria-label={`waypoint ${i + 1} altitude`}
                          />
                        </TableCell>
                        <TableCell className="py-1.5">
                          <Input
                            type="number"
                            min={0}
                            value={w.hover_s}
                            onChange={(e) => patchWaypoint(w.key, { hover_s: Number(e.target.value) || 0 })}
                            className="h-7 w-12 px-1 text-xs"
                            aria-label={`waypoint ${i + 1} hover seconds`}
                          />
                        </TableCell>
                        <TableCell className="py-1.5">
                          <Button
                            size="icon"
                            variant="ghost"
                            className="size-6"
                            onClick={() => onWaypointRemove(w.key)}
                            aria-label={`remove waypoint ${i + 1}`}
                          >
                            <Trash2 className="size-3" aria-hidden="true" />
                          </Button>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>

              <div className="flex flex-wrap items-center gap-1.5">
                <Select
                  value={uploadMode}
                  onValueChange={(v) => setUploadMode(v as 'append' | 'replace')}
                  aria-label="upload mode"
                >
                  <SelectTrigger className="h-8 w-28 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="append">append</SelectItem>
                    <SelectItem value="replace">replace</SelectItem>
                  </SelectContent>
                </Select>
                <Button
                  size="sm"
                  className="gap-1.5"
                  disabled={busy || waypoints.length === 0}
                  onClick={() => void doUpload()}
                >
                  <Send className="size-3.5" aria-hidden="true" /> Upload
                </Button>
                <Button
                  size="sm"
                  variant="secondary"
                  className="gap-1.5"
                  disabled={busy}
                  onClick={() => void cmd('Start mission', () => op.startMission())}
                  title="the auction flies the uploaded tasks"
                >
                  <Play className="size-3.5" aria-hidden="true" /> Start mission
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="gap-1.5"
                  disabled={busy}
                  onClick={() => void cmd('Clear mission', () => op.clearMission())}
                  title="drop queued operator tasks"
                >
                  <Eraser className="size-3.5" aria-hidden="true" /> Clear
                </Button>
              </div>
            </CardContent>
          </Card>
        </div>
      </div>

      {/* task board + event log */}
      <div className="grid gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="flex items-center gap-2 text-sm">
              <Flag className="size-4 text-muted-foreground" aria-hidden="true" />
              Task board
              <span className="ml-auto font-mono text-[11px] text-muted-foreground">
                {snap?.tasks.length ?? 0} tasks
              </span>
            </CardTitle>
          </CardHeader>
          <CardContent>
            <ScrollArea className="h-40">
              <div className="space-y-1 font-mono text-[11px]">
                {(snap?.tasks ?? []).map((t) => (
                  <div key={t.id} className="flex items-center gap-2">
                    <span className={cn('w-10 shrink-0', t.id.startsWith('op') ? 'font-semibold text-sky-600 dark:text-sky-400' : '')}>
                      {t.id}
                    </span>
                    <span className={cn('w-16 shrink-0', TASK_TONE[t.status] ?? '')}>{t.status}</span>
                    <span className="text-muted-foreground">
                      N {t.pos_ned_m[0].toFixed(0)} E {t.pos_ned_m[1].toFixed(0)} ·{' '}
                      {(-t.pos_ned_m[2]).toFixed(0)} m AGL
                    </span>
                    {t.assigned_to != null && (
                      <span className="ml-auto text-muted-foreground">→ {t.assigned_to}</span>
                    )}
                  </div>
                ))}
                {(snap?.tasks.length ?? 0) === 0 && (
                  <p className="text-xs text-muted-foreground">no tasks yet — upload a mission</p>
                )}
              </div>
            </ScrollArea>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="flex items-center gap-2 text-sm">
              Operator events
              <span className="ml-auto font-mono text-[11px] text-muted-foreground">
                {op.events.length} recent
              </span>
            </CardTitle>
          </CardHeader>
          <CardContent>
            <ScrollArea className="h-40">
              <div className="space-y-1 font-mono text-[11px]">
                {op.events
                  .slice()
                  .reverse()
                  .map((e, i) => (
                    <div key={`${e.t}-${i}`} className="flex gap-2">
                      <span className="shrink-0 text-muted-foreground">
                        {e.vehicle ? `${e.vehicle}` : 'fleet'}
                      </span>
                      <span
                        className={cn(
                          'min-w-0 break-words',
                          e.severity === 'critical' ? 'text-red-600 dark:text-red-400' : e.severity === 'warn' ? 'text-amber-600 dark:text-amber-400' : '',
                        )}
                      >
                        {e.detail}
                      </span>
                    </div>
                  ))}
                {op.events.length === 0 && (
                  <p className="text-xs text-muted-foreground">event tail lands here (fleet frame)</p>
                )}
              </div>
            </ScrollArea>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
