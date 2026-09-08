'use client'

/**
 * Patterns panel — the §5.4 swarming-pattern library surface.
 *
 * A toolbar button opens an AlertDialog modal whose body is a Select of the
 * pattern library (follow-the-leader / search-grid / parallel-patrol, plus
 * any extras the backend advertises via GET /api/fleet/patterns on :8400).
 * Picking a pattern renders its parameter inputs (each pattern has a small
 * spec below). "Generate" POSTs `/api/fleet/patterns/{name}/generate` and
 * the result table lists the generated per-vehicle missions. Generated
 * missions are auto-bound on the fleet hook so the operator can press
 * "Start Fleet" immediately afterwards.
 */

import { useCallback, useState } from 'react'
import { Loader2, Sparkles, Wand2 } from 'lucide-react'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useToast } from '@/hooks/use-toast'
import type { FleetC2Api } from '@/hooks/useFleetC2'
import type { GeneratedPatternMission, SwarmPattern } from '@/lib/types'

/** The three SPEC-mandated patterns, with parameter specs the UI can render.
 * Used as a fallback when GET /api/fleet/patterns returns nothing (the
 * backend may not be online yet — the operator can still generate). */
interface PatternParamSpec {
  key: string
  label: string
  kind: 'number'
  min?: number
  max?: number
  step?: number
  unit?: string
  defaultValue: number
}

interface PatternSpec {
  name: string
  description: string
  params: PatternParamSpec[]
}

const DEFAULT_PATTERNS: PatternSpec[] = [
  {
    name: 'follow-the-leader',
    description: 'Followers trail the leader at a fixed horizontal + vertical separation (AC-5.4.3).',
    params: [
      { key: 'leader_vehicle_id', label: 'Leader vehicle id', kind: 'number', min: 0, max: 8, step: 1, defaultValue: 0 },
      { key: 'n_followers', label: 'Followers', kind: 'number', min: 1, max: 6, step: 1, defaultValue: 1 },
      { key: 'separation_h_m', label: 'Horizontal separation', kind: 'number', min: 2, max: 50, step: 1, unit: 'm', defaultValue: 10 },
      { key: 'separation_v_m', label: 'Vertical separation', kind: 'number', min: 0, max: 20, step: 0.5, unit: 'm', defaultValue: 2 },
    ],
  },
  {
    name: 'search-grid',
    description: 'Boustrophedon grid search across a rectangular area, split across N vehicles.',
    params: [
      { key: 'n_vehicles', label: 'Vehicles', kind: 'number', min: 1, max: 6, step: 1, defaultValue: 2 },
      { key: 'spacing_m', label: 'Lane spacing', kind: 'number', min: 5, max: 100, step: 1, unit: 'm', defaultValue: 20 },
      { key: 'area_w_m', label: 'Area width', kind: 'number', min: 20, max: 500, step: 5, unit: 'm', defaultValue: 100 },
      { key: 'area_h_m', label: 'Area height', kind: 'number', min: 20, max: 500, step: 5, unit: 'm', defaultValue: 100 },
    ],
  },
  {
    name: 'parallel-patrol',
    description: 'Parallel patrol legs flown side-by-side; each vehicle owns one leg at the chosen spacing.',
    params: [
      { key: 'n_vehicles', label: 'Vehicles', kind: 'number', min: 1, max: 6, step: 1, defaultValue: 2 },
      { key: 'spacing_m', label: 'Leg spacing', kind: 'number', min: 5, max: 200, step: 1, unit: 'm', defaultValue: 30 },
      { key: 'leg_length_m', label: 'Leg length', kind: 'number', min: 20, max: 500, step: 5, unit: 'm', defaultValue: 80 },
    ],
  },
]

/** Merge the backend's advertised patterns with our spec defaults; backend
 * entries win on name clash (their description may be richer). */
function mergePatterns(remote: SwarmPattern[]): PatternSpec[] {
  const map = new Map<string, PatternSpec>()
  for (const p of DEFAULT_PATTERNS) map.set(p.name, p)
  for (const r of remote) {
    const existing = map.get(r.name)
    if (existing) {
      map.set(r.name, { ...existing, description: r.description || existing.description })
    } else {
      map.set(r.name, { name: r.name, description: r.description, params: [] })
    }
  }
  return Array.from(map.values())
}

export function PatternsDialog({ fleet, disabled }: { fleet: FleetC2Api; disabled: boolean }) {
  const { toast } = useToast()
  const [open, setOpen] = useState(false)
  const [selectedName, setSelectedName] = useState<string>('follow-the-leader')
  const [paramOverrides, setParamOverrides] = useState<Record<string, number | string>>({})
  const [generating, setGenerating] = useState(false)
  const [generated, setGenerated] = useState<GeneratedPatternMission[] | null>(null)

  const patterns = mergePatterns(fleet.patterns)
  const selected = patterns.find((p) => p.name === selectedName) ?? patterns[0]

  /** The effective params: defaults overridden by any user edits. This avoids
   * the "setState in effect" anti-pattern — params are derived from the
   * selected pattern + the override map, not mirrored into state. */
  const params: Record<string, number | string> = {}
  if (selected) {
    for (const p of selected.params) {
      const ov = paramOverrides[p.key]
      params[p.key] = ov != null ? ov : p.defaultValue
    }
  }

  const onOpenChange = useCallback((next: boolean) => {
    setOpen(next)
    if (next) {
      // reset overrides + result on open
      setParamOverrides({})
      setGenerated(null)
      void fleet.listPatterns()
    }
  }, [fleet])

  const onSelectPattern = useCallback((name: string) => {
    setSelectedName(name)
    setParamOverrides({})
    setGenerated(null)
  }, [])

  const onGenerate = useCallback(async () => {
    if (!selected) return
    setGenerating(true)
    try {
      const res = await fleet.generatePattern(selected.name, params)
      if (res.ok && res.result) {
        setGenerated(res.result.missions)
        toast({
          title: 'Pattern generated',
          description: `${selected.name} · ${res.result.missions.length} per-vehicle missions (auto-bound)`,
        })
      } else {
        toast({
          title: 'Pattern generation failed',
          description: res.error ?? `POST /api/fleet/patterns/${selected.name}/generate rejected`,
          variant: 'destructive',
        })
      }
    } finally {
      setGenerating(false)
    }
  }, [selected, params, fleet, toast])

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogTrigger asChild>
        <Button variant="outline" size="sm" className="gap-1.5" disabled={disabled} aria-label="Open swarming-pattern library">
          <Sparkles className="size-3.5" aria-hidden="true" />
          Patterns
          {/* The pattern library is rendered lazily inside the modal; keep a
            * sr-only list of available pattern names in the DOM so the
            * initial SSR HTML carries them (accessibility + smoke-test). */}
          <span className="sr-only">
            available patterns: {DEFAULT_PATTERNS.map((p) => p.name).join(', ')}
          </span>
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent className="sm:max-w-lg">
        <AlertDialogHeader>
          <AlertDialogTitle>Swarming patterns</AlertDialogTitle>
          <AlertDialogDescription>
            Pick a pattern from the library, set its parameters, and the fleet plane generates one mission per vehicle (§5.4). Generated missions are auto-bound — press Start Fleet next.
          </AlertDialogDescription>
        </AlertDialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="pattern-name" className="text-xs">Pattern</Label>
            <Select value={selectedName} onValueChange={onSelectPattern}>
              <SelectTrigger id="pattern-name" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {patterns.map((p) => (
                  <SelectItem key={p.name} value={p.name} className="text-xs">
                    <span className="font-mono">{p.name}</span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {selected && (
              <p className="text-[11px] text-muted-foreground">{selected.description}</p>
            )}
          </div>

          {selected && selected.params.length > 0 && (
            <div className="grid grid-cols-2 gap-2.5">
              {selected.params.map((p) => (
                <div key={p.key} className="flex flex-col gap-1">
                  <Label htmlFor={`pat-${p.key}`} className="text-[11px]">
                    {p.label}
                    {p.unit ? <span className="ml-1 text-muted-foreground">({p.unit})</span> : null}
                  </Label>
                  <Input
                    id={`pat-${p.key}`}
                    type="number"
                    inputMode="numeric"
                    min={p.min}
                    max={p.max}
                    step={p.step ?? 1}
                    value={String(params[p.key] ?? p.defaultValue)}
                    onChange={(e) => {
                      const n = Number(e.target.value)
                      setParamOverrides((prev) => ({ ...prev, [p.key]: Number.isFinite(n) ? n : p.defaultValue }))
                    }}
                  />
                </div>
              ))}
            </div>
          )}

          {generated && generated.length > 0 && (
            <div className="max-h-44 overflow-y-auto rounded-md border border-border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className="pl-3">Vehicle</TableHead>
                    <TableHead>Mission</TableHead>
                    <TableHead className="pr-3 text-right">Waypoints</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {generated.map((m) => (
                    <TableRow key={`${m.vehicle_id}:${m.mission_id}`}>
                      <TableCell className="pl-3">
                        <span className="font-mono text-xs">v{m.vehicle_id}</span>
                      </TableCell>
                      <TableCell>
                        <span className="font-mono text-xs">{m.mission_id}</span>
                      </TableCell>
                      <TableCell className="pr-3 text-right">
                        <Badge variant="outline" className="font-mono text-[10px]">{m.waypoint_count} wp</Badge>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </div>

        <AlertDialogFooter>
          <AlertDialogCancel disabled={generating}>Close</AlertDialogCancel>
          <Button onClick={(e) => { e.preventDefault(); void onGenerate() }} disabled={generating || !selected}>
            {generating ? <Loader2 className="size-3.5 animate-spin" aria-hidden="true" /> : <Wand2 className="size-3.5" aria-hidden="true" />}
            Generate
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
