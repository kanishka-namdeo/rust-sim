'use client'

/**
 * GCS v2 Operations Canvas — Notification stack (Zone G, M8 T-B1).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.6 + §2.2 zone G.
 *
 * P3 fix: real queue (max 5 visible, 6 s auto-dismiss for info/warn,
 * errors sticky + acknowledge, click-through opens source panel).
 *
 * Stack semantics per §7.3.4 (`role="log"`, `aria-live="polite"` for
 * info/warn; `aria-live="assertive"` for errors). Severity colors §5.4.
 * E-stop/battery-ladder events render as persistent status chips in
 * strip A until cleared (they are states, not events — M9 wires that
 * split; M8 ships the basic queue).
 *
 * Top-right, below the status strip. `pointer-events: none` on the stack
 * container, `pointer-events: auto` on each notification (so map gestures
 * pass through empty space — §4.4).
 */

import { useAppStore, dismissNotification, type Notification } from '@/state/app-store'

export function NotificationStack() {
  const app = useAppStore()

  return (
    <div
      data-rsim-zone="G"
      className="rsim-notify-stack"
      role="log"
      aria-live="polite"
      aria-label="Notifications"
    >
      {app.notifications.map((n) => (
        <NotificationCard key={n.id} note={n} onDismiss={() => dismissNotification(n.id)} />
      ))}
    </div>
  )
}

function NotificationCard({ note, onDismiss }: { note: Notification; onDismiss: () => void }) {
  const color = note.severity === 'error'
    ? 'var(--rsim-danger)'
    : note.severity === 'warn'
      ? 'var(--rsim-alert)'
      : 'var(--rsim-text-dim)'
  const background = note.severity === 'error'
    ? 'rgba(239, 68, 68, 0.10)'
    : note.severity === 'warn'
      ? 'rgba(245, 158, 11, 0.10)'
      : 'rgba(17, 22, 29, 0.85)'

  return (
    <div
      role={note.severity === 'error' ? 'alert' : 'status'}
      aria-live={note.severity === 'error' ? 'assertive' : 'polite'}
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 4,
        padding: 10,
        borderRadius: 'var(--rsim-radius-panel)',
        border: `1px solid ${color}`,
        background,
        boxShadow: 'var(--rsim-shadow-overlay)',
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, justifyContent: 'space-between' }}>
        <span className="rsim-mono" style={{ fontSize: 11, fontWeight: 600, color, letterSpacing: '0.04em' }}>
          {note.severity.toUpperCase()}
        </span>
        <button
          type="button"
          onClick={onDismiss}
          aria-label="Dismiss notification"
          style={{
            background: 'transparent',
            border: 'none',
            color: 'var(--rsim-text-dim)',
            cursor: 'pointer',
            fontSize: 14,
            lineHeight: 1,
            padding: 2,
          }}
        >
          ×
        </button>
      </div>
      <div style={{ fontSize: 12, color: 'var(--rsim-text)', fontWeight: 600 }}>{note.title}</div>
      {note.detail && (
        <div className="rsim-mono" style={{ fontSize: 11, color: 'var(--rsim-text-dim)' }}>
          {note.detail}
        </div>
      )}
      {note.sticky && (
        <div style={{ fontSize: 10, color: 'var(--rsim-text-dim)', marginTop: 2 }}>
          (sticky — click × to dismiss)
        </div>
      )}
    </div>
  )
}
