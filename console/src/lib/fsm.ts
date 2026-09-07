/** FSM → color mapping shared by the fleet table, map and safety panel. */

export interface FsmStyle {
  hex: string
  css: string
}

const MAP: Record<string, FsmStyle> = {
  INIT: { hex: '#71717a', css: 'border-zinc-500/40 bg-zinc-500/10 text-zinc-600 dark:text-zinc-400' },
  SPAWNING: { hex: '#fbbf24', css: 'border-amber-400/40 bg-amber-400/10 text-amber-700 dark:text-amber-300' },
  BOOTING: { hex: '#f59e0b', css: 'border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-400' },
  READY: { hex: '#10b981', css: 'border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400' },
  ACTIVE: { hex: '#059669', css: 'border-emerald-600/50 bg-emerald-600/15 text-emerald-700 dark:text-emerald-300' },
  RTL: { hex: '#fb923c', css: 'border-orange-400/50 bg-orange-400/10 text-orange-700 dark:text-orange-300' },
  LANDED: { hex: '#a1a1aa', css: 'border-zinc-400/40 bg-zinc-400/10 text-zinc-500 dark:text-zinc-400' },
  FAULT: { hex: '#f43f5e', css: 'border-rose-500/50 bg-rose-500/10 text-rose-700 dark:text-rose-400' },
}

export function fsmStyle(fsm: string): FsmStyle {
  return MAP[fsm.toUpperCase()] ?? { hex: '#71717a', css: 'border-border bg-muted text-muted-foreground' }
}
