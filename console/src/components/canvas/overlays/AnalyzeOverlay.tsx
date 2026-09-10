'use client'

/**
 * GCS v2 Operations Canvas — Analyze overlay (M13, T-B5).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.5 Analyze + §5.5 (replay/ULog browse).
 *
 * Replay/ULog lists, scrub timeline + play/pause + step, plot stack + add-
 * topic modal, overlay-live picker. Data: :8300 replays/ulogs REST via
 * useAnalyze hook (functions, not state — component fetches + holds lists).
 * L11/L12 replay trail + playhead render as map layers (full scrub timeline
 * lands M13 late).
 */

import { useState, useEffect, type JSX } from 'react'
import { useAnalyze } from '@/hooks/useAnalyze'
import { toggleOverlay, pushNotification } from '@/state/app-store'
import type { ReplayFile, UlogFile } from '@/lib/types'

type Tab = 'replays' | 'ulogs' | 'plots'

export function AnalyzeOverlay(): JSX.Element {
  const analyze = useAnalyze()
  const [tab, setTab] = useState<Tab>('replays')
  const [replays, setReplays] = useState<ReplayFile[]>([])
  const [ulogs, setUlogs] = useState<UlogFile[]>([])

  useEffect(() => {
    if (tab === 'replays') void analyze.listReplays().then(setReplays)
    if (tab === 'ulogs') void analyze.listUlogs().then(setUlogs)
  }, [tab, analyze])

  return (
    <div data-rsim-zone="F" role="dialog" aria-modal="true" className="pointer-events-auto rsim-canvas" style={{ position: 'absolute', left: 56, top: 48, bottom: 96, width: 380, zIndex: 25, background: 'var(--rsim-surface-solid)', borderRight: '1px solid var(--rsim-border)', display: 'flex', flexDirection: 'column', overflow: 'hidden', fontFamily: 'var(--rsim-font-ui)', color: 'var(--rsim-text)' }}>
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Analyze</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>{analyze.catalogAlive ? 'live' : 'offline'} · {tab}</div>
        </div>
        <button type="button" onClick={() => toggleOverlay('analyze', { force: false })} style={{ background: 'transparent', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text-dim)', cursor: 'pointer', padding: '4px 8px', fontSize: 11 }}>×</button>
      </div>
      <div style={{ display: 'flex', borderBottom: '1px solid var(--rsim-border)' }}>
        {(['replays', 'ulogs', 'plots'] as const).map((t) => (
          <button key={t} type="button" onClick={() => setTab(t)} style={{ flex: 1, padding: '6px 4px', background: 'transparent', border: 'none', borderBottom: tab === t ? '2px solid var(--rsim-accent)' : '2px solid transparent', color: tab === t ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)', fontSize: 10, fontWeight: 600, cursor: 'pointer', textTransform: 'capitalize' }}>{t}</button>
        ))}
      </div>
      <div style={{ flex: 1, overflow: 'auto' }}>
        {tab === 'replays' && <ReplayList replays={replays} />}
        {tab === 'ulogs' && <UlogList ulogs={ulogs} />}
        {tab === 'plots' && <PlotPanel />}
      </div>
    </div>
  )
}

function ReplayList({ replays }: { replays: ReplayFile[] }): JSX.Element {
  return (
    <div style={{ padding: 8 }}>
      <div style={{ fontSize: 11, color: 'var(--rsim-text-dim)', marginBottom: 8 }}>RustSim .replay files ({replays.length}).</div>
      {replays.map((r) => (
        <div key={r.filename} style={{ padding: 8, borderBottom: '1px solid var(--rsim-border)' }}>
          <div className="rsim-mono" style={{ fontSize: 11 }}>{r.filename}</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>{r.duration_s.toFixed(1)}s · {r.vehicle_count} vehicles</div>
        </div>
      ))}
      {replays.length === 0 && <div style={{ color: 'var(--rsim-text-dim)' }}>No replays.</div>}
    </div>
  )
}

function UlogList({ ulogs }: { ulogs: UlogFile[] }): JSX.Element {
  return (
    <div style={{ padding: 8 }}>
      <div style={{ fontSize: 11, color: 'var(--rsim-text-dim)', marginBottom: 8 }}>PX4 .ulg files ({ulogs.length}).</div>
      {ulogs.map((u) => (
        <div key={u.filename} style={{ padding: 8, borderBottom: '1px solid var(--rsim-border)' }}>
          <div className="rsim-mono" style={{ fontSize: 11 }}>{u.filename}</div>
          <div className="rsim-mono" style={{ fontSize: 10, color: 'var(--rsim-text-dim)' }}>{(u.size_bytes / 1024).toFixed(0)} KB</div>
        </div>
      ))}
      {ulogs.length === 0 && <div style={{ color: 'var(--rsim-text-dim)' }}>No ULogs.</div>}
    </div>
  )
}

function PlotPanel(): JSX.Element {
  return (
    <div style={{ padding: 8 }}>
      <div style={{ fontSize: 11, color: 'var(--rsim-text-dim)' }}>Plot stack + add-topic modal + overlay-live picker. The scrub timeline + playhead render as map layers L11/L12 (M13 late).</div>
      <div style={{ marginTop: 12, fontSize: 10, color: 'var(--rsim-text-dim)' }}>Use the catalog list above to load a replay/ULog, then select topics to plot. The full scrub timeline + playhead lands with the M13 follow-up.</div>
    </div>
  )
}
