'use client'

/**
 * Fault console (rustsitsim SPEC §7): active fault list with clear (DELETE
 * /api/faults/{id}), and the §7.1 catalog as an inject form (POST /api/faults).
 */

import { useMemo, useState } from 'react'
import { CheckCircle2, Gauge, Trash2, Zap } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useToast } from '@/hooks/use-toast'
import { FAULT_CATALOG, type ActiveFault, type ConnState } from '@/lib/types'

export function FaultConsole({
  faults,
  conn,
  onInject,
  onClear,
}: {
  faults: ActiveFault[]
  conn: ConnState
  onInject: (params: Record<string, number | string>) => Promise<boolean>
  onClear: (id: string) => Promise<boolean>
}) {
  const { toast } = useToast()
  const [type, setType] = useState(FAULT_CATALOG[0].type)
  const [values, setValues] = useState<Record<string, string>>(defaultValues(FAULT_CATALOG[0]))
  const [busy, setBusy] = useState(false)

  const def = useMemo(() => FAULT_CATALOG.find((d) => d.type === type) ?? FAULT_CATALOG[0], [type])

  const switchType = (t: string) => {
    setType(t)
    const d = FAULT_CATALOG.find((x) => x.type === t)
    if (d) setValues(defaultValues(d))
  }

  const inject = async () => {
    const params: Record<string, number | string> = { type: def.type }
    for (const f of def.fields) {
      const raw = values[f.key] ?? String(f.defaultValue)
      if (f.kind === 'number') {
        const n = Number(raw)
        if (!Number.isFinite(n) || (f.min != null && n < f.min) || (f.max != null && n > f.max)) {
          toast({
            title: 'Invalid fault parameter',
            description: `${f.label} must be between ${f.min ?? '-∞'} and ${f.max ?? '∞'}`,
            variant: 'destructive',
          })
          return
        }
        params[f.key] = n
      } else {
        params[f.key] = raw
      }
    }
    setBusy(true)
    const ok = await onInject(params)
    setBusy(false)
    if (ok) {
      toast({
        title: `Fault injected — ${def.label}`,
        description:
          conn === 'live'
            ? 'POST /api/faults?XTransformPort=8200 accepted'
            : `${def.code} applied to the internal simulator (backend offline)`,
      })
    } else {
      toast({ title: 'Fault injection failed', description: 'POST /api/faults rejected by the backend', variant: 'destructive' })
    }
  }

  const clear = async (id: string) => {
    const ok = await onClear(id)
    if (ok) {
      toast({ title: `Fault ${id} cleared`, description: conn === 'live' ? 'DELETE /api/faults/{id} accepted' : 'effect removed at end of tick' })
    } else {
      toast({ title: `Could not clear fault ${id}`, variant: 'destructive' })
    }
  }

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      {/* Active faults */}
      <div className="flex min-w-0 flex-col gap-2">
        <h4 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <Gauge className="size-3.5" aria-hidden="true" /> Active faults
          <Badge variant="secondary" className="font-mono">
            {faults.length}
          </Badge>
        </h4>
        {faults.length === 0 ? (
          <div className="flex h-full min-h-28 flex-col items-center justify-center gap-1.5 rounded-lg border border-dashed border-emerald-600/30 bg-emerald-500/5 p-4 text-center">
            <CheckCircle2 className="size-5 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
            <p className="text-sm font-medium text-emerald-700 dark:text-emerald-400">No active faults</p>
            <p className="text-xs text-muted-foreground">Model layers nominal — inject from the catalog</p>
          </div>
        ) : (
          <ScrollArea className="h-auto max-h-64 rounded-lg border border-border">
            <ul className="divide-y divide-border">
              {faults.map((f) => (
                <li key={f.id} className="flex items-center gap-2.5 px-3 py-2.5">
                  <Badge variant="destructive" className="font-mono text-[10px]">
                    {catalogCode(f.type)}
                  </Badge>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{catalogLabel(f.type)}</p>
                    <p className="truncate font-mono text-[10px] text-muted-foreground">
                      id {f.id}
                      {Object.keys(f.params).length > 0 && ` · ${paramSummary(f)}`}
                      {f.ttl_s != null && ` · ${(f.ttl_s ?? 0) > 0 ? `${Math.ceil(f.ttl_s)} s left` : 'expiring'}`}
                      {f.ttl_s == null && ' · persistent'}
                    </p>
                  </div>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => clear(f.id)}
                    aria-label={`Clear fault ${f.id}`}
                    className="size-8 shrink-0 text-muted-foreground hover:text-rose-600"
                  >
                    <Trash2 className="size-4" aria-hidden="true" />
                  </Button>
                </li>
              ))}
            </ul>
          </ScrollArea>
        )}
      </div>

      {/* Inject form */}
      <div className="flex min-w-0 flex-col gap-2">
        <h4 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <Zap className="size-3.5" aria-hidden="true" /> Inject fault
          <span className="font-normal normal-case text-muted-foreground/70">
            {conn === 'live' ? 'POST /api/faults?XTransformPort=8200' : 'applied to internal simulator'}
          </span>
        </h4>
        <div className="rounded-lg border border-border p-3">
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="grid gap-1.5">
              <Label htmlFor="fault-type" className="text-xs">
                Type (§7.1 catalog)
              </Label>
              <Select value={type} onValueChange={switchType}>
                <SelectTrigger id="fault-type" aria-label="Fault type">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {FAULT_CATALOG.map((d) => (
                    <SelectItem key={d.type} value={d.type}>
                      <span className="font-mono text-[10px] text-muted-foreground">{d.code}</span> {d.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {def.fields.map((f) =>
              f.kind === 'select' ? (
                <div key={f.key} className="grid gap-1.5">
                  <Label htmlFor={`fault-${f.key}`} className="text-xs">
                    {f.label}
                  </Label>
                  <Select value={values[f.key] ?? String(f.defaultValue)} onValueChange={(v) => setValues((s) => ({ ...s, [f.key]: v }))}>
                    <SelectTrigger id={`fault-${f.key}`} aria-label={f.label}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {f.options?.map((o) => (
                        <SelectItem key={o.value} value={o.value}>
                          {o.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              ) : (
                <div key={f.key} className="grid gap-1.5">
                  <Label htmlFor={`fault-${f.key}`} className="text-xs">
                    {f.label}
                    {f.unit ? <span className="text-muted-foreground"> ({f.unit})</span> : null}
                  </Label>
                  <Input
                    id={`fault-${f.key}`}
                    type="number"
                    inputMode="decimal"
                    min={f.min}
                    max={f.max}
                    step={f.step}
                    value={values[f.key] ?? String(f.defaultValue)}
                    onChange={(e) => setValues((s) => ({ ...s, [f.key]: e.target.value }))}
                    className="h-9 font-mono text-xs"
                  />
                </div>
              ),
            )}
          </div>

          <p className="mt-3 text-xs text-muted-foreground">{def.effect}</p>
          <Button onClick={inject} disabled={busy} className="mt-3 w-full gap-2 sm:w-auto" aria-label={`Inject ${def.label} fault`}>
            <Zap className="size-4" aria-hidden="true" />
            {busy ? 'Injecting…' : `Inject ${def.code}`}
          </Button>
        </div>
      </div>
    </div>
  )
}

function defaultValues(d: (typeof FAULT_CATALOG)[number]): Record<string, string> {
  const out: Record<string, string> = {}
  for (const f of d.fields) out[f.key] = String(f.defaultValue)
  return out
}

function catalogCode(type: string): string {
  return FAULT_CATALOG.find((d) => d.type === type)?.code ?? type.slice(0, 4)
}

function catalogLabel(type: string): string {
  return FAULT_CATALOG.find((d) => d.type === type)?.label ?? type
}

function paramSummary(f: ActiveFault): string {
  return Object.entries(f.params)
    .map(([k, v]) => `${k}=${v}`)
    .join(' ')
}
