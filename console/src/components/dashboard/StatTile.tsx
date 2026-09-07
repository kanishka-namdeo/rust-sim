'use client'

/** Compact labelled numeric tile used across both consoles. */

import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'

export type TileTone = 'default' | 'ok' | 'warn' | 'crit' | 'accent'

const toneClass: Record<TileTone, string> = {
  default: 'text-foreground',
  ok: 'text-emerald-600 dark:text-emerald-400',
  warn: 'text-amber-600 dark:text-amber-400',
  crit: 'text-rose-600 dark:text-rose-400',
  accent: 'text-emerald-600 dark:text-emerald-400',
}

export function StatTile({
  label,
  value,
  unit,
  sub,
  icon,
  tone = 'default',
  className,
}: {
  label: string
  value: ReactNode
  unit?: string
  sub?: string
  icon?: ReactNode
  tone?: TileTone
  className?: string
}) {
  return (
    <div
      className={cn(
        'flex flex-col gap-0.5 rounded-lg border border-border/70 bg-card/60 px-3 py-2.5 min-w-0',
        className,
      )}
    >
      <span className="flex items-center gap-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
        {icon != null && <span className="text-muted-foreground/80 [&>svg]:size-3">{icon}</span>}
        {label}
      </span>
      <span className={cn('flex items-baseline gap-1 truncate font-mono text-base font-semibold tabular-nums', toneClass[tone])}>
        <span className="truncate">{value}</span>
        {unit != null && <span className="text-[10px] font-normal text-muted-foreground">{unit}</span>}
      </span>
      {sub != null && <span className="truncate text-[10px] text-muted-foreground">{sub}</span>}
    </div>
  )
}
