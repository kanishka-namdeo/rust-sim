'use client'

/**
 * Mission Bindings panel — the §5.4 / §8.4 step-1..4 surface.
 *
 * One row per vehicle in the fleet snapshot. Each row's "Assign" Select lists
 * saved missions from the catalog (:8300); picking one flips the row to
 * "assigned" (optimistic local state — the binding is persisted when the
 * operator hits "Upload all"). "Upload all" POSTs every binding to
 * `/api/fleet/mission-bindings` on :8400; the backend uploads each mission
 * to its vehicle via the catalog's mission-upload path and returns the
 * refreshed bindings with `binding_state="uploaded"`.
 */

import { useCallback, useEffect, useState } from 'react'
import { Loader2, RefreshCw, Upload, X } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useToast } from '@/hooks/use-toast'
import { fetchGw, gw, unwrapEnvelope } from '@/lib/conn'
import type { FleetC2Api } from '@/hooks/useFleetC2'
import type { FleetVehicle, MissionBinding, MissionBindingState } from '@/lib/types'

const CATALOG_PORT = 8300

/** Catalog mission summary row (subset of PlanMissionSummary — kept local so
 * the panel doesn't have to mount the full usePlanCatalog hook + its 12 s
 * polling loop just to populate a dropdown). */
interface CatalogMission {
  id: string
  name: string
  waypoint_count: number
}

/** State-badge color tokens (kept in lock-step with the fleet phase palette). */
function bindingTone(state: MissionBindingState): string {
  switch (state) {
    case 'unassigned':
      return 'border-border bg-muted text-muted-foreground'
    case 'assigned':
      return 'border-amber-600/40 bg-amber-500/10 text-amber-700 dark:text-amber-400'
    case 'uploaded':
      return 'border-sky-600/40 bg-sky-500/10 text-sky-700 dark:text-sky-400'
    case 'active':
      return 'border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400'
    case 'complete':
      return 'border-emerald-700/40 bg-emerald-700/10 text-emerald-800 dark:text-emerald-300'
    case 'aborted':
      return 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
    default:
      return 'border-border bg-muted text-muted-foreground'
  }
}

export function MissionBindingsPanel({ fleet, vehicles }: { fleet: FleetC2Api; vehicles: FleetVehicle[] }) {
  const { toast } = useToast()
  const [missions, setMissions] = useState<CatalogMission[]>([])
  const [missionsLoading, setMissionsLoading] = useState(false)
  const [uploading, setUploading] = useState(false)
  const [refreshTick, setRefreshTick] = useState(0)

  /** Fetch the saved-mission dropdown from the catalog (:8300). */
  const refreshMissions = useCallback(async () => {
    setMissionsLoading(true)
    try {
      const res = await fetchGw(gw(CATALOG_PORT, '/api/missions'), { method: 'GET' }, 3000)
      if (!res.ok) {
        setMissions([])
        return
      }
      const j = await res.json()
      const data = unwrapEnvelope(j)
      const arr = Array.isArray(data) ? data : []
      const list: CatalogMission[] = arr
        .map((m): CatalogMission | null => {
          if (!m || typeof m !== 'object') return null
          const r = m as Record<string, unknown>
          const id = typeof r.id === 'string' ? r.id : ''
          if (!id) return null
          const name = typeof r.name === 'string' ? r.name : id
          const waypoint_count = typeof r.waypoint_count === 'number' ? r.waypoint_count : 0
          return { id, name, waypoint_count }
        })
        .filter((m): m is CatalogMission => m != null)
      setMissions(list)
    } catch {
      setMissions([])
    } finally {
      setMissionsLoading(false)
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    const tick = async () => {
      if (cancelled) return
      await refreshMissions()
    }
    void tick()
    return () => {
      cancelled = true
    }
  }, [refreshMissions, refreshTick])

  /** Build a row per vehicle, seeded from fleet.bindings when present. */
  const bindingByVehicle = new Map<number, MissionBinding>()
  for (const b of fleet.bindings) bindingByVehicle.set(b.vehicle_id, b)

  const rows = vehicles.map((v) => {
    const idx = v.index
    const b = bindingByVehicle.get(idx)
    return {
      vehicle: v,
      binding: b ?? { vehicle_id: idx, mission_id: '', binding_state: 'unassigned' as MissionBindingState },
    }
  })

  const assignedCount = rows.filter((r) => r.binding.binding_state !== 'unassigned').length
  const uploadedCount = rows.filter((r) => r.binding.binding_state === 'uploaded' || r.binding.binding_state === 'active' || r.binding.binding_state === 'complete').length

  /** Upload all — POST every currently-assigned binding to the fleet plane. */
  const onUploadAll = useCallback(async () => {
    const inputs = rows
      .filter((r) => r.binding.mission_id)
      .map((r) => ({ vehicle_id: r.binding.vehicle_id, mission_id: r.binding.mission_id }))
    if (inputs.length === 0) {
      toast({
        title: 'Nothing to upload',
        description: 'Assign at least one mission before uploading.',
      })
      return
    }
    setUploading(true)
    // Optimistically flip every assigned row to "uploaded" so the UI reacts
    // instantly; the POST response will correct any row that failed.
    fleet.markUploaded(inputs.map((i) => i.vehicle_id))
    try {
      const res = await fleet.setMissionBindings(inputs)
      if (res.ok) {
        toast({
          title: 'Upload complete',
          description: `POST /api/fleet/mission-bindings · ${res.bindings.length} binding(s) uploaded`,
        })
      } else {
        toast({
          title: 'Upload failed',
          description: res.error ?? 'POST /api/fleet/mission-bindings rejected',
          variant: 'destructive',
        })
        // refresh from server to restore accurate state
        void fleet.listMissionBindings()
      }
    } finally {
      setUploading(false)
    }
  }, [fleet, rows, toast])

  /** Per-row assign — optimistic; persisted on the next "Upload all". */
  const onAssign = useCallback(
    (vehicleId: number, missionId: string) => {
      fleet.assignLocal(vehicleId, missionId)
      const m = missions.find((mm) => mm.id === missionId)
      toast({
        title: 'Mission assigned',
        description: `Vehicle ${vehicleId} ← ${m?.name ?? missionId} (Upload all to persist + upload)`,
      })
    },
    [fleet, missions, toast],
  )

  /** Per-row clear — DELETE on :8400 if the binding was already persisted. */
  const onClear = useCallback(
    async (vehicleId: number) => {
      // optimistic local removal
      const ok = await fleet.clearMissionBinding(vehicleId)
      if (!ok) {
        toast({
          title: 'Clear failed',
          description: `DELETE /api/fleet/mission-bindings/${vehicleId} rejected`,
          variant: 'destructive',
        })
      }
    },
    [fleet, toast],
  )

  const live = fleet.conn === 'live'
  const disabled = !live || uploading || fleet.busy

  return (
    <Card>
      <CardHeader className="pb-2">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <CardTitle className="flex items-center gap-2 text-sm">
              Mission Bindings
              <Badge variant="outline" className="font-mono text-[10px]">
                {assignedCount}/{rows.length} assigned · {uploadedCount} uploaded
              </Badge>
            </CardTitle>
            <CardDescription>
              per-vehicle mission binding (§5.4 AC-5.4.1) · Upload all → POST /api/fleet/mission-bindings?XTransformPort=8400
            </CardDescription>
          </div>
          <div className="flex items-center gap-1.5">
            <Button
              variant="ghost"
              size="icon"
              className="size-8"
              aria-label="Refresh saved missions"
              disabled={missionsLoading}
              onClick={() => setRefreshTick((t) => t + 1)}
            >
              {missionsLoading ? (
                <Loader2 className="size-3 animate-spin" aria-hidden="true" />
              ) : (
                <RefreshCw className="size-3" aria-hidden="true" />
              )}
            </Button>
            <Button
              size="sm"
              className="gap-1.5"
              onClick={() => void onUploadAll()}
              disabled={disabled || assignedCount === 0}
              aria-label="Upload all bound missions to their vehicles"
            >
              {uploading ? <Loader2 className="size-3.5 animate-spin" aria-hidden="true" /> : <Upload className="size-3.5" aria-hidden="true" />}
              Upload all
            </Button>
          </div>
        </div>
      </CardHeader>
      <CardContent className="min-w-0 p-0">
        {rows.length === 0 ? (
          <Skeleton className="m-4 h-24 rounded-lg" />
        ) : (
          <div className="max-h-80 overflow-y-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="pl-4">Vehicle</TableHead>
                  <TableHead>Mission</TableHead>
                  <TableHead>Binding state</TableHead>
                  <TableHead className="pr-4 text-right">Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map(({ vehicle, binding }) => (
                  <TableRow key={vehicle.id}>
                    <TableCell className="pl-4">
                      <div className="flex flex-col">
                        <span className="font-mono text-xs font-medium">{vehicle.id}</span>
                        <span className="font-mono text-[10px] text-muted-foreground">
                          sysid {vehicle.sysid} · {vehicle.mode}
                        </span>
                      </div>
                    </TableCell>
                    <TableCell>
                      {binding.mission_id ? (
                        <span className="font-mono text-xs">{binding.mission_id}</span>
                      ) : (
                        <span className="font-mono text-xs text-muted-foreground">—</span>
                      )}
                    </TableCell>
                    <TableCell>
                      <Badge variant="outline" className={`font-mono text-[10px] ${bindingTone(binding.binding_state)}`}>
                        {binding.binding_state}
                      </Badge>
                    </TableCell>
                    <TableCell className="pr-4">
                      <div className="flex items-center justify-end gap-1.5">
                        <Select
                          value={binding.mission_id || undefined}
                          onValueChange={(v) => onAssign(binding.vehicle_id, v)}
                          disabled={disabled}
                        >
                          <SelectTrigger size="sm" className="h-7 w-[160px] gap-1 text-xs" aria-label={`Assign mission to ${vehicle.id}`}>
                            <SelectValue placeholder="Assign …" />
                          </SelectTrigger>
                          <SelectContent>
                            {missions.length === 0 ? (
                              <SelectItem value="__none__" disabled>
                                {missionsLoading ? 'loading …' : 'no saved missions'}
                              </SelectItem>
                            ) : (
                              missions.map((m) => (
                                <SelectItem key={m.id} value={m.id} className="text-xs">
                                  <span className="font-mono">{m.name}</span>
                                  <span className="ml-2 text-[10px] text-muted-foreground">{m.waypoint_count} wp</span>
                                </SelectItem>
                              ))
                            )}
                          </SelectContent>
                        </Select>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-7"
                          aria-label={`Clear mission binding for ${vehicle.id}`}
                          disabled={disabled || !binding.mission_id}
                          onClick={() => void onClear(binding.vehicle_id)}
                        >
                          <X className="size-3.5" aria-hidden="true" />
                        </Button>
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
        {!live && (
          <p className="px-4 pb-3 pt-2 text-[11px] text-muted-foreground">
            fleet backend :8400 offline — bindings stay editable locally and persist on the next live reconnect.
          </p>
        )}
      </CardContent>
    </Card>
  )
}
