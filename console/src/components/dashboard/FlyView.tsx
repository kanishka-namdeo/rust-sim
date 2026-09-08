'use client'

/**
 * Fly View (GCS_SPEC §5.2 + §8.2 — the QGC Fly View analog).
 *
 * Layout: 3-column grid (desktop) / stacked (mobile):
 *   - Left (25%):   attitude HUD + battery/signal/mode/GPS/EKF widgets +
 *                   pre-arm checklist panel
 *   - Center (50%): live Leaflet map (reuses <GeoMap>) + action bar
 *                   (ARM/DISARM, TAKEOFF, LAND, RTL, HOLD, START MISSION)
 *   - Right (25%):  vehicle selector + strip charts (reuses <StripChart>
 *                   with the useFlyView 60 s @ 10 Hz series for the active
 *                   vehicle: altitude, battery, velocity)
 *
 * The view consumes the page-level `op: OperatorMapApi` (so the fleet polling
 * engine survives tab switches) and adds active-vehicle selection + pre-arm
 * checks via the useFlyView hook. The pre-arm-checks endpoint is built by a
 * parallel agent — handled gracefully if it 404s.
 *
 * The attitude HUD is a pure SVG artificial horizon (no external lib): the
 * inner sky/ground group rotates by -roll and translates by +pitch; the
 * fixed aircraft symbol stays overlaid in the bezel frame.
 */

import { useCallback, useMemo, useState } from 'react'
import {
  Activity,
  BatteryFull,
  Compass,
  Crosshair,
  Gauge,
  Landmark,
  Mountain,
  PlaneTakeoff,
  Play,
  RadioTower,
  RotateCcw,
  Satellite,
  Send,
  ShieldCheck,
  Square,
  TriangleAlert,
  Waypoints,
  Zap,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Progress } from '@/components/ui/progress'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useToast } from '@/hooks/use-toast'
import { useFlyView } from '@/hooks/useFlyView'
import { batteryTone, clamp, fmt, fmtSigned, quatToEulerDeg } from '@/lib/format'
import type { FleetVehicle } from '@/lib/types'
import { cn } from '@/lib/utils'
import { ConnBadge } from './ConnBadge'
import { GeoMap, type GeoVehicleMarker } from './GeoMap'
import { StripChart } from './StripChart'
import type { OperatorMapApi } from '@/hooks/useOperatorMap'
import type { FlyViewApi } from '@/hooks/useFlyView'

const ACTIVE_COLOR = '#f59e0b' // amber — the active vehicle pops on the map

const EMERALD = '#10b981'
const AMBER = '#f59e0b'
const SKY = '#0ea5e9'

// ---------------------------------------------------------------------------
// FlyView
// ---------------------------------------------------------------------------

export function FlyView({ op }: { op: OperatorMapApi }) {
  const fly = useFlyView(op)
  const { toast } = useToast()
  const [busy, setBusy] = useState(false)
  const [gotoPending, setGotoPending] = useState<{ lat: number; lng: number } | null>(null)
  const [gotoAlt, setGotoAlt] = useState(10)
  const [follow, setFollow] = useState(true)

  const activeVehicle = fly.activeVehicle
  const snapshot = fly.snapshot
  const vehicles = fly.vehicles

  // active-vehicle attitude (the snapshot doesn't carry a quaternion — we
  // synthesize one from yaw_deg so the HUD reads the same heading as the map
  // arrow; roll/pitch fall back to 0 because the fleet plane doesn't expose
  // attitude yet. When the backend ships quaternion in the snapshot this
  // stays tolerant.)
  const att = useMemo(() => {
    if (!activeVehicle) return null
    const yawRad = (activeVehicle.yaw_deg * Math.PI) / 180
    // quaternion (w, x, y, z) for a pure yaw rotation about Z (down axis in NED)
    const q: [number, number, number, number] = [
      Math.cos(yawRad / 2),
      0,
      0,
      Math.sin(yawRad / 2),
    ]
    return quatToEulerDeg(q)
  }, [activeVehicle])

  // ----------------------------------------------------------- map markers
  const markers: GeoVehicleMarker[] = useMemo(() => {
    return vehicles
      .filter((v) => v.lat != null && v.lon != null)
      .map((v) => {
        const isActive = v.index === fly.activeVehicleId
        return {
          key: v.id,
          lat: v.lat as number,
          lng: v.lon as number,
          heading_deg: v.yaw_deg,
          label: `${isActive ? '▸ ' : ''}${v.id}${v.armed ? ' ⚔' : ''}`,
          color: isActive ? ACTIVE_COLOR : op.vehicleColors[v.index % op.vehicleColors.length],
          detail:
            `${v.id} (V${v.index + 1} · sysid ${v.sysid})\n` +
            `${v.fsm} · ${v.mode}${v.armed ? ' · ARMED' : ' · disarmed'}\n` +
            `AGL ${v.alt_agl_m != null ? v.alt_agl_m.toFixed(1) : '—'} m · ${v.battery_pct.toFixed(0)}% batt\n` +
            `${(v.lat as number).toFixed(6)}, ${(v.lon as number).toFixed(6)}` +
            (v.task_id ? `\ntask ${v.task_id}` : '') +
            (isActive ? '\n★ ACTIVE' : ''),
        }
      })
  }, [vehicles, fly.activeVehicleId, op.vehicleColors])

  const trailColors = useMemo(() => {
    const m: Record<string, string> = {}
    for (const v of vehicles) {
      m[v.id] = v.index === fly.activeVehicleId ? ACTIVE_COLOR : op.vehicleColors[v.index % op.vehicleColors.length]
    }
    return m
  }, [vehicles, fly.activeVehicleId, op.vehicleColors])

  // ---------------------------------------------------------- map handlers
  const onMapClick = useCallback(
    (lat: number, lng: number) => {
      if (!activeVehicle || activeVehicle.lat == null) return
      setGotoPending({ lat, lng })
    },
    [activeVehicle],
  )

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
        if (!ok) return r.reason ?? `not accepted (MAVLink result ${r.result ?? '—'})`
        toast({ title: `${label} accepted`, description: 'check the event log for the supervisor echo' })
        return null
      }),
    [run, toast],
  )

  // ------------------------------------------------------------- arm gate
  // §8.2 step 1: clicking ARM runs the pre-arm checks first; if any fail,
  // ARM is blocked with a red banner. If the endpoint is unavailable (404),
  // we surface a non-blocking warning and let the operator proceed at their
  // own risk via the dedicated DISARM/ARM fallback below.
  const doArm = useCallback(async () => {
    if (!activeVehicle) return
    const r = await fly.runPrearmChecks(activeVehicle.index)
    if (r.unavailable) {
      toast({
        title: 'Pre-arm checks unavailable',
        description: 'Endpoint GET /api/vehicles/{i}/prearm-checks returned 404 — arming without server-side gate.',
        variant: 'destructive',
      })
      void cmd('ARM', () => fly.arm(activeVehicle.index))
      return
    }
    if (!r.all_passed) {
      const failed = r.checks.filter((c) => !c.passed).map((c) => `${c.name}: ${c.message}`).join(' · ')
      toast({
        title: 'ARM blocked — pre-arm checks failed',
        description: failed || 'one or more pre-arm checks did not pass',
        variant: 'destructive',
      })
      return
    }
    void cmd('ARM', () => fly.arm(activeVehicle.index))
  }, [activeVehicle, fly, cmd, toast])

  // ------------------------------------------------------------- view
  const fence = snapshot?.geofence
  const missionFlying =
    activeVehicle != null && activeVehicle.fsm === 'ACTIVE' && activeVehicle.task_id != null
  const ekfBad =
    activeVehicle != null &&
    (activeVehicle.health.includes('LINK_STALE') || activeVehicle.health.includes('HEARTBEAT_LOST'))

  return (
    <div className="flex flex-col gap-4">
      {/* ----------------------------------------------------------- header */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <CardTitle className="flex flex-wrap items-center gap-2.5 text-base">
                <PlaneTakeoff className="size-4 text-amber-600 dark:text-amber-400" aria-hidden="true" />
                Fly View
                <span className="text-xs font-normal text-muted-foreground">— QGC Fly analog on live PX4 SITL</span>
                <Badge variant={snapshot?.phase === 'RUNNING' ? 'default' : 'secondary'} className="font-mono">
                  {snapshot?.phase ?? '—'}
                </Badge>
                <ConnBadge conn={op.conn} port={op.port} retryAt={op.retryAt} lastError={op.lastError} />
              </CardTitle>
              <CardDescription className="mt-1">
                Live map · attitude HUD · pre-arm checks · action bar — the operator flies the active vehicle; the fleet
                snapshot keeps every other vehicle visible at the same time.
              </CardDescription>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                size="sm"
                variant={follow ? 'secondary' : 'ghost'}
                className="gap-1.5"
                onClick={() => setFollow((f) => !f)}
                aria-pressed={follow}
                title="keep the active vehicle centered on the map"
              >
                <RadioTower className="size-3.5" aria-hidden="true" />
                {follow ? 'following' : 'free'}
              </Button>
            </div>
          </div>
        </CardHeader>
      </Card>

      <div className="grid gap-4 xl:grid-cols-4">
        {/* --------------------------------------------- left col: instruments */}
        <div className="flex flex-col gap-4 xl:col-span-1">
          {/* attitude HUD */}
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="flex items-center gap-2 text-sm">
                <Compass className="size-4 text-sky-600 dark:text-sky-400" aria-hidden="true" />
                Attitude HUD
              </CardTitle>
              <CardDescription className="text-[11px]">artificial horizon — roll / pitch / heading</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col items-center gap-2">
              <div className="relative aspect-square w-full max-w-[220px]">
                <AttitudeHud
                  rollDeg={att?.roll_deg ?? 0}
                  pitchDeg={att?.pitch_deg ?? 0}
                  yawDeg={activeVehicle?.yaw_deg ?? 0}
                  dim={!activeVehicle}
                />
              </div>
              <div className="grid w-full grid-cols-3 gap-2 font-mono text-[11px]">
                <div className="rounded-md border border-border/60 px-2 py-1">
                  <div className="text-[9px] uppercase tracking-wide text-muted-foreground">Roll</div>
                  <div className="font-semibold tabular-nums">{fmtSigned(att?.roll_deg, 1)}°</div>
                </div>
                <div className="rounded-md border border-border/60 px-2 py-1">
                  <div className="text-[9px] uppercase tracking-wide text-muted-foreground">Pitch</div>
                  <div className="font-semibold tabular-nums">{fmtSigned(att?.pitch_deg, 1)}°</div>
                </div>
                <div className="rounded-md border border-border/60 px-2 py-1">
                  <div className="text-[9px] uppercase tracking-wide text-muted-foreground">Yaw</div>
                  <div className="font-semibold tabular-nums">{fmt(activeVehicle?.yaw_deg, 0)}°</div>
                </div>
              </div>
            </CardContent>
          </Card>

          {/* instrument widgets */}
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="flex items-center gap-2 text-sm">
                <Gauge className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                Instruments
              </CardTitle>
              <CardDescription className="text-[11px]">live telemetry from the active vehicle</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              <BatteryWidget v={activeVehicle} />
              <SignalWidget v={activeVehicle} conn={op.conn} />
              <ModeWidget v={activeVehicle} />
              <GpsWidget v={activeVehicle} />
              <EkfWidget v={activeVehicle} bad={ekfBad} />
            </CardContent>
          </Card>

          {/* pre-arm checklist */}
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="flex items-center gap-2 text-sm">
                <ShieldCheck className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                Pre-arm checklist
              </CardTitle>
              <CardDescription className="text-[11px]">
                GET /api/vehicles/{'{i}'}/prearm-checks · :8400
              </CardDescription>
            </CardHeader>
            <CardContent>
              <PrearmChecklist fly={fly} onArm={doArm} busy={busy} missionFlying={missionFlying} />
            </CardContent>
          </Card>
        </div>

        {/* --------------------------------------------- center col: map + bar */}
        <div className="flex flex-col gap-4 xl:col-span-2">
          <Card>
            <CardHeader className="pb-2">
              <div className="flex flex-wrap items-center gap-2">
                <CardTitle className="flex items-center gap-2 text-sm">
                  <Crosshair className="size-4 text-sky-600 dark:text-sky-400" aria-hidden="true" />
                  Live map — click to Go To (active vehicle)
                </CardTitle>
                <span className="ml-auto hidden font-mono text-[11px] text-muted-foreground md:inline">
                  {vehicles.length} vehicle{vehicles.length === 1 ? '' : 's'} · fence{' '}
                  {fence ? `±${Math.abs(fence.points[1][0]).toFixed(0)} m / ${fence.ceiling_m.toFixed(0)} m` : '—'}
                </span>
              </div>
            </CardHeader>
            <CardContent className="p-2">
              <div className="h-[460px] w-full overflow-hidden rounded-md border border-border sm:h-[520px]">
                <GeoMap
                  origin={op.origin}
                  fenceGeo={op.fenceGeo}
                  fenceCeilingM={fence?.ceiling_m ?? 60}
                  vehicles={markers}
                  trails={op.trails}
                  trailColors={trailColors}
                  waypoints={[]}
                  planMode={false}
                  followKey={follow && activeVehicle ? activeVehicle.id : null}
                  onMapClick={onMapClick}
                  onWaypointDrag={() => {}}
                  onWaypointRemove={() => {}}
                  onTilesOffline={() => op.setTilesOffline(true)}
                />
              </div>
              {gotoPending && (
                <GotoConfirmBar
                  lat={gotoPending.lat}
                  lng={gotoPending.lng}
                  alt={gotoAlt}
                  onAltChange={setGotoAlt}
                  busy={busy}
                  disabled={!activeVehicle}
                  onConfirm={() => {
                    const g = gotoPending
                    setGotoPending(null)
                    if (activeVehicle) {
                      void cmd(`Go To ${g.lat.toFixed(5)}, ${g.lng.toFixed(5)}`, () =>
                        op.goto(fly.activeVehicleId, g.lat, g.lng, gotoAlt),
                      )
                    }
                  }}
                  onCancel={() => setGotoPending(null)}
                />
              )}
            </CardContent>
          </Card>

          {/* action bar */}
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="flex items-center gap-2 text-sm">
                <Zap className="size-4 text-amber-600 dark:text-amber-400" aria-hidden="true" />
                Action bar
                {activeVehicle && (
                  <Badge variant="outline" className="ml-auto gap-1.5 font-mono text-[10px]">
                    active: {activeVehicle.id}
                    {activeVehicle.armed ? (
                      <span className="text-amber-600 dark:text-amber-400">· ARMED</span>
                    ) : (
                      <span className="text-muted-foreground">· disarmed</span>
                    )}
                  </Badge>
                )}
              </CardTitle>
              <CardDescription className="text-[11px]">
                the same MAVLink the supervisor itself sends — every button posts through :8400
              </CardDescription>
            </CardHeader>
            <CardContent>
              <ActionBar
                fly={fly}
                busy={busy}
                missionFlying={missionFlying}
                ceilingM={fence?.ceiling_m ?? 60}
                onArm={doArm}
                onDisarm={() =>
                  activeVehicle && void cmd('DISARM', () => fly.disarm(activeVehicle.index))
                }
                onTakeoff={(altM) =>
                  activeVehicle &&
                  void cmd('TAKEOFF', () => fly.takeoff(activeVehicle.index, altM))
                }
                onLand={() => activeVehicle && void cmd('LAND', () => fly.land(activeVehicle.index))}
                onRtl={() => activeVehicle && void cmd('RTL', () => fly.rtl(activeVehicle.index))}
                onHold={() => activeVehicle && void cmd('HOLD', () => fly.hold(activeVehicle.index))}
                onStartMission={() => void cmd('Start mission', () => fly.startMission())}
              />
            </CardContent>
          </Card>
        </div>

        {/* --------------------------------------------- right col: selector + strip */}
        <div className="flex flex-col gap-4 xl:col-span-1">
          {/* vehicle selector */}
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="flex items-center gap-2 text-sm">
                <Waypoints className="size-4 text-sky-600 dark:text-sky-400" aria-hidden="true" />
                Active vehicle
              </CardTitle>
              <CardDescription className="text-[11px]">
                instruments + action bar follow this selection
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              <Select
                value={String(fly.activeVehicleId)}
                onValueChange={(v) => fly.setActiveVehicleId(Number(v))}
                aria-label="Select active vehicle"
              >
                <SelectTrigger className="h-9 w-full text-xs">
                  <SelectValue placeholder="select a vehicle" />
                </SelectTrigger>
                <SelectContent>
                  {vehicles.map((v) => (
                    <SelectItem key={v.id} value={String(v.index)} className="text-xs">
                      <span className="flex items-center gap-2">
                        <span
                          className={cn(
                            'inline-block size-2 rounded-full',
                            v.armed ? 'bg-amber-500' : 'bg-muted-foreground/40',
                          )}
                          aria-hidden="true"
                        />
                        <span className="font-mono">{v.id}</span>
                        <span className="text-muted-foreground">
                          V{v.index + 1} · {v.fsm} · {v.battery_pct.toFixed(0)}%
                        </span>
                      </span>
                    </SelectItem>
                  ))}
                  {vehicles.length === 0 && (
                    <div className="px-2 py-1.5 text-xs text-muted-foreground">no vehicles in snapshot</div>
                  )}
                </SelectContent>
              </Select>
              <ScrollArea className="max-h-72 rounded-md border border-border">
                <div className="divide-y divide-border">
                  {vehicles.map((v) => {
                    const isActive = v.index === fly.activeVehicleId
                    return (
                      <button
                        key={v.id}
                        type="button"
                        onClick={() => fly.setActiveVehicleId(v.index)}
                        aria-pressed={isActive}
                        className={cn(
                          'flex w-full items-center gap-2 px-2.5 py-2 text-left text-xs transition-colors hover:bg-accent',
                          isActive && 'bg-amber-500/10',
                        )}
                      >
                        <span
                          className={cn(
                            'inline-block size-2.5 shrink-0 rounded-full',
                            v.armed ? 'bg-amber-500' : 'bg-muted-foreground/40',
                          )}
                          aria-hidden="true"
                        />
                        <span className="min-w-0 flex-1 font-mono">
                          <span className={cn('font-semibold', isActive && 'text-amber-600 dark:text-amber-400')}>
                            {v.id}
                          </span>
                          <span className="ml-1.5 text-muted-foreground">V{v.index + 1}</span>
                        </span>
                        <span className="font-mono text-[10px] text-muted-foreground">{v.fsm}</span>
                        <span className="font-mono text-[10px] tabular-nums text-muted-foreground">
                          {v.battery_pct.toFixed(0)}%
                        </span>
                      </button>
                    )
                  })}
                </div>
              </ScrollArea>
            </CardContent>
          </Card>

          {/* strip charts */}
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="flex items-center gap-2 text-sm">
                <Activity className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                Strip charts
                <span className="ml-auto font-mono text-[10px] text-muted-foreground">
                  60 s @ ~10 Hz · {fly.series.alt.length} samples
                </span>
              </CardTitle>
              <CardDescription className="text-[11px]">
                altitude · battery · ground speed — for {activeVehicle?.id ?? '—'}
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              <div>
                <p className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">Altitude AGL</p>
                <StripChart
                  series={[{ label: 'alt', color: EMERALD, data: fly.series.alt }]}
                  unit="m"
                  minV={0}
                  ariaLabel="Altitude AGL strip chart for the active vehicle"
                  className="h-28 sm:h-32"
                />
              </div>
              <div>
                <p className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                  Battery %
                </p>
                <StripChart
                  series={[{ label: 'batt', color: AMBER, data: fly.series.battery }]}
                  unit="%"
                  minV={0}
                  maxV={100}
                  ariaLabel="Battery percentage strip chart for the active vehicle"
                  className="h-28 sm:h-32"
                />
              </div>
              <div>
                <p className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                  Ground speed
                </p>
                <StripChart
                  series={[{ label: 'gs', color: SKY, data: fly.series.velocity }]}
                  unit="m/s"
                  minV={0}
                  ariaLabel="Ground speed strip chart for the active vehicle"
                  className="h-28 sm:h-32"
                />
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Attitude HUD — pure SVG artificial horizon
// ---------------------------------------------------------------------------

function AttitudeHud({
  rollDeg,
  pitchDeg,
  yawDeg,
  dim,
}: {
  rollDeg: number
  pitchDeg: number
  yawDeg: number
  dim?: boolean
}) {
  const R = 80
  const pitchPx = clamp(pitchDeg, -45, 45) * 1.2 // 1° = 1.2 px on the inner horizon
  const ladder = [-30, -20, -10, 10, 20, 30]
  const rollTicks = [-60, -45, -30, -15, 15, 30, 45, 60]

  return (
    <svg
      viewBox="-100 -100 200 200"
      className={cn('h-full w-full', dim && 'opacity-40')}
      role="img"
      aria-label={`Attitude indicator: roll ${rollDeg.toFixed(0)} degrees, pitch ${pitchDeg.toFixed(0)} degrees, heading ${yawDeg.toFixed(0)} degrees`}
    >
      <defs>
        <clipPath id="rsim-ah-clip">
          <circle cx="0" cy="0" r={R} />
        </clipPath>
      </defs>
      {/* bezel ring */}
      <circle cx="0" cy="0" r={R + 8} fill="none" stroke="currentColor" strokeOpacity="0.25" strokeWidth="2" />
      <circle cx="0" cy="0" r={R} fill="hsl(var(--background, 0 0% 100%))" fillOpacity="0.4" />

      {/* rotating horizon (clipped to the bezel circle) */}
      <g clipPath="url(#rsim-ah-clip)">
        <g transform={`rotate(${-rollDeg})`}>
          <g transform={`translate(0 ${pitchPx})`}>
            {/* sky */}
            <rect x="-200" y="-200" width="400" height="200" fill="#3b82f6" opacity="0.78" />
            {/* ground */}
            <rect x="-200" y="0" width="400" height="200" fill="#78350f" opacity="0.88" />
            {/* horizon line */}
            <line x1="-120" y1="0" x2="120" y2="0" stroke="white" strokeWidth="1.5" />
            {/* pitch ladder */}
            {ladder.map((p) => {
              // ladder mark at pitch p°: appears at y = -p*1.2 in the rotated/translated frame
              // (positive p means nose-up → the mark sits ABOVE the horizon line, i.e. y<0)
              const y = -p * 1.2
              const w = Math.abs(p) >= 20 ? 22 : 12
              return (
                <g key={p}>
                  <line
                    x1={-w}
                    y1={y}
                    x2={w}
                    y2={y}
                    stroke="white"
                    strokeWidth="0.8"
                    strokeOpacity="0.75"
                  />
                  <text x={w + 3} y={y + 3} fontSize="7" fill="white" fillOpacity="0.85" fontFamily="ui-monospace, monospace">
                    {Math.abs(p)}
                  </text>
                  <text x={-w - 10} y={y + 3} fontSize="7" fill="white" fillOpacity="0.85" fontFamily="ui-monospace, monospace">
                    {Math.abs(p)}
                  </text>
                </g>
              )
            })}
          </g>
        </g>
      </g>

      {/* roll arc ticks (fixed bezel, not clipped) */}
      <g stroke="currentColor" strokeOpacity="0.55" strokeWidth="1">
        {rollTicks.map((r) => {
          const rad = ((r - 90) * Math.PI) / 180
          const r1 = R + 1
          const r2 = R + 7
          return (
            <line
              key={r}
              x1={r1 * Math.cos(rad)}
              y1={r1 * Math.sin(rad)}
              x2={r2 * Math.cos(rad)}
              y2={r2 * Math.sin(rad)}
            />
          )
        })}
      </g>
      {/* top index (always at 0° roll on the bezel) */}
      <polygon points={`0,${-R - 9} -4,${-R - 2} 4,${-R - 2}`} fill="currentColor" fillOpacity="0.8" />

      {/* roll pointer (rotates with the inner horizon) */}
      <g transform={`rotate(${-rollDeg})`}>
        <polygon points={`0,${-R + 2} -3,${-R + 9} 3,${-R + 9}`} fill={AMBER} />
      </g>

      {/* fixed aircraft symbol (always centered, drawn last) */}
      <g stroke="white" strokeWidth="2.5" fill="none" strokeLinecap="round" strokeLinejoin="round">
        <line x1="-32" y1="0" x2="-10" y2="0" />
        <line x1="10" y1="0" x2="32" y2="0" />
        <line x1="-10" y1="0" x2="-10" y2="6" />
        <line x1="10" y1="0" x2="10" y2="6" />
        <circle cx="0" cy="0" r="2.2" fill="white" stroke="none" />
      </g>

      {/* heading readout at the top */}
      <text
        x="0"
        y={-R - 14}
        fontSize="9"
        fontFamily="ui-monospace, monospace"
        fill="currentColor"
        fillOpacity="0.9"
        textAnchor="middle"
      >
        HDG {String(Math.round(yawDeg)).padStart(3, '0')}°
      </text>
    </svg>
  )
}

// ---------------------------------------------------------------------------
// Instrument widgets
// ---------------------------------------------------------------------------

function BatteryWidget({ v }: { v: FleetVehicle | null }) {
  const pct = v?.battery_pct ?? null
  const tone = batteryTone(pct)
  const barColor =
    tone === 'crit' ? '[&>div]:bg-rose-500' : tone === 'warn' ? '[&>div]:bg-amber-500' : '[&>div]:bg-emerald-500'
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between text-[11px]">
        <span className="flex items-center gap-1.5 font-medium uppercase tracking-wide text-muted-foreground">
          <BatteryFull className="size-3.5" aria-hidden="true" />
          Battery
        </span>
        <span
          className={cn(
            'font-mono font-semibold tabular-nums',
            tone === 'crit'
              ? 'text-rose-600 dark:text-rose-400'
              : tone === 'warn'
                ? 'text-amber-600 dark:text-amber-400'
                : 'text-foreground',
          )}
        >
          {fmt(pct, 1)}%
        </span>
      </div>
      <Progress value={clamp(pct ?? 0, 0, 100)} className={`h-2 ${barColor}`} aria-label={`Battery ${fmt(pct, 0)} percent`} />
      <div className="flex items-center justify-between font-mono text-[10px] text-muted-foreground">
        <span>{tone === 'crit' ? 'critical reserve' : tone === 'warn' ? 'low' : 'nominal'}</span>
        <span>{fmt(v?.voltage_v, 1)} V</span>
      </div>
    </div>
  )
}

function SignalWidget({ v, conn }: { v: FleetVehicle | null; conn: 'connecting' | 'live' | 'simulated' }) {
  const hbAge = v?.heartbeat_age_s ?? null
  const stale = v?.stale ?? false
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between text-[11px]">
        <span className="flex items-center gap-1.5 font-medium uppercase tracking-wide text-muted-foreground">
          <RadioTower className="size-3.5" aria-hidden="true" />
          Link / signal
        </span>
        <Badge
          variant="outline"
          className={cn(
            'gap-1 font-mono text-[10px]',
            conn === 'live'
              ? 'border-emerald-600/40 text-emerald-600 dark:text-emerald-400'
              : conn === 'simulated'
                ? 'border-amber-600/40 text-amber-600 dark:text-amber-400'
                : 'text-muted-foreground',
          )}
        >
          {conn === 'live' ? 'LIVE' : conn === 'simulated' ? 'SIMULATED' : 'CONNECTING'}
        </Badge>
      </div>
      <div className="flex items-center justify-between font-mono text-[10px] text-muted-foreground">
        <span>
          heartbeat age <span className="text-foreground">{fmt(hbAge, 2)} s</span>
        </span>
        <span className={cn(stale ? 'text-rose-600 dark:text-rose-400' : 'text-emerald-600 dark:text-emerald-400')}>
          {stale ? 'STALE' : 'fresh'}
        </span>
      </div>
    </div>
  )
}

function ModeWidget({ v }: { v: FleetVehicle | null }) {
  const mode = v?.mode ?? '—'
  const fsm = v?.fsm ?? '—'
  const armed = v?.armed ?? false
  const tone =
    fsm === 'ACTIVE' || fsm === 'RTL'
      ? 'text-amber-600 dark:text-amber-400'
      : fsm === 'FAULT'
        ? 'text-rose-600 dark:text-rose-400'
        : fsm === 'LANDED' || fsm === 'READY'
          ? 'text-emerald-600 dark:text-emerald-400'
          : 'text-foreground'
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between text-[11px]">
        <span className="flex items-center gap-1.5 font-medium uppercase tracking-wide text-muted-foreground">
          <Compass className="size-3.5" aria-hidden="true" />
          Mode
        </span>
        <Badge
          variant="outline"
          className={cn(
            'gap-1 font-mono text-[10px]',
            armed ? 'border-amber-600/40 text-amber-600 dark:text-amber-400' : 'text-muted-foreground',
          )}
        >
          {armed ? 'ARMED' : 'DISARMED'}
        </Badge>
      </div>
      <div className="flex items-center justify-between font-mono text-xs">
        <span className={cn('font-semibold', tone)}>{mode}</span>
        <span className="text-[10px] text-muted-foreground">fsm: {fsm}</span>
      </div>
    </div>
  )
}

function GpsWidget({ v }: { v: FleetVehicle | null }) {
  const lat = v?.lat ?? null
  const lon = v?.lon ?? null
  const alt = v?.alt_agl_m ?? null
  const fix = lat != null && lon != null
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between text-[11px]">
        <span className="flex items-center gap-1.5 font-medium uppercase tracking-wide text-muted-foreground">
          <Satellite className="size-3.5" aria-hidden="true" />
          GPS
        </span>
        <span
          className={cn(
            'font-mono text-[10px]',
            fix ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-600 dark:text-rose-400',
          )}
        >
          {fix ? '3D FIX' : 'NO FIX'}
        </span>
      </div>
      <div className="grid grid-cols-2 gap-x-3 gap-y-0.5 font-mono text-[10px] text-muted-foreground">
        <span>
          lat <span className="text-foreground">{lat != null ? lat.toFixed(6) : '—'}</span>
        </span>
        <span>
          lon <span className="text-foreground">{lon != null ? lon.toFixed(6) : '—'}</span>
        </span>
        <span>
          AGL <span className="text-foreground">{alt != null ? `${alt.toFixed(1)} m` : '—'}</span>
        </span>
        <span>
          MSL{' '}
          <span className="text-foreground">{v?.alt_msl_m != null ? `${v.alt_msl_m.toFixed(1)} m` : '—'}</span>
        </span>
      </div>
    </div>
  )
}

function EkfWidget({ v, bad }: { v: FleetVehicle | null; bad: boolean }) {
  const health = v?.health ?? []
  const flags = health.length === 0 ? [] : health
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between text-[11px]">
        <span className="flex items-center gap-1.5 font-medium uppercase tracking-wide text-muted-foreground">
          <Gauge className="size-3.5" aria-hidden="true" />
          EKF / health
        </span>
        <span
          className={cn(
            'font-mono text-[10px]',
            bad ? 'text-rose-600 dark:text-rose-400' : 'text-emerald-600 dark:text-emerald-400',
          )}
        >
          {bad ? 'DEGRADED' : 'OK'}
        </span>
      </div>
      <div className="flex flex-wrap gap-1">
        {flags.length === 0 ? (
        <span className="font-mono text-[10px] text-muted-foreground">no health flags</span>
        ) : (
          flags.map((f) => (
            <Badge
              key={f}
              variant="outline"
              className={cn(
                'font-mono text-[9px]',
                f === 'LINK_STALE' || f === 'HEARTBEAT_LOST'
                  ? 'border-rose-600/40 text-rose-600 dark:text-rose-400'
                  : f === 'BATTERY_CRIT'
                    ? 'border-rose-600/40 text-rose-600 dark:text-rose-400'
                    : f === 'BATTERY_LOW' || f === 'GEOFENCE_WARN'
                      ? 'border-amber-600/40 text-amber-600 dark:text-amber-400'
                      : 'text-muted-foreground',
              )}
            >
              {f}
            </Badge>
          ))
        )}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Pre-arm checklist
// ---------------------------------------------------------------------------

function PrearmChecklist({
  fly,
  onArm,
  busy,
  missionFlying,
}: {
  fly: FlyViewApi
  onArm: () => void
  busy: boolean
  missionFlying: boolean
}) {
  const checks = fly.prearm?.checks ?? []
  const allPassed = fly.prearm?.all_passed ?? false
  const unavailable = fly.prearm?.unavailable === true
  const error = fly.prearm?.error
  const hasRun = checks.length > 0
  const armDisabled =
    busy || missionFlying || unavailable || (hasRun && !allPassed)

  const tooltip = missionFlying
    ? 'vehicle is flying an autonomous mission'
    : unavailable
      ? 'pre-arm endpoint unavailable (404) — use the action-bar ARM at your own risk'
      : hasRun && !allPassed
        ? 'one or more pre-arm checks failed — re-run after resolving'
        : 'run pre-arm checks, then ARM'

  // The 5 spec checks; map the backend's response onto them so the panel
  // always shows the same rows even when the backend hasn't shipped all 5.
  const specChecks = ['EKF2', 'GPS Fix', 'Mode', 'Fence', 'Battery']
  const byName = new Map(checks.map((c) => [c.name, c]))

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1.5">
        {specChecks.map((name) => {
          const c = byName.get(name)
          const passed = c?.passed
          const message = c?.message
          const toneCls =
            passed == null
              ? 'border-border/60 bg-muted/30 text-muted-foreground'
              : passed
                ? 'border-emerald-600/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400'
                : 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
          const Icon = passed == null ? Square : passed ? ShieldCheck : TriangleAlert
          return (
            <div
              key={name}
              className={cn(
                'flex items-center gap-2 rounded-md border px-2 py-1.5 text-[11px]',
                toneCls,
              )}
              title={message ?? 'not yet run'}
            >
              <Icon className="size-3.5 shrink-0" aria-hidden="true" />
              <span className="min-w-0 flex-1 truncate font-medium">{name}</span>
              <span className="truncate font-mono text-[10px] opacity-80">
                {message ?? '—'}
              </span>
            </div>
          )
        })}
      </div>

      {unavailable && (
        <div className="flex items-start gap-2 rounded-md border border-amber-600/40 bg-amber-500/10 px-2.5 py-1.5 text-[11px] text-amber-700 dark:text-amber-400">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          <span>
            Pre-arm checks endpoint returned <span className="font-mono">404</span> — the parallel agent has not
            deployed <span className="font-mono">GET /api/vehicles/{'{i}'}/prearm-checks</span> yet. Use the action
            bar&apos;s ARM button to bypass at your own risk.
          </span>
        </div>
      )}
      {error && !unavailable && (
        <div className="flex items-start gap-2 rounded-md border border-rose-600/40 bg-rose-500/10 px-2.5 py-1.5 text-[11px] text-rose-700 dark:text-rose-400">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          <span className="font-mono">{error}</span>
        </div>
      )}

      <div className="flex items-center gap-2">
        <Button
          size="sm"
          variant="outline"
          className="gap-1.5"
          disabled={busy || fly.prearmLoading || !fly.activeVehicle}
          onClick={() => fly.activeVehicle && void fly.runPrearmChecks(fly.activeVehicle.index)}
        >
          {fly.prearmLoading ? (
            <Activity className="size-3.5 animate-pulse" aria-hidden="true" />
          ) : (
            <ShieldCheck className="size-3.5" aria-hidden="true" />
          )}
          Run checks
        </Button>
        <Button
          size="sm"
          className="ml-auto gap-1.5 bg-emerald-600 text-white hover:bg-emerald-600/90"
          disabled={armDisabled}
          title={tooltip}
          onClick={onArm}
        >
          <PlaneTakeoff className="size-3.5" aria-hidden="true" />
          ARM
        </Button>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Action bar
// ---------------------------------------------------------------------------

function ActionBar({
  fly,
  busy,
  missionFlying,
  ceilingM,
  onArm,
  onDisarm,
  onTakeoff,
  onLand,
  onRtl,
  onHold,
  onStartMission,
}: {
  fly: FlyViewApi
  busy: boolean
  missionFlying: boolean
  ceilingM: number
  onArm: () => void
  onDisarm: () => void
  onTakeoff: (altM: number) => void
  onLand: () => void
  onRtl: () => void
  onHold: () => void
  onStartMission: () => void
}) {
  const [takeoffAlt, setTakeoffAlt] = useState(12)
  const v = fly.activeVehicle
  const armed = v?.armed ?? false

  return (
    <div className="flex flex-wrap items-center gap-2">
      {/* ARM / DISARM state-dependent */}
      {armed ? (
        <Button
          size="sm"
          variant="destructive"
          className="gap-1.5"
          disabled={busy || !v || missionFlying}
          title="COMPONENT_ARM_DISARM(0)"
          onClick={onDisarm}
        >
          <Square className="size-3.5" aria-hidden="true" />
          DISARM
        </Button>
      ) : (
        <Button
          size="sm"
          className="gap-1.5 bg-emerald-600 text-white hover:bg-emerald-600/90"
          disabled={busy || !v || missionFlying}
          title={missionFlying ? 'vehicle is flying an autonomous mission' : 'COMPONENT_ARM_DISARM(1) — runs pre-arm checks first'}
          onClick={onArm}
        >
          <PlaneTakeoff className="size-3.5" aria-hidden="true" />
          ARM
        </Button>
      )}

      {/* takeoff with altitude input */}
      <div className="flex items-center gap-1.5">
        <Button
          size="sm"
          variant="outline"
          className="gap-1.5"
          disabled={busy || !v || armed || missionFlying}
          title="MAV_CMD_NAV_TAKEOFF"
          onClick={() => onTakeoff(Math.min(takeoffAlt, ceilingM))}
        >
          <Mountain className="size-3.5" aria-hidden="true" />
          TAKEOFF
        </Button>
        <Label className="flex items-center gap-1 text-[11px] text-muted-foreground">
          <span className="sr-only">Takeoff altitude AGL metres</span>
          <Input
            type="number"
            min={1}
            max={ceilingM}
            value={takeoffAlt}
            onChange={(e) => setTakeoffAlt(Number(e.target.value) || 12)}
            className="h-8 w-16 text-xs"
            aria-label="takeoff altitude AGL metres"
          />
          <span>m</span>
        </Label>
      </div>

      <Button
        size="sm"
        variant="outline"
        className="gap-1.5"
        disabled={busy || !v || missionFlying}
        title="AUTO.LAND + stream stop"
        onClick={onLand}
      >
        <Landmark className="size-3.5" aria-hidden="true" />
        LAND
      </Button>
      <Button
        size="sm"
        variant="outline"
        className="gap-1.5"
        disabled={busy || !v || missionFlying}
        title="AUTO.RTL + stream stop"
        onClick={onRtl}
      >
        <RotateCcw className="size-3.5" aria-hidden="true" />
        RTL
      </Button>
      <Button
        size="sm"
        variant="outline"
        className="gap-1.5"
        disabled={busy || !v || missionFlying}
        title="AUTO.LOITER (pause) + stream stop"
        onClick={onHold}
      >
        <Compass className="size-3.5" aria-hidden="true" />
        HOLD
      </Button>
      <Button
        size="sm"
        variant="secondary"
        className="ml-auto gap-1.5"
        disabled={busy || !v || !armed || missionFlying}
        title={armed ? 'MAV_CMD_MISSION_START — flies the uploaded plan' : 'arm first, then start mission'}
        onClick={onStartMission}
      >
        <Play className="size-3.5" aria-hidden="true" />
        START MISSION
      </Button>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Go-To confirm bar (QGC map position action, same pattern as OperatorMap)
// ---------------------------------------------------------------------------

function GotoConfirmBar({
  lat,
  lng,
  alt,
  onAltChange,
  busy,
  disabled,
  onConfirm,
  onCancel,
}: {
  lat: number
  lng: number
  alt: number
  onAltChange: (v: number) => void
  busy: boolean
  disabled: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  return (
    <div
      className="mt-2 flex flex-wrap items-center gap-2 rounded-md border border-sky-500/40 bg-sky-500/10 px-3 py-2"
      role="group"
      aria-label="Go to location confirm"
    >
      <Send className="size-3.5 text-sky-600 dark:text-sky-400" aria-hidden="true" />
      <span className="font-mono text-xs">
        Go To {lat.toFixed(6)}, {lng.toFixed(6)}
      </span>
      <Label className="ml-2 flex items-center gap-1 text-xs">
        <Mountain className="size-3" aria-hidden="true" />
        AGL
        <Input
          type="number"
          min={1}
          max={60}
          value={alt}
          onChange={(e) => onAltChange(Number(e.target.value) || 10)}
          className="h-7 w-16 text-xs"
          aria-label="go-to altitude AGL metres"
        />
        m
      </Label>
      <span className="hidden text-[11px] text-muted-foreground sm:inline">
        active vehicle flies there and holds
      </span>
      <div className="ml-auto flex gap-1.5">
        <Button size="sm" className="gap-1.5" disabled={busy || disabled} onClick={onConfirm}>
          Go
        </Button>
        <Button size="sm" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  )
}
