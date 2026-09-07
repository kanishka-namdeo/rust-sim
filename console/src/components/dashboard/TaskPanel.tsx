'use client'

/** Task board + auction/allocation log (mavfleet SPEC §6, §13). */

import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Gavel, ListChecks } from 'lucide-react'
import { fmtSigned } from '@/lib/format'
import type { AuctionEntry, FleetTask } from '@/lib/types'
import { cn } from '@/lib/utils'

const STATUS_STYLE: Record<string, string> = {
  pending: 'border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-400',
  assigned: 'border-orange-400/40 bg-orange-400/10 text-orange-700 dark:text-orange-300',
  in_progress: 'border-emerald-600/50 bg-emerald-600/15 text-emerald-700 dark:text-emerald-300',
  done: 'border-zinc-400/40 bg-zinc-400/10 text-zinc-500 dark:text-zinc-400',
  rejected: 'border-rose-500/40 bg-rose-500/10 text-rose-700 dark:text-rose-400',
}

export function TaskPanel({ tasks, auctions }: { tasks: FleetTask[]; auctions: AuctionEntry[] }) {
  const done = tasks.filter((t) => t.status === 'done').length
  const active = tasks.filter((t) => t.status === 'in_progress' || t.status === 'assigned').length
  const pending = tasks.filter((t) => t.status === 'pending').length

  return (
    <div className="flex min-h-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="secondary" className="font-mono">{tasks.length} tasks</Badge>
        <Badge variant="outline" className="border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400 font-mono">
          {active} active
        </Badge>
        <Badge variant="outline" className="border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-400 font-mono">
          {pending} pending
        </Badge>
        <Badge variant="outline" className="border-border bg-muted text-muted-foreground font-mono">{done} done</Badge>
      </div>

      <div>
        <h4 className="mb-1.5 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <ListChecks className="size-3.5" aria-hidden="true" /> Task board
        </h4>
        <ScrollArea className="max-h-72 rounded-lg border border-border">
          <ul className="divide-y divide-border">
            {tasks.length === 0 && <li className="px-3 py-6 text-center text-sm text-muted-foreground">No tasks in the set</li>}
            {tasks.map((t) => (
              <li key={t.id} className="px-3 py-2">
                <div className="flex items-center gap-2">
                  <span className="font-mono text-xs font-semibold">{t.id}</span>
                  <Badge variant="outline" className={cn('text-[9px] font-mono', STATUS_STYLE[t.status] ?? '')}>
                    {t.status}
                  </Badge>
                  <span className="ml-auto font-mono text-[10px] text-muted-foreground">
                    {t.assigned_to ? `→ ${t.assigned_to}` : 'unassigned'} · r{t.reward}
                  </span>
                </div>
                <div className="mt-1 flex items-center gap-2">
                  <span className="font-mono text-[10px] text-muted-foreground">
                    N {fmtSigned(t.pos_ned_m[0], 1)} / E {fmtSigned(t.pos_ned_m[1], 1)} / D {fmtSigned(t.pos_ned_m[2], 1)} · hover{' '}
                    {t.hover_s}s
                  </span>
                  {t.status === 'in_progress' && (
                    <div className="ml-auto flex w-20 items-center gap-1.5" aria-label={`${t.id} progress ${(t.progress * 100).toFixed(0)}%`}>
                      <div className="h-1 flex-1 overflow-hidden rounded bg-muted">
                        <div className="h-full rounded bg-emerald-500" style={{ width: `${Math.round(t.progress * 100)}%` }} />
                      </div>
                      <span className="font-mono text-[9px] text-muted-foreground">{Math.round(t.progress * 100)}%</span>
                    </div>
                  )}
                </div>
              </li>
            ))}
          </ul>
        </ScrollArea>
      </div>

      <div>
        <h4 className="mb-1.5 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <Gavel className="size-3.5" aria-hidden="true" /> Allocation log
          <span className="font-normal normal-case text-muted-foreground/70">contract-net rounds</span>
        </h4>
        <ScrollArea className="max-h-48 rounded-lg border border-border">
          <ul className="divide-y divide-border">
            {auctions.length === 0 && (
              <li className="px-3 py-6 text-center text-sm text-muted-foreground">No allocation rounds yet</li>
            )}
            {auctions
              .slice()
              .reverse()
              .map((a) => (
                <li key={`${a.round}-${a.task}`} className="px-3 py-1.5 font-mono text-[11px]">
                  <span className="text-muted-foreground">r{a.round}</span>{' '}
                  <span className="font-semibold">{a.task}</span> <span className="text-muted-foreground">→</span>{' '}
                  <span className="text-emerald-700 dark:text-emerald-400">{a.vehicle}</span>{' '}
                  <span className="text-muted-foreground">
                    bid {a.bid_s.toFixed(1)}s · {a.bidders} bidders
                  </span>
                </li>
              ))}
          </ul>
        </ScrollArea>
      </div>
    </div>
  )
}
