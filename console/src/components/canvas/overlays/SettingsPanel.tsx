'use client'

/**
 * GCS v2 Operations Canvas — Settings panel (M15, T-M15).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.5 Settings + §7.3.4 (WCAG 2.1.4 toggle).
 *
 * Units (metric default: m, m/s; imperial opt-in), coordinates (6-dp decimal
 * default; DMS opt-in), clock (UTC default; local opt-in), keyboard shortcuts
 * on/off (§7.3.4 compliance path), Reset layout (§2.2 F). Persisted in
 * localStorage (rsim.settings.v1).
 */

import { useState, type JSX } from 'react'
import { toggleOverlay } from '@/state/app-store'

const STORAGE_KEY = 'rsim.settings.v1'

interface Settings {
  units: 'metric' | 'imperial'
  coords: 'decimal' | 'dms'
  clock: 'utc' | 'local'
  shortcutsEnabled: boolean
}

function loadSettings(): Settings {
  if (typeof window === 'undefined') return { units: 'metric', coords: 'decimal', clock: 'utc', shortcutsEnabled: true }
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return { units: 'metric', coords: 'decimal', clock: 'utc', shortcutsEnabled: true }
    return { ...{ units: 'metric', coords: 'decimal', clock: 'utc', shortcutsEnabled: true }, ...JSON.parse(raw) as Partial<Settings> }
  } catch {
    return { units: 'metric', coords: 'decimal', clock: 'utc', shortcutsEnabled: true }
  }
}

function saveSettings(s: Settings): void {
  if (typeof window === 'undefined') return
  try { localStorage.setItem(STORAGE_KEY, JSON.stringify(s)) } catch { /* non-fatal */ }
}

/** The ShortcutsProvider reads this to toggle the keymap (§7.3.4). */
export function isShortcutsEnabled(): boolean {
  return loadSettings().shortcutsEnabled
}

/** Toggle the keymap on/off (§7.3.4 — WCAG 2.1.4 compliance). */
export function setShortcutsEnabled(on: boolean): void {
  const s = loadSettings()
  s.shortcutsEnabled = on
  saveSettings(s)
}

export function SettingsPanel(): JSX.Element {
  const [settings, setSettings] = useState<Settings>(loadSettings)

  const update = (partial: Partial<Settings>): void => {
    const next = { ...settings, ...partial }
    setSettings(next)
    saveSettings(next)
  }

  const resetLayout = (): void => {
    // Clear the rsim.layout.v1 localStorage key (§2.2 F overlay panel
    // positions + the map center/zoom persisted there).
    try { localStorage.removeItem('rsim.layout.v1') } catch { /* non-fatal */ }
  }

  return (
    <div data-rsim-zone="F" className="pointer-events-auto rsim-canvas" style={{ position: 'absolute', right: 0, top: 48, bottom: 96, width: 320, zIndex: 25, background: 'var(--rsim-surface-solid)', borderLeft: '1px solid var(--rsim-border)', display: 'flex', flexDirection: 'column', overflow: 'hidden', fontFamily: 'var(--rsim-font-ui)', color: 'var(--rsim-text)' }}>
      <div style={{ padding: 12, borderBottom: '1px solid var(--rsim-border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div style={{ fontSize: 13, fontWeight: 600 }}>Settings</div>
        <button type="button" onClick={() => toggleOverlay('settings', { force: false })} style={{ background: 'transparent', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text-dim)', cursor: 'pointer', padding: '4px 8px', fontSize: 11 }}>×</button>
      </div>
      <div style={{ flex: 1, overflow: 'auto', padding: 12, display: 'flex', flexDirection: 'column', gap: 16 }}>
        {/* Units */}
        <Section title="Units">
          <Toggle label="Metric (m, m/s)" checked={settings.units === 'metric'} onChange={() => update({ units: 'metric' })} />
          <Toggle label="Imperial (ft, mph)" checked={settings.units === 'imperial'} onChange={() => update({ units: 'imperial' })} />
        </Section>
        {/* Coordinates */}
        <Section title="Coordinates">
          <Toggle label="Decimal (6 dp)" checked={settings.coords === 'decimal'} onChange={() => update({ coords: 'decimal' })} />
          <Toggle label="DMS" checked={settings.coords === 'dms'} onChange={() => update({ coords: 'dms' })} />
        </Section>
        {/* Clock */}
        <Section title="Clock">
          <Toggle label="UTC" checked={settings.clock === 'utc'} onChange={() => update({ clock: 'utc' })} />
          <Toggle label="Local" checked={settings.clock === 'local'} onChange={() => update({ clock: 'local' })} />
        </Section>
        {/* Keyboard shortcuts — §7.3.4 WCAG 2.1.4 */}
        <Section title="Keyboard shortcuts">
          <Toggle label="Enabled (single-key operator verbs)" checked={settings.shortcutsEnabled} onChange={(v) => update({ shortcutsEnabled: v })} />
          <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)', marginTop: 4 }}>
            When off, only ? and Esc remain live (so the toggle can be re-enabled). WCAG 2.1.4 compliance path.
          </div>
        </Section>
        {/* Reset layout — §2.2 F */}
        <Section title="Layout">
          <button type="button" onClick={resetLayout} style={{ ...btnStyle, width: '100%' }}>Reset layout</button>
          <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)', marginTop: 4 }}>
            Clears the persisted overlay panel positions + map center/zoom.
          </div>
        </Section>
      </div>
    </div>
  )
}

function Section({ title, children }: { title: string; children: React.ReactNode }): JSX.Element {
  return (
    <div>
      <div style={{ fontSize: 10, fontWeight: 700, color: 'var(--rsim-accent)', textTransform: 'uppercase', letterSpacing: '0.08em', marginBottom: 8 }}>{title}</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>{children}</div>
    </div>
  )
}

function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }): JSX.Element {
  return (
    <button type="button" onClick={() => onChange(!checked)} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '6px 10px', borderRadius: 'var(--rsim-radius-control)', border: `1px solid ${checked ? 'var(--rsim-accent)' : 'var(--rsim-border)'}`, background: checked ? 'rgba(34, 211, 238, 0.08)' : 'transparent', color: checked ? 'var(--rsim-accent)' : 'var(--rsim-text)', fontSize: 11, cursor: 'pointer', textAlign: 'left' }}>
      <span>{label}</span>
      <span className="rsim-mono" style={{ fontSize: 10, opacity: 0.7 }}>{checked ? '●' : '○'}</span>
    </button>
  )
}

const btnStyle: React.CSSProperties = { background: 'rgba(17, 22, 29, 0.6)', border: '1px solid var(--rsim-border)', borderRadius: 'var(--rsim-radius-control)', color: 'var(--rsim-text)', fontSize: 11, padding: '4px 10px', cursor: 'pointer', fontFamily: 'var(--rsim-font-mono)' }
