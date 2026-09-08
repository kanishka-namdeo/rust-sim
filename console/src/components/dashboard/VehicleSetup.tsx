'use client'

/**
 * Vehicle Setup (ADR-0016) — the QGroundControl / Mission-Planner-style
 * configuration workflow for this fleet: Summary, Airframe, Sensors,
 * Power, Safety, Flight Modes, Parameters.
 *
 * Everything param-derived is the live vehicle's own configuration read
 * back over the param protocol (GET /api/vehicles/{i}/setup), and every
 * write goes through the endpoints (PARAM_SET echo-confirmed / COMMAND_LONG
 * ACKed) — this view never invents state.
 */

import { useCallback, useMemo, useState } from 'react'
import {
  Activity,
  BatteryCharging,
  Compass,
  Gauge,
  ListTree,
  Plane,
  RotateCw,
  Search,
  ShieldAlert,
  Wrench,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Progress } from '@/components/ui/progress'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { useToast } from '@/hooks/use-toast'
import { ConnBadge, ConnSubline } from './ConnBadge'
import { StatTile } from './StatTile'
import { fmt } from '@/lib/format'
import type { VehicleSetupApi } from '@/hooks/useVehicleSetup'
import type { CalSensor } from '@/lib/types'

type Section = 'summary' | 'airframe' | 'sensors' | 'power' | 'safety' | 'modes' | 'params'

export function VehicleSetup({ setup }: { setup: VehicleSetupApi }) {
  const { toast } = useToast()
  const [section, setSection] = useState<Section>('summary')
  const s = setup.summary
  const armed = s?.armed === true

  const runAction = useCallback(
    (label: string, fn: () => Promise<{ ok: boolean; detail?: string }>) => {
      void fn().then((r) => {
        toast({
          title: r.ok ? `${label} — accepted` : `${label} — rejected`,
          description: r.detail ?? (r.ok ? 'ok' : 'no detail'),
          ...(r.ok ? {} : { variant: 'destructive' as const }),
        })
      })
    },
    [toast],
  )

  return (
    <div className="flex flex-col gap-4">
      {/* -------------------------------------------------- setup header */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <CardTitle className="flex flex-wrap items-center gap-2.5 text-base">
                <Wrench className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                Vehicle Setup
                {s && (
                  <Badge variant="outline" className={`font-mono ${s.armed ? 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400' : 'border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400'}`}>
                    {s.armed ? 'ARMED' : 'DISARMED'}
                  </Badge>
                )}
                {s?.restart_pending && (
                  <Badge variant="outline" className="animate-pulse font-mono border-amber-600/40 bg-amber-500/10 text-amber-700 dark:text-amber-400">
                    <RotateCw className="size-3" aria-hidden="true" /> RESTARTING
                  </Badge>
                )}
                <ConnBadge conn={setup.conn} port={setup.port} retryAt={setup.retryAt} lastError={setup.lastError} />
              </CardTitle>
              <div className="mt-1 text-xs text-muted-foreground">
                <ConnSubline conn={setup.conn} port={setup.port} retryAt={setup.retryAt} lastError={setup.lastError} />
              </div>
            </div>
            <div className="flex items-center gap-1.5">
              {Array.from({ length: setup.vehicleCount }, (_, i) => (
                <Button
                  key={i}
                  size="sm"
                  variant={setup.index === i ? 'default' : 'outline'}
                  className="font-mono"
                  disabled={setup.busy}
                  onClick={() => setup.setIndex(i)}
                >
                  V{i + 1}
                </Button>
              ))}
            </div>
          </div>
        </CardHeader>
      </Card>

      {/* ------------------------------------------------------- the rail */}
      <div className="grid min-w-0 gap-4 lg:grid-cols-[13rem_1fr]">
        <Card className="h-fit">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Setup sections</CardTitle>
            <CardDescription>the QGC rail</CardDescription>
          </CardHeader>
          <CardContent className="p-2">
            <Tabs value={section} onValueChange={(v) => setSection(v as Section)} orientation="vertical">
              <TabsList className="grid h-auto w-full grid-cols-1 gap-1 bg-transparent p-0">
                {(
                  [
                    ['summary', 'Summary', <Gauge key="i" className="size-3.5" aria-hidden="true" />],
                    ['airframe', 'Airframe', <Plane key="i" className="size-3.5" aria-hidden="true" />],
                    ['sensors', 'Sensors', <Compass key="i" className="size-3.5" aria-hidden="true" />],
                    ['power', 'Power', <BatteryCharging key="i" className="size-3.5" aria-hidden="true" />],
                    ['safety', 'Safety', <ShieldAlert key="i" className="size-3.5" aria-hidden="true" />],
                    ['modes', 'Flight Modes', <ListTree key="i" className="size-3.5" aria-hidden="true" />],
                    ['params', 'Parameters', <Activity key="i" className="size-3.5" aria-hidden="true" />],
                  ] as [Section, string, React.ReactNode][]
                ).map(([v, label, icon]) => (
                  <TabsTrigger
                    key={v}
                    value={v}
                    className="flex w-full justify-start gap-2 px-3 py-2 data-[state=active]:bg-muted"
                  >
                    {icon}
                    <span className="text-xs">{label}</span>
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
          </CardContent>
        </Card>

        <Card className="min-w-0">
          <Tabs value={section}>
            <TabsContent value="summary" className="mt-0">
              <SummarySection setup={setup} />
            </TabsContent>
            <TabsContent value="airframe" className="mt-0">
              <AirframeSection setup={setup} onApply={(id) => runAction('Airframe apply', () => setup.applyAirframe(id))} />
            </TabsContent>
            <TabsContent value="sensors" className="mt-0">
              <SensorsSection setup={setup} onCalibrate={(sensor) => runAction(`Calibrate ${sensor}`, () => setup.calibrate(sensor))} />
            </TabsContent>
            <TabsContent value="power" className="mt-0">
              <PowerSafetySection
                title="Power (battery)"
                description="BAT_* parameters — cells, thresholds and pack resistance, written PARAM_SET echo-confirmed"
                icon={<BatteryCharging className="size-4" aria-hidden="true" />}
                params={s?.power ?? null}
                armed={armed}
                busy={setup.busy}
                onWrite={(id, v) => runAction(`Write ${id}`, () => setup.writeParam(id, v))}
              />
            </TabsContent>
            <TabsContent value="safety" className="mt-0">
              <PowerSafetySection
                title="Safety / failsafe"
                description="RC-loss, datalink-loss, low-battery and geofence actions — the policy ladder's vehicle-side twin"
                icon={<ShieldAlert className="size-4" aria-hidden="true" />}
                params={s?.safety ?? null}
                armed={armed}
                busy={setup.busy}
                onWrite={(id, v) => runAction(`Write ${id}`, () => setup.writeParam(id, v))}
              />
            </TabsContent>
            <TabsContent value="modes" className="mt-0">
              <ModesSection setup={setup} onSet={(m) => runAction(`Mode ${m}`, () => setup.setMode(m))} />
            </TabsContent>
            <TabsContent value="params" className="mt-0">
              <ParamsSection setup={setup} onRefresh={() => runAction('Parameter download', () => setup.refreshParams())} onWrite={(id, v) => runAction(`Write ${id}`, () => setup.writeParam(id, v))} />
            </TabsContent>
          </Tabs>
        </Card>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

function SummarySection({ setup }: { setup: VehicleSetupApi }) {
  const s = setup.summary
  const cal = s?.calibration
  const p = s?.params
  const calChips: { label: string; ok: boolean | null }[] = cal
    ? [
        { label: 'Accelerometer', ok: cal.accel },
        { label: 'Gyroscope', ok: cal.gyro },
        { label: 'Magnetometer 0', ok: cal.mag0 },
        { label: 'Level horizon', ok: cal.level_horizon },
      ]
    : []

  return (
    <div className="p-4">
      <CardTitle className="text-sm">Vehicle summary</CardTitle>
      <CardDescription className="mb-3 mt-1">
        identity, airframe, calibration and download state — everything read from the live param cache
      </CardDescription>
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-4">
        <StatTile label="Airframe" value={s?.airframe.name ?? '—'} sub={s?.airframe.category || undefined} tone="accent" />
        <StatTile label="SYS_AUTOSTART" value={s?.airframe.sys_autostart ?? '—'} sub={s?.airframe.frame_type || undefined} />
        <StatTile label="Flight mode" value={s?.mode ?? '—'} sub={`FSM ${s?.fsm ?? '—'}`} />
        <StatTile label="Battery" value={`${fmt(s?.battery_pct, 0)}%`} sub={s?.voltage_v != null ? `${fmt(s.voltage_v, 2)} V` : undefined} />
        <StatTile label="Autopilot" value={s?.autopilot.type ?? '—'} sub={s?.autopilot.version || undefined} />
        <StatTile
          label="Parameters"
          value={p ? `${p.received}/${p.total}` : '—'}
          sub={p ? String(p.state) : 'no link yet'}
          tone={p?.state === 'Complete' ? 'ok' : p?.state === 'Stalled' ? 'warn' : 'default'}
        />
        <StatTile
          label="Dynamics"
          value={s ? (s.airframe.dynamics_compatible ? 'compatible' : 'config-only') : '—'}
          sub={s?.airframe.dynamics_compatible ? 'quad HIL can fly it' : 'quad dynamics mismatch (ADR-0016)'}
          tone={s?.airframe.dynamics_compatible ? 'ok' : 'warn'}
        />
        <StatTile label="sysid/compid" value={s ? `${s.sysid}/${s.compid}` : '—'} sub={`V${(s?.index ?? 0) + 1}`} />
      </div>

      {p && p.state !== 'Complete' && (
        <div className="mt-4">
          <div className="mb-1 flex items-center justify-between text-xs text-muted-foreground">
            <span>parameter download</span>
            <span className="font-mono">
              {p.received}/{p.total} · {p.state}
            </span>
          </div>
          <Progress value={p.total > 0 ? (p.received / p.total) * 100 : 0} aria-label="parameter download progress" />
        </div>
      )}

      <div className="mt-4">
        <p className="mb-2 text-xs font-medium text-muted-foreground">Sensor calibration (CAL_*_ID rule)</p>
        <div className="flex flex-wrap gap-1.5">
          {calChips.length === 0 && <span className="text-xs text-muted-foreground">no param download yet — use Parameters → Download</span>}
          {calChips.map((c) => (
            <Badge
              key={c.label}
              variant="outline"
              className={
                c.ok === true
                  ? 'border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400'
                  : c.ok === false
                    ? 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
                    : ''
              }
            >
              {c.label}: {c.ok === true ? 'calibrated' : c.ok === false ? 'not calibrated' : 'unknown'}
            </Badge>
          ))}
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Airframe
// ---------------------------------------------------------------------------

function AirframeSection({
  setup,
  onApply,
}: {
  setup: VehicleSetupApi
  onApply: (id: number) => void
}) {
  const s = setup.summary
  const current = s?.airframe.sys_autostart ?? null
  const [filter, setFilter] = useState('')
  const f = filter.trim().toLowerCase()

  const groups = useMemo(
    () =>
      setup.groups
        .map((g) => ({
          ...g,
          airframes: g.airframes.filter(
            (a) => !f || a.name.toLowerCase().includes(f) || String(a.id).includes(f) || (a.sim_model ?? '').toLowerCase().includes(f),
          ),
        }))
        .filter((g) => g.airframes.length > 0),
    [setup.groups, f],
  )

  return (
    <div className="p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <CardTitle className="text-sm">Airframe (SYS_AUTOSTART)</CardTitle>
          <CardDescription className="mt-1 max-w-xl">
            ROMFS-derived catalog ({setup.groups.reduce((n, g) => n + g.airframes.length, 0)} frames). Apply writes
            SYS_AUTOSTART (echo-confirmed, PX4 autosaves) and restarts the sim+px4 pair — the QGC &quot;Apply and
            Restart&quot; flow. Requires disarmed.
          </CardDescription>
        </div>
        <div className="relative">
          <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
          <Input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="filter frames…"
            className="h-8 w-48 pl-8 text-xs"
            aria-label="filter airframes"
          />
        </div>
      </div>

      {s && (
        <div className="mt-3 rounded-lg border border-border bg-muted/30 p-3">
          <p className="text-xs text-muted-foreground">current</p>
          <p className="mt-0.5 text-sm font-medium">
            {s.airframe.name} <span className="ml-2 font-mono text-xs text-muted-foreground">#{s.airframe.sys_autostart}</span>
            <span className="ml-2 text-xs text-muted-foreground">{s.airframe.category}</span>
            {!s.airframe.dynamics_compatible && (
              <span className="ml-2 rounded bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-700 dark:text-amber-400">
                config-only vs quad HIL
              </span>
            )}
          </p>
        </div>
      )}

      <ScrollArea className="mt-3 h-[26rem] pr-3">
        <div className="flex flex-col gap-4">
          {groups.map((g) => (
            <div key={g.category}>
              <p className="mb-1.5 text-xs font-semibold tracking-wide text-muted-foreground">{g.category}</p>
              <div className="grid gap-1.5 sm:grid-cols-2">
                {g.airframes.map((a) => (
                  <div
                    key={a.id}
                    className={`flex items-center justify-between gap-2 rounded-lg border px-3 py-2 ${
                      current === a.id ? 'border-emerald-600/50 bg-emerald-600/5' : 'border-border'
                    }`}
                  >
                    <div className="min-w-0">
                      <p className="truncate text-xs font-medium">{a.name}</p>
                      <p className="truncate font-mono text-[10px] text-muted-foreground">
                        #{a.id}
                        {a.frame_type ? ` · ${a.frame_type}` : ''}
                        {a.sim_model ? ` · ${a.sim_model}` : ''}
                      </p>
                    </div>
                    <AlertDialog>
                      <AlertDialogTrigger asChild>
                        <Button
                          size="sm"
                          variant={current === a.id ? 'outline' : 'secondary'}
                          className="h-7 shrink-0 gap-1.5 px-2.5 text-[11px]"
                          disabled={setup.busy || s?.armed === true}
                        >
                          <RotateCw className="size-3" aria-hidden="true" />
                          {current === a.id ? 'Re-apply' : 'Apply'}
                        </Button>
                      </AlertDialogTrigger>
                      <AlertDialogContent>
                        <AlertDialogHeader>
                          <AlertDialogTitle>Apply airframe {a.name} (#{a.id})?</AlertDialogTitle>
                          <AlertDialogDescription>
                            Writes SYS_AUTOSTART={a.id} to vehicle V{(setup.index ?? 0) + 1} (PARAM_SET, echo-confirmed)
                            and restarts its sim+px4 pair. PX4 loads the new airframe&apos;s defaults at boot. The vehicle
                            must be disarmed{a.sim_model == null ? '' : ' and re-downloads its parameters afterwards'}.
                          </AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                          <AlertDialogCancel>Cancel</AlertDialogCancel>
                          <AlertDialogAction onClick={(e) => { e.preventDefault(); onApply(a.id) }}>
                            Apply and restart
                          </AlertDialogAction>
                        </AlertDialogFooter>
                      </AlertDialogContent>
                    </AlertDialog>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </ScrollArea>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Sensors
// ---------------------------------------------------------------------------

const CAL_ROWS: { sensor: CalSensor; label: string; help: string }[] = [
  { sensor: 'gyro', label: 'Gyroscope', help: 'MAV_CMD 241 param1=1 — CAL_GYRO0_ID flips nonzero' },
  { sensor: 'accel', label: 'Accelerometer', help: 'MAV_CMD 241 param5=1 — 6-position calibration' },
  { sensor: 'accel_quick', label: 'Accelerometer (quick)', help: 'MAV_CMD 241 param5=4 — single-position' },
  { sensor: 'mag', label: 'Magnetometer', help: 'MAV_CMD 241 param2=1 — CAL_MAG0_ID flips nonzero' },
  { sensor: 'level', label: 'Level horizon', help: 'MAV_CMD 241 param5=2 — board offset / level' },
  { sensor: 'baro', label: 'Barometer', help: 'MAV_CMD 241 param3=1 — ground pressure' },
  { sensor: 'airspeed', label: 'Airspeed', help: 'MAV_CMD 241 param6=2 — pitot zero (planes only)' },
]

function SensorsSection({
  setup,
  onCalibrate,
}: {
  setup: VehicleSetupApi
  onCalibrate: (sensor: CalSensor) => void
}) {
  const s = setup.summary
  const cal = s?.calibration
  const flag = (b: boolean | null) =>
    b === true ? 'calibrated' : b === false ? 'not calibrated' : 'unknown (no download)'
  const flagCls = (b: boolean | null) =>
    b === true
      ? 'border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400'
      : b === false
        ? 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
        : ''

  return (
    <div className="p-4">
      <CardTitle className="text-sm">Sensors &amp; calibration</CardTitle>
      <CardDescription className="mb-3 mt-1 max-w-xl">
        the QGC calibration buttons — MAV_CMD_PREFLIGHT_CALIBRATION (241) with PX4 v1.16 commander&apos;s own param
        matrix. Requires disarmed and an idle vehicle; the CAL_*_ID confirmation lands in the next parameter download.
      </CardDescription>

      <div className="mb-3 flex flex-wrap gap-1.5">
        <Badge variant="outline" className={flagCls(cal?.accel ?? null)}>Accelerometer: {flag(cal?.accel ?? null)}</Badge>
        <Badge variant="outline" className={flagCls(cal?.gyro ?? null)}>Gyroscope: {flag(cal?.gyro ?? null)}</Badge>
        <Badge variant="outline" className={flagCls(cal?.mag0 ?? null)}>Mag 0: {flag(cal?.mag0 ?? null)}</Badge>
        <Badge variant="outline" className={flagCls(cal?.level_horizon ?? null)}>Level: {flag(cal?.level_horizon ?? null)}</Badge>
      </div>

      <div className="flex flex-col gap-1.5">
        {CAL_ROWS.map((r) => (
          <div key={r.sensor} className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border px-3 py-2">
            <div className="min-w-0">
              <p className="text-xs font-medium">{r.label}</p>
              <p className="truncate text-[10px] text-muted-foreground">{r.help}</p>
            </div>
            <Button
              size="sm"
              variant="secondary"
              className="h-7 gap-1.5 px-2.5 text-[11px]"
              disabled={setup.busy || s?.armed === true}
              onClick={() => onCalibrate(r.sensor)}
            >
              <Compass className="size-3" aria-hidden="true" />
              Calibrate
            </Button>
          </div>
        ))}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Power / Safety (param editor sections)
// ---------------------------------------------------------------------------

const POWER_LABELS: Record<string, string> = {
  BAT1_N_CELLS: 'Battery cells',
  BAT1_V_EMPTY: 'Empty cell voltage (V)',
  BAT1_V_CHARGED: 'Full cell voltage (V)',
  BAT_LOW_THR: 'Low-battery threshold',
  BAT_CRIT_THR: 'Critical threshold',
  BAT_EMERGEN_THR: 'Emergency threshold',
  BAT1_R_INTERNAL: 'Internal resistance (Ω)',
}

const SAFETY_LABELS: Record<string, string> = {
  NAV_RCL_ACT: 'RC loss action',
  NAV_DLL_ACT: 'Datalink loss action',
  COM_LOW_BAT_ACT: 'Low-battery action',
  COM_OBL_RC_ACT: 'RC override action',
  GF_ACTION: 'Geofence action',
  GF_MAX_HOR_DIST: 'Geofence max horizontal (m)',
  GF_MAX_VER_DIST: 'Geofence max vertical (m)',
  RTL_RETURN_ALT: 'RTL return altitude (m)',
}

const SAFETY_HINTS: Record<string, string> = {
  NAV_RCL_ACT: '0=none 1=hold 2=rtl 3=land 5=terminate 6=lockdown',
  NAV_DLL_ACT: 'the GCS-connection arming gate (F-2): 0 may block arming',
  COM_LOW_BAT_ACT: '0=warning 1=return 2=land 3=hold',
  GF_ACTION: '0=none 1=warning 2=hold 3=return 4=terminate',
}

function PowerSafetySection({
  title,
  description,
  icon,
  params,
  armed,
  busy,
  onWrite,
}: {
  title: string
  description: string
  icon: React.ReactNode
  params: Record<string, number | null> | null
  armed: boolean
  busy: boolean
  onWrite: (id: string, v: number) => void
}) {
  const labels = title.startsWith('Power') ? POWER_LABELS : SAFETY_LABELS
  const hints = title.startsWith('Power') ? {} : SAFETY_HINTS
  const [drafts, setDrafts] = useState<Record<string, string>>({})

  return (
    <div className="p-4">
      <CardTitle className="flex items-center gap-2 text-sm">
        {icon}
        {title}
      </CardTitle>
      <CardDescription className="mb-3 mt-1 max-w-xl">{description}</CardDescription>

      {params == null && <p className="text-xs text-muted-foreground">no parameter download yet — use Parameters → Download</p>}

      {params != null && (
        <div className="flex flex-col gap-1.5">
          {Object.keys(labels)
            .filter((id) => id in params)
            .map((id) => {
              const value = params[id]
              const draft = drafts[id] ?? (value != null ? String(value) : '')
              const dirty = value != null && draft !== String(value)
              return (
                <div key={id} className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border px-3 py-2">
                  <div className="min-w-0 flex-1">
                    <Label htmlFor={`p-${id}`} className="text-xs font-medium">
                      {labels[id] ?? id}
                    </Label>
                    <p className="truncate font-mono text-[10px] text-muted-foreground">
                      {id}
                      {hints[id] ? ` — ${hints[id]}` : ''}
                    </p>
                  </div>
                  <div className="flex items-center gap-1.5">
                    <Input
                      id={`p-${id}`}
                      inputMode="decimal"
                      className="h-8 w-28 font-mono text-xs"
                      value={draft}
                      disabled={value == null || busy || armed}
                      onChange={(e) => setDrafts((d) => ({ ...d, [id]: e.target.value }))}
                    />
                    <Button
                      size="sm"
                      variant={dirty ? 'default' : 'outline'}
                      className="h-8 px-2.5 text-[11px]"
                      disabled={!dirty || value == null || busy || armed}
                      onClick={() => {
                        const v = Number(draft)
                        if (Number.isFinite(v)) onWrite(id, v)
                      }}
                    >
                      Write
                    </Button>
                  </div>
                </div>
              )
            })}
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Flight modes
// ---------------------------------------------------------------------------

function ModesSection({
  setup,
  onSet,
}: {
  setup: VehicleSetupApi
  onSet: (mode: string) => void
}) {
  const s = setup.summary
  return (
    <div className="p-4">
      <CardTitle className="text-sm">Flight modes</CardTitle>
      <CardDescription className="mb-3 mt-1 max-w-xl">
        DO_SET_MODE with fleet-modes-verified mode words — the same switch the supervisor itself uses. The mode echo
        confirms on the next heartbeat.
      </CardDescription>

      <div className="mb-3 flex items-center gap-2 text-sm">
        <span className="text-xs text-muted-foreground">current:</span>
        <Badge variant="outline" className="font-mono border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400">
          {s?.mode ?? '—'}
        </Badge>
        {s?.mode_word != null && <span className="font-mono text-[10px] text-muted-foreground">0x{(s.mode_word >>> 0).toString(16).toUpperCase().padStart(8, '0')}</span>}
      </div>

      <div className="grid grid-cols-2 gap-1.5 sm:grid-cols-3 lg:grid-cols-4">
        {setup.modes.map((m) => (
          <Button
            key={m.name}
            size="sm"
            variant={s?.mode === m.name ? 'default' : 'outline'}
            className="h-9 font-mono text-[11px]"
            disabled={setup.busy}
            onClick={() => onSet(m.name)}
          >
            {m.name}
          </Button>
        ))}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

function ParamsSection({
  setup,
  onRefresh,
  onWrite,
}: {
  setup: VehicleSetupApi
  onRefresh: () => void
  onWrite: (id: string, v: number) => void
}) {
  const store = setup.paramStore
  const [query, setQuery] = useState('')
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const q = query.trim().toUpperCase()

  const rows = useMemo(() => {
    let list = store?.params ?? []
    if (q) {
      // prefix match like QGC's search, plus infix for convenience
      const pref: typeof list = []
      const inf: typeof list = []
      for (const p of list) {
        if (p.id.startsWith(q)) pref.push(p)
        else if (p.id.includes(q)) inf.push(p)
      }
      list = [...pref, ...inf]
    }
    return list.slice(0, 400)
  }, [store, q])

  const pct = store && store.total > 0 ? Math.min(100, (store.received / store.total) * 100) : 0

  return (
    <div className="p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <CardTitle className="text-sm">Parameters ({store?.params.length ?? 0} cached)</CardTitle>
          <CardDescription className="mt-1 max-w-xl">
            the live PARAM_VALUE cache — every row was echoed by the vehicle itself. Writes are PARAM_SET with
            echo-confirmation; PX4 autosaves so values survive reboots.
          </CardDescription>
        </div>
        <div className="flex items-center gap-1.5">
          <div className="relative">
            <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="search params…"
              className="h-8 w-44 pl-8 font-mono text-xs"
              aria-label="search parameters"
            />
          </div>
          <Button size="sm" variant="secondary" className="h-8 gap-1.5 text-[11px]" disabled={setup.busy} onClick={onRefresh}>
            <RotateCw className="size-3" aria-hidden="true" />
            Download
          </Button>
        </div>
      </div>

      {store && (
        <div className="mt-3">
          <div className="mb-1 flex items-center justify-between text-xs text-muted-foreground">
            <span>
              download {store.state} · {store.received}/{store.total}
            </span>
            <span className="font-mono text-[10px]">{rows.length} shown</span>
          </div>
          <Progress value={pct} aria-label="parameter download progress" />
        </div>
      )}

      <ScrollArea className="mt-3 h-[24rem] pr-3">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="h-8 text-[11px]">id</TableHead>
              <TableHead className="h-8 w-32 text-[11px]">value</TableHead>
              <TableHead className="h-8 w-20 text-[11px]">type</TableHead>
              <TableHead className="h-8 w-24 text-[11px]"></TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((p) => {
              const draft = drafts[p.id] ?? String(p.value)
              const dirty = draft !== String(p.value)
              return (
                <TableRow key={p.id}>
                  <TableCell className="py-1 font-mono text-[11px]">{p.id}</TableCell>
                  <TableCell className="py-1">
                    <Input
                      inputMode="decimal"
                      className="h-7 font-mono text-[11px]"
                      value={draft}
                      disabled={setup.busy}
                      onChange={(e) => setDrafts((d) => ({ ...d, [p.id]: e.target.value }))}
                    />
                  </TableCell>
                  <TableCell className="py-1 font-mono text-[10px] text-muted-foreground">
                    {p.type === 9 ? 'f32' : p.type === 6 ? 'i32' : p.type === 1 ? 'i8' : `t${p.type}`}
                  </TableCell>
                  <TableCell className="py-1">
                    <Button
                      size="sm"
                      variant={dirty ? 'default' : 'ghost'}
                      className="h-7 px-2 text-[10px]"
                      disabled={!dirty || setup.busy}
                      onClick={() => {
                        const v = Number(draft)
                        if (Number.isFinite(v)) onWrite(p.id, v)
                      }}
                    >
                      Write
                    </Button>
                  </TableCell>
                </TableRow>
              )
            })}
            {rows.length === 0 && (
              <TableRow>
                <TableCell colSpan={4} className="py-6 text-center text-xs text-muted-foreground">
                  {store == null || store.params.length === 0
                    ? 'no parameters cached — press Download (PARAM_REQUEST_LIST)'
                    : 'no matches'}
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </ScrollArea>
    </div>
  )
}
